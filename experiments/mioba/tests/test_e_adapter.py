from experiments.mioba.mie.e_adapter import (E_ACTIONS, SENSOR_NAMES,
                                             ChannelMapping, EStyleEvent,
                                             EStreamToMieAdapter,
                                             FbaOutputToEActionAdapter,
                                             MieToFbaInputAdapter)


def _events():
    return [EStyleEvent(t=i, channel=ch, value=1.0)
            for i, ch in enumerate(SENSOR_NAMES)]


def test_e_to_mie_roundtrip():
    evs = EStreamToMieAdapter().to_sensor_events(_events())
    assert len(evs) == len(SENSOR_NAMES)
    assert all(e.domain == "synthetic" and e.source == "e-series"
               for e in evs)
    assert {e.signal_type for e in evs} == set(SENSOR_NAMES)


def test_mie_to_drive_disjoint_channels():
    mapping = ChannelMapping(n_neurons=160)
    adapter = MieToFbaInputAdapter(mapping)
    evs = EStreamToMieAdapter().to_sensor_events(_events())
    drive = adapter.to_input_drive(evs)
    assert len(drive["rates_hz"]) == len(SENSOR_NAMES)
    bounds = sorted(tuple(int(x) for x in k.split(":")[1].split("-"))
                    for k in drive["rates_hz"])
    for (a1, b1), (a2, b2) in zip(bounds, bounds[1:]):
        assert b1 <= a2


def test_shuffled_differs_from_correct():
    mapping = ChannelMapping(n_neurons=160)
    evs = EStreamToMieAdapter().to_sensor_events(
        [EStyleEvent(t=i, channel=ch, value=float(i) + 1)
         for i, ch in enumerate(SENSOR_NAMES)])
    correct = MieToFbaInputAdapter(mapping).to_input_drive(evs)
    shuffled = MieToFbaInputAdapter(mapping, shuffle=True).to_input_drive(evs)
    assert correct["rates_hz"] != shuffled["rates_hz"]


def test_delay_steps_queue():
    mapping = ChannelMapping(n_neurons=160)
    adapter = MieToFbaInputAdapter(mapping, delay_steps=2)
    evs = EStreamToMieAdapter().to_sensor_events(_events())
    d0 = adapter.to_input_drive(evs)
    d1 = adapter.to_input_drive(evs)
    d2 = adapter.to_input_drive(evs)
    assert d0["rates_hz"] == {} and d1["rates_hz"] == {}
    assert d2["rates_hz"] != {}


def test_fba_to_action():
    adapter = FbaOutputToEActionAdapter()
    acts = adapter.to_action({"all": [0.001, 0.2]})
    assert len(acts) == 2
    assert all(0 <= a < len(E_ACTIONS) for a in acts)
