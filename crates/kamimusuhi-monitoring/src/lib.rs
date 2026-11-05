//! Grafana dashboard construction, independent of runtime state and credentials.
use serde_json::{Value, json};

fn datasource() -> Value {
    json!({"type":"prometheus", "uid":"${DS_PROMETHEUS}"})
}

fn metric(name: &str, extra: &str) -> String {
    let extra = if extra.is_empty() {
        String::new()
    } else {
        format!(",{extra}")
    };
    format!(
        "kamimusuhi_usage_{name}{{job=\"kamimusuhi-resident\",node=~\"$node\",tier=~\"$tier\",model=~\"$model\",kind=\"$kind\"{extra}}}"
    )
}

fn panel(
    title: &str,
    kind: &str,
    position: [u32; 4],
    expression: Option<(String, &str)>,
    unit: &str,
) -> Value {
    let [x, y, w, h] = position;
    let targets: Vec<Value> = expression
        .into_iter()
        .map(|(expr, legend)| {
            json!({
                "refId":"A", "expr":expr, "legendFormat":legend, "datasource":datasource(),
                "range":kind == "timeseries", "instant":kind != "timeseries",
                "format":if kind == "table" { "table" } else { "time_series" }
            })
        })
        .collect();
    let options = match kind {
        "stat" => {
            json!({"reduceOptions":{"calcs":["lastNotNull"],"fields":"","values":false},"colorMode":"value","graphMode":"none","textMode":"auto"})
        }
        "timeseries" => {
            json!({"legend":{"displayMode":"table","placement":"bottom","calcs":["lastNotNull"]},"tooltip":{"mode":"multi"}})
        }
        "table" => json!({"showHeader":true,"cellHeight":"sm"}),
        _ => json!({}),
    };
    json!({"title":title,"type":kind,"datasource":datasource(),"description":"",
        "gridPos":{"x":x,"y":y,"w":w,"h":h}, "targets":targets,
        "fieldConfig":{"defaults":{"unit":unit,"noValue":"未収集","color":{"mode":"palette-classic"}},"overrides":[]}, "options":options})
}

fn variable(name: &str, label: &str, query: &str) -> Value {
    json!({"name":name,"label":label,"type":"query","datasource":datasource(),"query":query,"definition":query,
        "refresh":1,"sort":1,"includeAll":true,"allValue":".*","multi":false,
        "current":{"text":"All","value":"$__all"},"options":[]})
}

