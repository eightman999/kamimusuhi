import copy
import json
import math
import unittest

from experiments.k0_f_interoception.normalize import (
    DEFAULT_CONFIG, FRAME_NAMES, align_records, canonical_identity, normalize_pair, policy_input,
)


def raw(node, metrics, timestamp=100, receipt=None):
    result = {"schema_version": "k0f.raw.v1", "source_kind": "real", "node_id": node, "sequence": 4, "timestamp": timestamp, "metrics": metrics, "quality": {key: 1.0 for key in metrics}, "provenance": {key: "test.numeric_sensor" for key in metrics}}
    if receipt is not None:
        result["receipt_timestamp"] = receipt
    return result


def pair():
    master = raw("master", {"cpu_temperature_c": 57.5, "cpu_utilization": .3, "memory_pressure": .4, "io_pressure": .1,
        **{f"{gpu}_{key}": value for gpu in ("rtx3060", "p100") for key, value in (("temperature_c", 57.5), ("utilization", .8), ("vram_used_bytes", 100), ("vram_total_bytes", 200), ("power_w", 50), ("power_limit_w", 100))}})
    mac = raw("mac", {"thermal_pressure": 1 / 3, "cpu_utilization": .2, "memory_pressure": .6, "power_pressure": .1, "network_rtt_ms": 100, "network_loss": 0}, timestamp=99, receipt=99.5)
    return master, mac


class NormalizeTests(unittest.TestCase):
    def test_fixed_dimension_and_separate_mask_input(self):
        master, mac = pair()
        frame = normalize_pair(master, mac, now=100)
        self.assertEqual(len(FRAME_NAMES), 20)
        self.assertEqual(len(frame["values"]), 20)
        self.assertEqual(frame["values"][0], .5)
        self.assertEqual(frame["values"][6:8], [.5, .5])
        self.assertEqual(frame["mask"], [1.] * 20)
        self.assertEqual(frame["availability"], 1)
        self.assertEqual(len(policy_input(frame)), 40)
        self.assertEqual(frame["provenance"]["mac"]["receipt_timestamp"], 99.5)
        self.assertEqual(frame["provenance"]["mac"]["timestamp"], 99)
        self.assertEqual(frame["age_s"][12], 1)

    def test_missing_is_distinct_from_healthy_zero(self):
        master, mac = pair()
        mac["metrics"]["thermal_pressure"] = None
        frame = normalize_pair(master, mac)
        self.assertEqual(frame["values"][12], 0)
        self.assertEqual(frame["mask"][12], 0)
        self.assertEqual(frame["quality"][12], 0)
        self.assertEqual(frame["values"][17], 0)
        self.assertEqual(frame["mask"][17], 1)
        empty = normalize_pair(None, None, now=100)
        self.assertEqual(empty["values"][-2:], [1, 0])
        self.assertEqual(empty["mask"][:18], [0] * 18)
        self.assertEqual(empty["source_kind"], "missing")

    def test_stale_and_nonfinite_have_zero_quality(self):
        master, mac = pair()
        mac["timestamp"] = 0; mac["receipt_timestamp"] = 100
        master["metrics"]["cpu_utilization"] = math.nan
        master["quality"]["memory_pressure"] = 0
        frame = normalize_pair(master, mac, now=100)
        self.assertEqual(frame["mask"][1:3], [0, 0])
        self.assertEqual(frame["mask"][12:18], [0] * 6)
        json.dumps(frame, allow_nan=False)

    def test_clock_skew_does_not_become_negative_age(self):
        master, mac = pair()
        mac["timestamp"] = 1000; mac["receipt_timestamp"] = 99
        frame = normalize_pair(master, mac, now=100)
        self.assertEqual(frame["age_s"][12], 1)
        self.assertTrue(frame["provenance"]["mac"]["source_clock_future"])
        self.assertEqual(frame["provenance"]["mac"]["receipt_minus_source_s"], -901)

    def test_zero_denominator_and_negative_rtt_are_missing(self):
        master, mac = pair()
        master["metrics"]["rtx3060_vram_total_bytes"] = 0
        mac["metrics"]["network_rtt_ms"] = -5
        frame = normalize_pair(master, mac)
        self.assertEqual(frame["mask"][6], 0)
        self.assertEqual(frame["mask"][16], 0)

    def test_identity_and_provenance_stable_and_separated(self):
        self.assertEqual(canonical_identity(), canonical_identity(copy.deepcopy(DEFAULT_CONFIG)))
        self.assertNotEqual(canonical_identity(), canonical_identity({"max_age_s": 30}))
        master, mac = pair()
        mac["source_kind"] = "synthetic"
        original = copy.deepcopy((master, mac))
        aligned = align_records(master, mac)
        self.assertEqual(aligned["frame"]["source_kind"], "mixed")
        self.assertEqual(aligned["aligned"]["mac"], original[1])
        self.assertEqual(aligned["policy_input"]["values"], policy_input(aligned["frame"]))
        self.assertEqual((master, mac), original)
        self.assertNotIn("metrics", aligned["policy_input"])

    def test_unknown_schema_and_configuration_fail_closed(self):
        master, mac = pair()
        master["schema_version"] = "unversioned"
        with self.assertRaises(ValueError):
            normalize_pair(master, mac)
        with self.assertRaises(ValueError):
            normalize_pair(None, None, now=100, config={"frame_names": ["renamed"]})
        with self.assertRaises(ValueError):
            normalize_pair(None, None, now=100, config={"rtt_scale_ms": 0})

    def test_policy_input_rejects_inconsistent_masks(self):
        frame = normalize_pair(*pair())
        frame["mask"][0] = .5
        with self.assertRaises(ValueError):
            policy_input(frame)
        frame["mask"][0] = 0
        with self.assertRaises(ValueError):
            policy_input(frame)


if __name__ == "__main__":
    unittest.main()
