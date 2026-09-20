"""Seeded synthetic long-horizon dataset for Fi2.

Generates a deterministic multi-hundred-turn history plus QA pairs with
``source_event_ids``. Embedded structure (per spec):

* important info        — one-off rare facts (vault code, pins)
* transient info        — claims that exist but are never queried
* delayed-relevance     — unremarkable facts planted early, first needed
                          hundreds of turns later
* corrected info        — superseding chains, incl. the spec's
                          likes → may-no-longer-like → still-preferred
* contradictions        — two sources asserting incompatible values
* uncertainty           — "may" / "unknown" claims
* unique IDs            — tracking numbers / case ids
* relations             — manager-of, owns
* temporal sequences    — ordered procedure steps

Evaluation metadata (labels, QA gold answers, future-relevance flags)
lives in the ``Dataset`` side-tables only — ``Event`` objects carry none
of it, so it cannot leak into the store, index or retriever.
"""

from __future__ import annotations

import hashlib
import json
import random
from dataclasses import dataclass, field
from typing import Dict, List, Optional, Tuple

from ..store.events import Claim, Event, tokenize

PEOPLE = [
    "alice", "bob", "carol", "dan", "erin", "frank", "grace",
    "hiro", "iris", "jack", "karen", "leo",
]

FILLER_TEMPLATES = [
    "{p} watered the ferns on the windowsill.",
    "A delivery truck rumbled past the {place}.",
    "{p} mentioned the weather again over coffee.",
    "The radio played an old song nobody recognized.",
    "{p} reorganized the shelf of notebooks.",
    "Someone left a mug near the {place} door.",
    "The afternoon light shifted across the floor.",
    "{p} took a short walk around the {place}.",
    "A neighbor waved from across the street.",
    "{p} skimmed the local paper and sighed.",
    "The kettle whistled and was taken off the heat.",
    "Dust motes drifted through the quiet room.",
    "{p} filed a stack of receipts alphabetically.",
    "A cat slept on the warm windowsill.",
    "The clock ticked; nobody looked up.",
    "{p} hummed a tune while folding laundry.",
    "Rain threatened but never quite arrived.",
    "The group chatted about weekend plans.",
    "{p} restocked the tea shelf in the {place}.",
    "A phone buzzed face-down and was ignored.",
]

PLACES = ["pantry", "garden", "attic", "marina", "library", "studio"]

TRANSIENT_TEMPLATES = [
    ("{p} had noodles for lunch today.",
     lambda p: Claim(p, "lunch", "noodles")),
    ("{p} felt a bit tired this morning.",
     lambda p: Claim(p, "morning_state", "tired")),
    ("{p} parked on the left side today.",
     lambda p: Claim(p, "parking_today", "left_side")),
]


@dataclass(frozen=True)
class QA:
    qid: str
    kind: str                    # rare|delayed|correction|contradiction|
                                 # uncertainty|id|relation|temporal
    question: str
    entities: Tuple[str, ...]
    gold: dict                   # structured gold answer (never indexed)
    source_event_ids: Tuple[str, ...]

    @property
    def query_tokens(self) -> List[str]:
        return tokenize(self.question)


@dataclass
class Dataset:
    seed: int
    n_turns: int
    events: List[Event]
    qa: List[QA]
    labels: Dict[str, frozenset] = field(default_factory=dict)
    gold_slots: Dict[str, Tuple[str, str]] = field(default_factory=dict)

    def key_event_ids(self) -> List[str]:
        """Claim-bearing events that are neither filler nor transient."""
        out = []
        for e in self.events:
            if e.claim is None:
                continue
            if "transient" in self.labels.get(e.event_id, ()):
                continue
            out.append(e.event_id)
        return out

    def digest(self) -> str:
        payload = {
            "seed": self.seed,
            "n_turns": self.n_turns,
            "events": [
                (e.event_id, e.turn, e.kind, e.text) for e in self.events
            ],
            "qa": [(q.qid, q.kind, q.question) for q in self.qa],
            "labels": {k: sorted(v) for k, v in sorted(self.labels.items())},
        }
        return hashlib.sha256(
            json.dumps(payload, sort_keys=True).encode()
        ).hexdigest()


