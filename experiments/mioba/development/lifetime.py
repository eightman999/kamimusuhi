"""Lifetime plasticity state: the mutation/plasticity boundary (M2 §14-15).

``genome.plasticity_rules`` are *heritable rules* — genome content, hashed
and inherited like any other gene. The state they produce during a life
is the opposite: per-individual runtime state that is

- never written into the genome (``LifetimeState`` is not a genome
  field and does not appear in ``to_dict``),
- never part of ``genome_hash`` (it is not heritable content),
- never inherited (a child gets the parent's *rules*, not the parent's
  *adjustments* — Lamarck is not in this architecture).

The rules are deferred here by ``development/rules.py`` (the birth-stage
interpreter) and consume the deterministic evaluation stream, never a
shared RNG: ``apply`` takes the event index and the rule's own content;
the caller drives it with the per-lane disturbance/noise streams
(``fba/seeds.py``) so a plasticity event stays a property of the
recording.

Implemented ops (from the deferred rule list):

``SCALE_WEIGHT``      ``{"op","target","factor"}`` — multiply the
                      organ's running weight_scale
``GROW_AFTER_N_STEPS`` ``{"op","organ_id","steps","delta"}`` — when
                      ``step >= steps``, record +delta neurons of
                      accrued growth on that organ
``ENABLE_IF_SIGNAL`` / ``DISABLE_IF_RESOURCE_LOW``
                      ``{"op","organ_id",...}`` — enable/disable flags
                      applied to the runtime organ state

Anything else is recorded ``skipped`` — a rule the runtime does not
know is data, not a crash.
"""
from __future__ import annotations

from dataclasses import asdict, dataclass, field

LIFETIME_RULE_OPS = ("GROW_AFTER_N_STEPS", "ENABLE_IF_SIGNAL",
                     "DISABLE_IF_RESOURCE_LOW", "DUPLICATE_ON_THRESHOLD")


@dataclass
class PlasticityEvent:
    """One applied (or attempted) lifetime plasticity event."""
    index: int
    op: str
    outcome: str              # applied | pending | skipped
    target: str | None = None
    detail: dict = field(default_factory=dict)


@dataclass
class LifetimeState:
    """Per-individual runtime state produced by plasticity rules.

    ``adjustments`` maps organ_id -> accrued runtime adjustments:
    ``weight_scale`` multipliers, ``extra_neurons`` of lifetime growth,
    ``enabled`` overrides. It starts empty at birth — a new individual
    inherits the *rules*, never the parent's adjustments.
    """
    adjustments: dict = field(default_factory=dict)
    events: list = field(default_factory=list)
    step: int = 0

    def _organ(self, organ_id: str) -> dict:
        return self.adjustments.setdefault(
            organ_id, {"weight_scale": 1.0, "extra_neurons": 0,
                       "enabled": True})

    def apply(self, rule: dict, index: int | None = None) -> PlasticityEvent:
        """Apply one plasticity rule to this individual's runtime
        state. Deterministic given (rule, step, prior state)."""
        idx = len(self.events) if index is None else int(index)
        op = (rule or {}).get("op")
        oid = (rule or {}).get("organ_id") or (rule or {}).get("target")
        ev = PlasticityEvent(index=idx, op=op, outcome="applied",
                             target=oid)

        if op == "SCALE_WEIGHT" and oid:
            organ = self._organ(oid)
            organ["weight_scale"] = round(
                organ["weight_scale"] * float(rule.get("factor", 1.0)), 6)
            ev.detail = {"weight_scale": organ["weight_scale"]}
        elif op == "GROW_AFTER_N_STEPS":
            if self.step < int((rule or {}).get("steps", 0)):
                ev.outcome = "pending"
                ev.detail = {"reason": "before_threshold",
                             "step": self.step}
            else:
                organ = self._organ(oid or "")
                organ["extra_neurons"] += int(rule.get("delta", 0))
                ev.detail = {"extra_neurons": organ["extra_neurons"]}
        elif op == "ENABLE_IF_SIGNAL" and oid:
            self._organ(oid)["enabled"] = True
        elif op == "DISABLE_IF_RESOURCE_LOW" and oid:
            self._organ(oid)["enabled"] = False
        else:
            ev.outcome = "skipped"
            ev.detail = {"reason": "unknown_or_unsupported_op"}

        self.events.append(ev)
        return ev

    def effective_organ(self, organ_id: str) -> dict:
        """The runtime view of one organ: genome values plus accrued
        adjustments. The genome's record is not consulted or modified —
        this reads only runtime state."""
        return dict(self.adjustments.get(organ_id) or
                    {"weight_scale": 1.0, "extra_neurons": 0,
                     "enabled": True})

    def to_dict(self) -> dict:
        return {"step": self.step,
                "adjustments": {k: dict(v)
                                for k, v in self.adjustments.items()},
                "events": [asdict(e) for e in self.events]}


def lifetime_rules_of(genome) -> list:
    """The genome's plasticity rules plus the deferred development
    rules — everything that may fire during a life."""
    rules = list(getattr(genome, "plasticity_rules", None) or [])
    rules += [dict(r, _via="development_rules")
              for r in (getattr(genome, "development_rules", None) or [])
              if (r or {}).get("op") in LIFETIME_RULE_OPS]
    return rules
