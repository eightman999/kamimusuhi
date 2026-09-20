"""Bounded read-only Linux sensor daemon; no process, address or filename telemetry."""
from __future__ import annotations

import argparse
import csv
import io
import json
import math
import os
from pathlib import Path
import re
import resource
import shutil
import signal
import subprocess
import sys
import time

SCHEMA_ID = "k0f.raw.v1"
COMMON_METRICS = (
    "cpu_utilization", "cpu_load_1m", "cpu_load_5m", "cpu_load_15m", "cpu_count",
    "memory_total_bytes", "memory_available_bytes", "memory_pressure", "swap_total_bytes", "swap_used_bytes",
    "uptime_s", "network_rtt_ms", "network_loss", "network_connectivity", "daemon_cpu_fraction", "daemon_rss_bytes",
)
MASTER_METRICS = (
    "cpu_temperature_c", "cpu_iowait", "cpu_frequency_mhz", "run_queue", "major_page_faults",
    "memory_psi_some_avg10", "io_pressure", "disk_busy_fraction", "disk_read_bytes_s", "disk_write_bytes_s",
    "disk_free_bytes", "disk_total_bytes", "network_rx_bytes_s", "network_tx_bytes_s", "cpu_throttling_count",
)
GPU_SUFFIXES = (
    "temperature_c", "utilization", "memory_utilization", "vram_used_bytes", "vram_total_bytes", "power_w",
    "power_limit_w", "clock_graphics_mhz", "clock_memory_mhz", "throttle_reason_bits",
)
METRIC_ALLOWLIST = frozenset(COMMON_METRICS + MASTER_METRICS + tuple(f"{gpu}_{field}" for gpu in ("rtx3060", "p100") for field in GPU_SUFFIXES))


def _read(path: Path) -> str:
    try:
        return path.read_text()
    except (OSError, UnicodeError):
        return ""


def finite_number(value):
    try:
        result = float(value)
        return result if math.isfinite(result) else None
    except (TypeError, ValueError):
        return None


def parse_meminfo(text: str) -> dict:
    result = {}
    for line in text.splitlines():
        fields = line.replace(":", "").split()
        if len(fields) >= 2:
            value = finite_number(fields[1])
            if value is not None:
                result[fields[0]] = value * (1024 if len(fields) >= 3 and fields[2] == "kB" else 1)
    return result


def parse_cpu_stat(text: str) -> tuple[list[float], float | None]:
    ticks, running = [], None
    for line in text.splitlines():
        fields = line.split()
        if fields and fields[0] == "cpu":
            # guest/guest_nice are already included in user/nice.
            ticks = [float(value) for value in fields[1:9]]
        elif fields and fields[0] == "procs_running":
            running = finite_number(fields[1])
    return ticks, running


def cpu_fractions(previous: list[float], current: list[float]) -> tuple[float | None, float | None]:
    if len(previous) < 5 or len(current) != len(previous):
        return None, None
    delta = [now - old for old, now in zip(previous, current)]
    total = sum(delta)
    if total <= 0 or any(value < 0 for value in delta):
        return None, None
    return max(0.0, min(1.0, (total - delta[3] - delta[4]) / total)), max(0.0, min(1.0, delta[4] / total))


def parse_psi(text: str) -> float | None:
    match = re.search(r"^some\s+.*?avg10=([\d.]+)", text, re.MULTILINE)
    return finite_number(match.group(1)) / 100 if match else None


def parse_network(text: str) -> tuple[float, float]:
    rx, tx = 0.0, 0.0
    for line in text.splitlines():
        if ":" not in line:
            continue
        interface, fields = line.split(":", 1)
        values = fields.split()
        if interface.strip() != "lo" and len(values) >= 16:
            rx += float(values[0])
            tx += float(values[8])
    return rx, tx


def parse_disks(text: str, devices: set[str]) -> dict[str, tuple[float, float, float]]:
    result = {}
    for line in text.splitlines():
        fields = line.split()
        if len(fields) >= 14 and fields[2] in devices:
            result[fields[2]] = (float(fields[5]) * 512, float(fields[9]) * 512, float(fields[12]))
    return result


def parse_gpu_csv(text: str) -> dict:
    result = {}
    for fields in csv.reader(io.StringIO(text)):
        if len(fields) < 10:
            continue
        model = fields[0].strip().lower()
        prefix = "rtx3060" if re.search(r"\b3060\b", model) else "p100" if "p100" in model else None
        if prefix is None:
            continue
        for index, suffix in enumerate(GPU_SUFFIXES[:-1], 1):
            value = finite_number(fields[index].strip())
            if value is not None:
                if suffix in {"utilization", "memory_utilization"}:
                    value /= 100
                elif suffix in {"vram_used_bytes", "vram_total_bytes"}:
                    value *= 1024 ** 2
            result[f"{prefix}_{suffix}"] = value
        bits = None
        if len(fields) > 10:
            try:
                bits = int(fields[10].strip(), 0)
            except ValueError:
                pass
        result[f"{prefix}_throttle_reason_bits"] = bits
    return result


