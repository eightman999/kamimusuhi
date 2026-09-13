import numpy as np

from experiments.g0_v6.src.health import latent_statistics
from experiments.g0_v6.src.retrieval import retrieval_from_latent


def test_low_rank_is_diagnostic_not_automatic_failure():
    values = np.arange(128, dtype=np.float64)[:, None] * np.linspace(1, 2, 8)[None, :]
    stats = latent_statistics(values)
    assert stats["dimensional_collapse_diagnostic"]
    assert stats["failure_class"] == "NONE"


def test_full_collapse_is_failure():
    stats = latent_statistics(np.ones((32, 8)))
    assert stats["full_collapse"]
    assert stats["failure_class"] == "COLLAPSE_FULL"


def test_temporal_retrieval_returns_fixed_metrics():
    rng = np.random.default_rng(9)
    latent = rng.normal(size=(6, 12, 5))
    result = retrieval_from_latent(latent, [1, 2], stride=2, max_episodes=6)
    assert result["n_queries"] > 0
    assert 0 <= result["recall_at_1"] <= 1
    assert set(result["by_horizon"]) == {"1", "2"}
