# MIOA architecture

**MIOA = Machine Information Organism Architecture.**

MIOA is the current architecture term for the **whole artificial organism**.
It deliberately does not contain “Brain”: a MIOA organism may distribute
sensing, state, computation, memory and action across multiple organs and
substrates, with no requirement for a single central brain.

> Compatibility note: `MIOBA` is the historical implementation / experiment
> name used by the existing `experiments/mioba/` package, CLI, environment
> variables, M0/M1 records and archived reports. Those identifiers are kept
> stable for reproducibility. New architecture prose should use **MIOA** for
> the organism as a whole and use **MIOBA** only when referring to that legacy
> code or an experiment recorded under that name.

## Terminology

| term | meaning |
|---|---|
| **MIOA** | Machine Information Organism Architecture: organism-level architecture |
| **MIO** | one Machine Information Organism / individual |
| **FBA** | Fly-Brain Architecture: fly-derived neural architecture |
| **FBA0** | the current immutable ancestral neural substrate used to found M-series organisms |
| **MIOBA** | compatibility / historical name for the current experimental implementation and its recorded M-series infrastructure |
| **M-series** | experimental lineage used to test evolution, organs, homeostasis and substrate departure |

FBA0 is therefore **not the organism** and is not required to remain the
organism's permanent “brain”. It is the current ancestral substrate from which
MIOA organisms begin.

## Organism model

The architecture is organism-centric rather than brain-centric:

```text
                           MIOA organism

        ┌───────────────────────────────────────────────┐
world ↔ │ sensory organs / transducers                  │
        │        ↕                                      │
        │ ancestral substrate (currently FBA0)          │
        │        ↕                                      │
        │ evolved artificial organs ↔ memory / state    │
        │        ↕                    ↕                 │
        │ machine interoception ↔ action/effectors      │
        └───────────────────────────────────────────────┘
                           ↕
                         habitat
```

Computation may occur in any of these organs. A future MIOA can therefore have
no uniquely identifiable “brain” while still having coherent organism-level
behaviour.

### Architectural invariants

1. **No mandatory central brain.** `sensor → brain → actuator` is one possible
   phenotype, not a required topology.
2. **Organs are causal parts of the organism.** Merely adding neurons or data
   channels does not make a useful organ; an organ must be able to affect the
   organism's state or behaviour and its contribution must be measurable.
3. **Semantics are not supplied by the transducer.** A sensor may transform
   photons, pressure waves, packets or machine state into signals, but labels
   such as “food”, “enemy”, “voice” or “object” belong to learned/evolved
   internal organization, not the input API.
4. **Biological senses are examples, not limits.** Vision and hearing are early
   test cases. Network activity, filesystem events, compute pressure, clock
   phase, latency and other machine-native signals may become equally valid
   senses.
5. **The habitat is part of the experiment, not hidden host state.** Scientific
   environmental inputs must be explicit, seeded/traceable where applicable,
   and separable from runtime telemetry.

## Sensory organs

MIOA should not hard-code `Eye` and `Ear` as privileged architecture classes.
The general pattern is:

```text
information source → transducer → organ encoding/dynamics → organism network
```

A human-facing presentation may describe an organ as an eye, ear, antenna,
resonator or other fantasy-species anatomy, while the experimental interface
remains general.

### Spatial / vision-like organ

An early spatial organ may consume a small display-space or camera-space field
(e.g. low-resolution intensity or event-like ON/OFF changes) and turn local
spatiotemporal change into neural signals. It should not receive pre-labelled
objects.

Candidate evolvable quantities include receptive-field geometry, gain,
threshold, temporal decay, sample rate, output population size and attachment
points.

### Resonance / hearing-like organ

An early resonance organ may consume a waveform or a compact frequency-bank
representation and expose amplitude / temporal change as signals. Speech
recognition is explicitly outside the primitive organ: linguistic meaning, if
it ever emerges, belongs above the transducer.

The same organ abstraction can later accept machine-native periodic streams,
packet timing or other temporal signals without changing the organism model.

## Habitat: inside the display

The preferred early sandbox is **inside the display** rather than a miniature
physical-world simulator. The display is a boundary between the human world
and the information organism's first habitat, not merely a visualization of a
separate world.

```text
human / physical world
   ↓ mouse, touch, voice, camera, optional external signals
┌──────────────────────── display boundary ────────────────────────┐
│                                                                  │
│   MIOA habitat: spatial fields, moving signals, sound/resonance,  │
│   resources, hazards, other organisms, portals / interfaces      │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
   ↑ pixels, sound, motion, organism behaviour
```

The habitat does not need to copy Earth physics. Information-native variables
such as visibility, distance, bandwidth, latency, memory/compute budget,
attention and persistence can be first-class environmental quantities.

A useful staged boundary is:

```text
closed display habitat
        ↓
optional camera / microphone transducers
        ↓
filesystem / network / machine-native senses
        ↓
other hosts and other MIOA organisms
```

Opening a new interface should therefore be treated as adding or evolving a
new sensory/action organ, not as silently granting the substrate arbitrary
host access.

## FBA0 ancestry and departure

The current implementation describes an artificial organism as an immutable
FBA0 reference plus artificial organs, attachments and parameter mutations.
That is the **founder condition**, not the intended definition of MIOA.

Two distinct forms of departure must be tested separately.

### Functional departure