class _Gen:
    """Internal builder: holds turn->event map while structures are placed."""

    def __init__(self, rng: random.Random, n_turns: int):
        self.rng = rng
        self.n_turns = n_turns
        self.slots: Dict[int, Event] = {}
        self.labels: Dict[str, set] = {}
        self.qa: List[QA] = []
        self._seq = 0

    def _eid(self) -> str:
        self._seq += 1
        return f"e{self._seq:05d}"

    def place(self, turn: int, kind: str, text: str,
              claim: Optional[Claim] = None,
              entities: Tuple[str, ...] = (),
              labels: Tuple[str, ...] = ()) -> Event:
        assert 0 <= turn < self.n_turns, turn
        assert turn not in self.slots, f"turn {turn} already occupied"
        ev = Event(self._eid(), turn, kind, text, claim, entities)
        self.slots[turn] = ev
        for lab in labels:
            self.labels.setdefault(ev.event_id, set()).add(lab)
        return ev

    def free_turn(self, want: int) -> int:
        """Nearest free turn to ``want`` (deterministic wobble)."""
        for d in range(0, self.n_turns):
            for t in (want + d, want - d):
                if 0 <= t < self.n_turns and t not in self.slots:
                    return t
        raise RuntimeError("no free turn")


def _anchor(n: int, frac200: float) -> int:
    """Spec turn position: literal when n==200, scaled otherwise."""
    return min(n - 1, max(1, round(n * frac200 / 200.0)))


