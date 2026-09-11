import torch

from models import MODEL_REGISTRY, build_model


def test_shapes_all_models():
    for name in MODEL_REGISTRY:
        m = build_model(name, obs_dim=16, n_actions=4)
        obs = torch.randn(3, 10, 16)
        act = torch.randint(0, 4, (3, 10))
        pred, lat, hT = m(obs, act)
        assert pred.shape == (3, 10, 16)
        assert lat.shape[:2] == (3, 10)
        if m.is_recurrent:
            assert hT is not None


def test_action_input_used_only_when_enabled():
    obs = torch.randn(2, 8, 16)
    act_a = torch.zeros(2, 8, dtype=torch.long)
    act_b = torch.ones(2, 8, dtype=torch.long)
    for name, spec in MODEL_REGISTRY.items():
        torch.manual_seed(0)
        m = build_model(name, 16, 4)
        pa, _, _ = m(obs, act_a)
        pb, _, _ = m(obs, act_b)
        same = torch.allclose(pa, pb)
        assert same != spec["use_action"], name


def test_target_delta():
    m = build_model("mlp_state_action", 16, 4, target_delta=True)
    obs = torch.randn(1, 4, 16)
    act = torch.randint(0, 4, (1, 4))
    pred, _, _ = m(obs, act)
    assert not torch.allclose(pred, obs)
    m2 = build_model("mlp_state_action", 16, 4, target_delta=False)
    pred2, _, _ = m2(obs, act)
    assert pred2.shape == pred.shape
