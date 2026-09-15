"""Frozen synthetic generator. Oracle-bearing files are evaluator-only.

No oracle is exposed by the strict training loader. Seed identities and hashes are
recorded before fitting. Observation coordinates mix cause, context, nuisance and
dynamic phase; no coordinate is a class label.
"""
from pathlib import Path
import argparse
import hashlib
import json
import numpy as np

VERSION = 'g0-v5-synthetic-1'
N_CAUSES, N_CONTEXTS = 8, 4


def load_training(path):
    with np.load(path, allow_pickle=False) as data:
        if set(data.files) != {'obs'}:
            raise ValueError('DATA_LEAK: training archive must contain only obs')
        obs = data['obs'].copy()
    if obs.ndim != 3 or not np.isfinite(obs).all():
        raise ValueError('invalid observation tensor')
    return obs


def generate_dataset(output: Path, seed=20260913, n_train=1024, n_eval=256,
                     length=48, obs_dim=24):
    output = Path(output)
    if output.exists() and any(output.iterdir()):
        raise FileExistsError('dataset output must be empty; never overwrite a frozen dataset')
    if length < 16 or obs_dim != 24 or n_eval < 24:
        raise ValueError('require length >= 16, obs_dim=24, n_eval >= 24')
    output.mkdir(parents=True, exist_ok=True)
    rng = np.random.default_rng(seed)
    # Fixed random mechanisms shared across every split, never fitted from labels.
    causes = rng.normal(size=(N_CAUSES, 6))
    rotations = np.stack([np.linalg.qr(rng.normal(size=(6, 6)))[0] for _ in range(N_CONTEXTS)])
    mix = rng.normal(size=(18, obs_dim)) / np.sqrt(18)
    offsets = rng.normal(scale=.7, size=(N_CONTEXTS, 6))
    allowed = [(c, k) for c in range(N_CAUSES) for k in range(N_CONTEXTS) if c % 4 != k]

    def render(c, k, noise, nuisance):
        phase = np.sin(np.arange(length)[:, None] * np.array([.13, .23]) + noise['phase'])
        body = np.einsum('tij,tj->ti', rotations[k], causes[c]) + offsets[k]
        # Shared cause signal makes cross-context identity identifiable in principle.
        # Context-dependent sensor components are equally represented, not labels.
        features = np.concatenate([causes[c], body, nuisance, phase], axis=1)
        return (np.tanh(features @ mix) + noise['sensor']).astype(np.float32)

    def exogenous():
        return {'phase': rng.uniform(-np.pi, np.pi, size=2),
                'sensor': rng.normal(scale=.035, size=(length, obs_dim))}

    def nuisance():
        innovations = rng.normal(scale=.25, size=(length, 4))
        for t in range(1, length):
            innovations[t] += .65 * innovations[t-1]
        return innovations

    def trajectory(c, k, dynamic=False, iid=False):
        cs, ks = np.full(length, c), np.full(length, k)
        if iid:
            for t in range(0, length - 12, 12):
                cs[t:t+12] = rng.choice([v for v in range(8) if v % 4 != k])
        if dynamic:
            # Variable 3..17-step segments; final segment is >= 5 steps so
            # endpoint identity is not merely an instantaneous switch transient.
            stop = length - 5
            t = 0
            while t < stop:
                end = min(stop, t + int(rng.integers(3, 18)))
                cs[t:end] = rng.choice([v for v in range(N_CAUSES) if v % 4 != k])
                t = end
        return render(cs, ks, exogenous(), nuisance()), cs, ks

    def save(name, **arrays):
        folder = output / ('evaluation' if name.startswith('eval_') else 'training')
        folder.mkdir(exist_ok=True)
        np.savez_compressed(folder / name.removeprefix('eval_'), **arrays)

    for name, n in [('train', n_train), ('validation', max(32, n_train // 8))]:
        observations = []
        for _ in range(n):
            cs, ks = np.empty(length, int), np.empty(length, int)
            k = int(rng.integers(4))
            for t in range(0, length, 12):
                c = int(rng.choice([v for v in range(8) if v % 4 != k]))
                cs[t:t+12], ks[t:t+12] = c, k
            observations.append(render(cs, ks, exogenous(), nuisance()))
        save(name + '.npz', obs=np.stack(observations))

    # Every ordered context pair contributes one positive and one negative in
    # each block. Paired examples are independently simulated, including phase
    # and noise. A context-only embedding therefore has exactly AUC=.5.
    pairs = [(a, b) for a in range(4) for b in range(4) if a != b]
    pair_count = max(96, ((n_eval + 95) // 96) * 96)
    for split in ['iid_test', 'match_ood', 'dynseg_ood', 'combo_oodctx']:
        obs, cs_all, ks_all, labels = [], [], [], []
        for i in range(pair_count // 2):
            ka, kb = pairs[i % len(pairs)]
            candidates = ([c for c in range(8) if c % 4 == ka]
                          if split == 'combo_oodctx' else
                          [c for c in range(8) if c % 4 not in (ka, kb)])
            slot = (i // len(pairs)) % len(candidates)
            ca = candidates[slot]
            for same in [True, False]:
                cb = ca if same else candidates[(slot + 1) % len(candidates)]
                for c, k in [(ca, ka), (cb, kb)]:
                    x, cs, ks = trajectory(c, k, split == 'dynseg_ood', split == 'iid_test')
                    obs.append(x); cs_all.append(cs); ks_all.append(ks)
                labels.append(int(same))
        save('eval_' + split + '.npz', obs=np.stack(obs), cause=np.stack(cs_all),
             context=np.stack(ks_all), pair_index=np.arange(2*pair_count).reshape(-1, 2),
             pair_label=np.array(labels, dtype=np.int8))

    obs, cs_all, ks_all = [], [], []
    for _ in range(n_eval):
        c = int(rng.integers(8))
        ka, kb = rng.choice([k for k in range(4) if k != c % 4], 2, replace=False)
        cs = np.full(length, c); ks = np.full(length, ka); ks[length//2:] = kb
        obs.append(render(cs, ks, exogenous(), nuisance())); cs_all.append(cs); ks_all.append(ks)
    save('eval_midctx.npz', obs=np.stack(obs), cause=np.stack(cs_all), context=np.stack(ks_all),
         switch_time=np.array(length//2))

    obs, cs_all, ks_all, nuis_all, noise_all, phase_all = [], [], [], [], [], []
    for _ in range(n_eval):
        c, k = allowed[int(rng.integers(len(allowed)))]; c2 = (c + int(rng.integers(1, 8))) % 8
        k2 = (k + int(rng.integers(1, 4))) % 4
        noise, nu, nu2 = exogenous(), nuisance(), nuisance()
        for ci, ki, ni in [(c,k,nu), (c2,k,nu), (c,k2,nu), (c,k,nu2)]:
            cs, ks = np.full(length, ci), np.full(length, ki)
            obs.append(render(cs, ks, noise, ni)); cs_all.append(cs); ks_all.append(ks); nuis_all.append(ni); noise_all.append(noise['sensor']); phase_all.append(noise['phase'])
    save('eval_intervention.npz', obs=np.stack(obs), cause=np.stack(cs_all), context=np.stack(ks_all),
         nuisance=np.stack(nuis_all), sensor_noise=np.stack(noise_all), phase=np.stack(phase_all), intervention_index=np.arange(n_eval*4).reshape(-1,4))
    files = {str(p.relative_to(output)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(output.rglob('*.npz'))}
    manifest = dict(generator_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(), dataset_version=VERSION, seed=seed, n_train=n_train, n_eval=n_eval,
                    effective_matching_pairs=pair_count, length=length, obs_dim=obs_dim,
                    training_keys=['obs'], oracle_isolation='evaluation/ only; trainer is passed training/ only',
                    heldout_rule='cause % 4 == context', files=files,
                    mechanism='six shared cause features plus six context-rotated cause features, four nuisance features, two dynamic phase features, fixed dense mixing and tanh',
                    train_schedule='context fixed per episode; independently sampled allowed cause every 12 steps',
                    midctx_schedule='fixed cause with context switch at half episode; both contexts are allowed for cause',
                    dynseg_schedule='context fixed and allowed cause support unchanged; variable 3..17-step segments, final segment 5 steps',
                    matching_protocol='iid_test has train schedule; match_ood uses fixed-cause independent trajectories across distinct contexts, never temporally paired in training; context pair frequencies exactly balanced across same/different labels',
                    limitation='Synthetic identity is identifiable in principle because of shared cause channels; this does not demonstrate grounding in real sensors. Matching is conditioned on eligible cross-context endpoint causes. IID refers to rollout schedule and allowed support, not unconditioned endpoint frequency. All distance evaluation is label-free; oracle only determines pair labels.')
    (output/'dataset_manifest.json').write_text(json.dumps(manifest, indent=2)+'\n')
    return manifest


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--seed', type=int, default=20260913)
    parser.add_argument('--n-train', type=int, default=1024)
    parser.add_argument('--n-eval', type=int, default=256)
    args = parser.parse_args()
    generate_dataset(args.output, args.seed, args.n_train, args.n_eval)
