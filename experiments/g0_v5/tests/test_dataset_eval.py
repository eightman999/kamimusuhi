import importlib.util
from pathlib import Path
import hashlib
import numpy as np
import pytest

ROOT = Path(__file__).resolve().parents[1]

def module(name):
    spec = importlib.util.spec_from_file_location(name, ROOT/'src'/f'{name}.py')
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value

dataset = module('dataset')
evaluate = module('evaluate')

@pytest.fixture(scope='module')
def data(tmp_path_factory):
    path = tmp_path_factory.mktemp('g0v5') / 'data'
    dataset.generate_dataset(path, n_train=32, n_eval=96)
    return path


def oracle_encoder(path, key):
    # Test-only ideal encoder lookup. Production encoders never see this map.
    mapping = {}
    classes = 8 if key == 'cause' else 4
    for file in (path/'evaluation').glob('*.npz'):
        with np.load(file) as ds:
            for obs, truth in zip(ds['obs'], ds[key]):
                mapping[obs.tobytes()] = np.eye(classes)[truth]
    def encode(observations):
        return np.stack([mapping[x.tobytes()] for x in observations])
    return encode


def test_training_firewall_and_hashes(data, tmp_path):
    import json
    manifest = json.loads((data/'dataset_manifest.json').read_text())
    assert dataset.load_training(data/'training'/'train.npz').shape == (32,48,24)
    assert set(p.name for p in (data/'training').iterdir()) == {'train.npz', 'validation.npz'}
    np.savez(tmp_path/'tampered.npz', obs=np.zeros((1,48,24)), cause=np.array([1]))
    with pytest.raises(ValueError, match='DATA_LEAK'):
        dataset.load_training(tmp_path/'tampered.npz')
    for name, checksum in manifest['files'].items():
        assert hashlib.sha256((data/name).read_bytes()).hexdigest() == checksum
    with pytest.raises(FileExistsError):
        dataset.generate_dataset(data)


def test_fixed_generator_reproducible(data, tmp_path):
    dataset.generate_dataset(tmp_path/'copy', n_train=32, n_eval=96)
    for original in data.rglob('*.npz'):
        with np.load(original) as a, np.load(tmp_path/'copy'/original.relative_to(data)) as b:
            assert a.files == b.files
            for key in a.files:
                np.testing.assert_array_equal(a[key], b[key])


def test_split_contract_and_balanced_context_pairs(data):
    for split in ['iid_test','match_ood','dynseg_ood','combo_oodctx']:
        with np.load(data/'evaluation'/f'{split}.npz') as ds:
            c, k, idx, y = ds['cause'], ds['context'], ds['pair_index'], ds['pair_label']
            assert np.all(k[idx[:,0],-1] != k[idx[:,1],-1])
            np.testing.assert_array_equal(c[idx[:,0],-1] == c[idx[:,1],-1], y == 1)
            for label in [0,1]:
                pairs, counts = np.unique(k[idx[y == label],-1], axis=0, return_counts=True)
                assert len(pairs) == 12 and len(set(counts.tolist())) == 1
            # Endpoint cause marginals match across positive and negative labels.
            for side in [0,1]:
                np.testing.assert_array_equal(np.bincount(c[idx[y == 0,side],-1],minlength=8),
                                              np.bincount(c[idx[y == 1,side],-1],minlength=8))
            if split == 'iid_test':
                assert np.all(k == k[:,:1])
                assert np.all(c % 4 != k)
                changes = np.nonzero(c[:,1:] != c[:,:-1])[1] + 1
                assert len(changes) and np.all(changes % 12 == 0)
            if split == 'match_ood':
                assert np.all(c == c[:,:1]) and np.all(k == k[:,:1])
                assert np.all(c % 4 != k)
                assert not np.array_equal(ds['obs'][idx[1,0]], ds['obs'][idx[1,1]])
            if split == 'combo_oodctx':
                assert np.all(c[idx[:,0]] % 4 == k[idx[:,0]])
                assert np.all(c[idx[:,1]] % 4 != k[idx[:,1]])
            if split == 'dynseg_ood':
                assert np.all(k == k[:,:1])
                assert np.all(c % 4 != k)
                changes = np.nonzero(c[:,1:] != c[:,:-1])[1] + 1
                assert np.any(changes % 12 != 0)
    with np.load(data/'evaluation'/'midctx.npz') as ds:
        assert np.all(ds['cause'] == ds['cause'][:,:1])
        assert np.all(ds['cause'] % 4 != ds['context'])
        assert np.all(ds['context'][:,:24] == ds['context'][:,:1])
        assert np.all(ds['context'][:,24:] == ds['context'][:,24:25])
        assert np.all(ds['context'][:,23] != ds['context'][:,24])


