"""Static checks for the Japanese Observatory presentation layer.

The SPA keeps API keys / enums / event types English and translates only
what the user sees. These tests read ``gui/static/index.html``, drop the
optional ``en`` dictionary, and check that (a) the required Japanese
labels exist, (b) no major English UI label survives outside the ``en``
dictionary, (c) times are rendered in Asia/Tokyo, (d) the GUI stays
read-only (no control endpoint is called from the page).
"""
import re
from pathlib import Path

import pytest

HTML = (Path(__file__).resolve().parents[1] / "gui" / "static" /
        "index.html").read_text(encoding="utf-8")


def _without_en_dict(html: str) -> str:
    start = html.index("/* i18n-en-start */")
    end = html.index("/* i18n-en-end */")
    return html[:start] + html[end:]


def _presentation_only(html: str) -> str:
    """Strip the ``en`` dictionary and CSS so only the ja UI + logic remain."""
    s = _without_en_dict(html)
    s = re.sub(r"<style>.*?</style>", "", s, flags=re.S)
    s = re.sub(r"^\s*//.*$", "", s, flags=re.M)
    return s


REQUIRED_JA = [
    "ダッシュボード", "個体群・系譜", "個体インスペクター", "テレメトリ",
    "イベント", "ワーカー", "ジョブ", "実行環境",
    "実行中", "一時停止", "停止処理中", "停止", "待機中", "成功", "失敗",
    "状態不明", "キャンセル", "オンライン", "オフライン", "切断",
    "ライブ", "記録済み", "算出値",
    "実験", "状態", "稼働時間", "個体数", "評価記録数", "出生数",
    "成功評価数", "失敗評価数",
    "ワーカーID", "デバイス", "GPU使用率", "GPU温度", "実行バッチ",
    "実行中ジョブ", "完了", "失敗ジョブ", "最終ハートビート",
    "ワーカーとの接続が失われました", "実行中ジョブを状態不明として記録しました",
    "ジョブを再投入しました", "実験条件が変更されています",
    "実行環境設定が変更されました", "センサー異常を検出しました",
    "古いワーカー結果を拒否しました", "バックグラウンド処理でエラーが発生しました",
    "世代", "出生番号", "親個体", "子個体", "系統", "ゲノムID", "人工器官",
    "変異", "適応度", "FBA0由来率", "祖先系譜",
    "基盤", "パラメータ", "パラメータ変異", "接続", "接続元", "接続先",
    "方向", "順方向", "双方向", "重み倍率", "由来",
    "MIE / テレメトリ", "GPU温度", "GPU使用率", "VRAM使用量", "信号",
    "発生源", "時刻", "データなし",
]


@pytest.mark.parametrize("label", REQUIRED_JA)
def test_required_japanese_labels_present(label):
    assert label in HTML, f"missing Japanese UI label: {label!r}"


FORBIDDEN_EN = [
    "Dashboard", "Population", "Workers", "Events", "Telemetry",
    "Current Job", "Failed", "Unknown", "No data", "Organism Inspector",
    "Lineage", "Uptime", "Births", "Last Heartbeat", "Completed Jobs",
    "Temperature", "Utilization", "Mutations", "Attachments", "Provenance",
]


@pytest.mark.parametrize("label", FORBIDDEN_EN)
def test_no_english_ui_label_outside_en_dict(label):
    body = _presentation_only(HTML)
    # Whole-word match: camelCase dictionary keys such as ``noWorkers`` or
    # ``compFailed`` are internal identifiers, not rendered text.
    pat = re.compile(r"(?<![A-Za-z])" + re.escape(label) + r"(?![a-z])")
    assert not pat.search(body), (
        f"English UI label {label!r} still present outside the 'en' dict")


def test_default_language_is_japanese():
    assert '<html lang="ja">' in HTML
    assert "localStorage.getItem('mioba.lang') || 'ja'" in HTML