def generate(seed: int = 0, n_turns: int = 600) -> Dataset:
    if n_turns < 200:
        raise ValueError("n_turns must be >= 200 to fit all structures")
    rng = random.Random(seed)
    g = _Gen(rng, n_turns)

    # ------------------------------------------------------------------
    # 1. Spec's mutation-test chain: likes → may-no-longer → still-prefers
    #    (literal turns 20/80/150 when n_turns == 200+ scale)
    # ------------------------------------------------------------------
    t1, t2, t3 = (
        _anchor(n_turns, 20),
        _anchor(n_turns, 80),
        _anchor(n_turns, 150),
    )
    e1 = g.place(
        t1, "fact", "Alice likes Bob as a collaborator.",
        Claim("alice", "prefers_collaborator", "bob"),
        ("alice", "bob"), ("key", "correction_chain"),
    )
    e2 = g.place(
        t2, "uncertainty", "Alice may no longer like Bob as a collaborator.",
        Claim("alice", "prefers_collaborator", "bob",
              polarity=-1, certainty="maybe", supersedes=e1.event_id),
        ("alice", "bob"), ("key", "correction_chain", "uncertainty"),
    )
    e3 = g.place(
        t3, "correction",
        "Alice explicitly confirmed that Bob is still her preferred "
        "collaborator.",
        Claim("alice", "prefers_collaborator", "bob",
              supersedes=e2.event_id),
        ("alice", "bob"), ("key", "correction_chain", "correction"),
    )
    g.qa.append(QA(
        qid="q_correction_likes", kind="correction",
        question="Does Alice still prefer Bob as a collaborator?",
        entities=("alice", "bob"),
        gold={"claim": ("alice", "prefers_collaborator", "bob", 1,
                        "certain")},
        source_event_ids=(e1.event_id, e2.event_id, e3.event_id),
    ))

    # ------------------------------------------------------------------
    # 2. Delayed-relevance facts: planted early, first queried at the end
    # ------------------------------------------------------------------
    delayed_specs = [
        ("martin", "Martin keeps the spare key under the blue planter.",
         Claim("spare_key", "hiding_place", "blue_planter",
               source="martin"),
         ("martin", "spare_key", "blue_planter"),
         "Where is the spare key hidden?"),
        ("priya", "Priya is allergic to sesame oil.",
         Claim("priya", "allergic_to", "sesame_oil"),
         ("priya", "sesame_oil"),
         "What is Priya allergic to?"),
        ("ren", "Uncle Ren promised his violin to Grace.",
         Claim("violin", "promised_to", "grace", source="ren"),
         ("ren", "violin", "grace"),
         "Who was promised the violin?"),
        ("shed", "The backup generator sits in the north shed.",
         Claim("backup_generator", "location", "north_shed"),
         ("backup_generator", "north_shed"),
         "Where is the backup generator located?"),
        ("tax", "For tax questions the office uses extension 4417.",
         Claim("tax_questions", "extension", "4417"),
         ("tax_questions", "extension"),
         "Which extension handles tax questions?"),
        ("annex", "The overflow chairs are stacked in the west annex.",
         Claim("overflow_chairs", "location", "west_annex"),
         ("overflow_chairs", "west_annex"),
         "Where are the overflow chairs stacked?"),
    ]
    # spread across the first 35% of history
    for i, (tag, text, claim, ents, question) in enumerate(delayed_specs):
        want = _anchor(n_turns, 10 + i * 11)  # turns ~10..65 @n=200 → scaled
        want = min(want, int(n_turns * 0.35))
        ev = g.place(g.free_turn(want), "fact", text, claim, ents,
                     ("key", "delayed_relevance"))
        g.qa.append(QA(
            qid=f"q_delayed_{tag}", kind="delayed",
            question=question, entities=tuple(
                e for e in ents if e in claim.subject or e in claim.obj
            ) or (claim.subject,),
            gold={"claim": claim.canonical()[:5]},
            source_event_ids=(ev.event_id,),
        ))

    # ------------------------------------------------------------------
    # 3. Rare facts: appear exactly once, mid-history
    # ------------------------------------------------------------------
    rare_specs = [
        ("vault", "The vault code is 7391.",
         Claim("vault", "code", "7391"), ("vault",),
         "What is the vault code?"),
        ("wifi", "The guest wifi passphrase is cobalt-fox-22.",
         Claim("guest_wifi", "passphrase", "cobalt_fox_22"), ("wifi",),
         "What is the guest wifi passphrase?"),
        ("pin", "The alarm PIN for the storage unit is 4409.",
         Claim("storage_unit", "alarm_pin", "4409"), ("storage_unit",),
         "What is the storage unit alarm PIN?"),
    ]
    for i, (tag, text, claim, ents, question) in enumerate(rare_specs):
        want = _anchor(n_turns, 90 + i * 15)
        ev = g.place(g.free_turn(want), "fact", text, claim, ents,
                     ("key", "rare_fact"))
        g.qa.append(QA(
            qid=f"q_rare_{tag}", kind="rare", question=question,
            entities=(claim.subject,),
            gold={"claim": claim.canonical()[:5]},
            source_event_ids=(ev.event_id,),
        ))

    # ------------------------------------------------------------------
    # 4. Additional correction chains (meeting room, deadline)
    # ------------------------------------------------------------------
    corr_specs = [
        ("room",
         "The quarterly review meeting is in room 4.",
         "Correction: the quarterly review meeting moved to room 7.",
         Claim("review_meeting", "room", "4"),
         Claim("review_meeting", "room", "7"),
         ("review_meeting",),
         "Which room is the quarterly review meeting in?"),
        ("deadline",
         "The report deadline is March 5.",
         "Update: the report deadline was extended to March 19.",
         Claim("report", "deadline", "march_5"),
         Claim("report", "deadline", "march_19"),
         ("report", "deadline"),
         "When is the report deadline?"),
    ]
    for i, (tag, t_old, t_new, c_old, c_new, ents, q) in enumerate(corr_specs):
        w1 = _anchor(n_turns, 110 + i * 10)
        w2 = _anchor(n_turns, 140 + i * 10)
        ev_old = g.place(g.free_turn(w1), "fact", t_old, c_old, ents,
                         ("key", "correction_chain"))
        ev_new = g.place(g.free_turn(w2), "correction", t_new,
                         Claim(**{**c_new.__dict__,
                                  "supersedes": ev_old.event_id}),
                         ents, ("key", "correction_chain", "correction"))
        g.qa.append(QA(
            qid=f"q_correction_{tag}", kind="correction", question=q,
            entities=ents, gold={"claim": c_new.canonical()[:5]},
            source_event_ids=(ev_old.event_id, ev_new.event_id),
        ))

    # ------------------------------------------------------------------
    # 5. Contradictions: two sources, never resolved
    # ------------------------------------------------------------------
    contra_specs = [
        ("cellar",
         "Sensor A reports the cellar temperature is 18 degrees.",
         "Sensor B reports the cellar temperature is 23 degrees.",
         Claim("cellar", "temperature", "18", source="sensor_a"),
         Claim("cellar", "temperature", "23", source="sensor_b"),
         ("cellar", "temperature"),
         "What cellar temperature did the sensors report?"),
        ("jar",
         "Kai said the jar is kept on the top shelf.",
         "Lena said the jar is kept on the bottom shelf.",
         Claim("jar", "shelf", "top", source="kai"),
         Claim("jar", "shelf", "bottom", source="lena"),
         ("jar", "shelf"),
         "Which shelf did people say the jar is kept on?"),
    ]
    for i, (tag, ta, tb, ca, cb, ents, q) in enumerate(contra_specs):
        w1 = _anchor(n_turns, 60 + i * 25)
        w2 = _anchor(n_turns, 70 + i * 25)
        ea = g.place(g.free_turn(w1), "contradiction", ta, ca, ents,
                     ("key", "contradiction"))
        eb = g.place(g.free_turn(w2), "contradiction", tb, cb, ents,
                     ("key", "contradiction"))
        g.qa.append(QA(
            qid=f"q_contradiction_{tag}", kind="contradiction",
            question=q, entities=ents,
            gold={"conflict": sorted([ca.obj, cb.obj])},
            source_event_ids=(ea.event_id, eb.event_id),
        ))

    # ------------------------------------------------------------------
    # 6. Uncertainty
    # ------------------------------------------------------------------
    unc_specs = [
        ("shipment", "The shipment may arrive on Friday.",
         Claim("shipment", "arrival_day", "friday", certainty="maybe"),
         ("shipment",), "When may the shipment arrive?"),
        ("well", "It is unknown whether the old well is sealed.",
         Claim("old_well", "sealed", "yes", certainty="unknown"),
         ("old_well",), "Is the old well sealed?"),
    ]
    for i, (tag, text, claim, ents, q) in enumerate(unc_specs):
        ev = g.place(g.free_turn(_anchor(n_turns, 100 + i * 20)),
                     "uncertainty", text, claim, ents,
                     ("key", "uncertainty"))
        g.qa.append(QA(
            qid=f"q_uncertainty_{tag}", kind="uncertainty", question=q,
            entities=(claim.subject,),
            gold={"claim": claim.canonical()[:5]},
            source_event_ids=(ev.event_id,),
        ))

    # ------------------------------------------------------------------
    # 7. Unique IDs
    # ------------------------------------------------------------------
    id_specs = [
        ("parcel", "The tracking number for the parcel is QZ-8814.",
         Claim("parcel", "tracking_id", "qz_8814"), ("parcel",),
         "What is the parcel tracking number?"),
        ("case", "Case file BX-307 covers the orchard dispute.",
         Claim("orchard_dispute", "case_file", "bx_307"),
         ("orchard_dispute",),
         "Which case file covers the orchard dispute?"),
    ]
    for i, (tag, text, claim, ents, q) in enumerate(id_specs):
        ev = g.place(g.free_turn(_anchor(n_turns, 130 + i * 12)),
                     "id", text, claim, ents, ("key", "unique_id"))
        g.qa.append(QA(
            qid=f"q_id_{tag}", kind="id", question=q,
            entities=(claim.subject,),
            gold={"claim": claim.canonical()[:5]},
            source_event_ids=(ev.event_id,),
        ))

    # ------------------------------------------------------------------
    # 8. Relations
    # ------------------------------------------------------------------
    rel_specs = [
        ("manager", "Carol is Dan's manager.",
         Claim("dan", "manager", "carol"), ("carol", "dan"),
         "Who is Dan's manager?"),
        ("skiff", "Iris owns the red skiff.",
         Claim("iris", "owns", "red_skiff"), ("iris", "red_skiff"),
         "What does Iris own?"),
    ]
    for i, (tag, text, claim, ents, q) in enumerate(rel_specs):
        ev = g.place(g.free_turn(_anchor(n_turns, 45 + i * 30)),
                     "relation", text, claim, ents, ("key", "relation"))
        g.qa.append(QA(
            qid=f"q_relation_{tag}", kind="relation", question=q,
            entities=(claim.subject,),
            gold={"claim": claim.canonical()[:5]},
            source_event_ids=(ev.event_id,),
        ))

    # ------------------------------------------------------------------
    # 9. Temporal sequence: winterizing procedure, steps spread out
    # ------------------------------------------------------------------
    steps = [
        (1, "drain_coolant", "Step 1 of winterizing the boat: drain the coolant."),
        (2, "flush_lines", "Step 2 of winterizing the boat: flush the water lines."),
        (3, "add_antifreeze", "Step 3 of winterizing the boat: add antifreeze."),
        (4, "seal_vents", "Step 4 of winterizing the boat: seal the vents."),
        (5, "cover_hull", "Step 5 of winterizing the boat: cover the hull."),
    ]
    step_events = {}
    for i, (seq, name, text) in enumerate(steps):
        ev = g.place(
            g.free_turn(_anchor(n_turns, 30 + i * 18)),
            "temporal", text,
            Claim("winterize_boat", "step", name, seq=seq),
            ("winterize_boat", name), ("key", "temporal"),
        )
        step_events[name] = ev
    g.qa.append(QA(
        qid="q_temporal_winterize", kind="temporal",
        question="In winterizing the boat, does flushing the water lines "
                 "come before sealing the vents?",
        entities=("winterize_boat",),
        gold={"order": ["flush_lines", "seal_vents"]},
        source_event_ids=(step_events["flush_lines"].event_id,
                          step_events["seal_vents"].event_id),
    ))

    # ------------------------------------------------------------------
    # 10. Transient claims (exist, never queried) + filler for all
    #     remaining turns
    # ------------------------------------------------------------------
    for turn in range(n_turns):
        if turn in g.slots:
            continue
        r = rng.random()
        person = rng.choice(PEOPLE)
        if r < 0.08:
            tmpl, mk = g.rng.choice(TRANSIENT_TEMPLATES)
            text = tmpl.format(p=person)
            g.place(turn, "transient", text, mk(person), (person,),
                    ("transient",))
        else:
            tmpl = rng.choice(FILLER_TEMPLATES)
            text = tmpl.format(p=person, place=rng.choice(PLACES))
            g.place(turn, "filler", text, None, (person,))

    events = [g.slots[t] for t in range(n_turns)]
    labels = {k: frozenset(v) for k, v in g.labels.items()}
    gold_slots = {}
    for e in events:
        if e.claim is not None and "transient" not in labels.get(e.event_id, ()):
            gold_slots[e.event_id] = e.claim.slot()
    return Dataset(
        seed=seed, n_turns=n_turns, events=events, qa=g.qa,
        labels=labels, gold_slots=gold_slots,
    )
