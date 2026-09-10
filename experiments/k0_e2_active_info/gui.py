"""Read-only Japanese desktop dashboard for synchronized E2 artifacts.

No torch, remote server, training controls, or writes to experiment artifacts.
Run: python -m experiments.k0_e2_active_info.gui --artifacts PATH
"""
from __future__ import annotations
import argparse
import json
import math
import sys
from datetime import datetime
from pathlib import Path

from PyQt5 import QtCore, QtGui, QtWidgets
import pyqtgraph as pg

ACTIONS = ("無視", "待機", "注意を向ける", "観察", "想起", "言語器官を呼ぶ")
SCENARIOS = {"language": "言語だけで判別できる課題", "memory": "遅延記憶", "orient": "注意による観測改善",
             "observe": "追加センサー取得", "habituation": "慣れと再反応", "irrelevant": "無関係な刺激"}
SENSORS = ("動き", "動きの変化", "音量", "明るさ", "近接", "接触", "温度の変化", "課題の関連度",
           "発話活動", "取得情報の値", "取得情報の信頼度", "資源", "言語呼び出し費用", "言語応答の信頼性",
           "最終判断の準備", "言語応答の遅延")
STATUS = {"CALL": "言語呼び出し", "WAITING": "応答待ち", "RESPONSE": "情報受信", "NEXT_ACTION": "次の行動", "running": "学習中", "complete": "完了", "failed": "失敗", "queued": "待機中", "paused": "一時停止中",
          "imitation": "教師模倣", "ppo": "PPO", "missing": "未取得", "pending": "応答待ち", "ok": "正常",
          "timeout": "時間切れ", "LANGUAGE_BACKEND_UNAVAILABLE": "言語器官を利用できません"}
PLOTS = [("voi_cost_curve.png", "費用と呼び出し率"), ("voi_performance_curve.png", "費用と課題性能"),
         ("reliability_curve.png", "信頼性と成功率"), ("latency_curve.png", "遅延と性能"),
         ("policy_intervention.png", "言語ゲートへの介入"), ("response_ablation.png", "応答情報への介入"),
         ("retention_curve.png", "記憶保持曲線"), ("ppo_ablation.png", "PPOの要因比較"),
         ("ood_heatmap.png", "未知分布への耐性"), ("pareto.png", "性能と費用の比較")]
FORBIDDEN = {"latent", "latent_state", "true_target", "target", "truth", "answer", "hidden_truth", "oracle_answer"}


def display(value):
    if value is None:
        return "—"
    if isinstance(value, bool):
        return "はい" if value else "いいえ"
    if isinstance(value, float):
        return f"{value:.4g}" if math.isfinite(value) else "—"
    if isinstance(value, (dict, list)):
        return json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    return str(value)


def public_information(value):
    if isinstance(value, dict):
        return {key: public_information(item) for key, item in value.items()
                if key.lower() not in FORBIDDEN and "latent" not in key.lower()}
    if isinstance(value, list):
        return [public_information(item) for item in value]
    return value


def japanese_font():
    available = set(QtGui.QFontDatabase().families())
    for family in ("Hiragino Sans", "Hiragino Kaku Gothic ProN", "Noto Sans CJK JP", "Yu Gothic"):
        if family in available:
            return QtGui.QFont(family, 11)
    return QtWidgets.QApplication.font()


def table(headers):
    result = QtWidgets.QTableWidget(0, len(headers))
    result.setHorizontalHeaderLabels(headers)
    result.setEditTriggers(QtWidgets.QAbstractItemView.NoEditTriggers)
    result.setSelectionBehavior(QtWidgets.QAbstractItemView.SelectRows)
    result.setSelectionMode(QtWidgets.QAbstractItemView.SingleSelection)
    result.setAlternatingRowColors(True)
    result.verticalHeader().setVisible(False)
    result.horizontalHeader().setStretchLastSection(True)
    return result