/// Rebuild the dashboard from metric names, layout, and PromQL expressions.
pub fn make_dashboard() -> Value {
    let mut panels = Vec::new();
    let mut intro = panel(
        "Kamimusuhi · モデル利用監視",
        "text",
        [0, 0, 24, 3],
        None,
        "short",
    );
    intro["options"] = json!({"mode":"markdown","content":"期間とモデルで絞り込めます。自動更新は1分。要求（`request`）と上流試行（`attempt`）は別集計です。\n\n料金は実額・推定・不明を区別します。複数ノードを選ぶと中継分も含みます。導入前の利用は含みません。"});
    panels.push(intro);
    let mut health = panel(
        "収集接続（1=正常）",
        "stat",
        [0, 3, 4, 4],
        Some((
            "min(up{job=\"kamimusuhi-resident\",node=~\"$node\"})".into(),
            "",
        )),
        "short",
    );
    health["fieldConfig"]["defaults"]["thresholds"] = json!({"mode":"absolute","steps":[{"color":"red","value":null},{"color":"green","value":1}]});
    health["fieldConfig"]["defaults"]["color"] = json!({"mode":"thresholds"});
    panels.push(health);
    let count = metric("events_total", "");
    let good = metric("events_total", "outcome=\"success\"");
    let actual_cost = metric("cost_usd_total", "cost_kind=\"actual\"");
    let estimated_cost = metric("cost_usd_total", "cost_kind=\"estimate\"");
    let actual_reports = metric("cost_reports_total", "cost_kind=\"actual\"");
    let estimated_reports = metric("cost_reports_total", "cost_kind=\"estimate\"");
    for (title, x, expr, unit) in [
        (
            "期間内の呼び出し数（推定）",
            4,
            format!("sum(increase({count}[$__range]))"),
            "short",
        ),
        (
            "成功率",
            8,
            format!(
                "100 * (sum(increase({good}[$__range])) or vector(0)) / sum(increase({count}[$__range]))"
            ),
            "percent",
        ),
        (
            "平均応答時間",
            12,
            format!(
                "sum(increase({}[$__range])) / sum(increase({}[$__range]))",
                metric("latency_seconds_sum", ""),
                metric("latency_seconds_count", "")
            ),
            "s",
        ),
        (
            "報告された実額",
            16,
            format!(
                "sum(increase({actual_cost}[$__range])) and (sum(increase({actual_reports}[$__range])) > 0)"
            ),
            "currencyUSD",
        ),
        (
            "料金不明の件数",
            20,
            format!(
                "sum(increase({}[$__range]))",
                metric("cost_unknown_total", "")
            ),
            "short",
        ),
    ] {
        panels.push(panel(title, "stat", [x, 3, 4, 4], Some((expr, "")), unit));
    }
    panels[3]["description"] = json!(
        "要求失敗は応答モデル未確定のためunknownに集計します。モデル別の失敗率は集計単位attemptで確認してください。"
    );
    panels[5]["fieldConfig"]["defaults"]["noValue"] = json!("報告なし");
    for (title, position, expr, legend, unit) in [
        (
            "モデル別の呼び出し推移 / 分",
            [0, 7, 12, 8],
            format!("sum by (tier, model) (rate({count}[$__rate_interval])) * 60"),
            "{{tier}} / {{model}}",
            "short",
        ),
        (
            "成功・失敗の推移 / 分",
            [12, 7, 12, 8],
            format!("sum by (outcome) (rate({count}[$__rate_interval])) * 60"),
            "{{outcome}}",
            "short",
        ),
        (
            "モデル別 応答時間 p95",
            [0, 15, 12, 8],
            format!(
                "histogram_quantile(0.95, sum by (le, tier, model) (rate({}[$__rate_interval])))",
                metric("latency_seconds_bucket", "")
            ),
            "{{tier}} / {{model}}",
            "s",
        ),
        (
            "入出力・キャッシュのトークン / 分",
            [12, 15, 12, 8],
            format!(
                "sum by (token_type) (rate({}[$__rate_interval])) * 60",
                metric("tokens_total", "")
            ),
            "{{token_type}}",
            "short",
        ),
        (
            "料金の推移 / 時間（実額・推定別）",
            [0, 23, 12, 8],
            format!(
                "sum by (cost_kind) (rate({}[$__rate_interval])) * 3600 and (sum by (cost_kind) (increase({}[$__rate_interval])) > 0)",
                metric("cost_usd_total", ""),
                metric("cost_reports_total", "")
            ),
            "{{cost_kind}}",
            "currencyUSD",
        ),
        (
            "トークン数が未報告の呼び出し / 分",
            [12, 23, 12, 8],
            format!(
                "sum by (token_type) (rate({}[$__rate_interval])) * 60",
                metric("tokens_unknown_total", "")
            ),
            "{{token_type}}",
            "short",
        ),
    ] {
        panels.push(panel(
            title,
            "timeseries",
            position,
            Some((expr, legend)),
            unit,
        ));
    }
    panels[10]["description"] =
        json!("cached は prompt の内数で、合計に加算しません。報告がない値は不明件数を参照。");
    panels[11]["fieldConfig"]["defaults"]["noValue"] = json!("報告なし");
    let mut recent = panel(
        "使用モデルと最終呼び出し時刻",
        "table",
        [0, 31, 24, 8],
        Some((
            format!(
                "max by (node, tier, model, outcome) ({}) * 1000",
                metric("last_request_timestamp_seconds", "")
            ),
            "",
        )),
        "short",
    );
    recent["description"] =
        json!("選択期間の終端時点で観測した最終呼び出し。個別要求の全件一覧ではありません。");
    recent["transformations"] = json!([{"id":"organize","options":{"excludeByName":{"Time":true},"renameByName":{"node":"ノード","tier":"経路","model":"モデル","outcome":"結果","Value":"最終呼び出し"}}}]);
    recent["fieldConfig"]["overrides"] = json!([{"matcher":{"id":"byName","options":"最終呼び出し"},"properties":[{"id":"unit","value":"dateTimeAsIso"}]}]);
    recent["options"]["sortBy"] = json!([{"displayName":"最終呼び出し","desc":true}]);
    panels.push(recent);
    panels.push(panel(
        "期間内のモデル別呼び出し数（推定）",
        "table",
        [0, 39, 12, 8],
        Some((
            format!("sum by (node, tier, model, outcome) (increase({count}[$__range]))"),
            "",
        )),
        "short",
    ));
    panels.push(panel(
        "期間内のモデル別トークン数（報告分）",
        "table",
        [12, 39, 12, 8],
        Some((
            format!(
                "sum by (model, token_type) (increase({}[$__range]))",
                metric("tokens_total", "")
            ),
            "",
        )),
        "short",
    ));
    panels.push(panel("履歴保存（1=正常）","stat",[0,47,6,4],Some(("min(kamimusuhi_usage_persistence_healthy{job=\"kamimusuhi-resident\",node=~\"$node\"})".into(),"")),"short"));
    panels.push(panel("履歴保存エラー（起動後）","stat",[6,47,6,4],Some(("sum(kamimusuhi_usage_persistence_errors_total{job=\"kamimusuhi-resident\",node=~\"$node\"})".into(),"")),"short"));
    let mut estimate = panel(
        "報告・単価に基づく推定額",
        "stat",
        [12, 47, 6, 4],
        Some((
            format!(
                "sum(increase({estimated_cost}[$__range])) and (sum(increase({estimated_reports}[$__range])) > 0)"
            ),
            "",
        )),
        "currencyUSD",
    );
    estimate["fieldConfig"]["defaults"]["noValue"] = json!("報告なし");
    panels.push(estimate);
    let mut total = panel(
        "起動後の呼び出し数",
        "stat",
        [18, 47, 6, 4],
        Some((format!("sum({count})"), "")),
        "short",
    );
    total["description"] = json!("選択期間終端のカウンタ値。resident再起動で0に戻ります。");
    panels.push(total);
    for (i, panel) in panels.iter_mut().enumerate() {
        panel["id"] = json!(i + 1);
    }
    json!({"id":null,"uid":"kamimusuhi-model-usage","title":"Kamimusuhi · モデル利用監視",
        "tags":["kamimusuhi","llm"],"timezone":"browser","schemaVersion":39,"version":1,"editable":true,"refresh":"1m",
        "time":{"from":"now-3h","to":"now"},"timepicker":{"refresh_intervals":["15s","30s","1m","5m"]},
        "templating":{"list":[
            {"name":"DS_PROMETHEUS","label":"データソース","type":"datasource","query":"prometheus","current":{"text":"prometheus","value":"efx213ho34jr4d"},"refresh":1},
            variable("node","ノード","label_values(up{job=\"kamimusuhi-resident\"}, node)"),
            variable("tier","経路","label_values(kamimusuhi_usage_events_total{node=~\"$node\"}, tier)"),
            variable("model","モデル","label_values(kamimusuhi_usage_events_total{node=~\"$node\",tier=~\"$tier\"}, model)"),
            {"name":"kind","label":"集計単位","type":"custom","query":"request,attempt","current":{"text":"request","value":"request"},"options":[{"text":"request","value":"request","selected":true},{"text":"attempt","value":"attempt","selected":false}]}
        ]},"panels":panels})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_legacy_dashboard() {
        // Captured before the Rust port; also checked against the Python generator.
        let legacy: Value = serde_json::from_str(include_str!(
            "../../../deploy/resident/monitoring/kamimusuhi-model-usage.json"
        ))
        .unwrap();
        assert_eq!(make_dashboard(), legacy);
    }
}