def ping_peer(peer_host: str | None, timeout: float = 0.7) -> dict:
    missing = {"network_rtt_ms": None, "network_loss": None, "network_connectivity": None}
    if not peer_host:
        return missing
    executable = shutil.which("ping")
    if executable is None:
        return missing
    # Peer is used only as argv, never copied into errors or output records.
    try:
        result = subprocess.run([executable, "-n", "-c", "1", "-W", "1", "--", peer_host], capture_output=True, text=True, timeout=timeout, check=False)
        match = re.search(r"time[=<]([\d.]+)\s*ms", result.stdout)
        return {"network_rtt_ms": finite_number(match.group(1)) if match else None, "network_loss": float(result.returncode != 0), "network_connectivity": float(result.returncode == 0)}
    except (OSError, subprocess.TimeoutExpired):
        return {"network_rtt_ms": None, "network_loss": 1.0, "network_connectivity": 0.0}


class LinuxSensor:
    def __init__(self, peer_host: str | None = None, interval: float = 1.0, proc: Path = Path("/proc"), sys_root: Path = Path("/sys")):
        if interval < 0.5:
            raise ValueError("sampling interval must be at least 0.5 seconds")
        self.peer_host, self.interval, self.proc, self.sys_root = peer_host, interval, proc, sys_root
        self.sequence = 0
        self._last_time = time.monotonic()
        self._last_cpu = parse_cpu_stat(_read(proc / "stat"))[0]
        self._last_net = parse_network(_read(proc / "net/dev"))
        self._last_disks = self._disk_snapshot()
        self._last_process_cpu = time.process_time()
        self._gpu_extended = True

    def _disk_snapshot(self):
        directory = self.sys_root / "block"
        try:
            devices = {p.name for p in directory.iterdir() if not p.name.startswith(("loop", "ram", "dm-"))}
        except OSError:
            devices = set()
        return parse_disks(_read(self.proc / "diskstats"), devices)

    def _gpu_metrics(self):
        executable = shutil.which("nvidia-smi")
        if executable is None:
            return {}
        fields = "name,temperature.gpu,utilization.gpu,utilization.memory,memory.used,memory.total,power.draw,power.limit,clocks.current.graphics,clocks.current.memory"
        def query(extended):
            return subprocess.run([executable, "--query-gpu=" + fields + (",clocks_event_reasons.active" if extended else ""), "--format=csv,noheader,nounits"], capture_output=True, text=True, timeout=1.2, check=False)
        try:
            result = query(self._gpu_extended)
            if result.returncode and self._gpu_extended:
                self._gpu_extended = False
                result = query(False)
            return parse_gpu_csv(result.stdout) if result.returncode == 0 else {}
        except (OSError, subprocess.TimeoutExpired):
            return {}

    def collect(self) -> dict:
        now, monotonic = time.time(), time.monotonic()
        elapsed = monotonic - self._last_time
        metrics = {key: None for key in sorted(METRIC_ALLOWLIST)}
        provenance = {}
        def add(values, source):
            for key, value in values.items():
                if key not in METRIC_ALLOWLIST:
                    raise ValueError("metric is outside privacy allowlist")
                metrics[key] = finite_number(value)
                provenance[key] = source
        ticks, running = parse_cpu_stat(_read(self.proc / "stat"))
        busy, iowait = cpu_fractions(self._last_cpu, ticks)
        add({"cpu_utilization": busy, "cpu_iowait": iowait, "run_queue": running, "cpu_count": os.cpu_count()}, "linux.proc.stat")
        try:
            add(dict(zip(("cpu_load_1m", "cpu_load_5m", "cpu_load_15m"), os.getloadavg())), "posix.getloadavg")
        except OSError:
            pass
        memory = parse_meminfo(_read(self.proc / "meminfo"))
        total, available = memory.get("MemTotal"), memory.get("MemAvailable")
        swap_total, swap_free = memory.get("SwapTotal"), memory.get("SwapFree")
        add({"memory_total_bytes": total, "memory_available_bytes": available, "memory_pressure": 1 - available / total if total and available is not None else None, "swap_total_bytes": swap_total, "swap_used_bytes": swap_total - swap_free if swap_total is not None and swap_free is not None else None}, "linux.proc.meminfo")
        add({"memory_psi_some_avg10": parse_psi(_read(self.proc / "pressure/memory"))}, "linux.proc.pressure.memory")
        psi_io = parse_psi(_read(self.proc / "pressure/io"))
        add({"io_pressure": psi_io if psi_io is not None else iowait}, "linux.proc.pressure.io" if psi_io is not None else "linux.proc.stat.iowait_proxy")
        frequencies = [float(match) for match in re.findall(r"^cpu MHz\s*:\s*([\d.]+)", _read(self.proc / "cpuinfo"), re.MULTILINE)]
        add({"cpu_frequency_mhz": sum(frequencies) / len(frequencies) if frequencies else None}, "linux.proc.cpuinfo.numeric_frequency")
        vm = re.search(r"^pgmajfault\s+(\d+)", _read(self.proc / "vmstat"), re.MULTILINE)
        add({"major_page_faults": int(vm.group(1)) if vm else None}, "linux.proc.vmstat")
        uptime = _read(self.proc / "uptime").split()
        add({"uptime_s": uptime[0] if uptime else None}, "linux.proc.uptime")
        temps = []
        for chip in (self.sys_root / "class/hwmon").glob("hwmon*"):
            if _read(chip / "name").strip() not in {"coretemp", "k10temp", "zenpower", "cpu_thermal"}:
                continue
            for path in chip.glob("temp*_input"):
                value = finite_number(_read(path).strip())
                if value is not None and -20000 < value < 200000:
                    temps.append(value / 1000)
        add({"cpu_temperature_c": max(temps) if temps else None}, "linux.sysfs.hwmon.cpu_max")
        throttle = []
        for path in (self.sys_root / "devices/system/cpu").glob("cpu[0-9]*/thermal_throttle/core_throttle_count"):
            value = finite_number(_read(path).strip())
            if value is not None:
                throttle.append(value)
        add({"cpu_throttling_count": sum(throttle) if throttle else None}, "linux.sysfs.core_throttle_count_sum")
        disks = self._disk_snapshot()
        common = set(disks) & set(self._last_disks)
        deltas = [tuple(a - b for a, b in zip(disks[d], self._last_disks[d])) for d in common]
        valid_disks = elapsed > 0 and deltas and all(v >= 0 for delta in deltas for v in delta)
        add({"disk_read_bytes_s": sum(delta[0] for delta in deltas) / elapsed if valid_disks else None, "disk_write_bytes_s": sum(delta[1] for delta in deltas) / elapsed if valid_disks else None, "disk_busy_fraction": min(1.0, max(delta[2] for delta in deltas) / (elapsed * 1000)) if valid_disks else None}, "linux.proc.diskstats.whole_devices")
        try:
            disk = os.statvfs("/")
            add({"disk_free_bytes": disk.f_bavail * disk.f_frsize, "disk_total_bytes": disk.f_blocks * disk.f_frsize}, "posix.statvfs.root")
        except OSError:
            pass
        net = parse_network(_read(self.proc / "net/dev"))
        add({"network_rx_bytes_s": max(0.0, net[0] - self._last_net[0]) / elapsed if elapsed > 0 else None, "network_tx_bytes_s": max(0.0, net[1] - self._last_net[1]) / elapsed if elapsed > 0 else None}, "linux.proc.net.dev.aggregate")
        add(self._gpu_metrics(), "nvidia-smi.query-gpu")
        add(ping_peer(self.peer_host), "icmp.one_echo")
        # Includes spawned sensor helpers (nvidia-smi and ping), as cumulative child CPU deltas.
        usage = resource.getrusage(resource.RUSAGE_SELF)
        children = resource.getrusage(resource.RUSAGE_CHILDREN)
        process_cpu = usage.ru_utime + usage.ru_stime + children.ru_utime + children.ru_stime
        add({"daemon_cpu_fraction": max(0.0, process_cpu - self._last_process_cpu) / elapsed if elapsed > 0 and self.sequence else None, "daemon_rss_bytes": usage.ru_maxrss * 1024}, "posix.getrusage.peak_rss_self_plus_child_cpu")
        self._last_time, self._last_cpu, self._last_net, self._last_disks, self._last_process_cpu = monotonic, ticks, net, disks, process_cpu
        record = {"schema_version": SCHEMA_ID, "source_kind": "real", "node_id": "master", "sequence": self.sequence, "timestamp": now, "monotonic_s": monotonic, "sampling_interval_s": self.interval, "metrics": metrics, "quality": {key: float(value is not None) for key, value in metrics.items()}, "provenance": provenance}
        self.sequence += 1
        return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--peer-host")
    parser.add_argument("--interval", type=float, default=1.0)
    parser.add_argument("--duration", type=float, default=60.0)
    parser.add_argument("--samples", type=int)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("master daemon requires Linux; use mac/k-sense-mac on macOS")
    if args.duration <= 0 or (args.samples is not None and args.samples <= 0):
        parser.error("duration and samples must be positive")
    sensor = LinuxSensor(args.peer_host, args.interval)
    stopped = False
    def stop(_signum, _frame):
        nonlocal stopped
        stopped = True
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    stream = args.output.open("a", buffering=1) if args.output else sys.stdout
    start = time.monotonic()
    deadline = start
    try:
        while not stopped and time.monotonic() - start < args.duration and (args.samples is None or sensor.sequence < args.samples):
            if time.monotonic() < deadline:
                time.sleep(min(0.1, deadline - time.monotonic()))
                continue
            stream.write(json.dumps(sensor.collect(), allow_nan=False, separators=(",", ":")) + "\n")
            stream.flush()
            deadline += args.interval
            if deadline < time.monotonic():
                deadline = time.monotonic()
    finally:
        if args.output:
            stream.close()


if __name__ == "__main__":
    main()
