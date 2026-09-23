
import numpy as np

from experiments.g0_v6.src.evaluate import evaluate_representation


def _fixture(root):
    evaluation = root / "evaluation"
    evaluation.mkdir()
    rng = np.random.default_rng(12)
    n, length, dim = 8, 8, 4
    observations = rng.normal(size=(n, length, dim)).astype(np.float32)
    causes = np.arange(n)[:, None] % 4 * np.ones((1, length), dtype=np.int8)
    contexts = (np.arange(n)[:, None] % 2) * np.ones((1, length), dtype=np.int8)
    labels = np.array([1, 0, 1, 0], dtype=np.int8)
    pairs = np.array([[0, 1], [2, 3], [4, 5], [6, 7]])
    for name in ("iid_test", "match_ood", "dynseg_ood", "combo_oodctx"):
        np.savez(
            evaluation / f"{name}.npz",
            obs=observations,
            cause=causes,
            context=contexts,
            pair_index=pairs,
            pair_label=labels,
        )
    mid_context = contexts.copy()
    mid_context[:, 4:] = 1 - mid_context[:, :1]
    np.savez(evaluation / "midctx.npz", obs=observations, cause=causes, context=mid_context, switch_time=4)
    nuisance = rng.normal(size=(n, length, 2)).astype(np.float32)
    noise = rng.normal(size=observations.shape).astype(np.float32)
    phase = rng.normal(size=(n, 2)).astype(np.float32)
    groups = np.arange(n).reshape(-1, 1)[:2] * 4 + np.arange(4)
    np.savez(
        evaluation / "intervention.npz",
        obs=np.concatenate([observations[:2] for _ in range(4)]),
        cause=np.concatenate([causes[:2] for _ in range(4)]),
        context=np.concatenate([contexts[:2] for _ in range(4)]),
        nuisance=np.concatenate([nuisance[:2] for _ in range(4)]),
        sensor_noise=np.concatenate([noise[:2] for _ in range(4)]),
        phase=np.concatenate([phase[:2] for _ in range(4)]),
        intervention_index=groups,
    )


def test_evaluation_is_repeatable(tmp_path):
    _fixture(tmp_path)

    def encode(observations):
        return observations

    first = evaluate_representation(encode, tmp_path)
    second = evaluate_representation(encode, tmp_path)
    first.pop("runtime")
    second.pop("runtime")
    assert first == second
    assert first["failure_class"] in {"NONE", "SHORTCUT_CONTEXT"}
