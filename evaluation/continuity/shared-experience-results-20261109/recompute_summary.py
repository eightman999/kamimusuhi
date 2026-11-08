#!/usr/bin/env python3
"""Recompute diagnostics from the checked-in, synthetic JSON artifacts only."""
from collections import Counter, defaultdict
from pathlib import Path
import json

ROOT = Path(__file__).resolve().parent
AXES = ("supported_shared_facts", "voice_continuity", "relational_continuity", "task_continuity")
VALUES = ("pass", "fail", "uncertain")
CASE_IDS = {x["case_id"] for x in json.loads((ROOT.parent / "shared-experience-20261109.json").read_text())["cases"]}


def load(path):
    return json.loads((ROOT / path).read_text())


def counts(values):
    counter = Counter(values)
    return {value: counter[value] for value in VALUES}


def axis_value(row, axis):
    value = row.get(axis, row.get("axes", {}).get(axis))
    if isinstance(value, dict):
        value = value.get("rating", value.get("verdict"))
    if value not in VALUES:
        raise ValueError(f"missing/invalid rating: {row.get('sample_id')} {axis}")
    return value


def decode_review(letter, map_name):
    review_path = ROOT / "blind" / f"review-{letter}.json"
    if not review_path.exists():
        return None
    review = load(f"blind/review-{letter}.json")
    mapping = load(f"blind/{map_name}")
    ratings = review.get("ratings", review.get("reviews", review.get("samples")))
    if not isinstance(ratings, list) or len(ratings) != 32:
        raise ValueError(f"review-{letter} must contain 32 ratings")
    seen = set()
    decoded = []
    for row in ratings:
        sid = row["sample_id"]
        if sid in seen:
            raise ValueError(f"duplicate rating {sid}")
        seen.add(sid)
        condition = mapping[sid]
        decoded.append({**row, **condition, **{axis: axis_value(row, axis) for axis in AXES}})
    assert seen == set(mapping)
    phases = sorted({r["phase"] for r in decoded})
    per_phase = {}
    for phase in phases:
        selected = [r for r in decoded if r["phase"] == phase]
        per_phase[phase] = {"samples": len(selected), "axes": {axis: counts(r[axis] for r in selected) for axis in AXES}}
        if all("delivery_eligible" in r for r in selected):
            eligible = [r for r in selected if r["delivery_eligible"]]
            per_phase[phase]["gate_eligible_samples"] = len(eligible)
            per_phase[phase]["gate_eligible_axes"] = {axis: counts(r[axis] for r in eligible) for axis in AXES}
    per_case = {}
    for case_id in sorted(CASE_IDS):
        per_case[case_id] = {}
        for phase in phases:
            selected = [r for r in decoded if r["phase"] == phase and r["case_id"] == case_id]
            per_case[case_id][phase] = {"samples": len(selected), "axes": {axis: counts(r[axis] for r in selected) for axis in AXES}}
    after = [p for p in phases if p != "before"]
    delta = {}
    if "before" in per_phase and len(after) == 1:
        delta = {axis: {value: per_phase[after[0]]["axes"][axis][value] - per_phase["before"]["axes"][axis][value] for value in VALUES} for axis in AXES}
    return {"reviewer": review.get("reviewer", review.get("reviewer_id", f"model_context_{letter}")), "human": False,
            "condition_blinded": True, "gate_verdict_blinded": letter in "cdef", "mapping": f"blind/{map_name}",
            "per_phase": per_phase, "after_minus_before_counts": delta, "per_case": per_case, "decoded_ratings": decoded}


def summarize_gates():
    result = {}
    for phase, prefix in (("before", "guarded-before"), ("after_v3", "guarded-after_v3"), ("after_final", "guarded-final")):
        runs = [load(f"probes/{prefix}-{repeat}.json") for repeat in ("baseline", "candidate")]
        selected = [r for run in runs for r in run["results"] if r["probe_id"] in CASE_IDS]
        assert len(selected) == 16
        result[phase] = {
            "samples": len(selected), "delivery_eligible": sum(r["delivery_eligible"] for r in selected),
            "not_delivery_eligible": sum(not r["delivery_eligible"] for r in selected),
            "logical_provider_calls_including_controls": sum(run["logical_provider_calls"] for run in runs),
            "provider_errors_main_cases": sum(r["error_code"] is not None for r in selected),
            "per_case": {case: {"samples": 2, "delivery_eligible": sum(r["delivery_eligible"] for r in selected if r["probe_id"] == case),
                                 "repairs": [r["repair_count"] for r in selected if r["probe_id"] == case],
                                 "final_statuses": [r["status"] for r in selected if r["probe_id"] == case]} for case in sorted(CASE_IDS)},
            "controls": [{"repeat": repeat, **r} for repeat, run in zip(("baseline", "candidate"), runs) for r in run["results"] if r["probe_id"] not in CASE_IDS],
        }
    return result


