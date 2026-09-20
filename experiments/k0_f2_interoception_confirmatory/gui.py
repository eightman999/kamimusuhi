"""Japanese, read-only K0-F2 dashboard; reads artifacts without controlling hardware."""
from __future__ import annotations

import argparse
import html
import json
import math
import sys
import time
from datetime import datetime
from pathlib import Path

from PyQt5 import QtCore, QtGui, QtWidgets
import pyqtgraph as pg

FRAME_NAMES = (
    "master_cpu_thermal", "master_cpu_busy", "master_ram_pressure", "master_io_pressure",
    "rtx3060_thermal", "rtx3060_compute_busy", "rtx3060_vram_pressure", "rtx3060_power_pressure",
    "p100_thermal", "p100_compute_busy", "p100_vram_pressure", "p100_power_pressure",
    "mac_thermal", "mac_cpu_busy", "mac_memory_pressure", "mac_power_pressure",
    "network_latency", "network_loss", "body_staleness", "body_availability")
FRAME_LABELS = (
    "中央 CPU 温度圧", "中央 CPU 使用率", "中央 RAM 圧", "中央 I/O 圧",
    "RTX 3060 温度圧", "RTX 3060 計算負荷", "RTX 3060 VRAM 圧", "RTX 3060 電力圧",
    "P100 温度圧", "P100 計算負荷", "P100 VRAM 圧", "P100 電力圧",
    "Mac 熱圧", "Mac CPU 使用率", "Mac メモリー圧", "Mac 電源圧",
    "通信遅延", "パケット損失", "身体情報の古さ", "身体情報の取得率")
ACTIONS = ("RUN_CPU", "RUN_RTX3060", "RUN_P100", "WAIT")
ACTION_LABELS = {"WAIT": "20 ms 待機後 RTX 3060", "RUN_CPU": "CPU で実行", "RUN_RTX3060": "RTX 3060 で実行",
                 "RUN_P100": "P100 で実行", "RUN_DUAL_GPU": "両 GPU で実行",
                 "INVOKE_LANGUAGE": "言語器官を呼ぶ", "DEFER": "延期"}
SOURCE_LABELS = {"real": "実測", "synthetic": "合成", "mixed": "実測と合成の混合",
                 "real_telemetry_measured_cost_replay": "実測身体・実測費用の再生評価",
                 "real_new_hardware_job": "凍結方策による新規実ジョブ",
                 "synthetic_observation_perturbation": "合成センサー介入"}
PLOTS = (
    ("body_timeseries.png", "新規身体情報の時系列"),
    ("action_conditioned_probe.png", "行動別 outcome の予測誤差"),
    ("probe_regret.png", "予測に基づく資源選択の regret"),
    ("body_vs_blind.png", "BODY 対 独立学習 BLIND"),
    ("body_vs_shuffled.png", "BODY 対 SHUFFLED"),
    ("body_vs_stale.png", "BODY 対 30 秒 STALE"),
    ("live_body_vs_blind.png", "新規実ジョブの BODY 対 BLIND"),
    ("counterfactual_body.png", "最終身体情報への因果介入"),
    ("sensor_ood_heatmap.png", "未知のセンサー条件"),
    ("temporal_ood.png", "未知の時間条件"),
    ("policy_action_map.png", "身体状態ごとの行動"),
    ("pareto.png", "実測効用と遅延"))

COLORS = ("#1677a5", "#c34b32", "#69882b", "#9462ac", "#bf8530", "#5e7181")


