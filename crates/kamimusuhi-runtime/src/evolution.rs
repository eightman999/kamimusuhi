//! Minimal generational experiment on top of the selection harness.
//!
//! parent → offspring → evaluation → survivor → next generation, with
//! asexual reproduction (clone + mutate) and no crossover. There is still no
//! scalar fitness: an environment states a *survival rule* — completed the
//! scenario, met a minimum success rate, stayed within a resource budget —
//! and every individual that satisfies it may reproduce. Success, context
//! cost, call cost, exploration behaviour and completion are stored
//! separately for each individual in `lineage.jsonl`.
//!
//! Two panels of scenario seeds are kept apart. The *selection panel* is a
//! small seed set shared by every individual of one generation (common
//! random numbers) and changed from generation to generation; survival is
//! decided on it. The *validation panel* is a larger fixed seed set, identical
//! for every generation and never used for selection; it is what makes
//! generation 0 and the final generation comparable. Founders depend on the
//! base seed alone, so every environment starts from the same population.
//!
//! Everything is seeded: founders, mutation seeds, carry-over shuffles and
//! both panels derive from the configuration, so a run is reproducible
//! except for wall-clock time.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use kamimusuhi_core::genome::{GENE_KEYS, Genome, Phenotype, round_to_resolution};
use serde::{Deserialize, Serialize};

use crate::RuntimeError;
use crate::selection::{Aggregate, Candidate, HarnessConfig, MeanMetrics, Scenario, aggregate};

/// Lineage log inside the output directory: one line per individual per
/// generation, evaluation included.
pub const LINEAGE_FILE_NAME: &str = "lineage.jsonl";
/// Per-generation selection summaries.
pub const GENERATIONS_FILE_NAME: &str = "generations.jsonl";
/// Per-generation validation (fixed panel) summaries.
pub const ADAPTATION_FILE_NAME: &str = "adaptation.jsonl";

/// Resource limits an individual must respect to be eligible to reproduce.
/// Means are over the individual's evaluation runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Budget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_mean_context_records: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_mean_persona_calls: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_mean_tool_calls: Option<f64>,
}

/// A scenario plus the survival rule applied to its results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Environment {
    pub name: String,
    pub scenario: Scenario,
    /// Required when the scenario has a probe; ignored otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_success_rate: Option<f64>,
    #[serde(default)]
    pub budget: Budget,
    /// Genes pinned at each individual's founder value: neither mutation
    /// nor inheritance may change them, so the environment cannot select
    /// on them within a lineage. Used to isolate which trait a scenario
    /// actually selects for. Keys must be members of [`GENE_KEYS`].
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub fixed_genes: BTreeSet<String>,
}

impl Environment {
    /// Exploration pays: the fact can only be recalled by exploring. A modest
    /// success floor and no context budget.
    pub fn exploration_favoured() -> Self {
        Self {
            name: "exploration-favoured".to_owned(),
            scenario: Scenario::exploration(),
            min_success_rate: Some(0.25),
            budget: Budget::default(),
            fixed_genes: BTreeSet::new(),
        }
    }

    /// [`Self::exploration_favoured`] with `cognition.working_memory_span`
    /// pinned at each founder's value. The scenario is identical, so any
    /// remaining divergence of `exploration_probability` from the baseline
    /// environment is selection on the exploration genes alone — the
    /// history-window confound cannot change within a lineage.
    pub fn exploration_wm_fixed() -> Self {
        let mut env = Self::exploration_favoured();
        env.name = "exploration-wm-fixed".to_owned();
        env.fixed_genes = ["cognition.working_memory_span"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        env
    }

    /// [`Self::exploration_favoured`] with `temperament.curiosity` and
    /// `drives.novelty_seeking` — the genes behind `exploration_probability` —
    /// pinned at each founder's value. `working_memory_span` and every other
    /// gene still evolve. `curiosity` also feeds `recall_breadth_scale`, so
    /// freezing it freezes that phenotype too; the scenario's baseline hit
    /// stays unreachable either way.
    pub fn exploration_explore_fixed() -> Self {
        let mut env = Self::exploration_favoured();
        env.name = "exploration-explore-fixed".to_owned();
        env.fixed_genes = ["temperament.curiosity", "drives.novelty_seeking"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        env
    }

    /// Exploration costs: the fact is found by the baseline anyway, exploring
    /// only widens the context and, at a narrow recall top-k, can push the
    /// baseline hit out. A tight context budget and a high success floor.
    pub fn distractor_heavy() -> Self {
        Self {
            name: "distractor-heavy".to_owned(),
            scenario: Scenario::distractor(),
            min_success_rate: Some(0.75),
            budget: Budget {
                max_mean_context_records: Some(5.0),
                max_mean_persona_calls: None,
                max_mean_tool_calls: None,
            },
            fixed_genes: BTreeSet::new(),
        }
    }

    /// Nothing to find and nothing to pay for: completion is the only rule,
    /// so the population drifts.
    pub fn baseline() -> Self {
        Self {
            name: "baseline".to_owned(),
            scenario: Scenario::baseline(),
            min_success_rate: None,
            budget: Budget::default(),
            fixed_genes: BTreeSet::new(),
        }
    }

    pub fn named(name: &str) -> Option<Self> {
        match name {
            "exploration" | "exploration-favoured" => Some(Self::exploration_favoured()),
            "exploration-wm-fixed" => Some(Self::exploration_wm_fixed()),
            "exploration-explore-fixed" => Some(Self::exploration_explore_fixed()),
            "distractor" | "distractor-heavy" => Some(Self::distractor_heavy()),
            "baseline" => Some(Self::baseline()),
            _ => None,
        }
    }

    /// The survival rule. Returns the reasons an individual fails it; empty
    /// means eligible to reproduce.
    pub fn failures(&self, evaluation: &Aggregate) -> Vec<String> {
        let mut reasons = Vec::new();
        if evaluation.completed_runs < evaluation.runs {
            reasons.push(format!(
                "completed {}/{} runs",
                evaluation.completed_runs, evaluation.runs
            ));
        }
        if let (Some(floor), Some(rate)) = (self.min_success_rate, evaluation.success_rate)
            && rate < floor
        {
            reasons.push(format!("success_rate {rate:.2} < {floor:.2}"));
        }
        let over = |name: &str, value: f64, limit: Option<f64>| -> Option<String> {
            limit
                .filter(|limit| value > *limit)
                .map(|limit| format!("{name} {value:.2} > {limit:.2}"))
        };
        reasons.extend(over(
            "mean context_records",
            evaluation.mean.context_records,
            self.budget.max_mean_context_records,
        ));
        reasons.extend(over(
            "mean persona_calls",
            evaluation.mean.persona_calls,
            self.budget.max_mean_persona_calls,
        ));
        reasons.extend(over(
            "mean tool_calls",
            evaluation.mean.tool_calls,
            self.budget.max_mean_tool_calls,
        ));
        reasons
    }
}

/// What the experiment runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvolutionConfig {
    /// Base seed for founders, mutation and carry-over shuffles, and the
    /// per-generation selection seeds. Founders depend on this and
    /// `population_size` only, so the same seed gives every environment the
    /// same founding population.
    pub seed: u64,
    pub population_size: usize,
    /// Generations to evaluate, founders included: `generations = 5` means
    /// generation 0 (founders) through 4 are evaluated, and generation 5 is
    /// bred but not evaluated.
    pub generations: u32,
    /// Selection panel: scenario seeds per individual per generation. Every
    /// individual of one generation sees the same seeds (common random
    /// numbers); the set changes from generation to generation.
    pub repeats: u32,
    pub mutation_rate: f64,
    /// Validation panel: the same `validation_panel` seeds starting at
    /// `validation_seed`, for every generation. Never used for selection;
    /// it is what the experiment's design and thresholds are tuned against.
    pub validation_seed: u64,
    pub validation_panel: u32,
    /// Final-test panel: evaluated once, at the end of the run, on the
    /// founders and on the last evaluated generation. Never used for
    /// selection and never consulted while tuning; it only confirms.
    pub final_test_seed: u64,
    pub final_test_panel: u32,
    pub environment: Environment,
}

impl Default for EvolutionConfig {
    fn default() -> Self {
        Self {
            seed: 1,
            population_size: 8,
            generations: 5,
            repeats: 4,
            mutation_rate: 0.5,
            validation_seed: 900_000,
            validation_panel: 16,
            final_test_seed: 950_000,
            final_test_panel: 16,
            environment: Environment::exploration_favoured(),
        }
    }
}

/// How an individual came to be in its generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    /// Generation 0, a seeded random genome.
    Founder,
    /// Survived the previous generation and was carried over unchanged.
    Survivor,
    /// Clone + mutate of a survivor.
    Offspring,
}

