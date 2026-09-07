# Speech and Vocal Expression System

Status: **living design note**

Kamimusuhi should not treat speech as `text -> TTS` glued onto the end of cognition. Spoken interaction is an **active motor and social-control system** involving timing, turn-taking, prosody, interruption, backchannels, vocal identity, self-monitoring, and non-lexical expression.

This document describes a technical architecture for speech production and conversational voice as part of Kamimusuhi's distributed artificial nervous system.

## 1. Design thesis

The Persona Core should decide **what Kamimusuhi means**. A dedicated vocal-expression stack should decide **how that intention becomes an embodied utterance over time**.

```text
Persona Core / Global Workspace
            │
            │ communicative intention
            ▼
      Speech-Act Planner
            │
      Utterance Planner
            │
      Prosody / Affect Plan
            │
      Incremental Speech Plan
            │
      Streaming Speech Model
            │
       Audio Motor Output
            │
        speaker/device
            │
            └──── auditory self-monitoring ────► sensory system
```

This separation allows the same identity to speak through different devices, speakers, synthesizers, and future robot bodies without reducing the identity to one TTS checkpoint.

## 2. Speech is a motor modality

In a biological analogy, speech belongs closer to **motor control** than to passive text rendering.

The system must coordinate:

- lexical content;
- sentence structure;
- speech act / conversational intent;
- onset timing;
- turn yielding or turn holding;
- pacing and pause placement;
- pitch, energy, and rhythm;
- emphasis and contrastive stress;
- uncertainty/hesitation behavior;
- laughter, breath, sighs, fillers, and other non-lexical vocal acts;
- interruption and cancellation;
- adaptation to another speaker's timing;
- device/audio-path conditions.

The implementation does not need to imitate human motor neurobiology literally. The functional decomposition is the important part.

## 3. The utterance should be planned at multiple levels

### 3.1 Communicative intention

The Persona Core should produce a structured intention before final surface realization where practical.

Conceptual example:

```json
{
  "speech_act": "disagree_gently",
  "content": {
    "claim": "the second assumption is unsupported",
    "evidence_refs": ["mem_12", "src_41"]
  },
  "relationship_context": "close",
  "stance": "confident",
  "urgency": "normal",
  "turn_policy": "hold_until_explanation_complete"
}
```

The goal is not to force every utterance through verbose JSON. The goal is to preserve a boundary between **intention** and **surface voice**.

### 3.2 Linguistic realization

A realization layer converts intention into lexical/syntactic output while preserving Persona Core style.

It should support:

- fragmentary speech instead of always-complete written sentences;
- contractions and colloquial forms;
- discourse markers;
- callbacks to previous conversation;
- ellipsis;
- deliberate under-explanation;
- self-correction;
- appropriate hesitation;
- response length matched to the interaction state.

Written prose quality and spoken naturalness are different objectives.

### 3.3 Prosodic realization

Prosody should be first-class structured state, not inferred only from punctuation.

Possible representation:

```text
valence-like tone
activation / intensity
confidence
warmth
social distance
irony / teasing
urgency
speech rate
pitch range
energy range
pause tendency
```

These values may be influenced by Persona Core style, relationship state, digital interoception, and the immediate conversational situation.

## 4. Full-duplex conversation is the target

Traditional voice assistants often implement:

```text
listen -> detect end of turn -> ASR -> LLM -> TTS -> play
```

This pipeline creates unnatural dead time and cannot naturally model overlap, interruptions, interjections, or active listening.

Moshi (Kyutai, 2024) is an important precedent. It models separate user and system speech streams and performs real-time speech-to-speech generation without requiring rigid speaker turns. The paper reports a theoretical latency of about 160 ms and approximately 200 ms in practice.

Reference:

- Défossez et al., *Moshi: a speech-text foundation model for real-time dialogue*, 2024. https://arxiv.org/abs/2410.00037

The 2026 full-duplex speech-dialogue survey literature now explicitly treats behaviors such as simultaneous listening/speaking, interruption, backchanneling, and turn-state decisions as distinct system capabilities rather than treating "full duplex" as a binary marketing term.

Reference:

- *Speaking While Listening: A Survey and Empirical Audit of Full-Duplex Spoken Dialogue Systems*, EMNLP 2026. https://github.com/MM-Speech/DuplexSurvey

Kamimusuhi should therefore maintain an explicit interaction state such as:

```text
IDLE
LISTEN
SPEAK
WAIT
DUAL       # listening while speaking
INTERRUPT
YIELD
```

A dedicated turn-taking controller may initially be rule-based and later learned.

## 5. Backchannels and micro-utterances

