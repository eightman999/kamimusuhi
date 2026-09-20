import json
import sqlite3
import sys
import time
from pathlib import Path
from unittest.mock import patch

import pytest
from fastapi.testclient import TestClient
from experiments.k0_brainstem.telemetry.store import MetricStore
from experiments.k0_brainstem.telemetry.system_stats import snapshot
from experiments.k0_brainstem.monitor.server import create_app


def seed_run(root, run_id="test-1", state="running", device="cpu"):
    run = root / "runs" / run_id
    run.mkdir(parents=True)
    (run / "status.json").write_text(json.dumps({"status": state, "device": device}))
    (run / "config.json").write_text(json.dumps({"architecture": "gru64", "seed": 0}))
    return run


def test_wal_metrics_and_reader(tmp_path):
    with MetricStore(tmp_path, "test-1") as store:
        store.write({"training_step": 10, "reward_mean": .5})
        with sqlite3.connect(tmp_path / "metrics.sqlite") as reader:
            assert reader.execute("PRAGMA journal_mode").fetchone()[0] == "wal"
            assert reader.execute("SELECT count(*) FROM metrics").fetchone()[0] == 1
        with TestClient(create_app(tmp_path)) as client:
            record = client.get("/api/runs/test-1").json()
            assert record["metrics"][0]["reward_mean"] == .5
    assert len((tmp_path / "runs/test-1/metrics.jsonl").read_text().splitlines()) == 1
    with pytest.raises(ValueError):
        MetricStore(tmp_path, "../escape")


def test_control_atomic_and_bounded(tmp_path):
    run = seed_run(tmp_path)
    with TestClient(create_app(tmp_path)) as client:
        for command in ["pause", "resume", "stop"]:
            assert client.post("/api/run/" + command, json={"run_id": "test-1"}).status_code == 200
            assert json.loads((run / "control.json").read_text())["command"] == command
        assert client.post("/api/run/pause", json={"run_id": "unknown"}).status_code == 404
        assert client.post("/api/run/start", json={"config": "../../etc/passwd"}).status_code == 422
        assert client.post("/api/run/start", json={"command": "touch /tmp/not-allowed"}).status_code == 422
        assert client.post("/api/run/stop", json={"run_id": "../escape"}).status_code == 422
        assert client.post("/api/run/stop", json={"run_id": "test-1"}, headers={"Origin": "https://evil.invalid"}).status_code == 403
    assert not list(run.glob("*.tmp"))


def test_complete_run_not_controllable(tmp_path):
    seed_run(tmp_path, state="complete")
    with TestClient(create_app(tmp_path)) as client:
        assert client.post("/api/run/stop", json={"run_id": "test-1"}).status_code == 409


def test_no_gpu_overlap_and_detached_launch(tmp_path):
    seed_run(tmp_path, device="cuda:0")
    with TestClient(create_app(tmp_path)) as client:
        with patch("experiments.k0_brainstem.monitor.server.subprocess.Popen") as popen:
            popen.return_value.pid = 12345
            blocked = client.post("/api/run/start", json={"config": "smoke.yaml", "device": "cuda:0"})
            assert blocked.status_code == 409
            popen.assert_not_called()
            started = client.post("/api/run/start", json={"config": "smoke.yaml", "run_id": "gui-run", "device": "cpu"})
            assert started.status_code == 200
            assert popen.call_args.kwargs["start_new_session"] is True
            assert isinstance(popen.call_args.args[0], list)
            assert "shell" not in popen.call_args.kwargs
            assert client.post("/api/run/start", json={"config": "smoke.yaml", "run_id": "gui-run"}).status_code == 409


def test_websocket_reconnect_and_viewer_data(tmp_path):
    run = seed_run(tmp_path)
    (run / "episodes.json").write_text('[{"time":0,"sensor_vector":[0.1]}]')
    (run / "language_events.jsonl").write_text('{"j72_called":true}\n{"unfinished":')
    with TestClient(create_app(tmp_path)) as client:
        for _ in range(2):
            with client.websocket_connect("/ws/metrics") as ws:
                payload = ws.receive_json()
                assert payload["type"] == "snapshot"
                assert payload["runs"][0]["run_id"] == "test-1"
        assert client.get("/api/runs/test-1/episodes").json()[0]["time"] == 0
        assert client.get("/api/runs/test-1/language-events").json() == [{"j72_called": True}]


def test_symlink_run_cannot_escape(tmp_path):
    outside = tmp_path / "outside"
    outside.mkdir()
    (tmp_path / "runs").mkdir()
    (tmp_path / "runs/escape").symlink_to(outside, target_is_directory=True)
    with TestClient(create_app(tmp_path)) as client:
        assert client.get("/api/runs/escape").status_code == 404
        assert client.get("/api/runs").json() == []