/// Lineage metadata carried by every individual.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lineage {
    pub generation: u32,
    pub origin: Origin,
    /// The genome this one was mutated from. `None` for founders; for a
    /// carried-over survivor it is its own original parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// The generation-0 ancestor. A founder is its own root.
    pub founder_id: String,
    /// Generation the genome itself was born in (a survivor keeps its own).
    pub born_in_generation: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation_seed: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Individual {
    pub genome: Genome,
    pub lineage: Lineage,
}

/// The population at one generation.
#[derive(Debug, Clone, PartialEq)]
pub struct Population {
    pub generation: u32,
    pub individuals: Vec<Individual>,
}

/// One panel's verdict on one individual, stored separately for the
/// selection panel and the validation panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelResult {
    /// First seed of the panel.
    pub seed: u64,
    pub runs: usize,
    pub completed_runs: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success_rate: Option<f64>,
    pub successes: usize,
    pub mean: MeanMetrics,
}

impl From<&Aggregate> for PanelResult {
    fn from(a: &Aggregate) -> Self {
        Self {
            seed: 0,
            runs: a.runs,
            completed_runs: a.completed_runs,
            success_rate: a.success_rate,
            successes: a.successes,
            mean: a.mean.clone(),
        }
    }
}

/// One line of `lineage.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineageEntry {
    pub generation: u32,
    pub genome_id: String,
    pub genome_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub founder_id: String,
    pub origin: Origin,
    pub born_in_generation: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation_seed: Option<u64>,
    pub phenotype: Phenotype,
    pub genes: BTreeMap<String, f64>,
    /// The panel selection was decided on.
    pub selection: PanelResult,
    /// The fixed panel, identical for every generation.
    pub validation: PanelResult,
    pub survived: bool,
    /// Why the survival rule failed, when it did.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<String>,
    /// Carried into the next generation unchanged (chosen by seeded shuffle
    /// among survivors).
    pub carried_over: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Distribution {
    pub mean: f64,
    pub variance: f64,
    pub min: f64,
    pub max: f64,
}

impl Distribution {
    fn of(values: impl Iterator<Item = f64>) -> Self {
        let values: Vec<f64> = values.collect();
        if values.is_empty() {
            return Self::default();
        }
        let n = values.len() as f64;
        let mean = values.iter().sum::<f64>() / n;
        let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
        Self {
            mean: round_to_resolution(mean),
            variance: round_to_resolution(variance),
            min: values.iter().copied().fold(f64::INFINITY, f64::min),
            max: values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        }
    }
}

/// Population-level numbers from one panel.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PanelSummary {
    pub seed: u64,
    pub runs_per_individual: usize,
    /// Mean of the individuals' success rates; `None` without a probe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean_success_rate: Option<f64>,
    pub mean_context_records: f64,
    pub mean_persona_calls: f64,
    pub mean_exploration_turns_attempted: f64,
    pub mean_candidates_promoted: f64,
    pub completed_all_runs: usize,
}