def test_interventions_change_exactly_one_factor(data):
    with np.load(data/'evaluation'/'intervention.npz') as ds:
        groups = ds['intervention_index']
        for i in range(1,4):
            changed = ['cause','context','nuisance'][i-1]
            for factor in ['cause','context','nuisance','sensor_noise','phase']:
                a, b = ds[factor][groups[:,0]], ds[factor][groups[:,i]]
                if factor == changed:
                    assert np.all(np.any(a.reshape(len(a),-1) != b.reshape(len(b),-1), axis=1))
                else:
                    np.testing.assert_array_equal(a,b)


def test_auc_ties_and_null():
    assert evaluate.auc_score([0,1,0,1],[0,0,0,0]) == .5
    assert evaluate.auc_score([0,1,0,1],[0,1,0,1]) == 1
    rng = np.random.default_rng(77)
    y = np.repeat([0,1],5000)
    assert abs(evaluate.auc_score(y,rng.normal(size=len(y)))-.5) < .025


def test_oracle_context_control_and_deterministic_evaluation(data, tmp_path):
    context_encoder = oracle_encoder(data,'context')
    context_vectors = np.random.default_rng(917).normal(size=(4,9))
    context_result = evaluate.evaluate_representation(
        lambda obs: context_encoder(obs) @ context_vectors, data)
    for key in ['iid','match_ood','dynseg_ood','combo_oodctx']:
        assert context_result[key]['auc'] == .5
    assert context_result['failure_class'] == 'SHORTCUT_CONTEXT'
    encoder = oracle_encoder(data,'cause')
    a = evaluate.evaluate_representation(encoder, data, tmp_path/'result')
    b = evaluate.evaluate_representation(encoder, data)
    a.pop('runtime'); b.pop('runtime')
    assert a == b
    for key in ['iid','match_ood','dynseg_ood','combo_oodctx']:
        assert a[key]['auc'] > .9
    assert a['midctx']['stability'] == 1
    assert a['intervention']['cause_distance'] > 1
    assert a['intervention']['context_distance'] == 0
    assert a['intervention']['nuisance_distance'] == 0
    assert (tmp_path/'result'/'eval_predictions.npz').exists()
    with np.load(tmp_path/'result'/'eval_predictions.npz') as predictions:
        rng = np.random.default_rng(501)
        null_auc = [evaluate.auc_score(rng.permutation(predictions['match_ood_label']),
                    predictions['match_ood_score']) for _ in range(200)]
        assert abs(np.mean(null_auc) - .5) < .025
    with np.load(tmp_path/'result'/'latent_stats.npz') as stats:
        assert 'covariance_spectrum' in stats.files
    with pytest.raises(FileExistsError):
        evaluate.evaluate_representation(encoder,data,tmp_path/'result')


def test_collapse_and_invalid_latents(data):
    assert evaluate.collapse_stats(np.ones((20,8)))['full_collapse']
    x = np.zeros((100,32)); x[:,0] = np.arange(100)
    assert evaluate.collapse_stats(x)['dimensional_collapse']
    assert evaluate.collapse_stats(np.repeat(x[:,:1],32,axis=1))['dimensional_collapse']
    result = evaluate.evaluate_representation(lambda x: np.zeros((*x.shape[:2],8)),data)
    assert result['failure_class'] == 'COLLAPSE_FULL'
    with pytest.raises(FloatingPointError):
        evaluate.evaluate_representation(lambda x: x * np.nan,data)