def test_snapshot_has_unknowns_without_gpu():
    with patch("experiments.k0_brainstem.telemetry.system_stats.shutil.which", return_value=None):
        stats = snapshot()
    assert stats["gpus"] == []
    assert "cpu_percent" in stats and "ram_percent" in stats


def test_qt_live_websocket_and_reconnect(tmp_path, monkeypatch):
    import os
    import socket
    import subprocess
    monkeypatch.setenv("QT_QPA_PLATFORM", "offscreen")
    pytest.importorskip("PyQt5.QtWidgets")
    from PyQt5 import QtWidgets
    from experiments.k0_brainstem.monitor.mac_gui import Monitor
    run = seed_run(tmp_path)
    with MetricStore(tmp_path, "test-1") as store:
        for i in range(5):
            store.write({"timestamp": time.time()+i, "training_step": i*100,
                         "reward_mean": i*.1, "task_success": .7, "llm_call_rate": .1,
                         "missed_llm_rate": .03, "false_llm_call_rate": .04,
                         "steps_per_second": 8000, "action_distribution": [.2,.2,.2,.2,.1,.1], "hidden_norm": 1.2})
    (run / "episodes.json").write_text('[{"time":0,"sensor_vector":[0.1],"chosen_action":2,"oracle_action":2,"reward":1}]')
    (run / "language_events.jsonl").write_text('{"j72_called":true,"scenario":5}\n')
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    command = [sys.executable, "-m", "experiments.k0_brainstem.monitor.server", "--artifacts", str(tmp_path), "--port", str(port)]
    process = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    application = QtWidgets.QApplication.instance() or QtWidgets.QApplication([])
    window = Monitor(f"http://127.0.0.1:{port}")
    window.show()
    def until(predicate, timeout=10):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            application.processEvents()
            if predicate():
                return
            time.sleep(.01)
        raise AssertionError("Qt condition timeout")
    def stop_server():
        if process.poll() is None:
            process.terminate()
        try:
            process.wait(timeout=8)
        except subprocess.TimeoutExpired:
            # Always reap a fixture, even when its graceful shutdown regresses.
            process.kill()
            process.wait(timeout=5)
            pytest.fail("monitor server exceeded bounded graceful shutdown")
    try:
        until(lambda: "接続済み" in window.connection_label.text() and window.episodes.rowCount() == 1)
        assert window.runs.rowCount() == 1
        assert len(window.curves["reward_mean"].getData()[0]) == 5
        assert window.languages.rowCount() == 1
        assert "内部状態ノルム" in window.brain.toPlainText()
        if os.environ.get("K0_GUI_TEST_SCREENSHOT"):
            window.grab().save(os.environ["K0_GUI_TEST_SCREENSHOT"])
        monkeypatch.setattr(Monitor, "confirm_stop", lambda self: False)
        window.control("stop")
        assert not (run / "control.json").exists()
        stop_server()
        until(lambda: "切断" in window.connection_label.text())
        process = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        until(lambda: "接続済み" in window.connection_label.text())
        # Closing only the client must leave the independent server alive.
        window.close()
        application.processEvents()
        assert process.poll() is None
    finally:
        window.close()
        application.processEvents()
        stop_server()


def test_physical_gpu_occupancy_and_queue_block(tmp_path):
    run = seed_run(tmp_path, device="cuda:0")
    (run / "gpu.json").write_text(json.dumps({"index": 1, "uuid": "GPU-p100", "name": "P100"}))
    hardware = {"gpus": [{"index": 0, "uuid": "GPU-3060", "name": "3060"}, {"index": 1, "uuid": "GPU-p100", "name": "P100"}], "gpu_processes": [{"gpu_uuid": "GPU-3060", "pid": 42}]}
    with TestClient(create_app(tmp_path)) as client, patch("experiments.k0_brainstem.monitor.server.snapshot", return_value=hardware), patch("experiments.k0_brainstem.monitor.server.subprocess.Popen") as popen:
        assert client.get("/api/runs/test-1").json()["physical_gpu"]["index"] == 1
        for device in ["cuda:0", "cuda:1"]:
            assert client.post("/api/run/start", json={"config": "smoke.yaml", "device": device}).status_code == 409
        hardware["gpu_processes"] = []
        popen.return_value.pid = 123
        assert client.post("/api/run/start", json={"config": "smoke.yaml", "device": "cuda:0"}).status_code == 200
        assert popen.call_args.kwargs["env"]["CUDA_VISIBLE_DEVICES"] == "GPU-3060"
        (tmp_path / "queue.json").write_text('{"pending":["full-run"],"active":{}}')
        assert client.post("/api/run/start", json={"config": "smoke.yaml", "device": "cpu"}).status_code == 409


