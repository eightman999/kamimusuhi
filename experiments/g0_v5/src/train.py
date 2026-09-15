"""G0-v5 strictly observation-only training and checkpoint inference.

These sources derive from this session's unexecuted drafts, not old G0
checkpoints. Validation SSL loss alone selects the best checkpoint.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import socket
import subprocess
import time
from datetime import datetime, timezone

import numpy as np
os.environ.setdefault('CUBLAS_WORKSPACE_CONFIG', ':4096:8')
import torch
import yaml

from .models import build_model
from .losses import objective
from .telemetry import latent_statistics


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def _command(args):
    try:
        result = subprocess.run(args, capture_output=True, text=True, timeout=10)
        return result.stdout.strip() if result.returncode == 0 else None
    except (OSError, subprocess.TimeoutExpired):
        return None


def load_observations(path):
    with np.load(path, allow_pickle=False) as archive:
        if set(archive.files) != {'obs'}:
            raise ValueError('training archive must contain only obs')
        observations = np.asarray(archive['obs'], dtype=np.float32)
    if observations.ndim != 3 or min(observations.shape) < 2:
        raise ValueError('observations must be nonempty [N,T,D]')
    if not np.isfinite(observations).all():
        raise FloatingPointError('nonfinite observations')
    return observations


def _write_json(path, data):
    Path(path).write_text(json.dumps(data, indent=2, allow_nan=False) + '\n')


def _checkpoint(path, model, config, epoch, seed):
    torch.save(dict(config=config, state_dict=model.state_dict(), epoch=epoch, seed=seed), path)


def _read_checkpoint(path, device):
    checkpoint = torch.load(path, map_location=device, weights_only=True)
    model = build_model(checkpoint['config']).to(device)
    model.load_state_dict(checkpoint['state_dict'])
    model.eval()
    return model, checkpoint


def load_encoder(checkpoint: Path, device: str = 'cpu'):
    model, saved = _read_checkpoint(checkpoint, device)

    @torch.no_grad()
    def encode(observations):
        values = np.asarray(observations, dtype=np.float32)
        if values.ndim != 3 or values.shape[-1] != saved['config']['obs_dim']:
            raise ValueError('expected observations [N,T,D]')
        return np.concatenate([model.encoder(torch.from_numpy(values[i:i + 64]).to(device)).cpu().numpy()
                               for i in range(0, len(values), 64)])
    return encode


@torch.no_grad()
def validation_loss(model, data, config, device):
    model.eval()
    total, count = 0.0, 0
    for start in range(0, len(data), config['batch_size']):
        batch = torch.from_numpy(data[start:start + config['batch_size']]).to(device)
        loss, _ = objective(model, batch, config)
        if not torch.isfinite(loss):
            raise FloatingPointError('nonfinite validation loss')
        total += float(loss) * len(batch)
        count += len(batch)
    return total / count


@torch.no_grad()
def _statistics(model, data, config, device):
    model.eval()
    latent = np.concatenate([model.encoder(torch.from_numpy(data[i:i + 64]).to(device)).cpu().numpy()
                             for i in range(0, len(data), 64)])
    return latent_statistics(latent, config['collapse_std_threshold'], config['collapse_active_fraction'])


def run(config_path, data_dir, output, seed=0, device='cpu', max_epochs=None, initialize_only=False):
    output, data_dir, config_path = Path(output), Path(data_dir), Path(config_path)
    output.mkdir(parents=True, exist_ok=False)
    started = time.monotonic()
    config_bytes = config_path.read_bytes()
    (output / 'config.yaml').write_bytes(config_bytes)
    os.chmod(output / 'config.yaml', 0o444)
    config = yaml.safe_load(config_bytes)
    config_hash = hashlib.sha256(config_bytes).hexdigest()
    source_root = Path(__file__).parent
    manifest = dict(experiment_id='G0-v5', config_hash=config_hash, seed=seed, device=device,
                    encoder='GRU plus linear projection', predictor='per-horizon MLP', objective=config['method'],
                    hostname=socket.gethostname(), platform=platform.platform(),
                    git_commit=os.environ.get('G0V5_BASE_GIT_COMMIT') or _command(['git', 'rev-parse', 'HEAD']),
                    git_commit_scope='base commit; uncommitted independent experiment identified by source_hashes',
                    source_hashes={str(p.relative_to(source_root)): sha256(p) for p in sorted(source_root.rglob('*.py'))},
                    provenance='new independent G0-v5; unexecuted this-session drafts; no old checkpoints',
                    pytorch=str(torch.__version__), cuda=torch.version.cuda,
                    driver=_command(['nvidia-smi', '--query-gpu=index,name,uuid,driver_version', '--format=csv,noheader']),
                    gpu_name=None, gpu_uuid=None,
                    start_time=datetime.now(timezone.utc).isoformat(), status='running', failure_class='NONE',
                    max_epochs_override=max_epochs, initialize_only=initialize_only, training_selection='validation SSL loss only',
                    temporal_loss_caveat='JEPA EMA targets drift; loss reduction alone does not establish learned improvement')
    _write_json(output / 'manifest.json', manifest)
    try:
        if config['experiment_id'] != 'G0-v5':
            raise ValueError('incorrect experiment identity')
        if device.startswith('cuda'):
            if not torch.cuda.is_available():
                raise RuntimeError('requested CUDA unavailable')
            torch.cuda.set_device(device)
            props = torch.cuda.get_device_properties(device)
            manifest.update(gpu_name=props.name, gpu_uuid=str(getattr(props, 'uuid', 'unavailable')))
        torch.manual_seed(seed)
        np.random.seed(seed)
        torch.use_deterministic_algorithms(True)
        train = load_observations(data_dir / 'train.npz')
        validation = load_observations(data_dir / 'validation.npz')
        manifest['dataset_hashes'] = {name: sha256(data_dir / name) for name in ['train.npz', 'validation.npz']}
        manifest['dataset_version'] = hashlib.sha256(json.dumps(manifest['dataset_hashes'], sort_keys=True).encode()).hexdigest()
        for values in [train, validation]:
            if values.shape[-1] != config['obs_dim'] or max(config['horizons']) >= values.shape[1]:
                raise ValueError('observation dimensions or horizons incompatible')
        model = build_model(config).to(device)
        _checkpoint(output / 'checkpoint_initial.pt', model, config, 0, seed)
        manifest['initial_checkpoint_hash'] = sha256(output / 'checkpoint_initial.pt')
        if initialize_only:
            manifest.update(status='initialized', epochs_completed=0)
            return manifest
        optimizer = torch.optim.AdamW([p for p in model.parameters() if p.requires_grad],
                                      lr=config['learning_rate'], weight_decay=config['weight_decay'])
        initial_loss = validation_loss(model, validation, config, device)
        manifest['initial_heldout_ssl_loss'] = initial_loss
        _write_json(output / 'manifest.json', manifest)
        best_loss, best_epoch = float('inf'), 0
        rng = np.random.default_rng(seed)
        epochs = int(max_epochs if max_epochs is not None else config['epochs'])
        if epochs < 1:
            raise ValueError('epochs must be positive')
        stats_records = []
        with (output / 'train_log.jsonl').open('x') as log:
            for epoch in range(1, epochs + 1):
                model.train()
                order = rng.permutation(len(train))
                total, count = 0.0, 0
                epoch_started = time.monotonic()
                for start in range(0, len(order), config['batch_size']):
                    batch = torch.from_numpy(train[order[start:start + config['batch_size']]]).to(device)
                    optimizer.zero_grad(set_to_none=True)
                    loss, _ = objective(model, batch, config)
                    if not torch.isfinite(loss):
                        raise FloatingPointError('nonfinite training loss')
                    loss.backward()
                    torch.nn.utils.clip_grad_norm_(model.parameters(), config['grad_clip'], error_if_nonfinite=True)
                    optimizer.step()
                    model.update_target(config['ema_decay'])
                    total += float(loss.detach()) * len(batch)
                    count += len(batch)
                heldout_loss = validation_loss(model, validation, config, device)
                stats = _statistics(model, validation, config, device)
                stats_records.append(stats)
                record = dict(epoch=epoch, train_loss=total / count, validation_loss=heldout_loss,
                              wall_clock=time.monotonic() - epoch_started, latent=stats)
                log.write(json.dumps(record, allow_nan=False) + '\n')
                log.flush()
                _checkpoint(output / 'checkpoint_final.pt', model, config, epoch, seed)
                if heldout_loss < best_loss:
                    best_loss, best_epoch = heldout_loss, epoch
                    _checkpoint(output / 'checkpoint_best.pt', model, config, epoch, seed)
                if stats['failure_class'] != 'NONE':
                    manifest['failure_class'] = stats['failure_class']
                    break
        np.savez_compressed(output / 'latent_stats.npz',
                            epoch=np.arange(1, len(stats_records) + 1),
                            latent_mean=np.array([s['latent_mean'] for s in stats_records]),
                            per_dimension_std=np.array([s['per_dimension_std'] for s in stats_records]),
                            covariance_spectrum=np.array([s['covariance_spectrum'] for s in stats_records]),
                            effective_rank=np.array([s['effective_rank'] for s in stats_records]))
        manifest.update(best_epoch=best_epoch, epochs_completed=len(stats_records),
                        best_heldout_ssl_loss=best_loss, final_heldout_ssl_loss=heldout_loss,
                        status='complete' if manifest['failure_class'] == 'NONE' else 'failed')
        if sha256(output / 'config.yaml') != config_hash or sha256(config_path) != config_hash:
            raise RuntimeError('configuration changed during run')
    except Exception as exc:
        if isinstance(exc, torch.cuda.OutOfMemoryError):
            classification = 'OOM'
        elif isinstance(exc, FloatingPointError) or 'non-finite' in str(exc):
            classification = 'NAN'
        elif isinstance(exc, ValueError) and 'only obs' in str(exc):
            classification = 'DATA_LEAK'
        else:
            classification = 'SYSTEM_ERROR'
        manifest.update(status='failed', failure_class=classification, error=str(exc), error_type=type(exc).__name__)
        raise
    finally:
        manifest.update(end_time=datetime.now(timezone.utc).isoformat(), wall_clock=time.monotonic() - started)
        _write_json(output / 'manifest.json', manifest)
    return manifest


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--config', required=True)
    parser.add_argument('--data-dir', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--seed', type=int, default=0)
    parser.add_argument('--device', default='cpu')
    parser.add_argument('--max-epochs', type=int)
    parser.add_argument('--initialize-only', action='store_true')
    args = parser.parse_args()
    run(args.config, args.data_dir, args.output, args.seed, args.device, args.max_epochs, args.initialize_only)


if __name__ == '__main__':
    main()
