# Sensory Nervous System

Status: **living engineering design note**

Kamimusuhi should not treat camera, microphone, touch, environmental sensors, and machine telemetry as unrelated plugins. They are parts of a distributed **artificial sensory nervous system** that converts heterogeneous raw signals into attributed events, reflexes, salience signals, and ultimately unified experience.

The objective is functional engineering, not biological imitation.

## 1. Sensory architecture

```text
                      RECEPTORS
       ┌──────────┬──────────┬──────────┬──────────┐
       │          │          │          │          │
     Vision     Hearing     Tactile   Environment  Body/Node
       │          │          │          │          │
       └──────────┴──────────┴──────────┴──────────┘
                              │
                    Peripheral Processing
                  filter / detect / compress
                              │
                       Sensory Event Bus
                              │
                 ┌────────────┴────────────┐
                 │                         │
             Reflex Arc                Salience
                 │                         │
            immediate action          Global Workspace
                                           │
                                      Persona Core
```

Raw high-rate streams remain near sensors. The central cognition layer consumes compact, timestamped, provenance-carrying **sensory events** rather than raw samples whenever possible.

## 2. Functional sense families

### 2.1 Vision

Physical vision:

- RGB cameras;
- RGB-D/depth cameras;
- stereo vision;
- event cameras;
- robot/body cameras;
- remote authenticated camera feeds.

Digital vision:

- desktop framebuffer / screen capture;
- accessibility/UI trees;
- browser rendering state;
- images/video/documents;
- visual state from remote devices.

The important distinction is not camera vs screen. Both are visual surfaces belonging to the distributed embodiment.

Peripheral visual processing may emit:

```text
person_entered
object_moved
text_changed
hazard_detected
gesture_detected
scene_changed
motion_direction
identity_candidate
```

#### 2.1.1 Gaze and attention sensing

Gaze SHOULD be treated as one source of **attention evidence**, not as direct access to a user's intent, preference, emotion, or mental state.

The preferred near-term hot path is compatible with non-deep processing:

```text
eye / face region
  -> pupil or iris localization
  -> optional head pose
  -> calibration
  -> gaze vector / coarse target zone
  -> fixation / saccade / blink state
  -> SensoryEvent
```

Raw eye/face frames should normally remain at the receptor or edge node. Higher layers should receive compact observations with target, timing, quality, confidence, and provenance.

A fixation on object A may support the hypothesis that A is a current attention target, but it MUST NOT silently become claims such as "the user likes A", "the user is bored", or "the user wants action on A".

The broader attention layer should fuse gaze with other available evidence:

```text
gaze
head orientation
cursor / pointer
active window / UI focus
touch / object interaction
body orientation
speech-reference cues
```

This allows the same interface to work on devices without eye tracking and supports future user/agent/shared attention and deictic-reference grounding.

Detailed design: [`attention-sensing-and-joint-attention.md`](./attention-sensing-and-joint-attention.md).

### 2.2 Hearing

The auditory path should preserve more than transcript text.

Raw/near-sensor processing:

- voice activity detection;
- streaming ASR;
- sound-event detection;
- speaker identification/diarization;
- direction-of-arrival / source localization;
- acoustic-scene classification;
- paralinguistic cues such as volume, rate, pauses, laughter, sighing.

Possible events:

```text
speech_started
name_called
person_laughing
alarm_detected
object_impact
rain_started
fan_noise_anomaly
```

A transcript is evidence about what was said, not the entire auditory experience.

### 2.3 Tactile / artificial skin

Tactile sensing can include:

- contact;
- normal pressure;
- shear force;
- slip;
- vibration;
- texture;
- strain;
- proximity;
- local temperature.

A future robotic body may distribute such sensors across hands, arms, and body surfaces.

Reflex-worthy signals such as slip should be processed locally rather than waiting for high-level deliberation.

### 2.4 Environmental sensing

Environmental sensing acts as a broader functional "skin" around an embodiment surface:

- air temperature;
- humidity;
- barometric pressure;
- airflow;
- illuminance;
- ambient noise;
- CO2;
- VOC/gases;
- smoke/air-quality indicators.

Temperature and humidity are therefore useful, but they are environmental interfacial senses rather than a complete substitute for tactile sensing.

### 2.5 Olfaction

Electronic noses / gas-sensor arrays may eventually supply:

- gas/VOC classification;
- smoke/burn indicators;
- concentration estimation;
- odor-source localization.

Open-world human-like odor generalization remains difficult because of drift, mixtures, calibration, humidity/temperature effects, and sensor diversity. Initial Kamimusuhi olfaction should be functional and task-oriented.

### 2.6 Gustation

Electronic-tongue systems exist for liquid chemical discrimination and quality analysis, but this is lower priority unless Kamimusuhi gains food/chemical manipulation roles.

### 2.7 Proprioception / body schema

For a distributed digital organism, proprioception includes both physical pose and **embodiment topology**.

Examples:

- IMU;
- joint encoders / torque;
- odometry / GPS;
- robot pose;
- device location;
- sensor availability;
- active microphones/cameras;
- node connectivity;
- latency between organs;
- K-Edge/K-Core/K-Deep availability.

Conceptual body schema:

```yaml
embodiments:
  phone:
    status: online
    capabilities: [audio, camera, imu]
    role: K-Edge
  home_core:
    status: online
    capabilities: [deep_compute, full_memory]
    role: K-Core
  robot_1:
    status: offline
    capabilities: [vision, tactile, locomotion]
```

The answer to "where am I?" may therefore be a topology, not one coordinate.

### 2.8 Interoception

Interoception is handled more fully in Issue #14, but sensory integration should expose internal signals such as:

- compute pressure;
- power/battery;
- memory integrity;
- network health;
- resource availability;
- consolidation debt / sleep pressure.

This gives three explicit domains:

```text
exteroception  -> external world
proprioception -> distributed body/topology
interoception  -> internal operational state
```

## 3. Event-driven peripheral nervous system

Sensor rates differ by many orders of magnitude. A single central polling loop is therefore undesirable.

Typical classes:

```text
audio raw           tens of kHz
tactile/force       hundreds of Hz to kHz
camera              tens/hundreds of frames/s
event camera         asynchronous events
IMU                 hundreds of Hz
temperature/humidity fractions of Hz to several Hz
system telemetry     low Hz to event driven
```

The preferred architecture is asynchronous and event-driven.

High-rate raw data should be filtered/compressed near the sensor and promoted only when semantically or behaviorally relevant.

## 4. Common SensoryEvent envelope

A unified event format allows heterogeneous modalities to participate in the same salience/workspace system without pretending their raw streams are identical.

```json
{
  "modality": "tactile",
  "source": "robot_1.left_hand.skin.12",
  "timestamp": "...",
  "event": "slip",
  "value": 0.61,
  "confidence": 0.99,
  "salience_hint": 0.8,
  "location": {
    "embodiment": "robot_1",
    "region": "left_palm"
  },
  "provenance": {
    "processor": "tactile_slip_v1",
    "raw_ref": "..."
  }
}
```

Requirements:

- globally comparable timestamp/time-domain metadata;
- source and embodiment identity;
- modality and event type;
- confidence/quality;
- provenance to raw or derived source;
- spatial/body location where relevant;
- optional salience hint, never an unchallengeable priority.

## 5. Reflex arc

Time-critical behavior must bypass deep Persona Core cognition while remaining observable and policy-bounded.

Example:

```text
slip receptor
  -> local reflex controller
  -> increase grip
  -> emit "grip_reflex" event to higher cognition
```

Kamimusuhi may become consciously aware of an action after the local reflex has already occurred. This is intentional.

Reflex is not a policy bypass for consequential arbitrary actions. Only narrowly defined low-latency capabilities should be exposed to reflex controllers.

## 6. Multisensory binding

The difficult problem is not merely implementing five sensor types. It is binding asynchronous evidence into one world event.

Example inputs:

```text
vision:  A's mouth moved
hearing: speech from right-front
skin:    right shoulder contact
```

Desired integrated event:

```text
A spoke from the right while touching my right shoulder
```

Required mechanisms include:

- temporal alignment;
- spatial calibration;
- cross-device clock correction;
- entity tracking;
- causal/association inference;
- uncertainty fusion;
- cross-modal conflict detection.

No single modality should silently overwrite another. Contradictory sensory evidence should remain representable.

Attention evidence follows the same rule. Gaze, head pose, cursor, touch, and speech-reference cues may disagree; fusion should preserve that disagreement instead of selecting a psychologically loaded interpretation by default.

## 7. Transport and runtime

For physical robotics, ROS 2 / DDS / Zenoh-like middleware are practical transport candidates, but transport must remain below the Kamimusuhi sensory abstraction.

```text
sensor/robot middleware
  -> transport adapter
  -> SensoryEvent
  -> Kamimusuhi nervous system
```

This prevents ROS-specific types from becoming the organism's canonical cognitive interface and allows phones, desktops, browsers, microcontrollers, and robots to coexist.

## 8. Neuromorphic path

Long-term research may replace software peripheral processors with event-driven/neuromorphic implementations:

- event cameras;
- neuromorphic tactile sensors;
- SNN-based local classifiers;
- FPGA / logic-network reflexes;
- memristive/artificial synaptic devices.

The interface should remain stable so backend implementation can change without altering higher cognition.

A 2025 review explicitly surveys artificial nervous systems combining multimodal perception, neural signal processing, synaptic/neuromorphic devices, and reflex-driven effectors:

- Yang et al., *Artificial Nervous Systems*, Advanced Science (2025): https://doi.org/10.1002/advs.202511478

The review covers tactile, visual, olfactory, gustatory, auditory, and multisensory artificial nervous systems and notes that autonomous adaptability remains an open challenge.

## 9. Implementation roadmap

### Phase A — cheap distributed senses

- microphone + streaming ASR/VAD;
- camera/screen perception;
- coarse attention evidence from active-window/cursor/head orientation;
- temperature/humidity;
- IMU/device pose;
- machine telemetry;
- common event schema.

### Phase B — auditory/visual localization and fusion

- sound event + source localization;
- entity tracking;
- optional non-deep calibrated gaze and fixation events;
- gaze/head/cursor/UI evidence fusion;
- multimodal temporal alignment;
- basic cross-modal binding.

### Phase C — physical touch/body

- force/tactile sensors;
- slip/contact reflex;
- robot proprioception;
- body schema integration;
- user/agent/shared-attention experiments for embodied interaction.

### Phase D — event/neuromorphic sensing

- event cameras;
- neuromorphic tactile processing;
- edge/FPGA reflex processors.

### Phase E — chemical senses

- functional e-nose;
- e-tongue only where an application justifies it.

## 10. Evaluation

Metrics should include:

- raw-to-event latency;
- reflex latency;
- event precision/recall;
- salience promotion precision;
- bandwidth reduction vs raw streams;
- cross-modal binding accuracy;
- clock/spatial calibration error;
- attention-target confidence calibration and stale-target rate where attention sensing is enabled;
- sensory provenance completeness;
- failure behavior under missing/conflicting modalities;
- K-Edge -> K-Core sensory handoff continuity.

The central success criterion is not "has five sensors." It is whether one persistent individual can integrate heterogeneous sensation into a coherent, attributable, temporally continuous experience.