Before physically deleting FBA0, test whether evolution transfers causal
responsibility away from it. For matched environments/seeds, compare at least:

- full organism;
- artificial-organ lesion / ablation;
- FBA0 lesion (graded where possible);
- FBA0-only or founder baseline.

A useful departure signature is simultaneous:

- preserved task/homeostasis/survival performance;
- increasing tolerance to FBA0 lesions;
- increasing performance loss when evolved artificial organs are ablated;
- artificial organs lying on causal paths used by successful behaviour.

Activity difference alone is insufficient: a broken or noisy organism can be
very different from FBA0 without having evolved a useful departure.

Controls should include a parameter-only lineage and a structural-mutation
lineage without the same selection pressure, so “changed” can be separated
from “selected to become less FBA-dependent”.

### Structural replacement

Only after functional departure is demonstrated should later experiments allow
mutations such as disabling, bypassing, pruning or replacing FBA0 regions.
That experiment asks whether ancestry can physically shrink while viability is
preserved. It is intentionally distinct from the current immutable-FBA0 M1
condition.

## Current implementation mapping

Today, the legacy `experiments/mioba/` implementation provides the first MIOA
experimental substrate:

- FBA0 reference: FlyWire v783 connectivity simulated with the Shiu et al.
  2024 LIF + delayed alpha-synapse model;
- genome lineage with artificial organs and attachments;
- structural growth/pruning and parameter mutation;
- seeded virtual disturbances and homeostatic debt;
- machine-interoceptive channels;
- separated task, homeostasis, recovery, efficiency, structural and novelty
  metrics;
- coordinator, GPU workers, lineage SQLite storage and Observatory GUI.

It does **not** yet establish long-run open-ended evolution, a mature sensory
organ system, or physical replacement of FBA0. Those remain experimental
questions.

## Runtime components (legacy MIOBA implementation)

```text
            ┌─────────────────────────────────────────────────────┐
            │ coordinator (FastAPI + uvicorn, single writer)       │
            │  app.py        HTTP API + worker protocol            │
            │  service.py    MiobaService: DB, RNG, bg loop (1 Hz) │
            │  lifecycle.py  pause/resume/checkpoint/stop/resume   │
            │  jobs.py       claim/finish/requeue policy           │
            └──────┬──────────────────┬──────────────────┬────────┘
                   │                  │                  │
   POST /api/worker/*          SQLite (WAL)        mount_gui()
   register/heartbeat/    lineage.sqlite +        / + /api/gui/*
   claim/result           checkpoints/,           static SPA
                   │      telemetry/, traces/          │
            ┌──────┴──────┐                            │
            │  workers    │                    Observatory GUI
            │ worker.py   │                    (read-only SPA)
            │ bench.py    │
            │ gpu_info.py │
            └──────┬──────┘
                   │ develops genome → FBA backend
            ┌──────┴───────────────────────────┐
            │ fba/: mock | torch | genn        │
            │ (Shiu 2024 LIF + alpha synapse,  │
            │  delayed propagation)            │
            └──────────────────────────────────┘

   MIE collectors (coordinator-side + worker heartbeats):
   gpu/cpu/ram/network_latency → telemetry_samples + samples.jsonl
```

- **Coordinator** is the only writer to `lineage.sqlite`; the GUI and CLI do
  not own scientific state.
- **Workers** are stateless executors: register → benchmark → claim → develop →
  evaluate → result.
- **MIE telemetry** and **scientific interoception** are distinct. Host GPU/CPU
  telemetry must not affect fitness unless explicitly introduced as a traced
  experimental input.

## LIVE / RECORDED / DERIVED

Every API payload carries `kind`:

- `LIVE` — current runtime state (status, workers, runtime info).
- `RECORDED` — rows read from the DB (genomes, evaluations, events,
  telemetry).
- `DERIVED` — computed on request from recorded rows (throughput, ancestry
  chains, phenotype summaries, latest-per-signal telemetry).

## Job state machine

```text
QUEUED ──claim (atomic UPDATE ... WHERE status='QUEUED')──▶ RUNNING
RUNNING ──worker result──▶ SUCCEEDED | FAILED
RUNNING ──stale worker or coordinator restart────────────▶ UNKNOWN
UNKNOWN ──requeue policy─────────────────────────────────▶ QUEUED | FAILED
any non-terminal ──cancel────────────────────────────────▶ CANCELLED
```

`RUNNING → SUCCEEDED` is never inferred; only an explicit result from the
worker currently holding the claim finishes a job. Pause stops new claims;
gracious stop waits for running work, checkpoints and exits.

## Checkpoint / resume

Runtime state remains under the legacy MIOBA paths. Checkpoint manifests and
`lineage.sqlite` are the source of truth for an experiment. Existing M0/M1
records must continue to be replayed with the implementation and scientific
configuration that produced them; the MIOA terminology change is not a reason
to rewrite historical experiment identity.

## Boundaries

- **vs Kamimusuhi canonical identity:** MIOA/MIOBA genome lineage is experiment
  data in `lineage.sqlite`; it is not canonical Kamimusuhi identity lineage.
- **vs fly-brain:** FBA0 is an ancestral experimental substrate. The upstream
  fly-brain code remains external; data/licensing provenance must remain
  explicit.
- **vs embodiment:** a display habitat, camera, microphone or network interface
  is not implicitly part of the organism. The organism/habitat boundary and
  each transducer must be declared by the experiment.