def test_qt_queued_selection_clears_stale_metrics_and_formats_values(monkeypatch):
    from datetime import datetime
    monkeypatch.setenv("QT_QPA_PLATFORM", "offscreen")
    pytest.importorskip("PyQt5.QtWidgets")
    from PyQt5 import QtWidgets
    from experiments.k0_brainstem.monitor.mac_gui import Monitor
    monkeypatch.setattr(Monitor, "request", lambda *args, **kwargs: None)
    monkeypatch.setattr(Monitor, "connect_ws", lambda *args: None)
    application = QtWidgets.QApplication.instance() or QtWidgets.QApplication([])
    window = Monitor("http://127.0.0.1:1")
    try:
        window.update_runs([
            {"run_id": "finished", "status": "complete", "latest": {
                "timestamp": 1700000000.0, "reward_mean": .7, "hidden_norm": 2.0,
                "action_distribution": [.1,.1,.1,.1,.1,.5]}},
            {"run_id": "pending", "status": "queued", "latest": {}}])
        assert window.selected == "finished"
        assert len(window.curves["reward_mean"].getData()[0]) == 1
        assert "内部状態ノルム" in window.brain.toPlainText()
        window.runs.selectRow(1)
        assert window.selected == "pending"
        for curve in window.curves.values():
            x, _ = curve.getData()
            assert x is None or len(x) == 0
        assert "内部状態ノルム" not in window.brain.toPlainText()
        assert list(window.action_bars.opts["height"]) == [0] * 6
        window.update_system({"gpus": [{"index": 0, "name": "3060", "memory_used_mb": 8192.0, "memory_total_mb": 12288.0}]})
        assert "VRAM 8,192/12,288 MiB" in window.system_label.text()
        window.set_languages([{"timestamp": 1700000000.0}])
        assert window.languages.item(0, 0).text() == datetime.fromtimestamp(1700000000).astimezone().strftime("%Y-%m-%d %H:%M:%S %Z")
        window.set_episodes([{"time": 0, "chosen_action": 5, "oracle_action": 2}])
        assert window.episodes.item(0, 3).text() == "言語系を呼ぶ"
        assert window.episodes.item(0, 4).text() == "注意を向ける"
    finally:
        window.close()
        application.processEvents()


def test_qt_japanese_labels_dialog_font_and_unchanged_protocol(monkeypatch):
    import os
    monkeypatch.setenv("QT_QPA_PLATFORM", "offscreen")
    pytest.importorskip("PyQt5.QtWidgets")
    from PyQt5 import QtCore, QtGui, QtWidgets
    from experiments.k0_brainstem.monitor.mac_gui import Monitor, japanese_font
    requests = []
    monkeypatch.setattr(Monitor, "request", lambda self, route, callback=None, body=None: requests.append((route, body)))
    monkeypatch.setattr(Monitor, "connect_ws", lambda *args: None)
    application = QtWidgets.QApplication.instance() or QtWidgets.QApplication([])
    window = Monitor("http://127.0.0.1:1")
    try:
        assert window.windowTitle().endswith("K0 人工脳幹")
        assert [window.period.itemText(i) for i in range(3)] == ["1分", "5分", "全期間"]
        assert {b.text() for b in window.findChildren(QtWidgets.QPushButton)} >= {"開始", "一時停止", "再開", "停止"}
        window.update_runs([{"run_id": "sample", "status": "queued", "latest": {}}])
        assert window.runs.item(0, 8).text() == "待機中"
        window.set_languages([{"scenario": 5, "oracle_required": True, "j72_called": False, "status": "VIRTUAL_ORACLE"}])
        assert window.languages.item(0, 2).text() == "言語処理が必要"
        assert window.languages.item(0, 5).text() == "はい"
        assert window.languages.item(0, 6).text() == "いいえ"
        assert window.languages.item(0, 8).text() == "仮想オラクル"
        window.control("start")
        assert requests[-1] == ("/api/run/start", {"config": "smoke.yaml", "device": "cpu"})
        observed = []
        def cancel_dialog():
            dialog = window.findChild(QtWidgets.QMessageBox)
            observed.extend([dialog.text(), *[b.text() for b in dialog.buttons()]])
            if os.environ.get("K0_GUI_JA_DIALOG_SCREENSHOT"):
                dialog.grab().save(os.environ["K0_GUI_JA_DIALOG_SCREENSHOT"])
            next(b for b in dialog.buttons() if b.text() == "キャンセル").click()
        QtCore.QTimer.singleShot(0, cancel_dialog)
        assert window.confirm_stop() is False
        assert set(observed) == {"学習を停止しますか？", "停止する", "キャンセル"}
        if sys.platform == "darwin":
            font = japanese_font()
            assert font.family() == "Hiragino Sans"
            metrics = QtGui.QFontMetrics(font)
            assert all(metrics.inFont(ch) for ch in "人工脳幹平均報酬課題成功率学習処理速度内部状態言語ゲート一時停止再開")
    finally:
        window.close()
        application.processEvents()
