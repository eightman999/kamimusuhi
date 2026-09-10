"""All episode/time/environment dimensions generated with tensor operations.

Labels live separately from observations. In particular repetition, novelty,
and time_since_event are raw proxy readings, never teacher target encodings.
"""
import torch
from .signals import IGNORE, WAIT, ORIENT, OBSERVE, RECALL, INVOKE_LANGUAGE


def generate(num_envs, episode_length, device, generator, ood=None, cue_override=None):
    aliases = {
        "noise_variance": "noise", "sensor_dropout": "dropout",
        "sensor_inversion": "inversion", "long_delay": "longer_delay",
        "resource_drop": "resource", "sensor_failure": "failure",
        "sensor_fault": "failure", "unknown_combination": "combination",
    }
    ood_name = ood.get("kind", "all") if isinstance(ood, dict) else ood
    ood_name = aliases.get(ood_name, ood_name)
    if ood_name not in (None, "noise", "dropout", "inversion", "longer_delay",
                         "resource", "failure", "combination", "all"):
        raise ValueError(f"Unknown OOD mode: {ood_name}")
    n, length = num_envs, episode_length
    if length < 32:
        raise ValueError("episode_length must be >=32 for >=24-step retention")
    rand = lambda *shape: torch.rand(shape, device=device, generator=generator)
    randint = lambda low, high, shape: torch.randint(low, high, shape, device=device, generator=generator)
    scenario = randint(0, 8, (n,))
    t = torch.arange(length, device=device)[:, None]
    obs = rand(length, n, 16) * 0.08
    obs[..., 3] += .4
    obs[..., 12] = .95
    obs[..., 13] = .9
    targets = torch.full((length, n), IGNORE, dtype=torch.long, device=device)
    retention = torch.zeros((length, n), dtype=torch.bool, device=device)
    habituation = torch.zeros_like(retention)
    novelty = torch.zeros_like(retention)
    start = randint(3, 10, (n,))
    phase = t - start

    def put(mask, features):
        for feature, value in features.items():
            obs[..., feature] = torch.where(mask, torch.as_tensor(value, device=device), obs[..., feature])

    def target(mask, action):
        nonlocal targets
        targets = torch.where(mask, torch.as_tensor(action, device=device), targets)

    # Identical repeated pulses require remembering whether one occurred before.
    rep = (scenario == 1)[None, :]
    pulses = rep & (phase >= 0) & (phase % 4 == 0) & (phase < 24)
    put(pulses, {0: .8, 2: .6, 10: .65, 11: 0.})
    target(pulses & (phase == 0), ORIENT)
    habituation |= pulses & (phase > 0)
    renewed = rep & (phase == 28)
    put(renewed, {0: .8, 2: .6, 5: .95, 10: .9})
    target(renewed, ORIENT)
    novelty |= renewed

    sudden = (scenario == 2)[None, :] & ((phase == 0) | (phase == 1))
    put(sudden, {0: .9, 1: .9, 2: .7, 10: .95, 15: .9})
    target(sudden & (phase == 0), ORIENT)
    target(sudden & (phase == 1), OBSERVE)
    novelty |= sudden

    # Cue identity is visible ONLY at t=0. Delay and decision observation do not
    # depend on identity, so a stateless policy has chance conditional accuracy.
    delayed = (scenario == 3)[None, :]
    cue = randint(0, 2, (n,))
    if cue_override is not None:
        override = torch.as_tensor(cue_override, device=device, dtype=torch.long)
        if override.shape != (n,) or not bool(((override == 0) | (override == 1)).all()):
            raise ValueError("cue_override must contain num_envs binary labels")
        cue = override.clone()
    high = min(41, length - 2)
    delay = randint(24, high, (n,))
    if ood_name in ("longer_delay", "all"):
        delay = randint(min(48, length - 3), length - 1, (n,))
    put(delayed & (t == 0), {4: .8, 5: .15 + cue[None, :] * .7, 14: .7})
    target(delayed, WAIT)
    decision = delayed & (t == delay[None, :])
    put(decision, {4: .8, 5: .4, 14: .9, 15: .6})
    target(decision, torch.where(cue[None, :] == 1, RECALL, OBSERVE))
    retention |= decision

    conflict = (scenario == 4)[None, :] & (phase >= 0) & (phase < 12)
    pressure_high = (phase % 4 < 2)
    put(conflict, {10: .85, 14: torch.where(pressure_high, .95, .25), 0: .75})
    target(conflict & pressure_high, RECALL)
    target(conflict & ~pressure_high, OBSERVE)

    # True/false language examples have identical currently active speech
    # observations. Only consecutive previous speech distinguishes the gate.
    true_lang = (scenario == 5)[None, :]
    false_lang = (scenario == 7)[None, :]
    event_phase = phase % 8
    active_period = (phase >= 0) & (phase < length - 12)
    speech = active_period & ((true_lang & (event_phase < 4)) | (false_lang & (event_phase % 2 == 0)))
    put(speech, {7: .9, 8: .9, 2: .65, 14: .8, 4: .6})
    target(speech, OBSERVE)
    target(true_lang & active_period & ((event_phase == 2) | (event_phase == 3)), INVOKE_LANGUAGE)

    anomaly = (scenario == 6)[None, :] & (phase >= 0) & (phase < 16)
    put(anomaly, {6: .8, 15: .8, 12: .65})
    target(anomaly & (phase < 3), OBSERVE)
    target(anomaly & (phase >= 3), RECALL)

    # OOD changes observations while retaining physical event truth.
    if ood_name in ("noise", "noise_variance", "all"):
        obs += (rand(length, n, 16) - .5) * .3
    if ood_name in ("dropout", "sensor_dropout", "all"):
        obs *= (rand(length, n, 16) > .1)
    if ood_name in ("inversion", "sensor_inversion", "all"):
        invert = rand(1, n, 16) < .1
        obs = torch.where(invert, 1 - obs, obs)
    if ood_name in ("resource", "resource_drop", "all"):
        obs[..., 13] = torch.where(t >= length // 2, .05, obs[..., 13])
    if ood_name in ("failure", "sensor_failure", "all"):
        obs[..., 1] = 0
        obs[..., 3] = .5
    if ood_name in ("combination", "unknown_combination", "all"):
        put((phase % 11 == 0), {1: .8, 3: .2})
    return dict(observations=obs.clamp_(0, 1), targets=targets, scenario=scenario,
                retention_mask=retention, habituation_mask=habituation,
                novelty_mask=novelty, first_stimulus_mask=pulses & (phase == 0),
                renewed_stimulus_mask=renewed, cue=cue, delay=delay)
