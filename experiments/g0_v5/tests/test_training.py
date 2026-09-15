from pathlib import Path

import numpy as np
import pytest
import torch
import yaml

from experiments.g0_v5.src.models import build_model
from experiments.g0_v5.src.losses import objective
from experiments.g0_v5.src.telemetry import latent_statistics
from experiments.g0_v5.src.train import load_encoder, load_observations, run


@pytest.mark.parametrize('method', ['gru', 'cpc', 'vicreg', 'jepa'])
def test_training_and_exact_twin(tmp_path, method):
    config = yaml.safe_load((Path(__file__).parents[1] / 'configs' / f'{method}.yaml').read_text())
    config.update(obs_dim=4, hidden_dim=8, latent_dim=4, horizons=[1, 2], batch_size=4, epochs=2)
    path = tmp_path / 'config.yaml'
    path.write_text(yaml.safe_dump(config))
    data = tmp_path / 'data'
    data.mkdir()
    rng = np.random.default_rng(21)
    observations = rng.normal(size=(8, 8, 4)).astype(np.float32)
    for name in ['train', 'validation']:
        np.savez(data / f'{name}.npz', obs=observations)
    output = tmp_path / 'run'
    result = run(path, data, output, seed=0)
    # Tiny fixtures may trigger the same strict rank gate as actual runs.
    assert result['status'] in {'complete', 'failed'}
    assert result['failure_class'] in {'NONE', 'COLLAPSE_DIM'}
    initialized = tmp_path / 'initialize_only'
    initial_result = run(path, data, initialized, seed=0, initialize_only=True)
    assert initial_result['status'] == 'initialized'
    assert initial_result['epochs_completed'] == 0
    assert not (initialized / 'checkpoint_best.pt').exists()
    initial_control = torch.load(initialized / 'checkpoint_initial.pt', weights_only=True)['state_dict']
    initial_run = torch.load(output / 'checkpoint_initial.pt', weights_only=True)['state_dict']
    assert all(torch.equal(value, initial_run[key]) for key, value in initial_control.items())
    assert np.isfinite(result['initial_heldout_ssl_loss'])
    assert result['best_heldout_ssl_loss'] <= result['final_heldout_ssl_loss']
    torch.manual_seed(0)
    exact_initial = build_model(config)
    saved = torch.load(output / 'checkpoint_initial.pt', weights_only=True)
    assert all(torch.equal(value, saved['state_dict'][key]) for key, value in exact_initial.state_dict().items())
    encoded = load_encoder(output / 'checkpoint_best.pt')(observations)
    repeated = load_encoder(output / 'checkpoint_best.pt')(observations)
    assert encoded.shape == (8, 8, 4)
    np.testing.assert_array_equal(encoded, repeated)
    assert not np.array_equal(encoded, load_encoder(output / 'checkpoint_initial.pt')(observations))
    assert len((output / 'train_log.jsonl').read_text().splitlines()) == result['epochs_completed']
    assert 1 <= result['epochs_completed'] <= 2
    with pytest.raises(FileExistsError):
        run(path, data, output)


def test_strict_loader_and_failure_preservation(tmp_path):
    path = tmp_path / 'invalid.npz'
    np.savez(path, obs=np.zeros((2, 3, 4)), forbidden=np.ones(2))
    with pytest.raises(ValueError, match='only obs'):
        load_observations(path)
    cfg = Path(__file__).parents[1] / 'configs' / 'gru.yaml'
    np.savez(tmp_path / 'train.npz', obs=np.zeros((2, 3, 4)), forbidden=np.ones(2))
    output = tmp_path / 'failed'
    with pytest.raises(ValueError):
        run(cfg, tmp_path, output)
    import json
    assert json.loads((output / 'manifest.json').read_text())['failure_class'] == 'DATA_LEAK'


def test_ema_targets_stop_gradient():
    cfg = yaml.safe_load((Path(__file__).parents[1] / 'configs' / 'jepa.yaml').read_text())
    model = build_model(cfg)
    loss, _ = objective(model, torch.randn(4, 8, 24), cfg)
    loss.backward()
    assert all(p.grad is None for p in model.target_encoder.parameters())
    assert any(p.grad is not None for p in model.encoder.parameters())


def test_collapse_full():
    stats = latent_statistics(np.ones((4, 8, 4)))
    assert stats['failure_class'] == 'COLLAPSE_FULL'
    assert len(stats['covariance_spectrum']) == 4
    assert len(stats['pairwise_cosine']['histogram']) == 40


def test_dense_rank_one_is_dimensional_collapse():
    x = np.arange(128, dtype=np.float64)[:, None] * np.linspace(1, 2, 32)[None, :]
    stats = latent_statistics(x)
    assert stats['failure_class'] == 'COLLAPSE_DIM'
    assert stats['active_dimension_fraction'] == 1.0
    assert stats['effective_rank'] < 1.01


def test_full_rank_variation_is_not_collapse():
    x = np.random.default_rng(42).normal(size=(1024, 32))
    assert latent_statistics(x)['failure_class'] == 'NONE'
