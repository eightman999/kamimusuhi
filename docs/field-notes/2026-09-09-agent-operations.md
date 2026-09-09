# Field Note — Agent Operations Meetup Distillation (2026-09-09)

> Status: observational / non-canonical. This note records operational evidence from a meetup conversation and does not by itself authorize architecture changes.

## Distilled observation

Multiple independently built demos converged on the same shape: **input / sensing → intent or routing → agent → remote resource or external action → artifact / feedback**. The practical bottlenecks discussed were less about raw model intelligence and more about connection, authority, supervision, and resource consumption.

## Operational evidence worth preserving

- A working remote experiment loop already exists in practice: plan on a mobile ChatGPT session, send the instruction to a home compute node, let the remote agent execute, retrieve a ZIP artifact, then return that artifact to the planner for the next iteration.
- The slowest node in that loop is the human bridge that manually transports instructions and artifacts between planner and executor.
- Other attendees independently demonstrated or discussed remote-machine allocation, webhook-triggered agents, wearable / voice capture, and agent-driven external actions.
- Persistent agents consume tokens / credits continuously enough that budget exhaustion becomes a first-class operational constraint.
- Giving an agent access to a real machine, network, credentials, or human account creates an authority problem comparable to giving software control of physical hardware.
- A useful supervision pattern emerged: an agent can monitor another agent rather than assuming one omnipotent autonomous worker is safe.

## Kamimusuhi implications

### 1. Close the planner–executor loop

Treat manual ZIP / copy-paste transport as a temporary prosthesis. K-Nerve / runtime infrastructure should eventually support a typed artifact loop:

```text
Planner
  -> mission / experiment spec
Executor
  -> run + logs + artifacts + machine-readable summary
Reviewer
  -> acceptance / next hypothesis
Planner
```

The goal is not autonomy for its own sake. The goal is to remove the human from mechanical transport while preserving human authority over irreversible decisions.

### 2. Authority must be capability-scoped

Do not model execution authority as a single `agent_can_act` bit. Distinguish capabilities such as:

- read experiment workspace
- write experiment workspace
- run local compute
- use GPU / accelerator
- access private network resources
- access the public network
- publish externally
- modify accounts or credentials
- control physical devices

Capabilities should be revocable and should not be inferred from remembered text. Existing runtime-authority boundaries remain canonical; this field note is practical support for them.

### 3. Cognitive Budget is also metabolism

Token, API credit, GPU time, wall-clock time, power, and network use are all scarce resources of a long-lived individual. Route work by required cognition rather than keeping the most expensive model continuously awake.

A useful biological analogy is:

- reflex / cheap local path: high-frequency, low-cost
- normal cognition: moderate-cost
- deep cognition: rare, expensive
- long experiments: delegated to compute nodes

This strengthens the existing heterogeneous cognitive substrate and Cognitive Budget direction.

### 4. Separate brain, nerves, senses, actuators, metabolism, and immunity

The meetup evidence reinforces treating Kamimusuhi as a whole organism rather than only a personality model:

- brain: reasoning / persona / memory
- nerves: event and artifact transport
- senses: voice, files, feeds, wearables, environment state
- actuators: remote computers, APIs, robots
- metabolism: token / compute / energy budgets
- immunity: capability policy, sandboxing, supervision, revocation

### 5. Learn from executable specimens

Agent platforms may encode meaningful behavior in routines, memories, skills, templates, and internal actions that are not obvious from marketing text. When studying agent systems, preserve executable configuration and official templates as specimens instead of relying only on documentation summaries.

## Non-goals from this note

- Do not grant broad machine or account access merely to reduce friction.
- Do not make background activity mandatory.
- Do not make frontier-model use the default for every event.
- Do not promote meetup anecdotes to canonical architecture without implementation tests or stronger evidence.