def test_en_dictionary_is_optional_structure():
    """Future English switch: dictionary present, but ja is the default."""
    assert "/* i18n-en-start */" in HTML and "/* i18n-en-end */" in HTML
    assert "const I18N = {" in HTML
    assert re.search(r"^\s+ja:\s*\{", HTML, flags=re.M)
    assert re.search(r"^\s+en:\s*\{", HTML, flags=re.M)


def test_time_rendered_in_asia_tokyo_with_utc_tooltip():
    assert "Asia/Tokyo" in HTML
    assert " JST'" in HTML
    assert 'title="UTC ${esc(iso)}"' in HTML


def test_unit_formatting_helpers():
    for helper in ("fmtBytes", "fmtVram", "fmtTemp", "fmtPct", "fmtDur"):
        assert f"const {helper} =" in HTML
    assert "℃" in HTML
    assert "GB" in HTML and "MB" in HTML


def test_internal_enums_untouched_by_presentation():
    """Status classes / enum keys stay English (mapping is display-only)."""
    for enum in ("RUNNING", "QUEUED", "SUCCEEDED", "FAILED", "UNKNOWN",
                 "CANCELLED", "online", "offline", "lost",
                 "LIVE", "RECORDED", "DERIVED"):
        assert f"{enum}:" in HTML, enum
    for api in ("/api/gui/dashboard", "/api/gui/genomes", "/api/gui/telemetry",
                "/api/events"):
        assert api in HTML


def test_gui_is_read_only():
    body = HTML.lower()
    assert "fetch(" in body
    assert "method:" not in body and "method :" not in body
    for ctl in ("/api/control", "/api/jobs", "x-mioba-token", "authorization"):
        assert ctl not in body, f"control path {ctl!r} referenced by GUI"
    assert "<button" not in body and "<form" not in body


def test_gpu_comparison_and_responsive_layout():
    assert "gpuCompareTable" in HTML
    for key in ("gpu_model", "compute_capability", "device", "gpu_uuid",
                "bench_sim_seconds_per_wall_second", "bench_throughput",
                "current_job_id", "completed_jobs", "failed_jobs"):
        assert key in HTML, key
    assert "@media (max-width:640px)" in HTML
    assert "overflow-x:hidden" in HTML
    assert "auto-fit, minmax(" in HTML


def test_dashboard_worker_payload_has_device_identity(client, service):
    client.post("/api/worker/register", json={
        "worker_id": "p100", "hostname": "gpuhost", "gpu": [],
        "device": "cuda:1",
        "runtime_info": {
            "hostname": "gpuhost", "backend": "torch", "device": "cuda:1",
            "gpu_index": 1, "gpu_uuid": "GPU-p100-uuid",
            "gpu_model": "Tesla P100-PCIE-16GB", "compute_capability": "6.0",
            "vram_total_bytes": 16 * 1024 ** 3, "driver": "560.35",
            "cuda_runtime": "12.6", "torch_version": "2.9.1+cu126"},
        "bench": [
            {"batch": 4, "ok": True, "sim_seconds_per_wall_second": 0.5,
             "throughput": 2.0, "vram_allocated_bytes": 1, "vram_reserved_bytes": 2,
             "vram_total_bytes": 16 * 1024 ** 3},
            {"batch": 8, "ok": False, "error": "OOM"}],
        "batch_size": 4})
    w = next(w for w in client.get("/api/gui/dashboard").json()["workers"]
             if w["worker_id"] == "p100")
    assert w["device"] == "cuda:1"
    assert w["gpu_model"] == "Tesla P100-PCIE-16GB"
    assert w["compute_capability"] == "6.0"
    assert w["gpu_uuid"] == "GPU-p100-uuid"
    assert w["gpu_index"] == 1
    assert w["vram_total_bytes"] == 16 * 1024 ** 3
    assert w["batch_size"] == 4
    assert w["bench_sim_seconds_per_wall_second"] == 0.5
    assert w["bench_throughput"] == 2.0
    assert w["bench"]["selected"]["batch"] == 4
    assert len(w["bench"]["rows"]) == 2