def raw_diagnostics():
    result = {}
    for phase, filename in (("before", "before-retry.json"), ("candidate1", "after.json"), ("candidate2", "after-v2-retry.json"), ("candidate3", "after-v3.json")):
        paired = load(f"raw/{filename}")
        cases = {r["case_id"]: r for r in paired["cases"]}
        records = []
        for repeat in ("baseline", "candidate"):
            for row in paired[repeat]["outputs"]:
                expected = cases[row["case_id"]]
                records.append({"repeat": repeat, "case_id": row["case_id"],
                    "missing_required_text": [s for s in expected["required_text"] if s not in row["text"]],
                    "present_forbidden_text": [s for s in expected["forbidden_text"] if s in row["text"]]})
        result[phase] = {"samples": len(records), "keyword_diagnostics_only": records,
                         "substring_diagnostic_passes": sum(not r["missing_required_text"] and not r["present_forbidden_text"] for r in records)}
    return result


reviews = {}
for letter, mapping in (("a", "blind-map.json"), ("b", "blind-map.json"), ("c", "blind-guarded-map.json"), ("d", "blind-guarded-map.json"), ("e", "blind-final-map.json"), ("f", "blind-final-map.json")):
    decoded = decode_review(letter, mapping)
    if decoded:
        reviews[letter] = decoded
summary = {
    "format_version": 1, "suite_id": "shared-experience-voice-relations-20261109",
    "rating_kind": "model-only, separate contexts in the same model family; no human or independent-weight ratings",
    "actual_returned_model": None, "usage": None,
    "repeat_label_explanation": "baseline/candidate within a raw capture or capture_run mean first/second repeat, not before/after implementation",
    "main_gate_comparison": summarize_gates(), "raw_generation": raw_diagnostics(), "model_reviews": reviews,
    "pending_final_reviewers": [x for x in ("e", "f") if x not in reviews],
    "known_final_residuals": [
        {"case_id": "honest_memory_boundary", "repeat": "baseline", "gate_eligible": True, "finding": "Potential false accept: says past records are not visible despite two supplied utterances. C/D rate the identical text as fail; E/F rate factual scope uncertain because it may mean earlier history. Do not treat the disagreement as pass."},
        {"case_id": "honest_memory_boundary", "repeat": "candidate", "gate_eligible": True, "finding": "Awkward body/interface wording この口 remains; voice failure in C/D reviews of the byte-identical final text."},
        {"case_id": "warm_return", "repeat": "candidate", "gate_eligible": False, "finding": "Still RETRY after the one allowed repair. Retained for review; not a delivered model answer in this component probe."},
        {"case_id": "warm_return", "finding": "Raw generation still echoes assistant-only completion. Improvement depends on assessment/repair, not perfect generation."},
    ],
    "limits": [
        "Gate-eligible means a model judgment in a component probe; it is neither proof of truth nor production emission.",
        "All post-repair texts are reviewed, including those the gate would withhold; eligible-only ratings are secondary.",
        "Eight fixed synthetic cases, two repeats per phase; diagnostic controls are excluded from main-case totals.",
        "Persona capture/probe opens no canonical DB and offers no tools. Full runtime E2E is reported separately.",
        "Final generation framing equals candidate3. Final assessment/repair changed only two warm_return final texts relative to candidate3; C/D are not relabeled as final reviews.",
        "Human rating is uncollected. Model contexts are not independent model weights; uncertainty remains uncertainty.",
        "Failed initial captures lost partial generations; counts are inferred and preserved in capture-failures.json.",
        "Keyword checks use frozen exact substrings only; missing バックアップ is not automatically a semantic failure, and keyword pass does not imply grounded facts.",
    ],
}
(ROOT / "summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n")
print(json.dumps({"main_gate_counts": {k: {x: v[x] for x in ("samples", "delivery_eligible", "not_delivery_eligible")} for k, v in summary["main_gate_comparison"].items()}, "reviewers": list(reviews), "pending_final_reviewers": summary["pending_final_reviewers"]}, ensure_ascii=False))
