from experiments.mioba.mie.collectors import (CpuCollector, Normalizer,
                                              RamCollector)
from experiments.mioba.mie.environments import (known_environments,
                                                make_drive)
from experiments.mioba.mie.events import SensorEvent
from experiments.mioba.mie.registry import load_collectors


def test_collectors_wellformed_without_nvidia_smi():
    for c in (CpuCollector(), RamCollector()):
        for ev in c.collect():
            assert ev.source and ev.domain == "machine_interoception"
            assert ev.signal_type and ev.confidence >= 0


def test_normalizer_delta_and_scale():
    n = Normalizer()
    e1 = n.apply(SensorEvent(timestamp="t", source="s",
                             domain="machine_interoception",
                             signal_type="temperature_c", value=50.0))
    assert e1.delta is None and e1.normalized_value == 0.5
    e2 = n.apply(SensorEvent(timestamp="t", source="s",
                             domain="machine_interoception",
                             signal_type="temperature_c", value=60.0))
    assert e2.delta == 10.0 and e2.normalized_value == 0.6


def test_load_collectors_config():
    cols = load_collectors({"mie": {"collectors": ["cpu", "ram"]}})
    assert {c.name for c in cols} == {"cpu", "ram"}
    evs = cols[0].collect()
    assert evs and evs[0].normalized_value is not None


def test_environment_drive():
    drive = make_drive("synthetic-quiet-v0", 1000,
                       {"env": {"stim_fraction": 0.01, "stim_rate_hz": 50}})
    assert drive["rates_hz"] == {"slice:0-10": 50.0}
    assert "synthetic-quiet-v0" in known_environments()