/// One line of `generations.jsonl`: the selection side of a generation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerationSummary {
    pub generation: u32,
    pub environment: String,
    pub population: usize,
    pub survivors: usize,
    /// True when nobody satisfied the survival rule and the whole population
    /// was used as parents instead. Such a generation is not a selection.
    pub selection_failed: bool,
    pub carried_over: usize,
    pub selection: PanelSummary,
}

/// One line of `adaptation.jsonl`: the validation side of a generation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdaptationSummary {
    pub generation: u32,
    pub environment: String,
    pub population: usize,
    /// Copied from the selection side so a reader of this file alone can
    /// tell a selected generation from a fallback one.
    pub selection_failed: bool,
    pub validation: PanelSummary,
    pub exploration_probability: Distribution,
    pub exploration_turns_attempted: Distribution,
    pub candidates_promoted: Distribution,
    pub context_records: Distribution,
    /// Per-gene mean and variance across the population.
    pub genes: BTreeMap<String, Distribution>,
    /// Mean of the per-gene variances: one number for "how spread out".
    pub mean_gene_variance: f64,
    /// Distinct generation-0 ancestors still represented.
    pub founder_lineages: usize,
    /// Distinct immediate parents (founders count as their own).
    pub distinct_parents: usize,
    pub unique_genome_hashes: usize,
}

/// Derive a sub-seed from the base seed and a role, so founders, mutations,
/// shuffles and harness runs never share a stream.
fn sub_seed(base: u64, generation: u32, index: usize, role: u64) -> u64 {
    let mut z = base
        .wrapping_add(0x9E37_79B9_7F4A_7C15u64.wrapping_mul(u64::from(generation) + 1))
        .wrapping_add(0xBF58_476D_1CE4_E5B9u64.wrapping_mul(index as u64 + 1))
        .wrapping_add(role.wrapping_mul(0x94D0_49BB_1331_11EB));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

const ROLE_FOUNDER: u64 = 1;
const ROLE_MUTATION: u64 = 2;
const ROLE_SELECTION_PANEL: u64 = 3;
const ROLE_SHUFFLE: u64 = 4;

/// Selection-panel seed for one generation. Kept far below the validation
/// panel's seed range by construction: the two panels never overlap.
pub fn selection_seed(config: &EvolutionConfig, generation: u32) -> u64 {
    sub_seed(config.seed, generation, 0, ROLE_SELECTION_PANEL) % 100_000
}

/// Generation 0: seeded random genomes. Depends on `seed` and
/// `population_size` only.
pub fn founders(config: &EvolutionConfig) -> Population {
    let individuals = (0..config.population_size)
        .map(|i| {
            let id = format!("gen-g0-{i:02}");
            Individual {
                genome: Genome::random_with_id(
                    sub_seed(config.seed, 0, i, ROLE_FOUNDER),
                    id.clone(),
                ),
                lineage: Lineage {
                    generation: 0,
                    origin: Origin::Founder,
                    parent_id: None,
                    founder_id: id,
                    born_in_generation: 0,
                    mutation_seed: None,
                },
            }
        })
        .collect();
    Population {
        generation: 0,
        individuals,
    }
}

/// Run one panel over a population. Every individual sees seeds
/// `seed .. seed + runs`. Returns one aggregate per individual, in
/// population order.
pub fn evaluate_panel(
    dir: &Path,
    config: &EvolutionConfig,
    population: &Population,
    seed: u64,
    runs: u32,
) -> Result<Vec<Aggregate>, RuntimeError> {
    let harness = HarnessConfig {
        seed,
        repeats: runs,
        mutants: 0,
        mutation_rate: 0.0,
        reflection_interval_turns: 0,
        scenario: config.environment.scenario.clone(),
    };
    let candidates: Vec<Candidate> = population
        .individuals
        .iter()
        .map(|i| Candidate {
            label: i.genome.genome_id.clone(),
            genome: i.genome.clone(),
        })
        .collect();
    let results = crate::selection::run(dir, &harness, &candidates)?;
    let by_label: BTreeMap<String, Aggregate> = aggregate(&results)
        .into_iter()
        .map(|a| (a.label.clone(), a))
        .collect();
    Ok(population
        .individuals
        .iter()
        .map(|i| by_label[&i.genome.genome_id].clone())
        .collect())
}

/// Seeded Fisher–Yates order of `0..n`.
fn shuffled_indices(n: usize, seed: u64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).collect();
    let mut state = seed;
    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    for i in (1..n).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    order
}

/// At most this share of the population is carried over unchanged, so every
/// generation always breeds new offspring even when everyone survives.
pub const MAX_CARRIED_SHARE: usize = 2;

/// What breeding decided, so the lineage log can record it.
#[derive(Debug, Clone, PartialEq)]
pub struct Breeding {
    pub next: Population,
    pub selection_failed: bool,
    /// Population indices carried over unchanged.
    pub carried: Vec<usize>,
}

