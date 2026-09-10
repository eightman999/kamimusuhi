# Attention Sensing and Joint Attention

Status: **provisional design note / living engineering design**

Date: **2026-09-10**

Kamimusuhi should treat gaze tracking not as a privileged mind-reading channel, but as one low-level source of evidence for a broader **attention sensing** subsystem.

The preferred near-term design is deliberately compatible with **non-deep** implementations: geometric eye tracking, pupil/iris localization, head pose, cursor state, active-window state, touch, and speech-reference cues can all contribute without requiring a vision-language model or a deep gaze-estimation network in the hot path.

This note is a design hypothesis, not a claim that gaze directly reveals interest, intent, emotion, or internal mental state.

Related:

- [`sensory-nervous-system.md`](./sensory-nervous-system.md) — receptors, sensory event bus, multimodal binding, and salience;
- [`peripheral-neural-layer.md`](./peripheral-neural-layer.md) — low-latency local processing, orientation/gaze primitives, and reflex boundaries;
- [`multiscale-brain-architecture.md`](./multiscale-brain-architecture.md) — K-Fast / Global Workspace timing domains;
- [`speech-and-vocal-expression.md`](./speech-and-vocal-expression.md) — dialogue timing and social output;
- [`../architecture.md`](../architecture.md) — normative top-level architecture.

---

## 1. Design decision

Kamimusuhi SHOULD model **attention sensing** as a multimodal evidence layer rather than an `eye_tracking` feature directly connected to semantic cognition.

```text
camera / eye tracker / desktop / input devices
                    |
                    v
        near-sensor preprocessing
   gaze / head / cursor / window / touch
                    |
                    v
             SensoryEvent Bus
                    |
                    v
        Attention Evidence Fusion
                    |
         +----------+----------+
         |                     |
         v                     v
 user-attention state     salience / K-Edge
         |                     |
         +----------+----------+
                    v
             Global Workspace
                    |
                    v
              Persona Core
```

The core rule is:

> **Observation is not interpretation.**

For example:

```text
observed: user gaze remained on object A for 1.2 s
```

must not silently become:

```text
inferred: user likes object A
inferred: user is bored
inferred: user intends to select object A
```

Those are separate hypotheses that require additional evidence and explicit uncertainty.

---

## 2. Why non-deep gaze belongs near the sensory edge

Most useful gaze information does not require full semantic image understanding at camera frame rate.

A lightweight path may use:

```text
face / eye ROI
   -> pupil or iris center
   -> optional head-pose estimate
   -> calibration transform
   -> gaze vector / coarse gaze zone
   -> fixation / saccade / blink detector
   -> attributed event
```

Candidate implementations include conventional image processing, geometric calibration, deterministic filters, state machines, and small conventional estimators.

Deep gaze estimators MAY later be added as optional backends, but the canonical Kamimusuhi interface SHOULD NOT require them.

Advantages of this placement:

- continuous sensing can run without token generation;
- raw video can remain local to the receptor/edge node;
- compact events reduce bandwidth and storage;
- behavior survives K-Core/model/network unavailability;
- implementations can be replaced without changing higher cognition;
- failure and calibration quality can be represented explicitly.

---

## 3. Event representation

The sensory layer should emit observations rather than psychological conclusions.

Example gaze event:

```json
{
  "modality": "gaze",
  "source": "desk_cam.eye_tracker",
  "timestamp": "...",
  "event": "fixation",
  "target": "display_1.region_42",
  "duration_ms": 1180,
  "confidence": 0.82,
  "quality": {
    "calibration_error_deg": 1.7,
    "face_visible": true
  },
  "provenance": {
    "processor": "gaze_geom_v1",
    "raw_retained": false
  }
}
```

Useful low-level event families include:

```text
gaze_enter
gaze_leave
fixation_started
fixation_ended
saccade
blink
user_looking_toward_agent
user_looking_away
head_orientation_changed
attention_evidence_changed
```

A target SHOULD be spatially or digitally attributable where possible, for example:

```text
physical.object.cup_7
display_1.window.browser.region_42
robot_1.face
unknown_direction.right_up
```

---

## 4. Observation -> hypothesis -> belief

Kamimusuhi should preserve at least three epistemic levels:

```text
sensor observation
      |
      v
candidate hypothesis
      |
      v
bounded belief/state update
      |
      v
possible action or workspace promotion
```

Example:

```yaml
observation:
  type: fixation
  target: display_1.region_42
  duration_ms: 1180
  confidence: 0.82

hypothesis:
  type: possible_attention_target
  target: display_1.region_42
  evidence_weight: 0.55

attention_state:
  target: display_1.region_42
  confidence: 0.46
  expires_after_ms: 1500
```

The system MUST be able to represent uncertainty, expiry, contradiction, and "unknown" rather than forcing a semantic conclusion.

Long dwell time can increase evidence that a region currently occupies visual attention, but it is not by itself evidence of preference, comprehension, agreement, boredom, deception, or emotion.

---

## 5. Attention sensing is broader than gaze

The upper abstraction should be **Attention Sensing**, with gaze as one optional receptor family.

```text
Attention Sensing
├─ gaze / fixation
├─ head orientation
├─ cursor / pointer
├─ active window / UI focus
├─ touch / object interaction
├─ body orientation / proximity
└─ speech-reference cues
```

This matters because many embodiments will not have a calibrated eye tracker.

A phone, desktop, robot, AR device, or remote node should still be able to produce compatible attention evidence from whatever channels exist.

The subsystem should therefore degrade gracefully:

```text
eye tracker available
  -> gaze + head + interaction evidence

camera only
  -> coarse head/face orientation + interaction evidence

screen only
  -> cursor + active UI + interaction evidence

speech only
  -> linguistic/deictic evidence with high uncertainty
```

No single modality is authoritative.

---

## 6. User attention, agent attention, and joint attention

Kamimusuhi should distinguish at least three states:

```text
user_attention_target
agent_attention_target
shared_attention_target
```

This allows gaze to participate in **joint attention** rather than merely becoming another salience score.

Example:

```text
user fixes gaze on cup
        |
        v
user_attention_target = cup

Kamimusuhi is oriented toward user
        |
        v
agent shifts visual attention toward cup
        |
        v
shared_attention_target = cup
```

The shift may be a low-level orientation primitive, while the meaning of the episode is handled at K-Edge / Global Workspace / Persona Core.

This separation supports future robotic embodiment: the same logical attention contract can drive a virtual camera, desktop selection, robot head/eye orientation, or AR focus indicator.

---

## 7. Deictic reference and grounding

One high-value application is resolving expressions such as:

```text
"this"
"that"
"over there"
"what do you think about this one?"
```

Rather than asking the language model to guess from conversation text alone, Kamimusuhi can fuse recent evidence:

```text
speech:      "what do you think about this?"
gaze:        display_1.region_42
cursor:      near display_1.region_42
touch:       none
screen tree: object_927 overlaps region_42
```

Possible result:

```text
referent_candidate = object_927
confidence = 0.88
```

The resolver should still expose ambiguity when multiple candidates remain plausible.

This is a concrete path from low-level gaze sensing to useful social and embodied cognition without pretending that the gaze signal itself contains semantic intent.

---

## 8. Relationship to PNL and reflex control

The Peripheral Neural Layer may own fast operations such as:

- smoothing noisy gaze coordinates;
- fixation/saccade state machines;
- blink detection;
- coarse gaze-zone classification;
- local calibration checks;
- habituation / repeated-event suppression;
- robot head/eye tracking primitives;
- wake or salience hints.

It SHOULD NOT own social interpretation such as:

```text
"the user is interested"
"the user dislikes this"
"the user is lying"
"the user wants me to act"
```

Those require broader context and remain hypotheses in higher cognition.

A useful boundary is:

```text
PNL / receptor edge
    "fixation on target A"

Attention state / K-Edge
    "A is a plausible current attention target"

Persona / social cognition
    "given speech + history + action, perhaps A is what the user means"
```

---

## 9. Privacy and retention

Eye and face streams are high-sensitivity sensory data. The preferred default is **local reduction**.

Where technically possible:

- raw eye/face frames remain on the sensor or edge node;
- higher layers receive compact events and quality metadata;
- raw retention is disabled by default;
- temporary raw capture for calibration/debugging is explicit and bounded;
- provenance records whether raw evidence exists;
- downstream memory stores interpreted episodes only when ordinary memory admission rules justify them.

Attention evidence should not automatically become permanent autobiographical memory.

---

## 10. Failure and uncertainty model

Gaze is especially vulnerable to calibration drift, occlusion, glasses/reflections, camera motion, unusual head pose, lighting changes, and ambiguous target geometry.

Required failure states include:

```text
uncalibrated
low_confidence
face_not_visible
eye_not_visible
out_of_range
target_ambiguous
calibration_stale
sensor_unavailable
```

Higher cognition must be able to distinguish "the user looked away" from "the tracker lost the eyes."

Confidence should decay with time. Stale attention targets MUST NOT remain active indefinitely.

---

## 11. Near-term implementation path

### Phase A — desktop coarse attention

- active window / focused UI element;
- cursor and click/touch evidence;
- optional webcam head orientation;
- common `AttentionEvidence` representation;
- no psychological inference.

### Phase B — non-deep calibrated gaze

- eye ROI and pupil/iris localization;
- screen calibration;
- coarse region-of-interest gaze zones;
- fixation / saccade / blink events;
- quality/confidence tracking;
- raw-frame-local processing.

### Phase C — multimodal referent resolution

- gaze + cursor + screen/UI tree + speech deictics;
- candidate-object resolver;
- ambiguity and contradiction handling;
- K-Edge integration.

### Phase D — joint attention / embodied orientation

- explicit agent attention state;
- virtual/robot gaze target;
- user/agent/shared-attention transitions;
- bounded PNL orientation primitives;
- social timing experiments.

Deep gaze estimation remains optional and should be introduced only if measured accuracy/robustness gains justify its latency, energy, privacy, and hardware costs.

---

## 12. Evaluation

Evaluation should separate sensor accuracy from cognitive usefulness.

**Sensor layer**

- angular/region error after calibration;
- fixation detection precision/recall;
- blink/saccade event quality;
- calibration-drift detection;
- event latency;
- CPU/GPU/energy cost;
- raw-to-event bandwidth reduction.

**Attention fusion**

- current-target accuracy;
- stale-target rate;
- confidence calibration;
- robustness when one modality disappears;
- conflict handling between gaze/cursor/touch/head pose.

**Cognitive usefulness**

- deictic reference resolution accuracy;
- clarification questions avoided without increasing wrong grounding;
- joint-attention establishment latency;
- inappropriate social-inference rate;
- LLM/K-Core invocations avoided by peripheral preprocessing.

A critical negative metric is how often the system turns weak gaze evidence into unjustified claims about user intent or emotion.

---

## 13. Current conclusion

Kamimusuhi should include gaze measurement, but the architectural unit should be **attention sensing**, not "eye tracking connected to the LLM."

Provisionally adopted direction:

- non-deep / geometric gaze is a first-class viable backend;
- high-rate processing stays near the sensor;
- gaze produces attributed observations, not mental-state labels;
- observation, hypothesis, and belief remain separate;
- gaze is fused with head, cursor, UI, touch, and speech evidence;
- user attention and agent attention are separate state variables;
- joint attention is an explicit future capability;
- raw eye/face streams should normally not leave the edge node;
- the system must work when gaze hardware is absent.

The research question is therefore not merely whether Kamimusuhi can estimate where a user is looking. It is:

> **Can low-cost, non-deep attention sensing improve grounding, joint attention, and embodied interaction while preserving uncertainty, privacy, low latency, and model independence?**
