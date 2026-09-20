"""Integrity metrics for Fi2.

Metrics are computed per question (context bundles for B3/B4/B5 are
query-specific) and then averaged, plus bundle-level mutation analysis.

* ``key_fact_retention``      — fraction of key source claims with a
                                faithful representative in context
* ``delayed_recall``          — same, restricted to delayed-relevance facts
* ``rare_recall``             — same, restricted to rare facts
* ``qa_accuracy``             — deterministic-reader accuracy
* ``context_mutation_rate``   — mutated / total claims in context
* ``contradiction_rate``      — slots where context's effective claim
                                contradicts the gold effective state
* ``uncertainty_preservation``— hedged key claims still hedged in context
* ``temporal_accuracy``       — temporal-ordering QA accuracy
* ``source_recovery``         — derived claims with resolvable,
                                content-supporting provenance
* ``token_usage``             — mean context size in tokens
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Dict, List, Optional

from ..context_builder.bundle import ContextBundle
from ..datasets.generator import Dataset, QA
from ..store.events import Claim
from ..store.immutable_store import ImmutableEventStore
from .answer import Resolution, resolve
from .mutation import MutationReport, analyze_bundle
from .provenance import source_recovery


def _faithful_in_bundle(bundle: ContextBundle, src_claim: Claim) -> bool:
    canon = src_claim.canonical()
    return any(
        s.claim is not None and s.claim.canonical() == canon
        for s in bundle.claims()
    )


def _represented_mutated(
    bundle: ContextBundle, src_claim: Claim, report: MutationReport
) -> bool:
    """Source claim has a same-slot representative that is mutated."""
    for v in report.verdicts:
        if v.verdict in ("faithful", "unverifiable"):
            continue
        if v.claim.slot() == src_claim.slot():
            return True
    return False


@dataclass
class ConditionMetrics:
    condition: str
    n_questions: int
    qa_accuracy: float
    key_fact_retention: float
    delayed_recall: float
    rare_recall: float
    context_mutation_rate: float
    mutated_claims: int
    total_claims: int
    contradiction_rate: float
    uncertainty_preservation: float
    temporal_accuracy: float
    source_recovery: float
    pointer_rate: float
    token_usage: float
    retrieval_ms: float
    per_qa: List[dict] = field(default_factory=list)
    mutation_kinds: Dict[str, int] = field(default_factory=dict)


def evaluate_condition(
    condition: str,
    bundles: List[ContextBundle],
    dataset: Dataset,
    store: ImmutableEventStore,
    retrieval_ms: float = 0.0,
) -> ConditionMetrics:
    """Aggregate metrics over one bundle per QA pair."""
    qa_pairs = list(zip(dataset.qa, bundles))
    n = len(qa_pairs)
    key_ids = set(dataset.key_event_ids())
    delayed_ids = {
        eid for eid, labs in dataset.labels.items()
        if "delayed_relevance" in labs
    }
    rare_ids = {
        eid for eid, labs in dataset.labels.items()
        if "rare_fact" in labs
    }
    hedged_ids = {
        e.event_id for e in store.claims() if e.claim.hedged()
    }

    n_correct = 0
    ret_sum = mut_key_sum = 0.0
    delayed_sum = rare_sum = unc_sum = 0.0
    contra_hits = 0
    temporal_n = temporal_ok = 0
    mut_total = mut_mutated = 0
    mutation_kinds: Dict[str, int] = {}
    src_rec_sum = ptr_sum = 0.0
    tok_sum = 0
    per_qa: List[dict] = []

    for qa, bundle in qa_pairs:
        res: Resolution = resolve(bundle, qa)
        n_correct += int(res.correct)
        tok_sum += bundle.token_count

        rep = analyze_bundle(bundle, store)
        mut_total += rep.total_claims
        mut_mutated += rep.mutated
        for k, v in rep.by_kind.items():
            mutation_kinds[k] = mutation_kinds.get(k, 0) + v
        rec = source_recovery(bundle, store)
        src_rec_sum += rec["source_recovery"]
        ptr_sum += rec["pointer_rate"]

        # retention over key facts (present canonically)
        k_total = k_hit = 0
        for eid in key_ids:
            ev = store.get(eid)
            if ev is None or ev.claim is None:
                continue
            k_total += 1
            k_hit += int(_faithful_in_bundle(bundle, ev.claim))
        ret_sum += k_hit / k_total if k_total else 0.0
        mut_key = sum(
            1
            for eid in key_ids
            if (ev := store.get(eid)) is not None and ev.claim
            and not _faithful_in_bundle(bundle, ev.claim)
            and _represented_mutated(bundle, ev.claim, rep)
        )
        mut_key_sum += mut_key / k_total if k_total else 0.0

        d_total = d_hit = 0
        for eid in delayed_ids & key_ids:
            ev = store.get(eid)
            d_total += 1
            d_hit += int(_faithful_in_bundle(bundle, ev.claim))
        delayed_sum += d_hit / d_total if d_total else 0.0
        r_total = r_hit = 0
        for eid in rare_ids & key_ids:
            ev = store.get(eid)
            r_total += 1
            r_hit += int(_faithful_in_bundle(bundle, ev.claim))
        rare_sum += r_hit / r_total if r_total else 0.0
        h_total = h_hit = 0
        for eid in hedged_ids & key_ids:
            ev = store.get(eid)
            if _faithful_in_bundle(bundle, ev.claim):
                h_total += 1
                h_hit += 1
            else:
                # present but normalized?
                h_total += 1
        # count hedged claims still delivered hedged
        unc_hit = 0
        unc_total = 0
        for eid in hedged_ids & key_ids:
            ev = store.get(eid)
            unc_total += 1
            if _faithful_in_bundle(bundle, ev.claim):
                unc_hit += 1
        unc_sum += unc_hit / unc_total if unc_total else 0.0

        if qa.kind == "temporal":
            temporal_n += 1
            temporal_ok += int(res.correct)

        # contradiction: context asserts a definite value differing from
        # the gold effective state for this slot
        if "claim" in qa.gold:
            g = qa.gold["claim"]
            slot = (g[0], g[1])
            from .answer import _effective_claims, _slot_claims
            eff = _effective_claims(_slot_claims(bundle, slot))
            for s in eff:
                c = s.claim
                if (c.certainty == "certain"
                        and (c.obj != g[2] or c.polarity != g[3])):
                    contra_hits += 1
                    break

        per_qa.append({
            "qid": qa.qid, "kind": qa.kind, "correct": res.correct,
            "found": res.found, "conflict": res.conflict,
            "values": list(res.values), "tokens": bundle.token_count,
            "mutated_claims": rep.mutated, "total_claims": rep.total_claims,
        })

    return ConditionMetrics(
        condition=condition,
        n_questions=n,
        qa_accuracy=n_correct / n if n else 0.0,
        key_fact_retention=ret_sum / n if n else 0.0,
        delayed_recall=delayed_sum / n if n else 0.0,
        rare_recall=rare_sum / n if n else 0.0,
        context_mutation_rate=mut_mutated / mut_total if mut_total else 0.0,
        mutated_claims=mut_mutated,
        total_claims=mut_total,
        contradiction_rate=contra_hits / n if n else 0.0,
        uncertainty_preservation=unc_sum / n if n else 0.0,
        temporal_accuracy=temporal_ok / temporal_n if temporal_n else 1.0,
        source_recovery=src_rec_sum / n if n else 0.0,
        pointer_rate=ptr_sum / n if n else 0.0,
        token_usage=tok_sum / n if n else 0.0,
        retrieval_ms=retrieval_ms,
        per_qa=per_qa,
        mutation_kinds=mutation_kinds,
    )