/// Breed the next generation.
///
/// Parents are the survivors; with none, every individual is a parent and
/// `selection_failed` is set. Survivors are put in a seeded shuffled order
/// (seed, generation): the first `population/2` of that order are carried
/// over unchanged, and offspring are clone + mutate of the parents in that
/// same order, round-robin, until the population is full. No position in
/// the population array is privileged.
pub fn next_generation(
    config: &EvolutionConfig,
    population: &Population,
    survived: &[bool],
) -> Breeding {
    let next = population.generation + 1;
    let survivor_indices: Vec<usize> = (0..population.individuals.len())
        .filter(|i| survived[*i])
        .collect();
    let selection_failed = survivor_indices.is_empty();
    let parent_indices: Vec<usize> = if selection_failed {
        (0..population.individuals.len()).collect()
    } else {
        survivor_indices
    };
    let order = shuffled_indices(
        parent_indices.len(),
        sub_seed(config.seed, next, 0, ROLE_SHUFFLE),
    );
    let parents: Vec<&Individual> = order
        .iter()
        .map(|k| &population.individuals[parent_indices[*k]])
        .collect();
    let carried_count = if selection_failed {
        0
    } else {
        (config.population_size / MAX_CARRIED_SHARE)
            .max(1)
            .min(parents.len())
    };
    let carried: Vec<usize> = order[..carried_count]
        .iter()
        .map(|k| parent_indices[*k])
        .collect();
    let mut individuals: Vec<Individual> = parents
        .iter()
        .take(carried_count)
        .map(|s| Individual {
            genome: s.genome.clone(),
            lineage: Lineage {
                generation: next,
                origin: Origin::Survivor,
                parent_id: s.lineage.parent_id.clone(),
                founder_id: s.lineage.founder_id.clone(),
                born_in_generation: s.lineage.born_in_generation,
                mutation_seed: s.lineage.mutation_seed,
            },
        })
        .collect();
    let mut child_index = 0;
    while individuals.len() < config.population_size {
        let parent = parents[child_index % parents.len()];
        let mutation_seed = sub_seed(config.seed, next, child_index, ROLE_MUTATION);
        let mut genome = parent.genome.mutate_with_id(
            mutation_seed,
            config.mutation_rate,
            format!("gen-g{next}-{child_index:02}"),
        );
        // Fixed genes stay at the founder value: the mutation stream is
        // consumed identically in every environment, then the parent's —
        // i.e. the founder's — value is restored.
        for key in &config.environment.fixed_genes {
            let value = parent.genome.gene(key).expect("fixed genes validated");
            genome.set_gene(key, value).expect("fixed genes validated");
        }
        individuals.push(Individual {
            genome,
            lineage: Lineage {
                generation: next,
                origin: Origin::Offspring,
                parent_id: Some(parent.genome.genome_id.clone()),
                founder_id: parent.lineage.founder_id.clone(),
                born_in_generation: next,
                mutation_seed: Some(mutation_seed),
            },
        });
        child_index += 1;
    }
    Breeding {
        next: Population {
            generation: next,
            individuals,
        },
        selection_failed,
        carried,
    }
}

fn panel_summary(seed: u64, evaluations: &[Aggregate]) -> PanelSummary {
    let n = evaluations.len() as f64;
    let mean_of = |f: &dyn Fn(&Aggregate) -> f64| {
        round_to_resolution(evaluations.iter().map(f).sum::<f64>() / n)
    };
    let probed: Vec<f64> = evaluations.iter().filter_map(|a| a.success_rate).collect();
    PanelSummary {
        seed,
        runs_per_individual: evaluations.first().map_or(0, |a| a.runs),
        mean_success_rate: (!probed.is_empty())
            .then(|| round_to_resolution(probed.iter().sum::<f64>() / probed.len() as f64)),
        mean_context_records: mean_of(&|a| a.mean.context_records),
        mean_persona_calls: mean_of(&|a| a.mean.persona_calls),
        mean_exploration_turns_attempted: mean_of(&|a| a.mean.exploration_turns_attempted),
        mean_candidates_promoted: mean_of(&|a| a.mean.candidates_promoted),
        completed_all_runs: evaluations
            .iter()
            .filter(|a| a.completed_runs == a.runs)
            .count(),
    }
}

fn adaptation_summary(
    config: &EvolutionConfig,
    population: &Population,
    validation: &[Aggregate],
    selection_failed: bool,
) -> AdaptationSummary {
    let genomes = || population.individuals.iter().map(|i| &i.genome);
    let genes: BTreeMap<String, Distribution> = GENE_KEYS
        .iter()
        .map(|key| {
            (
                (*key).to_owned(),
                Distribution::of(genomes().map(|g| g.gene(key).unwrap())),
            )
        })
        .collect();
    let mean_gene_variance =
        round_to_resolution(genes.values().map(|d| d.variance).sum::<f64>() / genes.len() as f64);
    let distinct = |f: &dyn Fn(&Individual) -> String| {
        let mut values: Vec<String> = population.individuals.iter().map(f).collect();
        values.sort();
        values.dedup();
        values.len()
    };
    AdaptationSummary {
        generation: population.generation,
        environment: config.environment.name.clone(),
        population: population.individuals.len(),
        selection_failed,
        validation: panel_summary(config.validation_seed, validation),
        exploration_probability: Distribution::of(
            genomes().map(|g| g.phenotype().exploration_probability),
        ),
        exploration_turns_attempted: Distribution::of(
            validation
                .iter()
                .map(|a| a.mean.exploration_turns_attempted),
        ),
        candidates_promoted: Distribution::of(
            validation.iter().map(|a| a.mean.candidates_promoted),
        ),
        context_records: Distribution::of(validation.iter().map(|a| a.mean.context_records)),
        genes,
        mean_gene_variance,
        founder_lineages: distinct(&|i| i.lineage.founder_id.clone()),
        distinct_parents: distinct(&|i| {
            i.lineage
                .parent_id
                .clone()
                .unwrap_or_else(|| i.genome.genome_id.clone())
        }),
        unique_genome_hashes: distinct(&|i| i.genome.hash.clone()),
    }
}

/// The final-test panel's verdict: founders and the last evaluated
/// generation, each on the same untouched seeds. Written to
/// `final_test.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinalTest {
    pub seed: u64,
    pub runs_per_individual: usize,
    pub first_generation: u32,
    pub last_generation: u32,
    pub first: PanelSummary,
    pub last: PanelSummary,
    /// Per-individual results, population order, for both generations.
    pub first_individuals: Vec<PanelResult>,
    pub last_individuals: Vec<PanelResult>,
}

pub const FINAL_TEST_FILE_NAME: &str = "final_test.json";

/// Everything one run writes, returned so callers need not re-read files.
#[derive(Debug, Clone, PartialEq)]
pub struct Experiment {
    pub generations: Vec<GenerationSummary>,
    pub adaptation: Vec<AdaptationSummary>,
    pub lineage: Vec<LineageEntry>,
    pub final_test: FinalTest,
}

