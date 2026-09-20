"""Thin PyQt5 HTTP/WebSocket monitor. No model or training dependency."""
import argparse
import json
import sys
import time
from datetime import datetime

from PyQt5 import QtCore, QtGui, QtNetwork, QtWidgets, QtWebSockets
import pyqtgraph as pg

ACTIONS = ("IGNORE", "WAIT", "ORIENT", "OBSERVE", "RECALL", "INVOKE_LANGUAGE")
ACTION_LABELS = ("無視", "待機", "注意を向ける", "観察", "想起", "言語系を呼ぶ")
COMMAND_LABELS = {"start": "開始", "pause": "一時停止", "resume": "再開", "stop": "停止"}
STATUS_LABELS = {"queued": "待機中", "running": "学習中", "paused": "一時停止中", "complete": "完了", "failed": "失敗", "stopped": "停止済み", "pending": "受付待ち", "ok": "成功", "VIRTUAL_ORACLE": "仮想オラクル", "LANGUAGE_BACKEND_UNAVAILABLE": "言語バックエンド利用不可"}
SCENARIOS = ("待機・ノイズ", "反復刺激", "突然の新奇刺激", "遅延手がかり", "目標の競合", "言語処理が必要", "持続する異常", "言語系の誤誘発")
SCENARIO_KEYS = ("idle_noise", "repeated_stimulus", "sudden_novelty", "delayed_cue", "conflicting_goal", "language_required", "persistent_anomaly", "false_language_trigger")
STATE_LABELS = {"hidden_norm": "内部状態ノルム", "hidden_variance": "内部状態の分散", "state_retention_score": "状態保持スコア", "habituation_score": "慣れスコア", "habituation_sequence_score": "系列での慣れスコア", "novelty_response_score": "新奇刺激への反応", "novelty": "新奇性", "salience": "顕著性", "persistence": "持続性", "module_sensory": "感覚モジュール", "module_salience": "顕著性モジュール", "module_persistence": "状態保持モジュール", "module_action_selection": "行動選択モジュール"}
ERROR_LABELS = {
    "invalid run ID": "実験IDが正しくありません。", "unknown run": "指定した実験が見つかりません。",
    "unknown configuration": "指定した設定が見つかりません。", "run ID already exists": "同じ実験IDが既に存在します。",
    "run is not active": "この実験は現在実行中ではありません。", "trainer failed to launch": "学習プロセスを起動できませんでした。",
    "experiment queue owns scheduling; wait until the queue finishes": "実験キューが実行中です。全実験の終了後に開始してください。",
    "GPU process inventory unavailable": "GPUの利用状況を確認できません。", "requested GPU unavailable": "指定したGPUを利用できません。",
    "GPU occupied by an existing compute process": "指定したGPUは別の計算プロセスが使用中です。",
    "GPU already assigned to an active run": "指定したGPUには実行中の実験が割り当てられています。",
    "saved configuration or checkpoint unavailable": "保存済み設定またはチェックポイントが見つかりません。",
    "browser-origin controls disabled": "ブラウザー経由の操作は許可されていません。",
}
GRAPHS = (("reward_mean", "平均報酬"), ("task_success", "課題成功率"),
          ("llm_call_rate", "LLM呼び出し率"), ("missed_llm_rate", "必要なLLM呼び出しの見逃し率"),
          ("false_llm_call_rate", "不要なLLM呼び出し率"), ("steps_per_second", "学習処理速度（ステップ / 秒）"))


def display(value):
    if value is None:
        return "—"
    if isinstance(value, bool):
        return "はい" if value else "いいえ"
    if isinstance(value, float):
        return f"{value:.4g}"
    if isinstance(value, (dict, list)):
        return json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    return str(value)


def memory_mb(value):
    return "—" if value is None else f"{value:,.0f}"


def action_name(value):
    if isinstance(value, str) and value in ACTIONS:
        return ACTION_LABELS[ACTIONS.index(value)]
    return ACTION_LABELS[value] if isinstance(value, int) and 0 <= value < len(ACTIONS) else display(value)


def scenario_name(value):
    if isinstance(value, int) and 0 <= value < len(SCENARIOS):
        return SCENARIOS[value]
    if value in SCENARIO_KEYS:
        return SCENARIOS[SCENARIO_KEYS.index(value)]
    return "評価" if value == "evaluation" else display(value)