class E2Dashboard(QtWidgets.QMainWindow):
    def __init__(self, artifacts, poll_ms=5000):
        super().__init__()
        self.artifacts = Path(artifacts).expanduser().resolve()
        self.cache = {}
        self.run_rows = []
        self.representatives = []
        self.episodes = []
        self.steps = []
        self._image_signature = None
        self.setWindowTitle("カミムスヒ K0-E2 — 能動的情報取得")
        self.resize(1440, 960)
        self.setFont(japanese_font())
        self.setStyleSheet("QMainWindow{background:#f3f5f8;} QLabel#title{font-size:23px;font-weight:600;color:#182d45;} "
                           "QTableWidget{background:white;alternate-background-color:#f4f7fb;gridline-color:#e3e8ef;} "
                           "QHeaderView::section{background:#e5ebf3;border:0;padding:7px;color:#243b55;} "
                           "QTabWidget::pane{border:1px solid #d6dfeb;} QPushButton{padding:6px 13px;} "
                           "QGroupBox{font-weight:600;margin-top:10px;padding-top:12px;}")
        container = QtWidgets.QWidget()
        self.setCentralWidget(container)
        layout = QtWidgets.QVBoxLayout(container)
        title = QtWidgets.QLabel("KAMIMUSUHI  /  K0-E2 能動的情報取得")
        title.setObjectName("title")
        layout.addWidget(title)
        subtitle = QtWidgets.QLabel("同期済み成果物の閲覧専用  •  学習はリモート側で継続します")
        layout.addWidget(subtitle)
        path_row = QtWidgets.QHBoxLayout()
        self.path_label = QtWidgets.QLabel(str(self.artifacts))
        self.path_label.setTextFormat(QtCore.Qt.PlainText)
        self.path_label.setTextInteractionFlags(QtCore.Qt.TextSelectableByMouse)
        path_row.addWidget(self.path_label, 1)
        choose = QtWidgets.QPushButton("成果物フォルダーを選択")
        choose.clicked.connect(self.choose_artifacts)
        path_row.addWidget(choose)
        reload_button = QtWidgets.QPushButton("今すぐ更新")
        reload_button.clicked.connect(self.refresh)
        path_row.addWidget(reload_button)
        layout.addLayout(path_row)
        self.summary_label = QtWidgets.QLabel("成果物を読み込み中")
        layout.addWidget(self.summary_label)
        self.tabs = QtWidgets.QTabWidget()
        layout.addWidget(self.tabs, 1)
        self._build_runs()
        self._build_episode()
        self._build_images()
        self.timer = QtCore.QTimer(self)
        self.timer.setInterval(poll_ms)
        self.timer.timeout.connect(self.refresh)
        self.refresh()
        self.timer.start()

    def _build_runs(self):
        page = QtWidgets.QWidget()
        layout = QtWidgets.QVBoxLayout(page)
        self.runs_table = table(["実験", "構造", "seed", "比較条件", "状態", "更新", "成功率", "呼び出し率", "GPU"])
        self.runs_table.itemSelectionChanged.connect(self.select_run)
        for column, width in enumerate((210, 110, 60, 100, 100, 65, 90, 110)):
            self.runs_table.setColumnWidth(column, width)
        self.runs_table.setMinimumHeight(210)
        self.runs_table.setMaximumHeight(290)
        layout.addWidget(self.runs_table)
        self.run_detail = QtWidgets.QLabel("実験を選択してください")
        self.run_detail.setTextFormat(QtCore.Qt.PlainText)
        layout.addWidget(self.run_detail)
        graphs = QtWidgets.QWidget()
        grid = QtWidgets.QGridLayout(graphs)
        self.curves = {}
        for i, (key, name) in enumerate((("reward", "検証時の報酬"), ("task_success", "課題成功率"),
                                         ("call_rate", "言語呼び出し率"), ("steps_per_second", "学習速度（ステップ / 秒）"))):
            plot = pg.PlotWidget(background="w", title=name)
            plot.getPlotItem().setTitle(name, color="#334b66")
            plot.showGrid(x=True, y=True, alpha=.15)
            plot.setLabel("bottom", "更新回数")
            plot.getAxis("left").setTextPen("#42566f")
            self.curves[key] = plot.plot(pen=pg.mkPen("#227cba", width=2))
            grid.addWidget(plot, i // 2, i % 2)
        layout.addWidget(graphs, 1)
        self.scenario_table = table(["課題", "成功率"])
        self.scenario_table.setColumnWidth(0, 320)
        self.scenario_table.setMaximumHeight(220)
        self.scenario_table.setMinimumHeight(220)
        layout.addWidget(self.scenario_table)
        self.tabs.addTab(page, "実験と学習曲線")

    def _build_episode(self):
        page = QtWidgets.QWidget()
        layout = QtWidgets.QVBoxLayout(page)
        top = QtWidgets.QHBoxLayout()
        top.addWidget(QtWidgets.QLabel("代表実験"))
        self.representative_combo = QtWidgets.QComboBox()
        self.representative_combo.currentIndexChanged.connect(self.select_representative)
        top.addWidget(self.representative_combo, 1)
        top.addWidget(QtWidgets.QLabel("エピソード"))
        self.episode_combo = QtWidgets.QComboBox()
        self.episode_combo.currentIndexChanged.connect(self.select_episode)
        top.addWidget(self.episode_combo, 1)
        self.debug_checkbox = QtWidgets.QCheckBox("デバッグ：潜在状態を表示")
        self.debug_checkbox.toggled.connect(self.show_step)
        top.addWidget(self.debug_checkbox)
        layout.addLayout(top)
        self.episode_summary = QtWidgets.QLabel("J72評価の記録を待っています")
        layout.addWidget(self.episode_summary)
        splitter = QtWidgets.QSplitter()
        self.timeline_table = table(["時刻", "行動", "言語の状態", "取得情報", "費用"])
        for column, width in enumerate((65, 165, 150, 245)):
            self.timeline_table.setColumnWidth(column, width)
        self.timeline_table.itemSelectionChanged.connect(self.timeline_selected)
        splitter.addWidget(self.timeline_table)
        details = QtWidgets.QWidget()
        detail_layout = QtWidgets.QVBoxLayout(details)
        self.step_slider = QtWidgets.QSlider(QtCore.Qt.Horizontal)
        self.step_slider.valueChanged.connect(self.show_step)
        detail_layout.addWidget(self.step_slider)
        self.step_summary = QtWidgets.QLabel("ステップの記録なし")
        self.step_summary.setTextFormat(QtCore.Qt.PlainText)
        self.step_summary.setWordWrap(True)
        detail_layout.addWidget(self.step_summary)
        self.sensor_table = table(["センサー / インターフェース", "現在値"])
        self.sensor_table.setColumnWidth(0, 300)
        self.sensor_table.setRowCount(16)
        for i, name in enumerate(SENSORS):
            self.sensor_table.setItem(i, 0, QtWidgets.QTableWidgetItem(name))
        detail_layout.addWidget(self.sensor_table, 1)
        self.debug_label = QtWidgets.QLabel()
        self.debug_label.setTextFormat(QtCore.Qt.PlainText)
        self.debug_label.setWordWrap(True)
        self.debug_label.setStyleSheet("background:#fff4dd;padding:8px;color:#61451b;")
        self.debug_label.hide()
        detail_layout.addWidget(self.debug_label)
        splitter.addWidget(details)
        splitter.setSizes([750, 550])
        layout.addWidget(splitter, 1)
        self.tabs.addTab(page, "行動・取得情報・言語応答")

    def _build_images(self):
        page = QtWidgets.QWidget()
        layout = QtWidgets.QVBoxLayout(page)
        self.plot_combo = QtWidgets.QComboBox()
        for filename, title in PLOTS:
            self.plot_combo.addItem(title, filename)
        self.plot_combo.currentIndexChanged.connect(self.show_image)
        layout.addWidget(self.plot_combo)
        scroll = QtWidgets.QScrollArea()
        scroll.setWidgetResizable(True)
        self.image_label = QtWidgets.QLabel()
        self.image_label.setAlignment(QtCore.Qt.AlignCenter)
        scroll.setWidget(self.image_label)
        layout.addWidget(scroll)
        self.tabs.addTab(page, "評価グラフ")

    def _json(self, path, default):
        try:
            value = json.loads(path.read_text())
            self.cache[str(path)] = value
            return value
        except (OSError, ValueError):
            return self.cache.get(str(path), default)

    def choose_artifacts(self):
        chosen = QtWidgets.QFileDialog.getExistingDirectory(self, "E2成果物フォルダー", str(self.artifacts))
        if chosen:
            self.set_artifacts(chosen)

    def set_artifacts(self, path):
        self.artifacts = Path(path).expanduser().resolve()
        self.path_label.setText(str(self.artifacts))
        self.cache.clear()
        self._image_signature = None
        self.refresh()

    def refresh(self):
        selected = self.selected_run_id()
        rows = []
        for path in sorted((self.artifacts / "runs").glob("*/status.json")):
            row = self._json(path, {})
            if isinstance(row, dict) and row:
                row = dict(row, run_id=row.get("run_id", path.parent.name), _directory=path.parent)
                rows.append(row)
        self.run_rows = rows
        self.runs_table.blockSignals(True)
        self.runs_table.setRowCount(len(rows))
        selected_index = 0
        for i, row in enumerate(rows):
            if row["run_id"] == selected:
                selected_index = i
            val = row.get("validation") or {}
            values = [row["run_id"], row.get("architecture"), row.get("seed"), row.get("arm"),
                      STATUS.get(row.get("status"), row.get("status")), row.get("update"),
                      val.get("task_success"), val.get("call_rate"), row.get("gpu_uuid")]
            for j, value in enumerate(values):
                item = QtWidgets.QTableWidgetItem(display(value))
                if j == 0:
                    item.setData(QtCore.Qt.UserRole, row["run_id"])
                self.runs_table.setItem(i, j, item)
        if rows:
            self.runs_table.selectRow(selected_index)
        self.runs_table.blockSignals(False)
        completed = sum(row.get("status") == "complete" for row in rows)
        running = sum(row.get("status") == "running" for row in rows)
        failed = sum(row.get("status") == "failed" for row in rows)
        recorded_times = [row.get("timestamp") for row in rows if isinstance(row.get("timestamp"), (int, float))]
        latest = datetime.fromtimestamp(max(recorded_times)).strftime("%m/%d %H:%M:%S") if recorded_times else "—"
        self.summary_label.setText(f"保存済み {len(rows)} 実験  ｜  学習中 {running}  ｜  完了 {completed}  ｜  失敗 {failed}  ｜  最新記録 {latest}  ｜  読込 {datetime.now():%H:%M:%S}（5秒ごと）")
        self.select_run()
        current_rep = self.representative_combo.currentText()
        episode_index = self.episode_combo.currentIndex()
        step = self.step_slider.value()
        results = self._json(self.artifacts / "j72_results.json", {})
        representatives = results.get("representatives", []) if isinstance(results, dict) else []
        standalone = self._json(self.artifacts / "episode_trace.json", {})
        if not representatives and isinstance(standalone, dict) and standalone:
            representatives = standalone.get("representatives") or [standalone]
        self.representatives = [item for item in representatives if isinstance(item, dict)]
        self.representative_combo.blockSignals(True)
        self.representative_combo.clear()
        for item in self.representatives:
            self.representative_combo.addItem(str(item.get("run_id", "評価記録")))
        index = self.representative_combo.findText(current_rep)
        if index >= 0:
            self.representative_combo.setCurrentIndex(index)
        self.representative_combo.blockSignals(False)
        self.select_representative()
        if episode_index >= 0 and episode_index < self.episode_combo.count():
            self.episode_combo.setCurrentIndex(episode_index)
        self.step_slider.setValue(min(step, self.step_slider.maximum()))
        self.show_image()

    def selected_run_id(self):
        row = self.runs_table.currentRow() if hasattr(self, "runs_table") else -1
        item = self.runs_table.item(row, 0) if row >= 0 else None
        return item.data(QtCore.Qt.UserRole) if item else None

    def select_run(self):
        selected = self.selected_run_id()
        row = next((item for item in self.run_rows if item["run_id"] == selected), None)
        lines = []
        if row:
            try:
                for line in (row["_directory"] / "metrics.jsonl").read_text().splitlines():
                    try:
                        parsed = json.loads(line)
                        if isinstance(parsed, dict):
                            lines.append(parsed)
                    except ValueError:
                        continue
            except OSError:
                pass
        for key, curve in self.curves.items():
            x, y = [], []
            for line in lines:
                val = line.get(key) if key == "steps_per_second" else (line.get("validation") or {}).get(key)
                if isinstance(val, (int, float)) and math.isfinite(val):
                    x.append(line.get("update", len(x)))
                    y.append(val)
            curve.setData(x, y)
        if not row:
            self.run_detail.setText("同期された実験記録がまだありません")
        else:
            self.run_detail.setText(f"{row['run_id']}  ｜  {STATUS.get(row.get('stage'), row.get('stage', '—'))}  ｜  累計遷移 {display(row.get('transitions'))}")
        per_scenario = ((row or {}).get("validation") or {}).get("scenario_success", {})
        summary = self._json(self.artifacts / "run_summary.json", [])
        summary_rows = summary if isinstance(summary, list) else summary.get("runs", summary.get("rows", [])) if isinstance(summary, dict) else []
        if isinstance(summary_rows, list):
            for entry in summary_rows:
                if isinstance(entry, dict) and entry.get("run_id") == selected:
                    per_scenario = entry.get("scenario_success", entry.get("per_scenario", per_scenario))
        self.scenario_table.setRowCount(len(SCENARIOS))
        for i, (key, name) in enumerate(SCENARIOS.items()):
            value = per_scenario.get(key) if isinstance(per_scenario, dict) else None
            if isinstance(value, dict):
                value = value.get("success", value.get("task_success"))
            self.scenario_table.setItem(i, 0, QtWidgets.QTableWidgetItem(name))
            self.scenario_table.setItem(i, 1, QtWidgets.QTableWidgetItem(display(value)))

    def select_representative(self):
        index = self.representative_combo.currentIndex()
        representative = self.representatives[index] if 0 <= index < len(self.representatives) else {}
        self.episodes = representative.get("episodes", [])
        self.episode_combo.blockSignals(True)
        self.episode_combo.clear()
        for i, episode in enumerate(self.episodes):
            self.episode_combo.addItem(f"エピソード {episode.get('episode_id', i)}")
        self.episode_combo.blockSignals(False)
        self.select_episode()

    def select_episode(self):
        index = self.episode_combo.currentIndex()
        episode = self.episodes[index] if 0 <= index < len(self.episodes) else {}
        self.steps = episode.get("steps", [])
        self.episode_summary.setText(f"記録 {len(self.steps)} ステップ  ｜  最終成功：{display(episode.get('success'))}" if episode else "J72評価の記録を待っています")
        self.timeline_table.blockSignals(True)
        self.timeline_table.setRowCount(len(self.steps))
        for i, step in enumerate(self.steps):
            action = step.get("action")
            name = ACTIONS[action] if isinstance(action, int) and 0 <= action < 6 else display(action)
            values = [step.get("t", i), name, STATUS.get(step.get("status"), step.get("status")),
                      public_information(step.get("information")), step.get("cost")]
            for j, value in enumerate(values):
                self.timeline_table.setItem(i, j, QtWidgets.QTableWidgetItem(display(value)))
        self.timeline_table.blockSignals(False)
        self.step_slider.setRange(0, max(0, len(self.steps) - 1))
        self.step_slider.setValue(0)
        self.show_step()

    def timeline_selected(self):
        row = self.timeline_table.currentRow()
        if row >= 0:
            self.step_slider.setValue(row)

    def show_step(self, *_):
        i = self.step_slider.value()
        step = self.steps[i] if 0 <= i < len(self.steps) else {}
        observation = step.get("observation", [])
        for j in range(16):
            value = observation[j] if isinstance(observation, list) and j < len(observation) else None
            self.sensor_table.setItem(j, 1, QtWidgets.QTableWidgetItem(display(value)))
        action = step.get("action")
        action_name = ACTIONS[action] if isinstance(action, int) and 0 <= action < 6 else display(action)
        self.step_summary.setText(f"時刻 {display(step.get('t'))}  ｜  行動：{action_name}\n"
                                  f"内部状態ノルム {display(step.get('hidden_norm'))}  ｜  資源 {display(step.get('resource'))}\n"
                                  f"言語の状態：{STATUS.get(step.get('status'), display(step.get('status')))}  ｜  遅延 {display(step.get('language_latency'))}\n"
                                  f"費用 {display(step.get('cost'))}  ｜  取得情報：{display(public_information(step.get('information')))}")
        if self.debug_checkbox.isChecked():
            hidden = {key: value for key, value in step.items() if key.lower() in FORBIDDEN or "latent" in key.lower()}
            self.debug_label.setText("デバッグ専用・policy入力ではありません：" + display(hidden or None))
            self.debug_label.show()
        else:
            self.debug_label.clear()
            self.debug_label.hide()
        if self.steps:
            self.timeline_table.blockSignals(True)
            self.timeline_table.selectRow(i)
            self.timeline_table.blockSignals(False)

    def show_image(self, *_):
        name = self.plot_combo.currentData()
        if not name:
            return
        path = self.artifacts / name
        try:
            signature = (str(path), path.stat().st_mtime_ns)
        except OSError:
            signature = (str(path), None)
        if signature == self._image_signature:
            return
        self._image_signature = signature
        pixmap = QtGui.QPixmap(str(path)) if signature[1] is not None else QtGui.QPixmap()
        if pixmap.isNull():
            self.image_label.setPixmap(QtGui.QPixmap())
            self.image_label.setText("この評価グラフはまだ生成されていません")
        else:
            self.image_label.setText("")
            self.image_label.setPixmap(pixmap.scaledToWidth(min(1250, pixmap.width()), QtCore.Qt.SmoothTransformation))


def main():
    parser = argparse.ArgumentParser(description="K0-E2 日本語成果物ビューアー")
    parser.add_argument("--artifacts", required=True)
    args = parser.parse_args()
    application = QtWidgets.QApplication(sys.argv)
    application.setFont(japanese_font())
    window = E2Dashboard(args.artifacts)
    window.show()
    sys.exit(application.exec_())


if __name__ == "__main__":
    main()