/// Run the whole experiment. Writes `lineage.jsonl`, `generations.jsonl`
/// (selection side), `adaptation.jsonl` (validation side), `config.json`, and
/// per generation `gen-<g>/` (selection panel) and `validation-<g>/` (validation
/// panel) harness output under `out_dir`.
pub fn run(out_dir: &Path, config: &EvolutionConfig) -> Result<Experiment, RuntimeError> {
    if config.population_size == 0
        || config.generations == 0
        || config.repeats == 0
        || config.validation_panel == 0
    {
        return Err(RuntimeError::Usage(
            "population_size, generations, repeats and validation_panel must all be at least 1"
                .to_owned(),
        ));
    }
    for key in &config.environment.fixed_genes {
        if !GENE_KEYS.contains(&key.as_str()) {
            return Err(RuntimeError::Usage(format!(
                "environment {:?} fixes unknown gene {key:?}",
                config.environment.name
            )));
        }
    }
    std::fs::create_dir_all(out_dir).map_err(|source| RuntimeError::DirectoryIo {
        path: out_dir.display().to_string(),
        message: source.to_string(),
    })?;
    let mut generations = Vec::new();
    let mut adaptation = Vec::new();
    let mut lineage = Vec::new();
    let first_population = founders(config);
    let mut population = first_population.clone();
    let mut last_evaluated = first_population.clone();
    for _ in 0..config.generations {
        let g = population.generation;
        let selection_seed = selection_seed(config, g);
        let selection = evaluate_panel(
            &out_dir.join(format!("gen-{g}")),
            config,
            &population,
            selection_seed,
            config.repeats,
        )?;
        let validation = evaluate_panel(
            &out_dir.join(format!("validation-{g}")),
            config,
            &population,
            config.validation_seed,
            config.validation_panel,
        )?;
        let failures: Vec<Vec<String>> = selection
            .iter()
            .map(|e| config.environment.failures(e))
            .collect();
        let survived: Vec<bool> = failures.iter().map(Vec::is_empty).collect();
        let breeding = next_generation(config, &population, &survived);
        for (index, individual) in population.individuals.iter().enumerate() {
            let mut sel = PanelResult::from(&selection[index]);
            sel.seed = selection_seed;
            let mut held = PanelResult::from(&validation[index]);
            held.seed = config.validation_seed;
            lineage.push(LineageEntry {
                generation: g,
                genome_id: individual.genome.genome_id.clone(),
                genome_hash: individual.genome.hash.clone(),
                parent_id: individual.lineage.parent_id.clone(),
                founder_id: individual.lineage.founder_id.clone(),
                origin: individual.lineage.origin,
                born_in_generation: individual.lineage.born_in_generation,
                mutation_seed: individual.lineage.mutation_seed,
                phenotype: individual.genome.phenotype(),
                genes: individual.genome.genes(),
                selection: sel,
                validation: held,
                survived: survived[index],
                failures: failures[index].clone(),
                carried_over: breeding.carried.contains(&index),
            });
        }
        generations.push(GenerationSummary {
            generation: g,
            environment: config.environment.name.clone(),
            population: population.individuals.len(),
            survivors: survived.iter().filter(|s| **s).count(),
            selection_failed: breeding.selection_failed,
            carried_over: breeding.carried.len(),
            selection: panel_summary(selection_seed, &selection),
        });
        adaptation.push(adaptation_summary(
            config,
            &population,
            &validation,
            breeding.selection_failed,
        ));
        last_evaluated = population;
        population = breeding.next;
    }
    // Final test: once, at the end, on founders and on the last evaluated
    // generation. Nothing above depends on it.
    let first_test = evaluate_panel(
        &out_dir.join("final-test-first"),
        config,
        &first_population,
        config.final_test_seed,
        config.final_test_panel,
    )?;
    let last_test = evaluate_panel(
        &out_dir.join("final-test-last"),
        config,
        &last_evaluated,
        config.final_test_seed,
        config.final_test_panel,
    )?;
    let panel_results = |aggregates: &[Aggregate]| -> Vec<PanelResult> {
        aggregates
            .iter()
            .map(|a| {
                let mut r = PanelResult::from(a);
                r.seed = config.final_test_seed;
                r
            })
            .collect()
    };
    let experiment = Experiment {
        generations,
        adaptation,
        lineage,
        final_test: FinalTest {
            seed: config.final_test_seed,
            runs_per_individual: config.final_test_panel as usize,
            first_generation: first_population.generation,
            last_generation: last_evaluated.generation,
            first: panel_summary(config.final_test_seed, &first_test),
            last: panel_summary(config.final_test_seed, &last_test),
            first_individuals: panel_results(&first_test),
            last_individuals: panel_results(&last_test),
        },
    };
    let write = |name: &str, text: &str| -> Result<(), RuntimeError> {
        let path = out_dir.join(name);
        std::fs::write(&path, text).map_err(|source| RuntimeError::DirectoryIo {
            path: path.display().to_string(),
            message: source.to_string(),
        })
    };
    fn jsonl<T: Serialize>(items: &[T]) -> String {
        let mut text = String::new();
        for item in items {
            text.push_str(&serde_json::to_string(item).expect("serializes"));
            text.push('\n');
        }
        text
    }
    write(LINEAGE_FILE_NAME, &jsonl(&experiment.lineage))?;
    write(GENERATIONS_FILE_NAME, &jsonl(&experiment.generations))?;
    write(ADAPTATION_FILE_NAME, &jsonl(&experiment.adaptation))?;
    write(
        FINAL_TEST_FILE_NAME,
        &serde_json::to_string_pretty(&experiment.final_test).expect("serializes"),
    )?;
    write(
        "config.json",
        &serde_json::to_string_pretty(config).expect("config serializes"),
    )?;
    Ok(experiment)
}

fn read_jsonl<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>, RuntimeError> {
    let text = std::fs::read_to_string(path).map_err(|source| RuntimeError::DirectoryIo {
        path: path.display().to_string(),
        message: source.to_string(),
    })?;
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|source| RuntimeError::ConfigMalformed {
                path: path.display().to_string(),
                message: source.to_string(),
            })
        })
        .collect()
}

