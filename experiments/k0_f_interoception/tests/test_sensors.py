import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from experiments.k0_f_interoception import sensors


class SensorTests(unittest.TestCase):
    def test_cpu_guest_not_double_counted_and_reset_is_missing(self):
        ticks, queue = sensors.parse_cpu_stat("cpu 100 10 30 200 20 5 5 0 99 9\nprocs_running 3\n")
        self.assertEqual(len(ticks), 8)
        self.assertEqual(queue, 3)
        busy, waiting = sensors.cpu_fractions([0] * 8, ticks)
        self.assertAlmostEqual(busy, 150 / 370)
        self.assertAlmostEqual(waiting, 20 / 370)
        self.assertEqual(sensors.cpu_fractions(ticks, [0] * 8), (None, None))
        self.assertEqual(sensors.cpu_fractions(ticks, ticks), (None, None))

    def test_meminfo_units_and_pressure(self):
        parsed = sensors.parse_meminfo("MemTotal: 1024 kB\nMemAvailable: 768 kB\nHugePages_Total: 0\ninvalid\n")
        self.assertEqual(parsed["MemTotal"], 1024 ** 2)
        self.assertEqual(parsed["MemAvailable"], 768 * 1024)
        self.assertEqual(sensors.parse_psi("some avg10=12.50 avg60=3.0 total=10\nfull avg10=1.0"), .125)
        self.assertIsNone(sensors.parse_psi("not available"))

    def test_gpu_numeric_allowlist_filters_identity_and_missing(self):
        rows = "NVIDIA GeForce RTX 3060, 42, 20, 3, 100, 12288, 34.5, 170, 1200, 7000, 0x0000000000000001\nTesla P100-PCIE-16GB, 45, 0, 0, 50, 16384, [N/A], 250, 405, 715, [Not Supported]\nUnknown GPU, 55, 2, 3, 100, 1000, 5, 10, 100, 100\n"
        data = sensors.parse_gpu_csv(rows)
        self.assertAlmostEqual(data["rtx3060_utilization"], .2)
        self.assertEqual(data["rtx3060_vram_used_bytes"], 100 * 1024 ** 2)
        self.assertEqual(data["rtx3060_throttle_reason_bits"], 1)
        self.assertIsNone(data["p100_power_w"])
        self.assertIsNone(data["p100_throttle_reason_bits"])
        self.assertTrue(set(data) <= sensors.METRIC_ALLOWLIST)
        self.assertNotIn("NVIDIA", json.dumps(data))

    def test_disk_partitions_excluded_and_loopback_excluded(self):
        text = "259 0 nvme0n1 1 0 10 0 2 0 20 0 0 30 30\n259 1 nvme0n1p1 1 0 10 0 2 0 20 0 0 30 30\n"
        parsed = sensors.parse_disks(text, {"nvme0n1"})
        self.assertEqual(parsed, {"nvme0n1": (5120, 10240, 30)})
        net = "lo: 500 0 0 0 0 0 0 0 500 0 0 0 0 0 0 0\nprivate_interface: 100 0 0 0 0 0 0 0 200 0 0 0 0 0 0 0"
        self.assertEqual(sensors.parse_network(net), (100, 200))

    def test_ping_no_target_is_missing_not_healthy(self):
        self.assertEqual(sensors.ping_peer(None), {"network_rtt_ms": None, "network_loss": None, "network_connectivity": None})

    def test_collect_missing_hardware_masks_and_never_leaks_filenames(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            proc, sys_root = root / "proc", root / "sys"
            proc.mkdir(); sys_root.mkdir()
            (proc / "stat").write_text("cpu 1 2 3 4 5 0 0 0\nprocs_running 2\n")
            (proc / "meminfo").write_text("MemTotal: 100 kB\nMemAvailable: 40 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n")
            (proc / "uptime").write_text("123 20\n")
            with patch.object(sensors.LinuxSensor, "_gpu_metrics", return_value={}):
                sensor = sensors.LinuxSensor(proc=proc, sys_root=sys_root)
                record = sensor.collect()
            self.assertEqual(record["node_id"], "master")
            self.assertEqual(record["source_kind"], "real")
            self.assertEqual(record["metrics"]["memory_pressure"], .6)
            self.assertIsNone(record["metrics"]["cpu_temperature_c"])
            self.assertEqual(record["quality"]["cpu_temperature_c"], 0)
            self.assertEqual(set(record["metrics"]), sensors.METRIC_ALLOWLIST)
            encoded = json.dumps(record, allow_nan=False)
            self.assertNotIn(directory, encoded)
            self.assertNotIn("cmdline", encoded)
            self.assertNotIn("hostname", encoded)


if __name__ == "__main__":
    unittest.main()
