from pathlib import Path

from experiments.g0_v6.src.protocol import same_metrics, source_files


def test_scientific_comparison_ignores_runtime_only():
    a = {"score": 0.5, "runtime": {"evaluation_seconds": 1.0}}
    b = {"score": 0.5, "runtime": {"evaluation_seconds": 9.0}}
    assert same_metrics(a, b)
    assert not same_metrics({"score": 0.5}, {"score": 0.6})


def test_protocol_snapshot_excludes_outputs():
    paths = [str(path) for path in source_files(Path(__file__).parents[1])]
    assert not any("results/" in path or "data/" in path for path in paths)
    assert not any(path.endswith("G0_V6_REPORT.md") for path in paths)
