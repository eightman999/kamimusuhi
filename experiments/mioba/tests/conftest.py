import pytest
import yaml
from pathlib import Path

CFG = Path(__file__).resolve().parents[1] / "configs" / "smoke_mock.yaml"


def pytest_configure(config):
    config.addinivalue_line("markers", "slow: slower end-to-end tests")


@pytest.fixture
def smoke_config():
    return yaml.safe_load(CFG.read_text())


@pytest.fixture
def service(tmp_path, smoke_config):
    from experiments.mioba.coordinator.service import MiobaService
    svc = MiobaService(smoke_config, tmp_path / "runs")
    yield svc
    svc.db.close()


@pytest.fixture
def client(service):
    from fastapi.testclient import TestClient
    from experiments.mioba.coordinator.app import create_app
    return TestClient(create_app(service))


def run_worker_once(client, worker_id, device="cpu", batch_size=1):
    """In-process worker: register if needed, claim, run, report."""
    from experiments.mioba.workers.worker import run_job
    r = client.post("/api/worker/claim",
                    json={"worker_id": worker_id, "batch_size": batch_size})
    if r.status_code == 204:
        return None
    assert r.status_code == 200, r.text
    return run_job(client, worker_id, r.json(), device, batch_size)
