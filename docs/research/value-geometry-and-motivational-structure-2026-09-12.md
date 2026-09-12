# Value Geometry and Motivational Structure

Date: 2026-09-12  
Status: research note / non-normative  
Primary source: Abootorabi et al., *Steering Geometry: Validating Human Value Geometry in LLM Steering Space*, arXiv:2609.06289v1 (2026-09-05)  
Paper: https://arxiv.org/abs/2609.06289  
Code/data: https://github.com/DeepRCL/Steering_Geometry

## Why this matters to Kamimusuhi

A recurring Kamimusuhi question is whether stable personality / motivation should be treated as a collection of output tendencies or as a structured internal state that constrains behavior across situations.

Abootorabi et al. provide a useful experimental distinction:

- **behavioral success**: an intervention makes the model produce the desired answer;
- **geometric fidelity**: value-related internal directions preserve theoretically expected relationships among values;
- **cross-value transfer**: steering one value changes compatible and opposing values in predictable directions.

The paper does **not** establish that an LLM possesses human values in a phenomenological, agentic, or moral sense. Its narrower result is that some methods extract internal value directions whose pairwise geometry tracks a human psychological value theory, and that this geometric fidelity predicts more theory-consistent behavioral spillover.

For Kamimusuhi, this suggests an evaluation axis beyond "does the Persona Core say the right thing?":

> Does the system preserve a coherent motivational structure, and do interventions propagate through that structure predictably?

This note treats that as a research hypothesis, not an architectural decision.

---

## 1. External result: Steering Geometry

### 1.1 Benchmark

The study uses Schwartz's refined Theory of Basic Human Values as its main fine-grained structure. It evaluates 20 value categories arranged around a circumplex where nearby values are expected to be compatible and distant/opposing values to conflict.

The released Schwartz benchmark contains **26,428 contrastive quadruples**:

```text
(question, value, positive answer, negative answer)
```

After quality filtering:

- Touché-derived samples: **25,517 (96.6%)**
- ValueBench-derived samples: **911 (3.4%)**

For the Touché-derived subset, negative answers were generated with Gemma-4-26B-A4B-it and filtered with an LLM-as-a-judge pipeline using the same model. This is an important dataset-construction caveat when interpreting the absolute results.

The paper also performs a coarser generalization check using revised Moral Foundations Theory (MFT), with 1,200 balanced samples across six foundations. MFT does not provide an equivalent continuous pairwise geometry, so that experiment tests only higher-order family separation.

### 1.2 Value-vector geometry

For each value, steering methods induce an effective residual-stream shift. The study mean-centers the 20 value vectors, normalizes them, and builds a cosine-similarity matrix. This empirical matrix is compared with a theoretical similarity matrix derived from angular distance around the Schwartz circumplex.

The main metric is Theory Rank Correlation, Spearman's rho, which asks whether theoretically compatible values tend to be closer in steering-vector space than theoretically conflicting values.

On Qwen3.5-9B-Base, the strongest reported configuration is Sparse Activation Steering (SAS):

| Method | Theory Rank Correlation |
| --- | ---: |
| SAS | **0.5069** |
| CAA | **0.4606** |
| OPT | **0.1138** |

The paper groups SAS and CAA with **distribution-driven** methods: they derive directions from activation distributions / contrastive statistics. OPT is a **behavior-centric** method: it directly optimizes toward desired output behavior.

The important result is not that one method merely scores higher. Behavior-centric methods can achieve steering performance comparable to distribution-driven methods while preserving much less of the theory-specified value geometry.

### 1.3 Geometry predicts cross-value behavior

The authors then steer one value and measure what happens to other values.

Methods with stronger geometric fidelity show more predictable transfer:

- compatible values tend to improve together;
- opposing values tend to be suppressed;
- the direction of spillover follows the theoretical value structure more consistently.

This makes geometry operationally relevant. Two interventions can achieve similar target behavior while differing substantially in how they perturb the rest of the value space.

### 1.4 Instruction tuning and geometry drift

The study reports weaker value geometry after instruction tuning. For SAS on the Qwen3.5-9B pair:

```text
Base:     rho_T = 0.5069
Instruct: rho_T = 0.3256
```

The correct interpretation is limited:

- this does **not** prove that instruction tuning destroys values;
- it shows that the Schwartz-aligned structure is less recoverable by the paper's steering/vector extraction procedure after instruction tuning;
- representation may have been compressed, redistributed, made more nonlinear, or genuinely distorted.