def japanese_font():
    families = set(QtGui.QFontDatabase().families())
    for family in ("Hiragino Sans", "Hiragino Kaku Gothic ProN", "Noto Sans CJK JP", "Yu Gothic"):
        if family in families:
            return QtGui.QFont(family, 11)
    return QtWidgets.QApplication.font()


def human_time(value):
    if value is None:
        return "—"
    try:
        if isinstance(value, (int, float)):
            moment = datetime.fromtimestamp(value).astimezone()
        else:
            moment = datetime.fromisoformat(str(value).replace("Z", "+00:00")).astimezone()
        return moment.strftime("%Y-%m-%d %H:%M:%S %Z")
    except (ValueError, TypeError, OverflowError, OSError):
        return display(value)


def timestamp(value):
    try:
        return float(value)
    except (ValueError, TypeError):
        try:
            return datetime.fromisoformat(str(value).replace("Z", "+00:00")).timestamp()
        except ValueError:
            return 0.0


class Monitor(QtWidgets.QMainWindow):
    def __init__(self, server):
        super().__init__()
        application = QtWidgets.QApplication.instance()
        application.setFont(japanese_font())
        if not hasattr(application, "_k0_ja_translator"):
            translator = QtCore.QTranslator(application)
            if translator.load("qtbase_ja", QtCore.QLibraryInfo.location(QtCore.QLibraryInfo.TranslationsPath)):
                application.installTranslator(translator)
            application._k0_ja_translator = translator
        self.server = server.rstrip("/")
        self.histories = {}
        self.selected = None
        self.episode_data = []
        self.reconnect_seconds = 1
        self.closing = False
        self.setWindowTitle("KAMIMUSUHI · K0 人工脳幹")
        self.resize(1500, 1000)
        self.network = QtNetwork.QNetworkAccessManager(self)
        self.ws = QtWebSockets.QWebSocket()
        self.ws.connected.connect(self.connected)
        self.ws.disconnected.connect(self.disconnected)
        self.ws.textMessageReceived.connect(self.on_message)
        self.ws.error.connect(self.websocket_error)
        self.reconnect = QtCore.QTimer(self)
        self.reconnect.setSingleShot(True)
        self.reconnect.timeout.connect(self.connect_ws)
        self.refresh = QtCore.QTimer(self)
        self.refresh.timeout.connect(self.refresh_selected)
        self.build_ui()
        self.refresh.start(10000)
        self.request("/api/configs", self.set_configs)
        self.request("/api/runs", self.update_runs)
        self.request("/api/system", self.update_system)
        self.connect_ws()

    def build_ui(self):
        pg.setConfigOptions(antialias=True, background="#101c29", foreground="#c3d2e2")
        self.setStyleSheet("""
          QWidget { background: #101c29; color: #d8e7f5; font-size: 12px; }
          QMainWindow, QSplitter { background: #0b1420; }
          QTableWidget, QTextEdit, QComboBox { background: #152436; border: 1px solid #2b4156; }
          QHeaderView::section { background: #23384d; padding: 5px; border: 0; }
          QPushButton { background: #234b61; border: 1px solid #3c667c; padding: 7px 13px; border-radius: 3px; }
          QPushButton:hover { background: #32657c; }
          QGroupBox { border: 1px solid #294258; margin-top: 12px; padding-top: 10px; }
          QGroupBox::title { subcontrol-origin: margin; left: 10px; color: #6fe0cf; }
          QTabBar::tab { background: #20364a; padding: 7px 15px; }
          QTabBar::tab:selected { background: #326078; }
        """)
        central = QtWidgets.QWidget()
        self.setCentralWidget(central)
        layout = QtWidgets.QVBoxLayout(central)
        header = QtWidgets.QHBoxLayout()
        title = QtWidgets.QLabel("KAMIMUSUHI  <span style='color:#6fe0cf'>K0 人工脳幹</span>")
        title.setStyleSheet("font-size: 18px; font-weight: bold;")
        header.addWidget(title)
        header.addStretch()
        self.connection_label = QtWidgets.QLabel("接続しています…")
        self.connection_label.setMinimumWidth(170)
        header.addWidget(self.connection_label)
        layout.addLayout(header)
        self.system_label = QtWidgets.QLabel("llm_master：計測データを待っています")
        self.system_label.setWordWrap(True)
        layout.addWidget(self.system_label)
        controls = QtWidgets.QHBoxLayout()
        self.config_choice = QtWidgets.QComboBox()
        self.config_choice.setMinimumWidth(155)
        self.config_choice.addItem("smoke.yaml")
        self.device_choice = QtWidgets.QComboBox()
        self.device_choice.setMinimumWidth(90)
        self.device_choice.addItem("cpu")
        controls.addWidget(QtWidgets.QLabel("設定"))
        controls.addWidget(self.config_choice)
        controls.addWidget(QtWidgets.QLabel("実行デバイス"))
        controls.addWidget(self.device_choice)
        for command, label in COMMAND_LABELS.items():
            button = QtWidgets.QPushButton(label)
            button.clicked.connect(lambda checked=False, c=command.lower(): self.control(c))
            controls.addWidget(button)
        controls.addStretch()
        controls.addWidget(QtWidgets.QLabel("表示期間"))
        self.period = QtWidgets.QComboBox()
        self.period.setMinimumWidth(90)
        self.period.addItems(["1分", "5分", "全期間"])
        self.period.setCurrentText("全期間")
        self.period.currentTextChanged.connect(self.draw_selected)
        controls.addWidget(self.period)
        layout.addLayout(controls)
        self.runs = self.table(["実験", "モデル構成", "シード", "GPU / デバイス", "ステップ", "報酬", "課題成功率", "LLM呼び出し率 %", "状態"])
        self.runs.setMaximumHeight(190)
        self.runs.setSelectionBehavior(QtWidgets.QAbstractItemView.SelectRows)
        self.runs.setSelectionMode(QtWidgets.QAbstractItemView.SingleSelection)
        self.runs.itemSelectionChanged.connect(self.select_run)
        layout.addWidget(self.runs)
        graphs = QtWidgets.QGridLayout()
        self.plots, self.curves = {}, {}
        colors = ["#6fe0cf", "#8ebeff", "#efc66c", "#ff9797", "#d6adff", "#82d99d"]
        for index, ((key, label), color) in enumerate(zip(GRAPHS, colors)):
            plot = pg.PlotWidget(title=label)
            plot.showGrid(x=True, y=True, alpha=.15)
            plot.setLabel("bottom", "経過時間（秒）")
            if key in ("task_success", "llm_call_rate", "missed_llm_rate", "false_llm_call_rate"):
                plot.setYRange(0, 1)
            self.plots[key] = plot
            self.curves[key] = plot.plot(pen=pg.mkPen(color, width=2))
            graphs.addWidget(plot, index // 3, index % 3)
        layout.addLayout(graphs, 3)
        lower = QtWidgets.QHBoxLayout()
        self.actions = pg.PlotWidget(title="行動の分布")
        self.actions.setMaximumWidth(370)
        self.actions.getAxis("bottom").setTicks([list(enumerate(["無視", "待機", "注意", "観察", "想起", "言語"]))])
        self.actions.setToolTip("無視・待機・注意を向ける・観察・想起・言語系を呼ぶ、の選択割合")
        self.actions.setYRange(0, 1)
        self.action_bars = pg.BarGraphItem(x=list(range(6)), height=[0]*6, width=.6, brush="#6fe0cf")
        self.actions.addItem(self.action_bars)
        lower.addWidget(self.actions)
        brain_box = QtWidgets.QGroupBox("内部状態 / モジュール")
        brain_layout = QtWidgets.QVBoxLayout(brain_box)
        self.brain = QtWidgets.QTextEdit()
        self.brain.setReadOnly(True)
        self.brain.setMaximumWidth(300)
        brain_layout.addWidget(self.brain)
        lower.addWidget(brain_box)
        viewers = QtWidgets.QTabWidget()
        episode_widget = QtWidgets.QWidget()
        episode_layout = QtWidgets.QVBoxLayout(episode_widget)
        self.episode_choice = QtWidgets.QComboBox()
        self.episode_choice.currentIndexChanged.connect(self.show_episode)
        episode_layout.addWidget(self.episode_choice)
        self.episodes = self.table(["時点", "センサーベクトル", "内部状態", "選択した行動", "正解行動", "報酬"])
        for column in (1, 2):
            self.episodes.horizontalHeader().setSectionResizeMode(column, QtWidgets.QHeaderView.Interactive)
            self.episodes.setColumnWidth(column, 190)
        episode_layout.addWidget(self.episodes)
        viewers.addTab(episode_widget, "エピソード")
        self.languages = self.table(["日時", "実験", "シナリオ", "理由 / センサー値", "確信度", "呼び出し必要", "J72呼び出し", "待ち時間（秒）", "応答 / エラー"])
        for column in (0, 3, 8):
            self.languages.horizontalHeader().setSectionResizeMode(column, QtWidgets.QHeaderView.Interactive)
            self.languages.setColumnWidth(column, 160)
        viewers.addTab(self.languages, "言語ゲート")
        lower.addWidget(viewers, 2)
        layout.addLayout(lower, 2)
        self.statusBar().showMessage(self.server + " · GUIを切断しても学習は継続します")

    @staticmethod
    def table(headers):
        table = QtWidgets.QTableWidget(0, len(headers))
        table.setHorizontalHeaderLabels(headers)
        table.horizontalHeader().setSectionResizeMode(QtWidgets.QHeaderView.ResizeToContents)
        table.horizontalHeader().setStretchLastSection(True)
        table.setEditTriggers(QtWidgets.QAbstractItemView.NoEditTriggers)
        return table

    def request(self, route, callback=None, body=None):
        request = QtNetwork.QNetworkRequest(QtCore.QUrl(self.server + route))
        request.setTransferTimeout(5000)
        if body is None:
            reply = self.network.get(request)
        else:
            request.setHeader(QtNetwork.QNetworkRequest.ContentTypeHeader, "application/json")
            reply = self.network.post(request, json.dumps(body).encode())
        def finished():
            try:
                payload = json.loads(bytes(reply.readAll()).decode())
                if reply.error() != QtNetwork.QNetworkReply.NoError:
                    self.statusBar().showMessage(ERROR_LABELS.get(payload.get("detail") if isinstance(payload, dict) and isinstance(payload.get("detail"), str) else "", "操作に失敗しました。入力内容と接続状態を確認してください。"), 12000)
                elif callback:
                    callback(payload)
            except (ValueError, TypeError) as error:
                self.statusBar().showMessage("サーバーから応答を取得できません。接続状態を確認してください。", 10000)
            finally:
                reply.deleteLater()
        reply.finished.connect(finished)

    def connect_ws(self):
        if self.closing:
            return
        url = QtCore.QUrl(self.server + "/ws/metrics")
        url.setScheme("wss" if url.scheme() == "https" else "ws")
        self.ws.open(url)

    def connected(self):
        self.reconnect.stop()
        self.reconnect_seconds = 1
        self.connection_label.setText("● 接続済み · WebSocket")
        self.statusBar().showMessage(self.server + " · 接続済み。GUIを切断しても学習は継続します")
        self.request("/api/configs", self.set_configs)
        self.refresh_selected()

    def websocket_error(self, _):
        self.statusBar().showMessage("WebSocket接続に失敗しました。自動的に再接続します。", 15000)
        self.disconnected()

    def disconnected(self):
        if self.closing or self.reconnect.isActive():
            return
        self.connection_label.setText(f"○ 切断 · {self.reconnect_seconds}秒後に再接続")
        self.reconnect.start(self.reconnect_seconds * 1000)
        self.reconnect_seconds = min(30, self.reconnect_seconds * 2)

    def on_message(self, text):
        try:
            payload = json.loads(text)
            self.update_system(payload.get("system", {}))
            self.update_runs(payload.get("runs", []))
        except (ValueError, TypeError, KeyError) as error:
            self.statusBar().showMessage("計測データの形式が正しくありません。")

    def set_configs(self, configs):
        old = self.config_choice.currentText()
        self.config_choice.clear()
        self.config_choice.addItems(configs)
        if old in configs:
            self.config_choice.setCurrentText(old)

    def update_system(self, system):
        parts = [f"llm_master · CPU使用率 {display(system.get('cpu_percent'))}%", f"RAM使用率 {display(system.get('ram_percent'))}%"]
        devices = ["cpu"]
        for gpu in system.get("gpus", []):
            devices.append(f"cuda:{gpu['index']}")
            parts.append(f"GPU {gpu['index']} {gpu['name']} · 使用率 {display(gpu.get('utilization_percent'))}% · VRAM {memory_mb(gpu.get('memory_used_mb'))}/{memory_mb(gpu.get('memory_total_mb'))} MiB · 温度 {display(gpu.get('temperature_c'))} °C")
        self.system_label.setText("   |   ".join(parts))
        if devices != [self.device_choice.itemText(i) for i in range(self.device_choice.count())]:
            old = self.device_choice.currentText()
            self.device_choice.clear()
            self.device_choice.addItems(devices)
            if old in devices:
                self.device_choice.setCurrentText(old)

    def update_runs(self, records):
        self.runs.blockSignals(True)
        self.runs.setRowCount(len(records))
        selected_row = None
        for row, record in enumerate(records):
            run_id = record["run_id"]
            latest = record.get("latest", {})
            gpu = record.get("physical_gpu", {})
            device_label = f"{gpu.get('index', '?')}: {gpu.get('name', 'GPU')}" if gpu else record.get("device", record.get("gpu"))
            values = [run_id, record.get("architecture"), record.get("seed"), device_label, latest.get("training_step", record.get("training_step")), latest.get("reward_mean"), latest.get("task_success"), 100*latest["llm_call_rate"] if latest.get("llm_call_rate") is not None else None, STATUS_LABELS.get(record.get("status", "queued"), display(record.get("status")))]
            for column, value in enumerate(values):
                self.runs.setItem(row, column, QtWidgets.QTableWidgetItem(display(value)))
            if run_id == self.selected:
                selected_row = row
            history = self.histories.setdefault(run_id, [])
            if latest and (not history or latest.get("timestamp") != history[-1].get("timestamp")):
                history.append(latest)
        if selected_row is not None:
            self.runs.selectRow(selected_row)
        self.runs.blockSignals(False)
        if self.selected is None and records:
            self.runs.selectRow(0)
        self.draw_selected()

    def select_run(self):
        row = self.runs.currentRow()
        if row < 0:
            return
        run_id = self.runs.item(row, 0).text()
        if run_id != self.selected:
            self.selected = run_id
            self.draw_selected()
            self.set_episodes([])
            self.set_languages([])
            self.refresh_selected()

    def refresh_selected(self):
        if self.ws.state() != QtNetwork.QAbstractSocket.ConnectedState:
            self.request("/api/runs", self.update_runs)
            self.request("/api/system", self.update_system)
        if not self.selected:
            return
        run_id = self.selected
        def got_history(record):
            self.histories[run_id] = record.get("metrics", [])
            if run_id == self.selected:
                self.draw_selected()
        self.request(f"/api/runs/{run_id}?limit=20000", got_history)
        self.request(f"/api/runs/{run_id}/episodes", lambda data: self.set_episodes(data) if run_id == self.selected else None)
        self.request(f"/api/runs/{run_id}/language-events", lambda data: self.set_languages(data) if run_id == self.selected else None)

    def draw_selected(self, *_):
        history = self.histories.get(self.selected, [])
        if not history:
            for curve in self.curves.values():
                curve.setData([], [])
            self.action_bars.setOpts(height=[0] * len(ACTIONS))
            self.brain.setPlainText("この実験の計測データはまだありません")
            return
        origin = timestamp(history[0].get("timestamp"))
        end = timestamp(history[-1].get("timestamp"))
        duration = {"1分": 60, "5分": 300, "全期間": float("inf")}[self.period.currentText()]
        visible = [record for record in history if timestamp(record.get("timestamp")) >= end-duration]
        for key, curve in self.curves.items():
            points = [(timestamp(record.get("timestamp"))-origin, record[key]) for record in visible if isinstance(record.get(key), (int, float))]
            curve.setData([point[0] for point in points], [point[1] for point in points])
        latest = history[-1]
        distribution = latest.get("action_distribution", [0]*6)
        if isinstance(distribution, dict):
            distribution = [distribution.get(name, 0) for name in ACTIONS]
        if len(distribution) == 6:
            self.action_bars.setOpts(height=distribution)
        fields = {key: value for key, value in latest.items() if any(word in key.lower() for word in ("hidden", "module", "salience", "persistence", "novelty", "retention", "habituation"))}
        self.brain.setPlainText("\n".join(f"{STATE_LABELS.get(key, key)}：{display(value)}" for key, value in fields.items()) or "内部状態の計測データはありません")

    def set_episodes(self, data):
        if isinstance(data, dict):
            data = data.get("episodes", [data])
        if data and isinstance(data[0], dict) and ("sensor_vector" in data[0] or "chosen_action" in data[0]):
            data = [{"scenario": data[0].get("scenario"), "steps": data}]
        old = self.episode_choice.currentIndex()
        self.episode_data = data
        self.episode_choice.blockSignals(True)
        self.episode_choice.clear()
        self.episode_choice.addItems([f"エピソード {i} · {scenario_name(episode.get('scenario', 'evaluation'))}" if isinstance(episode, dict) else f"エピソード {i}" for i, episode in enumerate(data)])
        self.episode_choice.blockSignals(False)
        self.episode_choice.setCurrentIndex(min(max(0, old), len(data)-1))
        self.show_episode(self.episode_choice.currentIndex())

    def show_episode(self, index):
        if index < 0 or index >= len(self.episode_data):
            self.episodes.setRowCount(0)
            return
        episode = self.episode_data[index]
        rows = episode.get("steps", episode.get("trajectory", [])) if isinstance(episode, dict) else episode
        self.episodes.setRowCount(len(rows))
        for i, row in enumerate(rows):
            values = [row.get("time", row.get("step", i)), row.get("sensors", row.get("observation", row.get("sensor_vector"))), row.get("internal_state", row.get("state")), action_name(row.get("chosen_action", row.get("action"))), action_name(row.get("oracle_action")), row.get("reward")]
            for j, value in enumerate(values):
                text = display(value)
                item = QtWidgets.QTableWidgetItem(text)
                item.setToolTip(text)
                self.episodes.setItem(i, j, item)

    def set_languages(self, events):
        self.languages.setRowCount(len(events))
        for i, event in enumerate(reversed(events)):
            values = [human_time(event.get("timestamp")), event.get("run_id", self.selected), scenario_name(event.get("scenario")), event.get("reason", event.get("signals", event.get("event"))), event.get("core_confidence", event.get("confidence")), event.get("oracle_required", event.get("required_llm")), event.get("j72_called"), event.get("j72_latency", event.get("latency_ms")), event.get("response", event.get("error", STATUS_LABELS.get(event.get("status"), display(event.get("status")))))]
            for j, value in enumerate(values):
                item = QtWidgets.QTableWidgetItem(display(value)[:1000])
                item.setToolTip(display(value))
                self.languages.setItem(i, j, item)

    def confirm_stop(self):
        dialog = QtWidgets.QMessageBox(self)
        dialog.setWindowTitle("学習を停止しますか？")
        dialog.setText("学習を停止しますか？")
        dialog.setInformativeText(f"{self.selected} のチェックポイントを保存して停止します。")
        dialog.setIcon(QtWidgets.QMessageBox.Question)
        stop_button = dialog.addButton("停止する", QtWidgets.QMessageBox.AcceptRole)
        cancel_button = dialog.addButton("キャンセル", QtWidgets.QMessageBox.RejectRole)
        dialog.setDefaultButton(cancel_button)
        dialog.exec_()
        return dialog.clickedButton() == stop_button

    def control(self, command):
        if command == "start":
            body = {"config": self.config_choice.currentText(), "device": self.device_choice.currentText()}
        else:
            if not self.selected:
                self.statusBar().showMessage("先に実験を選択してください", 5000)
                return
            if command == "stop" and not self.confirm_stop():
                return
            body = {"run_id": self.selected}
        self.request("/api/run/" + command, lambda result: self.statusBar().showMessage(f"{result.get('run_id', '実験')}：{COMMAND_LABELS[command]}を要求しました（{STATUS_LABELS.get(result.get('status'), '受付済み')}）", 10000), body)

    def closeEvent(self, event):
        self.closing = True
        self.refresh.stop()
        self.reconnect.stop()
        # The application event loop may exit immediately after this callback.
        # Abort closes the transport synchronously; a close handshake would need
        # further Qt events and can leave the server waiting on a dead client.
        self.ws.abort()
        event.accept()


def main():
    parser = argparse.ArgumentParser(description="K0人工脳幹の学習モニター")
    parser.add_argument("--server", default="http://127.0.0.1:8097", help="監視サーバーの接続先URL")
    args = parser.parse_args()
    application = QtWidgets.QApplication(sys.argv)
    window = Monitor(args.server)
    window.show()
    sys.exit(application.exec_())


if __name__ == "__main__":
    main()