Human conversation contains many vocal events that do not require full Persona Core deliberation:

- "ん"
- "うん"
- "あー"
- acknowledgement sounds;
- laughter;
- surprise;
- brief correction;
- short turn-yielding cues.

These can be produced through a **vocal reflex / K-Edge path**.

```text
incoming speech event
      │
      ├── obvious backchannel opportunity
      │          ▼
      │    Vocal Reflex Policy
      │          ▼
      │    short vocal output
      │
      └── meaningful semantic response
                 ▼
            Persona Core
```

The reflex system should not fabricate semantic commitments. A backchannel such as acknowledgement is different from agreeing with a claim.

## 6. Incremental generation

Kamimusuhi should not need the complete sentence before beginning speech.

Possible pipeline:

```text
Persona Core streams semantic/linguistic chunks
              │
              ▼
Incremental Utterance Planner
              │
              ▼
Streaming TTS / speech-token model
              │
              ▼
first audio packet
```

Qwen3-TTS is a relevant current implementation precedent. Its official repository describes a dual-track streaming design and reports first-audio generation after a single input character with end-to-end synthesis latency as low as 97 ms. It also supports voice design and models paralinguistic/acoustic information using discrete speech tokens.

Reference:

- QwenLM, Qwen3-TTS. https://github.com/QwenLM/Qwen3-TTS

This does **not** imply Kamimusuhi should immediately adopt Qwen3-TTS. It establishes that sub-100-ms-class streaming synthesis is technically plausible on suitable hardware and that speech generation can expose richer control than conventional fixed-voice TTS.

## 7. Conversational speech models vs ordinary TTS

Sesame CSM is another useful precedent. It generates neural-codec audio codes conditioned on text and prior conversational audio and demonstrates that conversational context can improve prosodic continuity.

Reference:

- SesameAILabs, CSM. https://github.com/SesameAILabs/csm

Its public documentation also explicitly notes a current limitation: generating good conversational prosody is not equivalent to modeling full conversation structure such as turn taking, pauses, and pacing. That distinction is directly relevant to Kamimusuhi.

Therefore, Kamimusuhi should separate:

```text
Speech generation quality
!=
Conversation timing intelligence
```

A beautiful TTS voice with poor interruption and turn behavior will still feel non-living.

## 8. Voice identity

Kamimusuhi should have a recognizable voice identity, but the identity must not be stored only in one TTS model.

The durable voice specification should include attributes such as:

```text
base timbre identity / speaker embedding reference
preferred pitch region
pitch variability
speech-rate baseline
articulation tendency
breathiness / texture preferences
habitual pause patterns
lexical fillers
accent / dialect constraints
expressive limits
```

The actual synthesizer is a replaceable motor organ.

```text
Persistent Vocal Identity
         │
         ├── TTS backend A
         ├── speech-token model B
         ├── K-Edge low-cost voice
         └── future robot vocal tract / speaker
```

A synthesizer migration should be evaluated for **identity similarity**, intelligibility, latency, and expressive control.

## 9. Paralinguistic output

Speech contains information not carried by text alone.

Kamimusuhi should eventually model or control:

- laughter;
- sighs;
- breaths;
- hesitation;
- whisper-like delivery;
- raised/lowered intensity;
- sarcasm/irony cues;
- emotional restraint;
- emphasis;
- silence.

Silence is an output decision, not merely failure to generate.

The system should be capable of intentionally waiting rather than filling every gap.

## 10. Auditory self-monitoring

Speech output should return to the sensory nervous system through a self-monitoring path.

```text
planned speech
    │
    ▼
audio synthesis
    │
    ├──► physical speaker
    │
    └──► self-monitor stream
                 │
                 ▼
       expected vs actual output
```

This enables detection of:

- synthesis failure;
- clipping;
- wrong voice/backend;
- output-device loss;
- latency spikes;
- accidental duplicated speech;
- interruption by the user;
- echo/acoustic feedback conditions.

A future implementation may distinguish **efference-copy-like expected output** from actual microphone input so Kamimusuhi does not interpret its own speech as another speaker.

## 11. Acoustic transport requirements

A practical full-duplex deployment needs conventional audio engineering as much as AI:

- low-latency audio I/O;
- echo cancellation (AEC);
- noise suppression;
- microphone-array processing where available;
- voice activity / speech activity estimation;
- speaker tracking;
- buffering and jitter control;
- cancellation of already queued TTS audio;
- device handoff.

The speech model cannot compensate for a poor duplex audio path.

## 12. Interruptibility

A living conversational system should be interruptible.

Requirements:

1. Kamimusuhi must continue listening while speaking where the device permits.
2. A user interruption must be detected quickly.
3. Already synthesized but not yet played audio must be cancellable.
4. The Persona Core/workspace must be told what portion of its utterance was actually audible.
5. The interrupted utterance must not be recorded as fully communicated common ground.

This last point matters for relationship and memory correctness.

```text
planned utterance: 100%
played:            42%
interrupted

shared/common ground update must use <= 42%
```

## 13. Distributed vocal embodiment

Different K-Edge nodes may have different vocal capabilities.

Examples:

```text
phone
  speaker + microphone
  immediate conversational speech

home speaker
  room-scale voice
  microphone array

laptop
  local/private speech

robot body
  spatially embodied voice
  synchronized gaze/gesture
```

All are possible vocal organs of the same individual.

The continuity system should track where an utterance originated and which humans/devices could plausibly hear it.

## 14. Coordination with gaze and gesture

When physical embodiment exists, speech should eventually synchronize with:

- gaze direction;
- facial animation;
- head movement;
- gesture;
- posture;
- turn-taking cues.

A speech motor event should expose timing markers so other motor systems can coordinate with it.

Conceptual event:

```json
{
  "utterance_id": "utt_...",
  "start": 0.0,
  "segments": [
    {"t": 0.3, "kind": "emphasis", "target": "phrase_2"},
    {"t": 0.7, "kind": "yield_signal"}
  ]
}
```

## 15. Latency targets

Initial engineering targets should focus on perceived responsiveness rather than raw waveform throughput.

Provisional SLOs:

```text
vocal reflex/backchannel     ~50-150 ms decision
speech-start planning        <200-300 ms when warm
first audio packet           target <250 ms local path
interruption stop            target <150 ms
normal conversational gap    ~human-like adaptive, not fixed
```

These are design targets, not claims about current hardware.

Deep cognition may take longer, but Kamimusuhi can remain socially responsive through short acknowledgement, intentional hesitation, or explicit escalation behavior without pretending that a deep answer is already known.

## 16. Model architecture options

### Option A — modular cascade, recommended first

```text
Persona Core
 -> incremental text/intention
 -> prosody controller
 -> streaming TTS
```

Pros:

- easiest to inspect/debug;
- Persona Core remains text/cognition focused;
- TTS backend replaceable;
- easier local hardware optimization.

Cons:

- some non-linguistic information can be lost at interfaces;
- full-duplex timing requires separate control logic.

### Option B — contextual speech generator

Use CSM/Qwen3-TTS-class models with conversational/prosodic conditioning while retaining Persona Core as identity/cognition.

### Option C — native speech-to-speech Persona Core

A future Kamimusuhi-native model could jointly model linguistic cognition and audio tokens, Moshi-style.

This is attractive for latency and natural overlap but dangerous architecturally: vocal-acoustic behavior, semantic cognition, and identity may become entangled in one opaque checkpoint.

The project should only move here after the modular architecture establishes measurable requirements.

## 17. Evaluation

Speech evaluation should include more than MOS/naturalness.

### Acoustic

- intelligibility;
- speaker/voice consistency;
- prosody quality;
- artifact rate;
- latency / first audio packet;
- real-time factor.

### Conversational

- interruption response;
- backchannel timing;
- turn-yield accuracy;
- overlap behavior;
- silence/pause appropriateness;
- recovery after interruption;
- contextual prosody consistency.

### Identity

- recognizability across TTS backend swaps;
- consistency with Persona Core affect/style;
- non-sycophantic vocal expression;
- continuity across K-Edge/K-Core handoffs.

### System correctness

- actual-played vs planned utterance tracking;
- acoustic echo/self-speech separation;
- cancellation correctness;
- trace provenance.

## 18. Near-term implementation plan

### Phase 1

- streaming Japanese-capable TTS adapter;
- persistent vocal profile;
- incremental Persona Core text output;
- barge-in/cancel support;
- utterance lifecycle events;
- self-monitoring of playback state.

### Phase 2

- prosody control;
- explicit turn-state machine;
- vocal backchannel/reflex path;
- conversational audio context;
- latency benchmark suite.

### Phase 3

- full-duplex listening while speaking;
- non-lexical vocal actions;
- cross-device vocal embodiment;
- speech/gaze/gesture synchronization.

### Phase 4 research

- speech-token Persona Core experiments;
- joint semantic/acoustic recurrent cognition;
- learned turn-taking;
- dedicated K-Edge speech/reflex model.

## 19. Architectural rule

> **Kamimusuhi speaks; the TTS engine does not play Kamimusuhi.**

The voice synthesizer is a vocal organ. Communicative intent, relationship meaning, memory consequences, and the decision to speak or remain silent belong to the individual.