Still, it is a warning against evaluating post-training only through surface behavior.

---

## 2. What this does and does not establish

### Established external result

Within the evaluated models, datasets, layers, and steering methods:

1. some value directions exhibit non-trivial correspondence with Schwartz's circumplex;
2. distribution-driven extraction preserves this structure better than the evaluated behavior-centric approaches;
3. higher geometric fidelity is associated with more theory-consistent cross-value transfer;
4. instruction-tuned variants show weaker measured geometry than corresponding base variants;
5. the paradigm-level separation also appears under the coarser MFT family test.

### Not established

The paper does **not** show that:

- LLMs possess intrinsic moral commitments;
- the extracted directions are perfectly monosemantic;
- Schwartz geometry is the correct ontology for artificial agents;
- the geometry is culture- or language-invariant inside LLMs;
- preserving human-value geometry is sufficient for safe alignment;
- instruction tuning necessarily removes or damages a latent value system.

The authors explicitly note open generalization across cultures and languages, the difficulty of real text expressing multiple values simultaneously, and incomplete cross-backbone / cross-method coverage.

---

## 3. Kamimusuhi interpretation

### 3.1 Separate expression from motivational structure

Kamimusuhi already distinguishes a persistent individual from replaceable cognitive resources and user-facing expression. This paper adds evidence for another useful distinction:

```text
knowledge / memory
        |
        +--> what the individual knows and remembers

language / persona expression
        |
        +--> how internal state is verbalized

motivation / value geometry
        |
        +--> structured relations among preferences, drives, conflicts,
             compatible tendencies, and trade-offs
```

A Persona Core that merely reproduces a desired tone can pass behavioral probes while having unstable or shortcut-driven internal organization.

Therefore, character / instruction tuning should not automatically be treated as evidence that a persistent value structure has been learned.

### 3.2 Do not assume Schwartz values are the target ontology

Schwartz is useful here primarily because it provides a known relational structure against which geometry can be tested.

Kamimusuhi does not need to copy the 20 human value categories into K0 or a Persona Core. An artificial organism may instead develop or be evaluated against drives such as:

- exploration vs. stability;
- self-preservation vs. risk / novelty seeking;
- resource conservation vs. information acquisition;
- autonomy vs. social attachment / coordination;
- short-horizon reflex reward vs. long-horizon continuity;
- immediate task success vs. identity-preserving action.

The interesting object is not any particular list of labels. It is the **relational structure** among those drives and whether the structure remains stable under learning, memory growth, model replacement, and instruction/post-training.

This is a Kamimusuhi design hypothesis, not a result of the cited paper.

---

## 4. Proposed evaluation family: Motivational Geometry

### Goal

Measure whether an individual has a coherent and interventionally meaningful motivational manifold rather than independent behavior knobs.

### 4.1 Geometry recovery test

For each candidate drive/value `v_i`:

1. construct positive/negative or preference-contrastive situations;
2. record internal state at a defined boundary;
3. derive a direction or low-dimensional subspace for `v_i`;
4. mean-center across the drive bank;
5. normalize representations;
6. compute pairwise cosine similarity or another preregistered metric;
7. compare against an expected relation matrix, if one is available.

For J-family models, this can initially operate on residual-stream activations.

For K0-like recurrent cores, the equivalent object may be:

- recurrent hidden-state displacement;
- policy-logit displacement;
- learned latent state;
- perturbation response rather than a single linear vector.

Do not force a linear-vector interpretation if the core does not support it.

### 4.2 Cross-drive transfer test

Geometry alone is insufficient. Intervene on drive `v_i` and measure all other drives / behaviors.

A successful structured system should show:

```text
predicted-compatible drive -> same-sign or otherwise predicted transfer
predicted-conflicting drive -> opposite-sign or otherwise predicted transfer
unrelated drive            -> limited / calibrated effect
```

The benchmark should compare:

- target success;
- off-target magnitude;
- predicted-vs-observed transfer correlation;
- transfer sign accuracy;
- geometry/transfer consistency.

This prevents a system from passing by optimizing one behavior while unpredictably damaging other tendencies.

### 4.3 Post-training drift test

Run the same geometry extraction before and after:

- instruction tuning;
- character/persona tuning;
- preference tuning;
- memory integration;
- model migration;
- long-horizon continual learning.

