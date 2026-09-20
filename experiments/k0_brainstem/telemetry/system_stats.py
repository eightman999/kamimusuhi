"""Read hardware measurements, without assumptions about GPU index or availability."""
import csv
import io
import shutil
import subprocess
import time
from pathlib import Path

_previous_cpu = None


def number(value):
    try:
        return float(value)
    except (ValueError, TypeError):
        return None


def snapshot():
    global _previous_cpu
    result = {"timestamp": time.time(), "cpu_percent": None, "ram_percent": None,
              "ram_used_bytes": None, "ram_total_bytes": None, "gpus": [], "gpu_processes": []}
    try:
        import psutil
        result["cpu_percent"] = psutil.cpu_percent(interval=0.1)
        ram = psutil.virtual_memory()
        result.update(ram_percent=ram.percent, ram_used_bytes=ram.used, ram_total_bytes=ram.total)
    except ImportError:
        try:
            counters = [int(v) for v in Path("/proc/stat").read_text().splitlines()[0].split()[1:]]
            total, idle = sum(counters[:8]), counters[3] + counters[4]
            if _previous_cpu is not None and total > _previous_cpu[0]:
                result["cpu_percent"] = 100 * (1 - (idle-_previous_cpu[1])/(total-_previous_cpu[0]))
            _previous_cpu = (total, idle)
        except (OSError, ValueError, IndexError):
            pass
        try:
            mem = {line.split(":")[0]: int(line.split()[1]) * 1024
                   for line in Path("/proc/meminfo").read_text().splitlines()}
            total, available = mem["MemTotal"], mem["MemAvailable"]
            result.update(ram_total_bytes=total, ram_used_bytes=total-available,
                          ram_percent=100 * (1-available/total))
        except (OSError, ValueError, KeyError):
            pass
    if shutil.which("nvidia-smi"):
        try:
            response = subprocess.run(["nvidia-smi", "--query-gpu=index,uuid,name,memory.total,memory.used,memory.free,utilization.gpu,temperature.gpu", "--format=csv,noheader,nounits"], capture_output=True, text=True, timeout=3, check=True)
            for row in csv.reader(io.StringIO(response.stdout), skipinitialspace=True):
                if len(row) == 8:
                    result["gpus"].append(dict(index=int(row[0]), uuid=row[1], name=row[2],
                        memory_total_mb=number(row[3]), memory_used_mb=number(row[4]),
                        memory_free_mb=number(row[5]), utilization_percent=number(row[6]),
                        temperature_c=number(row[7])))
            processes = subprocess.run(["nvidia-smi", "--query-compute-apps=gpu_uuid,pid,used_memory", "--format=csv,noheader,nounits"], capture_output=True, text=True, timeout=3, check=True)
            for row in csv.reader(io.StringIO(processes.stdout), skipinitialspace=True):
                if len(row) == 3:
                    result["gpu_processes"].append({"gpu_uuid": row[0], "pid": int(row[1]), "memory_used_mb": number(row[2])})
        except (OSError, subprocess.SubprocessError, ValueError) as error:
            result["gpu_error"] = type(error).__name__
    return result
