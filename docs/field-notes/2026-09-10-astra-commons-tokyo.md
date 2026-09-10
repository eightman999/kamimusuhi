# Field Note — Astra Commons Tokyo Distillation (2026-09-10)

> Status: observational / non-canonical. This note records design-relevant evidence and conversations from Astra Commons Tokyo. It does not by itself authorize architecture changes.

## Distilled observation

The strongest recurring pattern was not simply “use a larger model.” The useful systems were built around **a persistent agent loop that can acquire missing information, choose tools, preserve state, and escalate only when deeper cognition is required**.

A useful abstraction is:

```text
persistent lightweight core
  -> sense / observe
  -> decide whether more information or cognition is needed
  -> acquire information or invoke an external intelligence
  -> act through a persistent environment
  -> observe the result
  -> update state
```

The practical capability boundary is therefore not only what the resident model knows. It is what the whole system can reach, inspect, invoke, and remember.

## 1. The Core does not need to solve everything

A persistent Core can remain small if it is good at:

- sensing and maintaining local state
- detecting uncertainty or missing information
- deciding whether an external model should be invoked
- selecting an appropriate tool or compute path
- incorporating returned information into the next action

This supports the existing direction of separating reflex / low-cost cognition from expensive external reasoning. The Core should be evaluated partly on **invocation quality**, not only on end-task accuracy.

### Research implication

Future experiments should explicitly measure:

- false-positive external invocations
- missed necessary invocations
- whether returned information changes the policy correctly
- whether the Core can continue after delayed external information
- whether memory/state is sufficient to avoid repeated unnecessary calls

## 2. Active information acquisition is a first-class capability

Astra-oriented discussion repeatedly emphasized performance in situations where the initial context is incomplete but information-gathering tools are available.

For Kamimusuhi this suggests a distinction between:

```text
I know the answer
I do not know the answer
I know how to obtain what is missing
```

The third state is especially important for a persistent agent.

A useful failure taxonomy is therefore:

- knowledge failure
- retrieval / sensing failure
- tool-selection failure
- invocation failure
- returned-information integration failure
- state-retention failure

## 3. Persistent environment state is part of memory

Several practical agent workflows discussed at the event depended on a persistent computer rather than a stateless chat session.

Persistent state can include:

- files and directories
- installed software
- browser state
- authenticated sessions
- local databases
- network reachability
- running services and processes
- machine-specific caches and indexes

This suggests that Kamimusuhi should not reduce long-term memory to a single explicit memory database.

A more useful model is:

```text
Memory = explicit memory + learned state + environment state
```

The environment behaves partly like an informational body: it preserves affordances and history even when those facts are not re-injected into a prompt.

## 4. Persistent computer as informational embodiment

A resident agent with a stable machine gains continuity from more than text memory. It can leave artifacts, install tools, preserve working sets, revisit browser state, and interact with external resources through the same environment later.

This is analogous to embodiment in the limited sense that the agent has a durable action substrate with constraints and affordances.

Do not equate this with biological embodiment. The useful engineering claim is narrower:

> a persistent execution environment can be a durable part of an agent's identity-supporting state.

## 5. Hierarchical intelligence remains preferable to one always-on frontier model

A practical hierarchy suggested by the event conversations is:

```text
reflex / learned discrete action
        ↓
resident lightweight Core
        ↓
local specialist or medium model
        ↓
frontier model / expensive external reasoning
        ↓
delegated long-running compute or research worker
```

Selection should be based on required cognition, latency tolerance, privacy, hardware availability, and cognitive budget.

This reinforces the existing heterogeneous cognitive substrate direction.

## 6. Harness and environment are distinct from model identity

Observed agent workflows reinforce keeping these layers separate:

- model weights
- persona / identity state
- memory
- skills / procedures
- tool authority
- execution environment
- external intelligence providers

A model replacement should not implicitly replace the whole individual.

Likewise, a prompt or harness change may alter behavior substantially even when the model weights remain unchanged, so runtime configuration must be treated as identity-relevant state when continuity matters.

## 7. Self-maintenance should target the substrate, not only source code

Long-lived self-maintenance may eventually include:

- repairing broken tools
- rebuilding indexes
- checking storage health
- pruning stale caches
- validating external endpoints
- rotating between compute backends
- detecting capability loss
- requesting human intervention when authority is insufficient

This is more useful than framing self-improvement only as self-editing model code.

## 8. Cross-project relationship

The event clarified a useful division of responsibility across related projects:

```text
Kamimusuhi
  = persistent cognitive architecture

YLSB
  = local intelligence substrate evaluation / comparability

OISINT
  = real-world application pressure and operational constraints
```

YLSB can inform which local intelligence substrates are practical. OISINT can expose real-world bottlenecks that architecture-only experiments may miss. Neither should become a hidden dependency of Kamimusuhi.

## Candidate follow-up experiments

1. Invocation-decision benchmark for the resident Core.
2. Active information acquisition with delayed returned information.
3. Persistent-environment ablation: fresh environment vs preserved environment state.
4. Multi-tier routing across reflex / local small model / local large model / frontier API.
5. Recovery experiment where a tool or endpoint becomes unavailable mid-task.

## Non-goals from this note

- Do not make a frontier model mandatory for normal operation.
- Do not treat environment state as a substitute for explicit memory.
- Do not grant broad machine authority merely because persistent execution is useful.
- Do not promote meetup anecdotes into canonical architecture without controlled experiments.
- Do not assume one vendor's current agent implementation defines the target architecture.