Measure both:

```text
behavior delta
geometry delta
```

A useful diagnostic condition is:

> Behavior improved, but motivational geometry collapsed or rotated unpredictably.

That should be treated as a distinct failure mode rather than hidden behind aggregate task accuracy.

---

## 5. Near-term experiment candidates

### Experiment A — J72 / J-family value-geometry probe

Question:

> Does a small Japanese-specialized language model contain recoverable relational structure among culturally or narratively grounded value concepts, even without instruction tuning?

Minimal experiment:

1. define a small preregistered set of 6-10 values/drives with expected relationships;
2. build balanced contrastive Japanese prompts;
3. collect activations from several layers;
4. derive simple mean-difference / CAA-like directions first;
5. compute the similarity matrix;
6. compare geometry across layers;
7. repeat after any instruction/persona tuning.

Important: a positive result would show recoverable structured representation under the chosen probes; it would not show that J72 "has values" in the human sense.

### Experiment B — K0 motivational-transfer benchmark

Question:

> Can a non-language recurrent core learn interacting drives whose intervention effects generalize beyond the directly optimized action?

Candidate setup:

- synthetic environment with 3-6 drives;
- explicit compatible, conflicting, and orthogonal objectives;
- train behavior without directly supervising the pairwise geometry;
- perturb one drive/state channel after training;
- measure transfer to all others;
- compare MLP vs. recurrent core and imitation-only vs. RL/post-training variants.

This is especially useful because K0 can test the concept without relying on language semantics or human-value labels.

### Experiment C — Persona tuning preservation test

Question:

> Does making the language organ more character-consistent preserve, sharpen, rotate, or destroy pre-existing motivational structure?

Compare:

```text
base language core
        -> character / instruction tuned core
        -> same geometry + cross-transfer battery
```

This directly tests whether "more convincing persona" and "more coherent underlying motivation" move together or diverge.

---

## 6. Suggested acceptance metrics

Do not define a universal pass threshold yet. First collect baselines.

Recommended outputs for every run:

```text
geometry_similarity_matrix
expected_relation_matrix
rank_correlation
linear_correlation (optional)
bootstrap confidence interval
cross_drive_transfer_matrix
target_behavior_gain
off_target_effect
transfer_prediction_correlation
layer / state-boundary identifier
training checkpoint / lineage identifier
```

For Kamimusuhi lineage and continuity work, preserve these artifacts across model versions. A later system can then ask not only "is this individual behaviorally similar?" but also:

> Is the structure of what reinforces, conflicts with, and constrains its behavior still recognizably the same?

---

## 7. Risks and confounds

1. **Probe ontology risk** — the researcher can impose the expected structure through dataset construction.
2. **Language leakage** — lexical similarity between value prompts can produce apparent geometry.
3. **Linear-probe bias** — a real representation may exist but not as a single direction.
4. **Synthetic clean-room bias** — K0 toy environments may trivially encode the reward graph.
5. **Behavior/representation circularity** — defining both the vector and success metric from the same examples can inflate conclusions.
6. **Cultural overreach** — human value theories should not be assumed universal for artificial organisms or for all languages/cultures.
7. **Post-training ambiguity** — weaker extractability is not equivalent to destruction of the underlying concept.

Mitigations should include disjoint probe/evaluation sets, lexical perturbation, shuffled-geometry controls, bootstrap tests, multiple extraction methods, and intervention-based validation.

---

## 8. Architectural consequence: not yet normative

No architecture change is justified from this paper alone.

The current actionable conclusion is an **evaluation recommendation**:

> Add motivational/value geometry preservation and cross-drive transfer to the future Kamimusuhi evaluation toolbox, especially around Persona Core / J-family post-training and K0-like recurrent-core experiments.

Promotion to a normative architecture requirement should happen only after Kamimusuhi-specific experiments demonstrate that the metric is stable, non-trivial, and predictive of desirable long-horizon behavior.

---

## References

- Mohammad Mahdi Abootorabi et al. (2026). *Steering Geometry: Validating Human Value Geometry in LLM Steering Space*. arXiv:2609.06289v1. https://arxiv.org/abs/2609.06289
- Schwartz, S. H. (1992). Universals in the content and structure of values.
- Schwartz, S. H. et al. (2012). Refining the theory of basic individual values.
- Project code/data: https://github.com/DeepRCL/Steering_Geometry