def finite(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def display(value, scale=1, suffix=""):
    if value is None or (isinstance(value, float) and not math.isfinite(value)):
        return "未取得"
    if isinstance(value, bool):
        return "はい" if value else "いいえ"
    if finite(value):
        return f"{value * scale:.4g}{suffix}"
    if isinstance(value, (list, dict)):
        return json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    return str(value)


def read_jsonl_tail(path, max_bytes=2_000_000, max_rows=1800):
    """Bound work for a growing daemon stream; ignore only the unfinished final row."""
    path = Path(path)
    if not path.exists():
        return [], []
    rows, errors = [], []
    with path.open("rb") as stream:
        size = path.stat().st_size
        offset = max(0, size - max_bytes)
        stream.seek(offset)
        if offset:
            stream.readline()
        lines = stream.read().splitlines(keepends=True)
    for index, line in enumerate(lines):
        try:
            value = json.loads(line)
            if not isinstance(value, dict):
                raise ValueError("object required")
            rows.append(value)
        except (ValueError, UnicodeDecodeError):
            if index != len(lines) - 1 or line.endswith(b"\n"):
                errors.append(f"{path.name}: 読み取れない完成行")
    return rows[-max_rows:], errors


def read_snapshot(root):
    root = Path(root).expanduser().resolve()
    snapshot = {"errors": [], "root": str(root)}
    def read_json(name):
        path = root / (name + ".json")
        if not path.exists():
            return None
        try:
            if path.stat().st_size > 8_000_000:
                raise ValueError("file too large")
            return json.loads(path.read_text())
        except (OSError, ValueError):
            snapshot["errors"].append(f"{path.name}: 読み取り失敗またはサイズ上限")
            return None
    from .report import probe_numerical_gate
    snapshot["probe_validation"] = read_json("probe_validation")
    probe = snapshot["probe_validation"] if isinstance(snapshot["probe_validation"], dict) else {}
    gate_pass = probe_numerical_gate(probe)[0] and probe.get("status") != "FAIL_AT_PROBE_GATE"
    for key, names in {
        "mac": ("raw_mac_telemetry.jsonl",), "master": ("raw_master_telemetry.jsonl",),
        "aligned": ("aligned_telemetry.jsonl",),
        "frames": ("interoceptive_frames.jsonl",),
        "core": ("live_jobs.jsonl", "policy_trace.jsonl", "policy_traces.jsonl", "core_trace.jsonl"),
    }.items():
        snapshot[key] = []
        if key == "core" and not gate_pass:
            continue
        for name in names:
            if (root / name).exists():
                try:
                    snapshot[key], errors = read_jsonl_tail(root / name)
                    snapshot["errors"].extend(errors)
                except OSError:
                    snapshot["errors"].append(f"{name}: 読み取り失敗")
                break
    for name in ("run_summary", "test_results", "live_results", "current_decision", "dashboard_state", "success_criteria", "final_runtime_state"):
        snapshot[name] = None if name in ("run_summary", "test_results", "live_results") and not gate_pass else read_json(name)
    return snapshot


def font():
    available = set(QtGui.QFontDatabase().families())
    for family in ("Hiragino Sans", "Hiragino Kaku Gothic ProN", "Noto Sans CJK JP", "Yu Gothic"):
        if family in available:
            return QtGui.QFont(family, 11)
    return QtWidgets.QApplication.font()


def table(headers):
    widget = QtWidgets.QTableWidget(0, len(headers))
    widget.setHorizontalHeaderLabels(headers)
    widget.setEditTriggers(QtWidgets.QAbstractItemView.NoEditTriggers)
    widget.setAlternatingRowColors(True)
    widget.verticalHeader().hide()
    widget.verticalHeader().setDefaultSectionSize(26)
    widget.horizontalHeader().setStretchLastSection(True)
    return widget


def fill(widget, rows):
    widget.setRowCount(len(rows))
    for row, values in enumerate(rows):
        for column, value in enumerate(values):
            item = QtWidgets.QTableWidgetItem(str(value))
            widget.setItem(row, column, item)


class SnapshotReader(QtCore.QThread):
    ready = QtCore.pyqtSignal(object)

    def __init__(self, root, parent=None):
        super().__init__(parent)
        self.root = root

    def run(self):
        try:
            self.ready.emit(read_snapshot(self.root))
        except Exception as error:
            self.ready.emit({"errors": [f"成果物読み取り失敗: {type(error).__name__}"], "root": str(self.root)})


class BodyDashboard(QtWidgets.QMainWindow):
    def __init__(self, artifacts, poll_ms=2000, autoload=True):
        super().__init__()
        self.artifacts = Path(artifacts).expanduser().resolve()
        self.snapshot = {}
        self.reader = None
        self.setWindowTitle("Kamimusuhi Body — K0-F2 機械的内受容")
        self.resize(1500, 1080)
        self.setFont(font())
        self.setStyleSheet("QMainWindow{background:#f2f5f9;} QLabel#title{font-size:23px;font-weight:600;color:#213b55;} "
                           "QTableWidget{background:white;alternate-background-color:#f3f7fb;gridline-color:#dce5ef;} "
                           "QHeaderView::section{background:#e5ecf4;border:0;padding:6px;} "
                           "QGroupBox{background:white;border:1px solid #d6e0ec;border-radius:7px;margin-top:12px;padding:15px 9px 7px;font-weight:600;} "
                           "QGroupBox::title{subcontrol-origin:margin;left:10px;} QPushButton{padding:5px 12px;}")
        container = QtWidgets.QWidget()
        self.setCentralWidget(container)
        layout = QtWidgets.QVBoxLayout(container)
        title = QtWidgets.QLabel("KAMIMUSUHI BODY  /  K0-F2 機械的内受容")
        title.setObjectName("title")
        layout.addWidget(title)
        layout.addWidget(QtWidgets.QLabel("保存された身体情報と実験結果を表示 • 数値の取得と研究成功を区別します"))
        path_row = QtWidgets.QHBoxLayout()
        self.path_label = QtWidgets.QLabel(str(self.artifacts))
        self.path_label.setTextFormat(QtCore.Qt.PlainText)
        self.path_label.setTextInteractionFlags(QtCore.Qt.TextSelectableByMouse)
        path_row.addWidget(self.path_label, 1)
        choose = QtWidgets.QPushButton("成果物フォルダーを選択")
        choose.clicked.connect(self.choose_folder)
        path_row.addWidget(choose)
        refresh = QtWidgets.QPushButton("今すぐ更新")
        refresh.clicked.connect(self.refresh)
        path_row.addWidget(refresh)
        layout.addLayout(path_row)
        self.status_label = QtWidgets.QLabel("記録を待っています — 未取得を正常値として扱いません")
        self.status_label.setWordWrap(True)
        layout.addWidget(self.status_label)
        self.tabs = QtWidgets.QTabWidget()
        layout.addWidget(self.tabs, 1)
        self._build_body()
        self._build_series()
        self._build_decision()
        self._build_experiment()
        self._build_plots()
        self.timer = QtCore.QTimer(self)
        self.timer.setInterval(max(500, poll_ms))
        self.timer.timeout.connect(self.refresh)
        self.apply_snapshot(read_snapshot(self.artifacts) if autoload else {"errors": [], "root": str(self.artifacts)})
        if autoload:
            self.timer.start()

    def _build_body(self):
        page = QtWidgets.QWidget()
        layout = QtWidgets.QVBoxLayout(page)
        grid = QtWidgets.QGridLayout()
        self.cards = {}
        for index, (key, title) in enumerate((("mac", "Mac — 末梢ノード"), ("master", "中央ノード — CPU / RAM / I/O"),
                                             ("rtx3060", "RTX 3060"), ("p100", "Tesla P100"), ("network", "ノード間通信"), ("core", "非言語 Core"))):
            group = QtWidgets.QGroupBox(title)
            inside = QtWidgets.QVBoxLayout(group)
            label = QtWidgets.QLabel("未取得")
            label.setWordWrap(True)
            label.setTextFormat(QtCore.Qt.RichText)
            label.setMinimumHeight(100)
            inside.addWidget(label)
            self.cards[key] = label
            grid.addWidget(group, index // 3, index % 3)
        layout.addLayout(grid)
        self.frame_summary = QtWidgets.QLabel("InteroceptiveFrame — 未取得")
        self.frame_summary.setWordWrap(True)
        layout.addWidget(self.frame_summary)
        self.frame_table = table(["身体量（固定 20 次元）", "値 [0, 1]", "mask", "取得時の age 秒", "品質", "状態"])
        for column, width in enumerate((325, 120, 70, 150, 100)):
            self.frame_table.setColumnWidth(column, width)
        layout.addWidget(self.frame_table, 1)
        self.tabs.addTab(page, "現在の身体・入力")

    def _build_series(self):
        page = QtWidgets.QWidget()
        layout = QtWidgets.QVBoxLayout(page)
        self.series_note = QtWidgets.QLabel("実測と合成の系列はラベルと線種で区別します。欠損区間は線をつなぎません。")
        layout.addWidget(self.series_note)
        grid = QtWidgets.QGridLayout()
        self.series_plots = {}
        for index, (key, title, unit) in enumerate((("temperature", "温度（Mac 生温度は任意センサー）", "℃"),
                ("utilization", "CPU / GPU 使用率", "%"), ("vram", "VRAM 使用量", "GiB"),
                ("memory", "メモリー圧 proxy / Mac 熱圧", "%"), ("network", "通信 RTT", "ms"), ("action", "Core 行動", "行動"))):
            plot = pg.PlotWidget(background="w")
            plot.setTitle(title, color="#2c465e")
            plot.setLabel("left", unit)
            plot.getAxis("left").enableAutoSIPrefix(False)
            plot.setLabel("bottom", "表示区間先頭からの秒")
            plot.showGrid(x=True, y=True, alpha=.15)
            plot.addLegend(labelTextColor="#294158", labelTextSize="9pt")
            self.series_plots[key] = plot
            grid.addWidget(plot, index // 2, index % 2)
        layout.addLayout(grid, 1)
        self.tabs.addTab(page, "身体と行動の時系列")

    def _build_experiment(self):
        page = QtWidgets.QWidget()
        layout = QtWidgets.QVBoxLayout(page)
        self.research_label = QtWidgets.QLabel("研究成功: 未判定")
        self.research_label.setWordWrap(True)
        layout.addWidget(self.research_label)
        self.runs_table = table(["構造", "seed", "条件", "効用", "成功率", "遅延 / 秒", "状態"])
        for column, width in enumerate((130, 70, 200, 140, 140, 150)):
            self.runs_table.setColumnWidth(column, width)
        layout.addWidget(self.runs_table, 1)
        layout.addWidget(QtWidgets.QLabel("Core の保存済み行動履歴（シミュレーション / 実ジョブの区別は出典を確認）"))
        self.core_table = table(["時刻 / ステップ", "条件", "行動", "hidden / state 要約", "出典"])
        for column, width in enumerate((165, 160, 210, 390)):
            self.core_table.setColumnWidth(column, width)
        layout.addWidget(self.core_table, 1)
        self.runtime_label = QtWidgets.QLabel("最終 runtime: 未取得")
        self.runtime_label.setWordWrap(True)
        layout.addWidget(self.runtime_label)
        self.tabs.addTab(page, "Core・実験結果")

    def _build_decision(self):
        page = QtWidgets.QWidget()
        layout = QtWidgets.QVBoxLayout(page)
        self.decision_note = QtWidgets.QLabel("記録された判断を表示します。未学習時の action・予測効用は未取得です。")
        self.decision_note.setWordWrap(True)
        layout.addWidget(self.decision_note)
        self.decision_table = table(["判断の情報", "保存値"])
        self.decision_table.setColumnWidth(0, 320)
        layout.addWidget(self.decision_table, 2)
        layout.addWidget(QtWidgets.QLabel("action-conditioned probe の予測効用（実測 outcome を予測値として表示しません）"))
        self.prediction_table = table(["候補 action", "予測 utility", "状態"])
        self.prediction_table.setColumnWidth(0, 400)
        self.prediction_table.setColumnWidth(1, 200)
        layout.addWidget(self.prediction_table, 1)
        self.tabs.addTab(page, "Task・予測・資源選択")

    def _build_plots(self):
        page = QtWidgets.QWidget()
        layout = QtWidgets.QVBoxLayout(page)
        self.plot_combo = QtWidgets.QComboBox()
        for filename, title in PLOTS:
            self.plot_combo.addItem(title, filename)
        self.plot_combo.currentIndexChanged.connect(self.show_image)
        layout.addWidget(self.plot_combo)
        scroll = QtWidgets.QScrollArea()
        scroll.setWidgetResizable(True)
        self.image_label = QtWidgets.QLabel("図は未取得")
        self.image_label.setAlignment(QtCore.Qt.AlignCenter)
        scroll.setWidget(self.image_label)
        layout.addWidget(scroll, 1)
        self.tabs.addTab(page, "研究グラフ")

    def choose_folder(self):
        selected = QtWidgets.QFileDialog.getExistingDirectory(self, "K0-F2 成果物フォルダー", str(self.artifacts))
        if selected:
            self.artifacts = Path(selected).resolve()
            self.path_label.setText(str(self.artifacts))
            self.refresh()

    def refresh(self):
        if self.reader is not None and self.reader.isRunning():
            return
        self.reader = SnapshotReader(self.artifacts, self)
        self.reader.ready.connect(self.apply_snapshot)
        self.reader.start()

    def apply_snapshot(self, data):
        if Path(data.get("root", "")).expanduser().resolve() != self.artifacts:
            return
        self.snapshot = data
        mac = data.get("mac", [])
        master = data.get("master", [])
        frames = data.get("frames", [])
        latest_mac = mac[-1] if mac else {}
        latest_master = master[-1] if master else {}
        frame = frames[-1] if frames else {}
        kinds = sorted({row.get("source_kind", "unknown") for row in mac + master + frames})
        kind_text = ", ".join(SOURCE_LABELS.get(kind, "出典不明") for kind in kinds) or "未取得"
        ages = []
        for label, row in (("Mac", latest_mac), ("中央", latest_master), ("frame", frame)):
            stamp = row.get("timestamp")
            ages.append(f"{label}: {max(0, time.time()-stamp):.0f} 秒前" if finite(stamp) else f"{label}: 未取得")
        self.status_label.setText(f"出典: {kind_text}  |  最終記録  {' / '.join(ages)}  |  閲覧専用（稼働状態は別途確認）" +
                                  ("\n" + " / ".join(data.get("errors", [])) if data.get("errors") else ""))
        self._raw_cards(latest_mac, latest_master)
        self._frame(frame)
        self._core(data.get("core", []))
        self._decision(data, frame)
        self._experiment(data)
        self._series(data)
        self.show_image()

    @staticmethod
    def _metric(row, key, scale=1, suffix=""):
        value = row.get("metrics", {}).get(key)
        quality = row.get("quality", {}).get(key, 1 if value is not None else 0)
        if not finite(quality) or quality <= 0 or value is None:
            return "未取得 / 無効"
        if key == "thermal_state":
            value = {"nominal": "平常", "fair": "やや高い", "serious": "高い", "critical": "危険"}.get(value, value)
        if key in ("ac_power", "battery_charging", "network_connectivity") and value in (0, 1):
            value = bool(value)
        return html.escape(display(value, scale, suffix)) + (f"（品質 {quality:.2f}）" if quality < 1 else "")

    def _raw_cards(self, mac, master):
        def card(key, entries):
            self.cards[key].setText("<br>".join(f"<b>{html.escape(label)}</b>　{value}" for label, value in entries))
        m = lambda key, scale=1, suffix="": self._metric(mac, key, scale, suffix)
        s = lambda key, scale=1, suffix="": self._metric(master, key, scale, suffix)
        card("mac", [("熱状態", m("thermal_state")), ("CPU / メモリー圧", m("cpu_utilization", 100, "%") + " / " + m("memory_pressure", 100, "%")),
                     ("バッテリー / 充電中", m("battery_fraction", 100, "%") + " / " + m("battery_charging")), ("AC 電源", m("ac_power"))])
        card("master", [("CPU 温度 / 使用率", s("cpu_temperature_c", suffix=" ℃") + " / " + s("cpu_utilization", 100, "%")),
                        ("CPU load / I/O wait", s("cpu_load_1m") + " / " + s("cpu_iowait", 100, "%")),
                        ("RAM 利用可能 / 圧", s("memory_available_bytes", 1/2**30, " GiB") + " / " + s("memory_pressure", 100, "%")),
                        ("ディスク / I/O 圧", s("disk_busy_fraction", 100, "%") + " / " + s("io_pressure", 100, "%"))])
        for gpu in ("rtx3060", "p100"):
            card(gpu, [("温度", s(gpu + "_temperature_c", suffix=" ℃")), ("使用率", s(gpu + "_utilization", 100, "%")),
                       ("VRAM 使用 / 総量", s(gpu + "_vram_used_bytes", 1/2**30, " GiB") + " / " + s(gpu + "_vram_total_bytes", 1/2**30, " GiB")),
                       ("電力 / 上限", s(gpu + "_power_w", suffix=" W") + " / " + s(gpu + "_power_limit_w", suffix=" W"))])
        card("network", [("Mac → 中央 RTT / loss", m("network_rtt_ms", suffix=" ms") + " / " + m("network_loss", 100, "%")),
                         ("中央 → Mac RTT / loss", s("network_rtt_ms", suffix=" ms") + " / " + s("network_loss", 100, "%")),
                         ("Mac 接続観測", m("network_connectivity")), ("中央 接続観測", s("network_connectivity"))])

    def _frame(self, frame):
        values, masks, qualities, ages = (frame.get(key, []) for key in ("values", "mask", "quality", "age_s"))
        valid_shape = all(len(array) == len(FRAME_NAMES) for array in (values, masks, qualities, ages))
        rows = []
        for index, name in enumerate(FRAME_LABELS):
            valid = valid_shape and masks[index] == 1 and finite(values[index])
            rows.append((name, display(values[index]) if valid else "欠損", display(masks[index]) if valid_shape else "未取得",
                         display(ages[index]) if valid_shape else "未取得", display(qualities[index]) if valid_shape else "未取得",
                         "保存時に取得" if valid else "値を利用しない"))
        fill(self.frame_table, rows)
        identity = frame.get("normalization_identity", "未取得")
        self.frame_summary.setText(f"InteroceptiveFrame  20 値 + 20 mask  |  保存時の取得率: {display(frame.get('availability'), 100, '%')}  |  "
                                    f"正規化: {str(identity)[:18]}  |  出典: {frame.get('source_kind', '未取得')}" +
                                    ("  |  固定長データ不整合" if frame and not valid_shape else ""))

    def _core(self, records):
        latest = records[-1] if records else {}
        action = latest.get("action", latest.get("action_name"))
        hidden = latest.get("hidden_summary", latest.get("state_summary", latest.get("hidden_norm")))
        action = ACTIONS[int(action)] if finite(action) and int(action) == action and 0 <= action < len(ACTIONS) else action
        label = ACTION_LABELS.get(action, display(action)) if isinstance(action, str) else display(action)
        source = SOURCE_LABELS.get(latest.get("source_kind"), display(latest.get("source_kind")))
        self.cards["core"].setText(f"<b>保存済み行動</b>　{html.escape(label)}<br><b>hidden / state</b>　{html.escape(display(hidden))}<br>"
                                   f"<b>条件</b>　{html.escape(display(latest.get('condition', latest.get('mode'))))}<br><b>出典</b>　{html.escape(source)}")
        rows = []
        for record in records[-200:]:
            action = record.get("action", record.get("action_name"))
            action = ACTIONS[int(action)] if finite(action) and int(action) == action and 0 <= action < len(ACTIONS) else action
            rows.append((display(record.get("timestamp", record.get("step"))), display(record.get("condition", record.get("mode"))),
                         ACTION_LABELS.get(action, display(action)) if isinstance(action, str) else display(action),
                         display(record.get("hidden_summary", record.get("state_summary", record.get("hidden_norm")))), display(record.get("source_kind"))))
        fill(self.core_table, rows)

    def _experiment(self, data):
        criteria = data.get("success_criteria")
        status = criteria.get("research_status", criteria.get("overall", criteria.get("status", "未判定"))) if isinstance(criteria, dict) else "未判定"
        probe = data.get("probe_validation")
        if isinstance(probe, dict) and probe.get("gate", {}).get("pass") is False:
            status = "FAIL_AT_PROBE_GATE — 予測gate不合格、policy・test・live未実施"
        self.research_label.setText(f"研究成功: {display(status)}  |  独立単位は training seed / workload block。1 Hz sample 数を独立 n としません。")
        records = data.get("test_results") or data.get("run_summary") or []
        if isinstance(records, dict):
            records = records.get("rows", records.get("records", records.get("results", records.get("runs", []))))
        live = data.get("live_results")
        if isinstance(live, dict):
            records = list(records) + [dict(row, architecture="GRU128", status="新規実ジョブ") for row in live.get("rows", [])]
        rows = []
        for record in records if isinstance(records, list) else []:
            metrics = record.get("metrics", record)
            condition = "独立学習 BLIND" if record.get("training_mode") == "BLIND" else record.get("condition", record.get("mode", record.get("training_mode", "未取得")))
            rows.append((record.get("architecture", "未取得"), display(record.get("seed")), condition,
                         display(metrics.get("utility", metrics.get("reward", record.get("best_validation_utility")))), display(metrics.get("deadline_success_rate", metrics.get("task_success", metrics.get("success_rate"))), 100, "%"),
                         display(metrics.get("latency_seconds", metrics.get("latency_s", metrics.get("completion_time_s")))), record.get("status", "学習完了・validation のみ" if "best_validation_utility" in record else "保存結果")))
        fill(self.runs_table, rows)
        runtime = data.get("final_runtime_state")
        self.runtime_label.setText("最終 runtime: " + (display(runtime)[:800] if runtime else "未取得 — 停止済みとは判断しません"))

    def _decision(self, data, frame):
        records = data.get("core", [])
        latest = records[-1] if records else {}
        state = data.get("current_decision") or data.get("dashboard_state")
        state = state if isinstance(state, dict) else latest
        group = state.get("group_id", state.get("episode_id"))
        related = [row for row in records if group is not None and row.get("group_id", row.get("episode_id")) == group and row.get("seed") == state.get("seed")]
        body = next((row for row in related if row.get("mode") == "BODY"), {})
        blind = next((row for row in related if row.get("mode") in ("TRAINED_BLIND", "independently_trained_BLIND")), {})
        def label(action):
            if finite(action) and int(action) == action and 0 <= action < len(ACTIONS):
                action = ACTIONS[int(action)]
            return ACTION_LABELS.get(action, display(action)) if isinstance(action, str) else display(action)
        measurement = state.get("measurement", {})
        measure = measurement if isinstance(measurement, dict) else {}
        source = state.get("source_kind")
        pairs = [
            ("task", display(state.get("current_task", state.get("task", state.get("task_features"))))),
            ("BODY の選択", label(state.get("body_action", body.get("action")))),
            ("独立学習 BLIND の選択", label(state.get("blind_action", blind.get("action")))),
            ("実際に選択された資源", label(state.get("actual_selected_resource", state.get("action")))),
            ("実測 latency 秒", display(state.get("actual_latency_seconds", state.get("actual_latency", state.get("latency_seconds", measure.get("latency_seconds")))))),
            ("hidden norm", display(state.get("hidden_norm"))),
            ("body timestamp", display(state.get("body_timestamp", frame.get("timestamp")))),
            ("decision timestamp", display(state.get("decision_timestamp", state.get("timestamp")))),
            ("staleness 秒", display(state.get("staleness_seconds", max([value for value in frame.get("age_s", []) if finite(value)], default=None)))),
            ("sensor availability", display(state.get("availability", frame.get("availability")), 100, "%")),
            ("出典", SOURCE_LABELS.get(source, display(source))),
        ]
        fill(self.decision_table, pairs)
        predicted = state.get("predicted_utility_by_action", state.get("probe_predicted_utility"))
        values = predicted if isinstance(predicted, list) and len(predicted) == 4 else [predicted.get(action) for action in ACTIONS] if isinstance(predicted, dict) else [None]*4
        fill(self.prediction_table, [(ACTION_LABELS[action], display(value), "予測記録" if finite(value) else "未取得") for action, value in zip(ACTIONS, values)])
        stamp = state.get("decision_timestamp", state.get("timestamp"))
        self.decision_note.setText(("record-only 収集の保存記録。policy 選択とprobe予測は未実施。" if state.get("stage") == "record_only" else "保存済み判断。") + (f"現在から {max(0, time.time()-stamp):.0f} 秒前の記録。" if finite(stamp) else "判断時刻は未取得。") + " BODY / BLIND の比較は同seed・同groupのみ。実稼働状態の確認とは別です。")

    def _series(self, data):
        for plot in self.series_plots.values():
            plot.clear()
            plot.getPlotItem().legend.clear()
        all_stamps = [row.get("timestamp") for key in ("mac", "master", "core") for row in data.get(key, []) if finite(row.get("timestamp"))]
        origin = min(all_stamps) if all_stamps else 0
        specs = {
            "temperature": (("master", "cpu_temperature_c", "中央 CPU", 1), ("master", "rtx3060_temperature_c", "RTX 3060", 1), ("master", "p100_temperature_c", "P100", 1)),
            "utilization": (("mac", "cpu_utilization", "Mac CPU", 100), ("master", "cpu_utilization", "中央 CPU", 100), ("master", "rtx3060_utilization", "RTX 3060", 100), ("master", "p100_utilization", "P100", 100)),
            "vram": (("master", "rtx3060_vram_used_bytes", "RTX 3060", 1/2**30), ("master", "p100_vram_used_bytes", "P100", 1/2**30)),
            "memory": (("mac", "memory_pressure", "Mac メモリー", 100), ("master", "memory_pressure", "中央 RAM", 100), ("mac", "thermal_pressure", "Mac 熱圧", 100)),
            "network": (("mac", "network_rtt_ms", "Mac → 中央", 1), ("master", "network_rtt_ms", "中央 → Mac", 1))}
        for key, series in specs.items():
            for index, (node, metric, label, scale) in enumerate(series):
                records = data.get(node, [])
                for kind in sorted({row.get("source_kind", "unknown") for row in records}):
                    selected = [row for row in records if finite(row.get("timestamp"))]
                    x, y = [], []
                    for row in selected:
                        value = row.get("metrics", {}).get(metric)
                        quality = row.get("quality", {}).get(metric, 1 if value is not None else 0)
                        x.append(row["timestamp"] - origin)
                        y.append(value * scale if row.get("source_kind", "unknown") == kind and finite(value) and finite(quality) and quality > 0 else float("nan"))
                    if any(math.isfinite(value) for value in y):
                        self.series_plots[key].plot(x, y, connect="finite", name=label + ("（実測）" if kind == "real" else "（合成）" if kind == "synthetic" else "（出典不明）"),
                            pen=pg.mkPen(COLORS[index], width=1.8, style=QtCore.Qt.SolidLine if kind == "real" else QtCore.Qt.DashLine))
        actions = data.get("core", [])
        groups = {(row.get("seed"), row.get("mode", row.get("condition")), row.get("training_mode")) for row in actions}
        action_title = "Core 行動（複数 seed / 条件の保存記録）" if len(groups) > 1 else "Core 行動（保存記録）"
        self.series_plots["action"].setTitle(action_title, color="#2c465e")
        x, y = [], []
        for index, record in enumerate(actions):
            action = record.get("action", record.get("action_name"))
            value = ACTIONS.index(action) if isinstance(action, str) and action in ACTIONS else action if finite(action) else float("nan")
            x.append(record["timestamp"] - origin if finite(record.get("timestamp")) else index)
            y.append(value)
        if y:
            self.series_plots["action"].plot(x, y, pen=pg.mkPen(COLORS[0], width=1.8), symbol="o", symbolSize=3, connect="finite", name="保存済み Core 行動")
        self.series_plots["action"].setLabel("bottom", "表示区間先頭からの秒" if actions and all(finite(row.get("timestamp")) for row in actions) else "保存済み行動の順序")
        self.series_plots["action"].getAxis("left").setTicks([[(index, ACTION_LABELS[action]) for index, action in enumerate(ACTIONS)]])
        no_data = [name for key, name in (("temperature", "温度"), ("utilization", "使用率"), ("vram", "VRAM"), ("memory", "メモリー"), ("network", "RTT"), ("action", "Core 行動")) if not self.series_plots[key].listDataItems()]
        self.series_note.setText("実測: 実線 / 合成: 破線。欠損は線をつなぎません。直近の保存記録を最大 1,800 行表示。" + ("  未取得: " + "、".join(no_data) if no_data else ""))

    def show_image(self):
        name = self.plot_combo.currentData()
        path = self.artifacts / str(name)
        if not path.exists():
            self.image_label.setPixmap(QtGui.QPixmap())
            self.image_label.setText("図は未取得です。実験結果の保存後に表示します。")
            return
        pixmap = QtGui.QPixmap(str(path))
        if pixmap.isNull():
            self.image_label.setText("画像の読み取りに失敗しました")
            return
        self.image_label.setPixmap(pixmap.scaled(1360, 820, QtCore.Qt.KeepAspectRatio, QtCore.Qt.SmoothTransformation))

    def closeEvent(self, event):
        self.timer.stop()
        if self.reader is not None and self.reader.isRunning():
            self.reader.wait(5000)
        super().closeEvent(event)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifacts", default="experiments/k0_f2_interoception_confirmatory/artifacts/review")
    parser.add_argument("--poll-ms", type=int, default=2000)
    args = parser.parse_args()
    application = QtWidgets.QApplication(sys.argv[:1])
    application.setFont(font())
    dashboard = BodyDashboard(args.artifacts, args.poll_ms)
    dashboard.show()
    return application.exec_()


if __name__ == "__main__":
    raise SystemExit(main())