pub fn read_experiment(out_dir: &Path) -> Result<Experiment, RuntimeError> {
    let path = out_dir.join(FINAL_TEST_FILE_NAME);
    let text = std::fs::read_to_string(&path).map_err(|source| RuntimeError::DirectoryIo {
        path: path.display().to_string(),
        message: source.to_string(),
    })?;
    let final_test =
        serde_json::from_str(&text).map_err(|source| RuntimeError::ConfigMalformed {
            path: path.display().to_string(),
            message: source.to_string(),
        })?;
    Ok(Experiment {
        generations: read_jsonl(&out_dir.join(GENERATIONS_FILE_NAME))?,
        adaptation: read_jsonl(&out_dir.join(ADAPTATION_FILE_NAME))?,
        lineage: read_jsonl(&out_dir.join(LINEAGE_FILE_NAME))?,
        final_test,
    })
}

fn opt_rate(rate: Option<f64>) -> String {
    rate.map_or_else(|| "-".to_owned(), |r| format!("{r:.2}"))
}

/// Selection table, validation adaptation table, generation 0 vs final
/// comparison on the validation panel, and the final generation's lineage.
pub fn report_markdown(experiment: &Experiment) -> String {
    let mut out = String::new();
    out.push_str("### selection panel (seeds change per generation; decides survival)\n\n");
    out.push_str(
        "| gen | n | seed | runs/ind | survivors | carried | selection_failed | success_rate mean | context_records mean | persona_calls mean | explore_turns mean | promoted mean |\n|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for s in &experiment.generations {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {:.2} | {:.1} | {:.2} | {:.2} |\n",
            s.generation,
            s.population,
            s.selection.seed,
            s.selection.runs_per_individual,
            s.survivors,
            s.carried_over,
            s.selection_failed,
            opt_rate(s.selection.mean_success_rate),
            s.selection.mean_context_records,
            s.selection.mean_persona_calls,
            s.selection.mean_exploration_turns_attempted,
            s.selection.mean_candidates_promoted,
        ));
    }
    out.push_str(
        "\n### validation panel (same seeds every generation; never used for selection)\n\n",
    );
    out.push_str(
        "| gen | selection_failed | success_rate mean | context_records mean (var) | p_explore mean (var, min–max) | explore_turns mean | promoted mean | mean gene variance | founder lineages | distinct parents | unique hashes |\n|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for a in &experiment.adaptation {
        out.push_str(&format!(
            "| {} | {} | {} | {:.2} ({:.2}) | {:.3} ({:.4}, {:.2}–{:.2}) | {:.2} | {:.2} | {:.4} | {} | {} | {} |\n",
            a.generation,
            a.selection_failed,
            opt_rate(a.validation.mean_success_rate),
            a.validation.mean_context_records,
            a.context_records.variance,
            a.exploration_probability.mean,
            a.exploration_probability.variance,
            a.exploration_probability.min,
            a.exploration_probability.max,
            a.validation.mean_exploration_turns_attempted,
            a.validation.mean_candidates_promoted,
            a.mean_gene_variance,
            a.founder_lineages,
            a.distinct_parents,
            a.unique_genome_hashes,
        ));
    }
    if let (Some(first), Some(last)) = (experiment.adaptation.first(), experiment.adaptation.last())
        && first.generation != last.generation
    {
        out.push_str(&format!(
            "\n### validation panel: generation {} → {} (seed {}, {} runs per individual)\n\n| quantity | gen {} | gen {} | delta |\n|---|---|---|---|\n",
            first.generation,
            last.generation,
            first.validation.seed,
            first.validation.runs_per_individual,
            first.generation,
            last.generation
        ));
        let row =
            |name: &str, a: f64, b: f64| format!("| {name} | {a:.3} | {b:.3} | {:+.3} |\n", b - a);
        if let (Some(a), Some(b)) = (
            first.validation.mean_success_rate,
            last.validation.mean_success_rate,
        ) {
            out.push_str(&row("success_rate mean", a, b));
        }
        out.push_str(&row(
            "context_records mean",
            first.validation.mean_context_records,
            last.validation.mean_context_records,
        ));
        out.push_str(&row(
            "exploration_probability mean",
            first.exploration_probability.mean,
            last.exploration_probability.mean,
        ));
        out.push_str(&row(
            "exploration_probability variance",
            first.exploration_probability.variance,
            last.exploration_probability.variance,
        ));
        out.push_str(&row(
            "exploration_turns_attempted mean",
            first.validation.mean_exploration_turns_attempted,
            last.validation.mean_exploration_turns_attempted,
        ));
        out.push_str(&row(
            "candidates_promoted mean",
            first.validation.mean_candidates_promoted,
            last.validation.mean_candidates_promoted,
        ));
        out.push_str(&row(
            "mean gene variance",
            first.mean_gene_variance,
            last.mean_gene_variance,
        ));
        out.push_str(&row(
            "founder lineages",
            first.founder_lineages as f64,
            last.founder_lineages as f64,
        ));
        out.push_str(&row(
            "unique genome hashes",
            first.unique_genome_hashes as f64,
            last.unique_genome_hashes as f64,
        ));
        for key in [
            "temperament.curiosity",
            "drives.novelty_seeking",
            "cognition.working_memory_span",
        ] {
            out.push_str(&row(
                &format!("gene {key} mean"),
                first.genes[key].mean,
                last.genes[key].mean,
            ));
        }
    }
    {
        let t = &experiment.final_test;
        out.push_str(&format!(
            "\n### final-test panel (seed {}, {} runs per individual; evaluated once at the end, never used for selection or tuning)\n\n| quantity | gen {} | gen {} | delta |\n|---|---|---|---|\n",
            t.seed, t.runs_per_individual, t.first_generation, t.last_generation
        ));
        let row =
            |name: &str, a: f64, b: f64| format!("| {name} | {a:.3} | {b:.3} | {:+.3} |\n", b - a);
        if let (Some(a), Some(b)) = (t.first.mean_success_rate, t.last.mean_success_rate) {
            out.push_str(&row("success_rate mean", a, b));
        }
        out.push_str(&row(
            "context_records mean",
            t.first.mean_context_records,
            t.last.mean_context_records,
        ));
        out.push_str(&row(
            "exploration_turns_attempted mean",
            t.first.mean_exploration_turns_attempted,
            t.last.mean_exploration_turns_attempted,
        ));
        out.push_str(&row(
            "candidates_promoted mean",
            t.first.mean_candidates_promoted,
            t.last.mean_candidates_promoted,
        ));
    }
    if let Some(last) = experiment.lineage.iter().map(|e| e.generation).max() {
        out.push_str(&format!("\n### lineage, generation {last}\n\n"));
        out.push_str(
            "| genome | origin | born | parent | founder | p_explore | selection success | validation success | validation context_records | survived | carried | failures |\n|---|---|---|---|---|---|---|---|---|---|---|---|\n",
        );
        for e in experiment.lineage.iter().filter(|e| e.generation == last) {
            out.push_str(&format!(
                "| {} | {:?} | g{} | {} | {} | {:.3} | {} | {} | {:.2} | {} | {} | {} |\n",
                e.genome_id,
                e.origin,
                e.born_in_generation,
                e.parent_id.as_deref().unwrap_or("-"),
                e.founder_id,
                e.phenotype.exploration_probability,
                opt_rate(e.selection.success_rate),
                opt_rate(e.validation.success_rate),
                e.validation.mean.context_records,
                e.survived,
                e.carried_over,
                e.failures.join("; "),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny(environment: Environment) -> EvolutionConfig {
        EvolutionConfig {
            seed: 21,
            population_size: 4,
            generations: 3,
            repeats: 2,
            mutation_rate: 0.5,
            validation_seed: 900_000,
            validation_panel: 2,
            final_test_seed: 950_000,
            final_test_panel: 2,
            environment,
        }
    }

    #[test]
    fn founders_are_matched_across_environments() {
        let a = founders(&tiny(Environment::baseline()));
        let b = founders(&tiny(Environment::distractor_heavy()));
        let c = founders(&tiny(Environment::exploration_favoured()));
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!(a.individuals.len(), 4);
        assert!(
            a.individuals
                .iter()
                .all(|i| i.lineage.origin == Origin::Founder
                    && i.lineage.founder_id == i.genome.genome_id)
        );
        let other_seed = founders(&EvolutionConfig {
            seed: 22,
            ..tiny(Environment::baseline())
        });
        assert_ne!(a, other_seed);
    }

    #[test]
    fn panels_are_separate_and_selection_seed_varies_per_generation() {
        let config = tiny(Environment::baseline());
        let seeds: Vec<u64> = (0..3).map(|g| selection_seed(&config, g)).collect();
        assert!(seeds.windows(2).any(|w| w[0] != w[1]));
        assert!(seeds.iter().all(|s| *s < 100_000));
        assert!(config.validation_seed >= 100_000);
    }

    #[test]
    fn carry_over_is_a_seeded_shuffle_not_array_position() {
        let config = tiny(Environment::baseline());
        let a = founders(&config);
        let all = [true; 4];
        let breeding = next_generation(&config, &a, &all);
        assert!(!breeding.selection_failed);
        assert_eq!(breeding.carried.len(), 2);
        assert_eq!(breeding.next.individuals.len(), 4);
        assert_eq!(breeding, next_generation(&config, &a, &all));
        // Across generations the shuffle differs, so no index is fixed.
        let mut carried_sets = std::collections::BTreeSet::new();
        for g in 0..6u32 {
            let population = Population {
                generation: g,
                individuals: a.individuals.clone(),
            };
            let mut carried = next_generation(&config, &population, &all).carried;
            carried.sort_unstable();
            carried_sets.insert(carried);
        }
        assert!(carried_sets.len() > 1, "{carried_sets:?}");
        // Survivors only, and lineage roots follow the parent.
        let some = [false, true, false, true];
        let breeding = next_generation(&config, &a, &some);
        assert!(breeding.carried.iter().all(|i| some[*i]));
        for child in &breeding.next.individuals {
            let parent_id = match child.lineage.origin {
                Origin::Survivor => child.genome.genome_id.clone(),
                Origin::Offspring => child.lineage.parent_id.clone().unwrap(),
                Origin::Founder => panic!(),
            };
            assert!(matches!(parent_id.as_str(), "gen-g0-01" | "gen-g0-03"));
            assert_eq!(child.lineage.founder_id, parent_id);
        }
        // Extinction: nobody carried, everybody a parent, flagged.
        let breeding = next_generation(&config, &a, &[false; 4]);
        assert!(breeding.selection_failed);
        assert!(breeding.carried.is_empty());
        assert!(
            breeding
                .next
                .individuals
                .iter()
                .all(|i| i.lineage.origin == Origin::Offspring)
        );
    }

    #[test]
    fn survival_rule_reports_reasons() {
        let env = Environment::distractor_heavy();
        let mut eval = Aggregate {
            label: "x".to_owned(),
            genome_id: "x".to_owned(),
            genome_hash: "h".to_owned(),
            parent_id: None,
            phenotype: Genome::with_id("x").phenotype(),
            genes_changed_from_parent: BTreeMap::new(),
            effective_reflection_interval: 0,
            runs: 2,
            completed_runs: 2,
            success_rate: Some(1.0),
            successes: 2,
            mean: MeanMetrics::default(),
        };
        assert!(env.failures(&eval).is_empty());
        eval.mean.context_records = 6.0;
        eval.success_rate = Some(0.5);
        eval.completed_runs = 1;
        let reasons = env.failures(&eval);
        assert_eq!(reasons.len(), 3, "{reasons:?}");
        assert!(Environment::baseline().failures(&eval).len() == 1);
    }

    fn strip_wall(experiment: &Experiment) -> Experiment {
        let mut e = experiment.clone();
        for entry in &mut e.lineage {
            entry.selection.mean.wall_ms = 0.0;
            entry.validation.mean.wall_ms = 0.0;
        }
        for r in e
            .final_test
            .first_individuals
            .iter_mut()
            .chain(e.final_test.last_individuals.iter_mut())
        {
            r.mean.wall_ms = 0.0;
        }
        e
    }

    #[test]
    fn experiment_is_reproducible_and_logs_both_panels() {
        let config = tiny(Environment::exploration_favoured());
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let a = run(dir_a.path(), &config).unwrap();
        let b = run(dir_b.path(), &config).unwrap();
        assert_eq!(strip_wall(&a), strip_wall(&b));
        assert_eq!(
            strip_wall(&read_experiment(dir_a.path()).unwrap()),
            strip_wall(&a)
        );
        assert_eq!(a.generations.len(), 3);
        assert_eq!(a.adaptation.len(), 3);
        assert_eq!(a.lineage.len(), 12);
        for e in &a.lineage {
            assert_eq!(e.selection.runs, 2);
            assert_eq!(e.validation.runs, 2);
            assert_eq!(e.validation.seed, 900_000);
            assert_eq!(e.selection.seed, selection_seed(&config, e.generation));
            assert_ne!(e.selection.seed, e.validation.seed);
        }
        // The validation seed is the same in every generation; the selection
        // seed is not.
        let held: std::collections::BTreeSet<u64> =
            a.lineage.iter().map(|e| e.validation.seed).collect();
        assert_eq!(held.len(), 1);
        let sel: std::collections::BTreeSet<u64> =
            a.lineage.iter().map(|e| e.selection.seed).collect();
        assert!(sel.len() > 1);
        for (g, adaptation) in a.generations.iter().zip(&a.adaptation) {
            assert_eq!(g.selection_failed, adaptation.selection_failed);
            let survived = a
                .lineage
                .iter()
                .filter(|e| e.generation == g.generation && e.survived)
                .count();
            assert_eq!(survived, g.survivors);
            let carried = a
                .lineage
                .iter()
                .filter(|e| e.generation == g.generation && e.carried_over)
                .count();
            assert_eq!(carried, g.carried_over);
            assert!(adaptation.founder_lineages <= 4);
            assert_eq!(adaptation.genes.len(), GENE_KEYS.len());
        }
        assert_eq!(a.final_test.first_generation, 0);
        assert_eq!(a.final_test.last_generation, 2);
        assert_eq!(a.final_test.seed, 950_000);
        assert_eq!(a.final_test.first_individuals.len(), 4);
        assert!(a.final_test.first_individuals.iter().all(|r| r.runs == 2));
        let table = report_markdown(&a);
        assert!(table.contains("### final-test panel"));
        assert!(table.contains("### selection panel"));
        assert!(table.contains("### validation panel"));
        assert!(table.contains("generation 0 → 2"));
        assert!(table.contains("### lineage, generation 2"));
    }

    /// Genes of every generation-0 individual, keyed by founder_id.
    fn founder_genes(experiment: &Experiment) -> BTreeMap<String, BTreeMap<String, f64>> {
        experiment
            .lineage
            .iter()
            .filter(|e| e.generation == 0)
            .map(|e| (e.founder_id.clone(), e.genes.clone()))
            .collect()
    }

    #[test]
    fn fixed_genes_stay_at_the_founder_value() {
        let config = tiny(Environment::exploration_wm_fixed());
        let dir = tempfile::tempdir().unwrap();
        let experiment = run(dir.path(), &config).unwrap();
        let founders = founder_genes(&experiment);
        assert_eq!(founders.len(), 4);
        for entry in &experiment.lineage {
            assert_eq!(
                entry.genes["cognition.working_memory_span"],
                founders[&entry.founder_id]["cognition.working_memory_span"],
                "generation {} {}",
                entry.generation,
                entry.genome_id
            );
        }
        // Mutation still runs on every other gene: at least one individual
        // differs from its own founder on an unfixed key.
        let moved = experiment.lineage.iter().any(|e| {
            e.generation > 0
                && GENE_KEYS.iter().any(|key| {
                    !config.environment.fixed_genes.contains(*key)
                        && e.genes[*key] != founders[&e.founder_id][*key]
                })
        });
        assert!(moved, "no individual differed from its founder");
    }

    #[test]
    fn fixed_exploration_genes_hold_the_phenotype_per_lineage() {
        let config = tiny(Environment::exploration_explore_fixed());
        let dir = tempfile::tempdir().unwrap();
        let experiment = run(dir.path(), &config).unwrap();
        let founders = founder_genes(&experiment);
        let founder_p: BTreeMap<String, f64> = experiment
            .lineage
            .iter()
            .filter(|e| e.generation == 0)
            .map(|e| (e.founder_id.clone(), e.phenotype.exploration_probability))
            .collect();
        for entry in &experiment.lineage {
            for key in ["temperament.curiosity", "drives.novelty_seeking"] {
                assert_eq!(entry.genes[key], founders[&entry.founder_id][key]);
            }
            assert_eq!(
                entry.phenotype.exploration_probability, founder_p[&entry.founder_id],
                "generation {} {}",
                entry.generation, entry.genome_id
            );
        }
    }

    #[test]
    fn fixed_environments_are_named_and_unknown_genes_rejected() {
        assert_eq!(
            Environment::named("exploration-wm-fixed"),
            Some(Environment::exploration_wm_fixed())
        );
        assert_eq!(
            Environment::named("exploration-explore-fixed"),
            Some(Environment::exploration_explore_fixed())
        );
        let mut environment = Environment::baseline();
        environment.fixed_genes.insert("bogus.gene".to_owned());
        let dir = tempfile::tempdir().unwrap();
        let error = run(dir.path(), &tiny(environment)).unwrap_err();
        assert!(matches!(error, RuntimeError::Usage(_)), "{error:?}");
    }
}
