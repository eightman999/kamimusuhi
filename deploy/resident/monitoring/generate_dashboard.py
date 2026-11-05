#!/usr/bin/env python3
"""Generate the portable Grafana dashboard (no credentials or site state)."""
import json
from pathlib import Path


DS = {"type": "prometheus", "uid": "${DS_PROMETHEUS}"}
FILTER = 'job="kamimusuhi-resident",node=~"$node",tier=~"$tier",model=~"$model",kind="$kind"'


def metric(name, extra=""):
    return f'kamimusuhi_usage_{name}{{{FILTER}{"," + extra if extra else ""}}}'


def make_dashboard():
    panels = []

    def panel(title, kind, x, y, w, h, expressions=(), unit="short", description=""):
        targets = [
            {"refId": chr(65 + i), "expr": expr, "legendFormat": legend,
             "datasource": DS, "range": kind == "timeseries", "instant": kind != "timeseries",
             "format": "table" if kind == "table" else "time_series"}
            for i, (expr, legend) in enumerate(expressions)
        ]
        p = {"id": len(panels) + 1, "title": title, "type": kind, "datasource": DS,
             "description": description, "gridPos": {"x": x, "y": y, "w": w, "h": h},
             "targets": targets, "fieldConfig": {"defaults": {"unit": unit,
                 "noValue": "未収集", "color": {"mode": "palette-classic"}}, "overrides": []},
             "options": {}}
        if kind == "stat":
            p["options"] = {"reduceOptions": {"calcs": ["lastNotNull"], "fields": "", "values": False},
                            "colorMode": "value", "graphMode": "none", "textMode": "auto"}
        elif kind == "timeseries":
            p["options"] = {"legend": {"displayMode": "table", "placement": "bottom",
                                        "calcs": ["lastNotNull"]}, "tooltip": {"mode": "multi"}}
        elif kind == "table":
            p["options"] = {"showHeader": True, "cellHeight": "sm"}
        panels.append(p)
        return p

    p = panel("Kamimusuhi · モデル利用監視", "text", 0, 0, 24, 3)
    p["options"] = {"mode": "markdown", "content":
        "期間とモデルで絞り込めます。自動更新は1分。要求（`request`）と上流試行（`attempt`）は別集計です。\n\n"
        "料金は実額・推定・不明を区別します。複数ノードを選ぶと中継分も含みます。導入前の利用は含みません。"}
    up = 'up{job="kamimusuhi-resident",node=~"$node"}'
    health = panel("収集接続（1=正常）", "stat", 0, 3, 4, 4, [(f"min({up})", "")])
    health["fieldConfig"]["defaults"]["thresholds"] = {"mode": "absolute", "steps": [
        {"color": "red", "value": None}, {"color": "green", "value": 1}]}
    health["fieldConfig"]["defaults"]["color"] = {"mode": "thresholds"}
    count = metric("events_total")
    good = metric("events_total", 'outcome="success"')
    actual_cost = metric("cost_usd_total", 'cost_kind="actual"')
    estimated_cost = metric("cost_usd_total", 'cost_kind="estimate"')
    actual_reports = metric("cost_reports_total", 'cost_kind="actual"')
    estimated_reports = metric("cost_reports_total", 'cost_kind="estimate"')
    for title, x, expr, unit in [
        ("期間内の呼び出し数（推定）", 4, f"sum(increase({count}[$__range]))", "short"),
        ("成功率", 8, f"100 * (sum(increase({good}[$__range])) or vector(0)) / sum(increase({count}[$__range]))", "percent"),
        ("平均応答時間", 12, f'sum(increase({metric("latency_seconds_sum")}[$__range])) / sum(increase({metric("latency_seconds_count")}[$__range]))', "s"),
        ("報告された実額", 16, f'sum(increase({actual_cost}[$__range])) and (sum(increase({actual_reports}[$__range])) > 0)', "currencyUSD"),
        ("料金不明の件数", 20, f'sum(increase({metric("cost_unknown_total")}[$__range]))', "short"),
    ]:
        panel(title, "stat", x, 3, 4, 4, [(expr, "")], unit)
    panels[3]["description"] = "要求失敗は応答モデル未確定のためunknownに集計します。モデル別の失敗率は集計単位attemptで確認してください。"
    panels[5]["fieldConfig"]["defaults"]["noValue"] = "報告なし"
    panel("モデル別の呼び出し推移 / 分", "timeseries", 0, 7, 12, 8,
          [(f"sum by (tier, model) (rate({count}[$__rate_interval])) * 60", "{{tier}} / {{model}}")])
    panel("成功・失敗の推移 / 分", "timeseries", 12, 7, 12, 8,
          [(f"sum by (outcome) (rate({count}[$__rate_interval])) * 60", "{{outcome}}")])
    panel("モデル別 応答時間 p95", "timeseries", 0, 15, 12, 8,
          [(f'histogram_quantile(0.95, sum by (le, tier, model) (rate({metric("latency_seconds_bucket")}[$__rate_interval])))', "{{tier}} / {{model}}")], "s")
    panel("入出力・キャッシュのトークン / 分", "timeseries", 12, 15, 12, 8,
          [(f'sum by (token_type) (rate({metric("tokens_total")}[$__rate_interval])) * 60', "{{token_type}}")],
          description="cached は prompt の内数で、合計に加算しません。報告がない値は不明件数を参照。")
    panel("料金の推移 / 時間（実額・推定別）", "timeseries", 0, 23, 12, 8,
          [(f'sum by (cost_kind) (rate({metric("cost_usd_total")}[$__rate_interval])) * 3600 and (sum by (cost_kind) (increase({metric("cost_reports_total")}[$__rate_interval])) > 0)', "{{cost_kind}}")], "currencyUSD")
    panels[-1]["fieldConfig"]["defaults"]["noValue"] = "報告なし"
    panel("トークン数が未報告の呼び出し / 分", "timeseries", 12, 23, 12, 8,
          [(f'sum by (token_type) (rate({metric("tokens_unknown_total")}[$__rate_interval])) * 60', "{{token_type}}")])
    recent = panel("使用モデルと最終呼び出し時刻", "table", 0, 31, 24, 8,
          [(f'max by (node, tier, model, outcome) ({metric("last_request_timestamp_seconds")}) * 1000', "")],
          description="選択期間の終端時点で観測した最終呼び出し。個別要求の全件一覧ではありません。")
    recent["transformations"] = [{"id": "organize", "options": {"excludeByName": {"Time": True},
        "renameByName": {"node": "ノード", "tier": "経路", "model": "モデル", "outcome": "結果", "Value": "最終呼び出し"}}}]
    recent["fieldConfig"]["overrides"] = [{"matcher": {"id": "byName", "options": "最終呼び出し"},
        "properties": [{"id": "unit", "value": "dateTimeAsIso"}]}]
    recent["options"]["sortBy"] = [{"displayName": "最終呼び出し", "desc": True}]
    panel("期間内のモデル別呼び出し数（推定）", "table", 0, 39, 12, 8,
          [(f"sum by (node, tier, model, outcome) (increase({count}[$__range]))", "")])
    panel("期間内のモデル別トークン数（報告分）", "table", 12, 39, 12, 8,
          [(f'sum by (model, token_type) (increase({metric("tokens_total")}[$__range]))', "")])
    panel("履歴保存（1=正常）", "stat", 0, 47, 6, 4,
          [('min(kamimusuhi_usage_persistence_healthy{job="kamimusuhi-resident",node=~"$node"})', "")])
    panel("履歴保存エラー（起動後）", "stat", 6, 47, 6, 4,
          [('sum(kamimusuhi_usage_persistence_errors_total{job="kamimusuhi-resident",node=~"$node"})', "")])
    panel("報告・単価に基づく推定額", "stat", 12, 47, 6, 4,
          [(f'sum(increase({estimated_cost}[$__range])) and (sum(increase({estimated_reports}[$__range])) > 0)', "")], "currencyUSD")
    panels[-1]["fieldConfig"]["defaults"]["noValue"] = "報告なし"
    panel("起動後の呼び出し数", "stat", 18, 47, 6, 4, [(f"sum({count})", "")],
          description="選択期間終端のカウンタ値。resident再起動で0に戻ります。")

    def variable(name, label, query):
        return {"name": name, "label": label, "type": "query", "datasource": DS,
                "query": query, "definition": query, "refresh": 1, "sort": 1,
                "includeAll": True, "allValue": ".*", "multi": False,
                "current": {"text": "All", "value": "$__all"}, "options": []}

    return {"id": None, "uid": "kamimusuhi-model-usage", "title": "Kamimusuhi · モデル利用監視",
            "tags": ["kamimusuhi", "llm"], "timezone": "browser", "schemaVersion": 39,
            "version": 1, "editable": True, "refresh": "1m", "time": {"from": "now-3h", "to": "now"},
            "timepicker": {"refresh_intervals": ["15s", "30s", "1m", "5m"]},
            "templating": {"list": [
                {"name": "DS_PROMETHEUS", "label": "データソース", "type": "datasource", "query": "prometheus",
                 "current": {"text": "prometheus", "value": "efx213ho34jr4d"}, "refresh": 1},
                variable("node", "ノード", 'label_values(up{job="kamimusuhi-resident"}, node)'),
                variable("tier", "経路", 'label_values(kamimusuhi_usage_events_total{node=~"$node"}, tier)'),
                variable("model", "モデル", 'label_values(kamimusuhi_usage_events_total{node=~"$node",tier=~"$tier"}, model)'),
                {"name": "kind", "label": "集計単位", "type": "custom", "query": "request,attempt",
                 "current": {"text": "request", "value": "request"},
                 "options": [{"text": k, "value": k, "selected": k == "request"} for k in ["request", "attempt"]]},
            ]}, "panels": panels}


if __name__ == "__main__":
    Path(__file__).with_name("kamimusuhi-model-usage.json").write_text(
        json.dumps(make_dashboard(), ensure_ascii=False, indent=2) + "\n")
