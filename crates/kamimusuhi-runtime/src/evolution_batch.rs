//! Replicated evolution: separate selection from drift.
//!
//! One population can fix a founder by chance, so a change seen once is not
//! evidence of selection. A batch runs `replicates` independent populations,
//! each from its own top-level seed, and within one replicate all
//! environments start from the *same* founders. For every replicate the
//! generation-0 → final change of each trait is computed per environment and
//! then paired against the baseline environment (no probe, no budget):
//! `Δ(env) − Δ(baseline)` removes the drift that the shared founders and
//! the shared breeding mechanics would produce anyway. What survives that
//! subtraction, consistently across replicates, is environment-specific
//! selection.
//!
//! No fitness score, no ranking between environments: the statistics are
//! per-quantity paired differences with mean, median, variance, a seeded
//! bootstrap 95% interval and a count of replicates whose sign matched the
//! stated expectation.

use std::collections::BTreeMap;
use std::path::Path;

use kamimusuhi_core::genome::round_to_resolution;
use serde::{Deserialize, Serialize};

use crate::RuntimeError;
use crate::evolution::{Environment, EvolutionConfig, Experiment, run as run_experiment};

pub const REPLICATES_FILE_NAME: &str = "replicates.jsonl";
pub const SUMMARY_FILE_NAME: &str = "batch_summary.json";

/// The environments every replicate runs, in this order. `baseline` is
/// the drift reference the others are paired against. The two `-*-fixed`
/// variants rerun the exploration scenario with one causal factor pinned
/// at the founder value, separating the `working_memory_span` confound
/// from selection on `exploration_probability`.
pub const ENVIRONMENTS: [&str; 5] = [
    "exploration-favoured",
    "exploration-wm-fixed",
    "exploration-explore-fixed",
    "distractor-heavy",
    "baseline",
];
pub const REFERENCE_ENVIRONMENT: &str = "baseline";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchConfig {
    /// Replicate `r` uses top-level seed `seed_base + r`.
    pub seed_base: u64,
    pub replicates: u32,
    pub population_size: usize,
    pub generations: u32,
    pub repeats: u32,
    pub mutation_rate: f64,
    pub validation_seed: u64,
    pub validation_panel: u32,
    pub final_test_seed: u64,
    pub final_test_panel: u32,
    /// Worker threads; replicates are independent, so they run in parallel.
    /// 0 = available parallelism. Thread count never changes any number.
    pub threads: usize,
    /// Bootstrap resamples and their seed, for the confidence intervals.
    pub bootstrap_resamples: u32,
    pub bootstrap_seed: u64,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            seed_base: 1000,
            replicates: 12,
            population_size: 12,
            generations: 6,
            repeats: 8,
            mutation_rate: 0.5,
            validation_seed: 900_000,
            validation_panel: 32,
            final_test_seed: 950_000,
            final_test_panel: 32,
            threads: 0,
            bootstrap_resamples: 2000,
            bootstrap_seed: 7,
        }
    }
}

impl BatchConfig {
    pub fn experiment(&self, replicate: u32, environment: Environment) -> EvolutionConfig {
        EvolutionConfig {
            seed: self.seed_base + u64::from(replicate),
            population_size: self.population_size,
            generations: self.generations,
            repeats: self.repeats,
            mutation_rate: self.mutation_rate,
            validation_seed: self.validation_seed,
            validation_panel: self.validation_panel,
            final_test_seed: self.final_test_seed,
            final_test_panel: self.final_test_panel,
            environment,
        }
    }
}

/// Generation-0 and final values of the tracked quantities, plus the change.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Endpoints {
    pub first: BTreeMap<String, f64>,
    pub last: BTreeMap<String, f64>,
    pub delta: BTreeMap<String, f64>,
}

/// One environment of one replicate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentOutcome {
    pub environment: String,
    pub first_generation: u32,
    pub last_generation: u32,
    pub selection_failed_generations: u32,
    /// Validation panel: genome traits and behaviour, generation 0 → final.
    pub validation: Endpoints,
    /// Final-test panel: behaviour only, evaluated once at the end.
    pub final_test: Endpoints,
}

/// One line of `replicates.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplicateOutcome {
    pub replicate: u32,
    pub seed: u64,
    /// Hash digest of the founders, identical across the three environments.
    pub founder_hashes: Vec<String>,
    pub environments: Vec<EnvironmentOutcome>,
    /// `Δ(env) − Δ(baseline)` on the validation panel, per environment
    /// (excluding baseline itself), per quantity.
    pub paired_validation: BTreeMap<String, BTreeMap<String, f64>>,
    /// Same on the final-test panel (behaviour quantities only).
    pub paired_final_test: BTreeMap<String, BTreeMap<String, f64>>,
}

/// Quantities read from the validation side (`adaptation.jsonl`).
pub const VALIDATION_QUANTITIES: [&str; 10] = [
    "exploration_probability",
    "curiosity",
    "novelty_seeking",
    "success_rate",
    "context_records",
    "exploration_turns_attempted",
    "candidates_promoted",
    "founder_lineages",
    "mean_gene_variance",
    "working_memory_span",
];

/// Quantities read from the final-test panel.
pub const FINAL_TEST_QUANTITIES: [&str; 4] = [
    "success_rate",
    "context_records",
    "exploration_turns_attempted",
    "candidates_promoted",
];

fn validation_values(experiment: &Experiment, index: usize) -> BTreeMap<String, f64> {
    let a = &experiment.adaptation[index];
    let mut m = BTreeMap::new();
    m.insert(
        "exploration_probability".to_owned(),
        a.exploration_probability.mean,
    );
    m.insert(
        "curiosity".to_owned(),
        a.genes["temperament.curiosity"].mean,
    );
    m.insert(
        "novelty_seeking".to_owned(),
        a.genes["drives.novelty_seeking"].mean,
    );
    m.insert(
        "working_memory_span".to_owned(),
        a.genes["cognition.working_memory_span"].mean,
    );
    if let Some(rate) = a.validation.mean_success_rate {
        m.insert("success_rate".to_owned(), rate);
    }
    m.insert(
        "context_records".to_owned(),
        a.validation.mean_context_records,
    );
    m.insert(
        "exploration_turns_attempted".to_owned(),
        a.validation.mean_exploration_turns_attempted,
    );
    m.insert(
        "candidates_promoted".to_owned(),
        a.validation.mean_candidates_promoted,
    );
    m.insert("founder_lineages".to_owned(), a.founder_lineages as f64);
    m.insert("mean_gene_variance".to_owned(), a.mean_gene_variance);
    m
}

fn final_test_values(panel: &crate::evolution::PanelSummary) -> BTreeMap<String, f64> {
    let mut m = BTreeMap::new();
    if let Some(rate) = panel.mean_success_rate {
        m.insert("success_rate".to_owned(), rate);
    }
    m.insert("context_records".to_owned(), panel.mean_context_records);
    m.insert(
        "exploration_turns_attempted".to_owned(),
        panel.mean_exploration_turns_attempted,
    );
    m.insert(
        "candidates_promoted".to_owned(),
        panel.mean_candidates_promoted,
    );
    m
}

fn endpoints(first: BTreeMap<String, f64>, last: BTreeMap<String, f64>) -> Endpoints {
    let delta = first
        .iter()
        .filter_map(|(k, a)| last.get(k).map(|b| (k.clone(), round_to_resolution(b - a))))
        .collect();
    Endpoints { first, last, delta }
}

/// Summarize one finished experiment.
pub fn environment_outcome(experiment: &Experiment) -> EnvironmentOutcome {
    let last = experiment.adaptation.len() - 1;
    EnvironmentOutcome {
        environment: experiment.adaptation[0].environment.clone(),
        first_generation: experiment.adaptation[0].generation,
        last_generation: experiment.adaptation[last].generation,
        selection_failed_generations: experiment
            .generations
            .iter()
            .filter(|g| g.selection_failed)
            .count() as u32,
        validation: endpoints(
            validation_values(experiment, 0),
            validation_values(experiment, last),
        ),
        final_test: endpoints(
            final_test_values(&experiment.final_test.first),
            final_test_values(&experiment.final_test.last),
        ),
    }
}

fn paired(
    environments: &[EnvironmentOutcome],
    pick: fn(&EnvironmentOutcome) -> &Endpoints,
) -> BTreeMap<String, BTreeMap<String, f64>> {
    let reference = environments
        .iter()
        .find(|e| e.environment == REFERENCE_ENVIRONMENT)
        .map(|e| &pick(e).delta);
    let Some(reference) = reference else {
        return BTreeMap::new();
    };
    environments
        .iter()
        .filter(|e| e.environment != REFERENCE_ENVIRONMENT)
        .map(|e| {
            let diffs = pick(e)
                .delta
                .iter()
                .filter_map(|(k, d)| {
                    reference
                        .get(k)
                        .map(|r| (k.clone(), round_to_resolution(d - r)))
                })
                .collect();
            (e.environment.clone(), diffs)
        })
        .collect()
}

/// Run one replicate: every environment, matched founders, sequentially.
pub fn run_replicate(
    out_dir: &Path,
    config: &BatchConfig,
    replicate: u32,
) -> Result<ReplicateOutcome, RuntimeError> {
    let mut environments = Vec::new();
    let mut founder_hashes: Option<Vec<String>> = None;
    for name in ENVIRONMENTS {
        let environment = Environment::named(name).expect("bundled environment");
        let experiment_config = config.experiment(replicate, environment);
        let dir = out_dir.join(format!("rep-{replicate:02}")).join(name);
        let experiment = run_experiment(&dir, &experiment_config)?;
        let hashes: Vec<String> = experiment
            .lineage
            .iter()
            .filter(|e| e.generation == 0)
            .map(|e| e.genome_hash.clone())
            .collect();
        match &founder_hashes {
            None => founder_hashes = Some(hashes),
            Some(expected) => {
                if *expected != hashes {
                    return Err(RuntimeError::Usage(format!(
                        "replicate {replicate}: founders differ between environments"
                    )));
                }
            }
        }
        environments.push(environment_outcome(&experiment));
    }
    Ok(ReplicateOutcome {
        replicate,
        seed: config.seed_base + u64::from(replicate),
        founder_hashes: founder_hashes.unwrap_or_default(),
        paired_validation: paired(&environments, |e| &e.validation),
        paired_final_test: paired(&environments, |e| &e.final_test),
        environments,
    })
}

/// Run every replicate, in parallel, and write `replicates.jsonl`,
/// `batch_summary.json` and `config.json`. Output is independent of the
/// thread count.
pub fn run(out_dir: &Path, config: &BatchConfig) -> Result<Batch, RuntimeError> {
    if config.replicates == 0 {
        return Err(RuntimeError::Usage(
            "replicates must be at least 1".to_owned(),
        ));
    }
    std::fs::create_dir_all(out_dir).map_err(|source| RuntimeError::DirectoryIo {
        path: out_dir.display().to_string(),
        message: source.to_string(),
    })?;
    let threads = if config.threads == 0 {
        std::thread::available_parallelism().map_or(1, |n| n.get())
    } else {
        config.threads
    }
    .clamp(1, config.replicates as usize);
    let replicates: Vec<u32> = (0..config.replicates).collect();
    let chunk = replicates.len().div_ceil(threads);
    let mut outcomes: Vec<Option<ReplicateOutcome>> = vec![None; replicates.len()];
    let mut first_error: Option<RuntimeError> = None;
    std::thread::scope(|scope| {
        let handles: Vec<_> = replicates
            .chunks(chunk)
            .map(|batch| {
                scope.spawn(move || {
                    batch
                        .iter()
                        .map(|r| (*r, run_replicate(out_dir, config, *r)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for handle in handles {
            for (r, result) in handle.join().expect("replicate thread panicked") {
                match result {
                    Ok(outcome) => outcomes[r as usize] = Some(outcome),
                    Err(error) => {
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                }
            }
        }
    });
    if let Some(error) = first_error {
        return Err(error);
    }
    let replicates: Vec<ReplicateOutcome> = outcomes.into_iter().map(Option::unwrap).collect();
    let summary = summarize(config, &replicates);
    let write = |name: &str, text: &str| -> Result<(), RuntimeError> {
        let path = out_dir.join(name);
        std::fs::write(&path, text).map_err(|source| RuntimeError::DirectoryIo {
            path: path.display().to_string(),
            message: source.to_string(),
        })
    };
    let mut lines = String::new();
    for r in &replicates {
        lines.push_str(&serde_json::to_string(r).expect("serializes"));
        lines.push('\n');
    }
    write(REPLICATES_FILE_NAME, &lines)?;
    write(
        SUMMARY_FILE_NAME,
        &serde_json::to_string_pretty(&summary).expect("serializes"),
    )?;
    write(
        "config.json",
        &serde_json::to_string_pretty(config).expect("serializes"),
    )?;
    Ok(Batch {
        replicates,
        summary,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Batch {
    pub replicates: Vec<ReplicateOutcome>,
    pub summary: BatchSummary,
}

/// Descriptive statistics of one quantity across replicates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    pub n: usize,
    pub values: Vec<f64>,
    pub mean: f64,
    pub median: f64,
    pub variance: f64,
    /// Seeded percentile bootstrap of the mean, 2.5% and 97.5%.
    pub ci95: (f64, f64),
    pub positive: usize,
    pub negative: usize,
    pub zero: usize,
    /// The sign selection is expected to produce, when one is stated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_sign: Option<i8>,
    /// Replicates whose sign matched the expectation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_expected: Option<usize>,
}

/// Expected direction of selection, relative to baseline drift. Anything
/// not listed is reported without an expectation.
pub fn expected_sign(environment: &str, quantity: &str) -> Option<i8> {
    match (environment, quantity) {
        ("exploration-favoured", "exploration_probability")
        | ("exploration-favoured", "curiosity")
        | ("exploration-favoured", "novelty_seeking")
        | ("exploration-favoured", "success_rate")
        | ("exploration-favoured", "exploration_turns_attempted")
        | ("exploration-favoured", "candidates_promoted") => Some(1),
        // With the history-window confound frozen, the hypothesis is that
        // selection on the exploration genes still produces these.
        ("exploration-wm-fixed", "exploration_probability")
        | ("exploration-wm-fixed", "curiosity")
        | ("exploration-wm-fixed", "novelty_seeking")
        | ("exploration-wm-fixed", "success_rate")
        | ("exploration-wm-fixed", "exploration_turns_attempted")
        | ("exploration-wm-fixed", "candidates_promoted") => Some(1),
        // With the exploration genes frozen, the hypothesis is that
        // working_memory_span converging into the reachable band still
        // improves success on its own.
        ("exploration-explore-fixed", "success_rate") => Some(1),
        ("distractor-heavy", "context_records") | ("distractor-heavy", "candidates_promoted") => {
            Some(-1)
        }
        ("distractor-heavy", "success_rate") => Some(1),
        _ => None,
    }
}

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Statistics over `values`, with a seeded bootstrap interval.
pub fn stats(values: &[f64], resamples: u32, seed: u64, expected: Option<i8>) -> Stats {
    let n = values.len();
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let m = if n == 0 { 0.0 } else { mean(values) };
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = match n {
        0 => 0.0,
        n if n % 2 == 1 => sorted[n / 2],
        n => (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0,
    };
    let variance = if n < 2 {
        0.0
    } else {
        values.iter().map(|v| (v - m).powi(2)).sum::<f64>() / (n as f64 - 1.0)
    };
    let ci95 = if n < 2 || resamples == 0 {
        (m, m)
    } else {
        let mut state = seed;
        let mut means: Vec<f64> = (0..resamples)
            .map(|_| {
                let sum: f64 = (0..n)
                    .map(|_| values[(splitmix(&mut state) % n as u64) as usize])
                    .sum();
                sum / n as f64
            })
            .collect();
        means.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let at = |q: f64| means[((means.len() - 1) as f64 * q).round() as usize];
        (at(0.025), at(0.975))
    };
    let positive = values.iter().filter(|v| **v > 0.0).count();
    let negative = values.iter().filter(|v| **v < 0.0).count();
    let zero = n - positive - negative;
    let matched_expected = expected.map(|sign| if sign > 0 { positive } else { negative });
    Stats {
        n,
        values: values.iter().map(|v| round_to_resolution(*v)).collect(),
        mean: round_to_resolution(m),
        median: round_to_resolution(median),
        variance: round_to_resolution(variance),
        ci95: (round_to_resolution(ci95.0), round_to_resolution(ci95.1)),
        positive,
        negative,
        zero,
        expected_sign: expected,
        matched_expected,
    }
}

/// `batch_summary.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchSummary {
    pub replicates: usize,
    /// environment → quantity → stats of the raw generation-0 → final change
    /// on the validation panel.
    pub raw_validation: BTreeMap<String, BTreeMap<String, Stats>>,
    /// environment → quantity → stats of `Δ(env) − Δ(baseline)` on the
    /// validation panel.
    pub paired_validation: BTreeMap<String, BTreeMap<String, Stats>>,
    /// Same on the final-test panel.
    pub raw_final_test: BTreeMap<String, BTreeMap<String, Stats>>,
    pub paired_final_test: BTreeMap<String, BTreeMap<String, Stats>>,
    /// environment → replicates with at least one selection_failed
    /// generation, and the total of such generations.
    pub selection_failed: BTreeMap<String, (usize, u32)>,
}

/// (environment, quantity, value) triples pulled from one replicate.
type Triples = Vec<(String, String, f64)>;

pub fn summarize(config: &BatchConfig, replicates: &[ReplicateOutcome]) -> BatchSummary {
    let collect = |get: &dyn Fn(&ReplicateOutcome) -> Triples,
                   pair: bool|
     -> BTreeMap<String, BTreeMap<String, Stats>> {
        let mut values: BTreeMap<(String, String), Vec<f64>> = BTreeMap::new();
        for r in replicates {
            for (env, quantity, value) in get(r) {
                values.entry((env, quantity)).or_default().push(value);
            }
        }
        let mut out: BTreeMap<String, BTreeMap<String, Stats>> = BTreeMap::new();
        for ((env, quantity), v) in values {
            let expected = if pair {
                expected_sign(&env, &quantity)
            } else {
                None
            };
            out.entry(env).or_default().insert(
                quantity,
                stats(
                    &v,
                    config.bootstrap_resamples,
                    config.bootstrap_seed,
                    expected,
                ),
            );
        }
        out
    };
    let raw = |pick: fn(&EnvironmentOutcome) -> &Endpoints| {
        move |r: &ReplicateOutcome| -> Triples {
            r.environments
                .iter()
                .flat_map(|e| {
                    pick(e)
                        .delta
                        .iter()
                        .map(|(q, v)| (e.environment.clone(), q.clone(), *v))
                        .collect::<Vec<_>>()
                })
                .collect()
        }
    };
    let paired_of = |pick: fn(&ReplicateOutcome) -> &BTreeMap<String, BTreeMap<String, f64>>| {
        move |r: &ReplicateOutcome| -> Triples {
            pick(r)
                .iter()
                .flat_map(|(env, m)| {
                    m.iter()
                        .map(|(q, v)| (env.clone(), q.clone(), *v))
                        .collect::<Vec<_>>()
                })
                .collect()
        }
    };
    let mut selection_failed: BTreeMap<String, (usize, u32)> = BTreeMap::new();
    for r in replicates {
        for e in &r.environments {
            let entry = selection_failed.entry(e.environment.clone()).or_default();
            if e.selection_failed_generations > 0 {
                entry.0 += 1;
            }
            entry.1 += e.selection_failed_generations;
        }
    }
    BatchSummary {
        replicates: replicates.len(),
        raw_validation: collect(&raw(|e| &e.validation), false),
        paired_validation: collect(&paired_of(|r| &r.paired_validation), true),
        raw_final_test: collect(&raw(|e| &e.final_test), false),
        paired_final_test: collect(&paired_of(|r| &r.paired_final_test), true),
        selection_failed,
    }
}

pub fn read_batch(out_dir: &Path) -> Result<Batch, RuntimeError> {
    let path = out_dir.join(REPLICATES_FILE_NAME);
    let text = std::fs::read_to_string(&path).map_err(|source| RuntimeError::DirectoryIo {
        path: path.display().to_string(),
        message: source.to_string(),
    })?;
    let replicates = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|source| RuntimeError::ConfigMalformed {
                path: path.display().to_string(),
                message: source.to_string(),
            })
        })
        .collect::<Result<Vec<ReplicateOutcome>, _>>()?;
    let path = out_dir.join(SUMMARY_FILE_NAME);
    let text = std::fs::read_to_string(&path).map_err(|source| RuntimeError::DirectoryIo {
        path: path.display().to_string(),
        message: source.to_string(),
    })?;
    let summary = serde_json::from_str(&text).map_err(|source| RuntimeError::ConfigMalformed {
        path: path.display().to_string(),
        message: source.to_string(),
    })?;
    Ok(Batch {
        replicates,
        summary,
    })
}

fn stats_table(
    out: &mut String,
    title: &str,
    table: &BTreeMap<String, BTreeMap<String, Stats>>,
    quantities: &[&str],
) {
    out.push_str(&format!("\n### {title}\n\n"));
    out.push_str(
        "| environment | quantity | n | mean | median | variance | 95% bootstrap CI | + / − / 0 | expected | matched |\n|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for (env, by_quantity) in table {
        for quantity in quantities {
            let Some(s) = by_quantity.get(*quantity) else {
                continue;
            };
            out.push_str(&format!(
                "| {env} | {quantity} | {} | {:+.3} | {:+.3} | {:.4} | [{:+.3}, {:+.3}] | {} / {} / {} | {} | {} |\n",
                s.n,
                s.mean,
                s.median,
                s.variance,
                s.ci95.0,
                s.ci95.1,
                s.positive,
                s.negative,
                s.zero,
                s.expected_sign
                    .map_or_else(|| "-".to_owned(), |e| if e > 0 { "+".to_owned() } else { "−".to_owned() }),
                s.matched_expected
                    .map_or_else(|| "-".to_owned(), |m| format!("{m}/{}", s.n)),
            ));
        }
    }
}

/// Per-replicate values, then the four statistics tables.
pub fn report_markdown(batch: &Batch) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "replicates: {}  (each replicate: one founder population shared by {} environments)\n",
        batch.replicates.len(),
        ENVIRONMENTS.len()
    ));
    out.push_str("\n### selection_failed generations per environment (replicates affected, total generations)\n\n| environment | replicates with ≥1 | total generations |\n|---|---|---|\n");
    for (env, (reps, total)) in &batch.summary.selection_failed {
        out.push_str(&format!("| {env} | {reps} | {total} |\n"));
    }
    out.push_str(
        "\n### per replicate: validation-panel Δ(gen 0 → final), then Δ(env) − Δ(baseline)\n\n",
    );
    out.push_str("| rep | seed | env | Δp_explore | Δcuriosity | Δnovelty | Δsuccess | Δcontext | Δexplore_turns | Δpromoted | Δfounder_lineages | sel_failed | paired Δp_explore | paired Δsuccess | paired Δcontext | paired Δexplore_turns |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    let f = |m: &BTreeMap<String, f64>, k: &str| {
        m.get(k)
            .map_or_else(|| "-".to_owned(), |v| format!("{v:+.3}"))
    };
    for r in &batch.replicates {
        for e in &r.environments {
            let d = &e.validation.delta;
            let p = r.paired_validation.get(&e.environment);
            let pf = |k: &str| p.map_or_else(|| "-".to_owned(), |m| f(m, k));
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                r.replicate,
                r.seed,
                e.environment,
                f(d, "exploration_probability"),
                f(d, "curiosity"),
                f(d, "novelty_seeking"),
                f(d, "success_rate"),
                f(d, "context_records"),
                f(d, "exploration_turns_attempted"),
                f(d, "candidates_promoted"),
                f(d, "founder_lineages"),
                e.selection_failed_generations,
                pf("exploration_probability"),
                pf("success_rate"),
                pf("context_records"),
                pf("exploration_turns_attempted"),
            ));
        }
    }
    stats_table(
        &mut out,
        "validation panel: raw Δ(gen 0 → final) per environment",
        &batch.summary.raw_validation,
        &VALIDATION_QUANTITIES,
    );
    stats_table(
        &mut out,
        "validation panel: paired Δ(env) − Δ(baseline)",
        &batch.summary.paired_validation,
        &VALIDATION_QUANTITIES,
    );
    stats_table(
        &mut out,
        "final-test panel: raw Δ(gen 0 → final) per environment (evaluated once at the end)",
        &batch.summary.raw_final_test,
        &FINAL_TEST_QUANTITIES,
    );
    stats_table(
        &mut out,
        "final-test panel: paired Δ(env) − Δ(baseline)",
        &batch.summary.paired_final_test,
        &FINAL_TEST_QUANTITIES,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> BatchConfig {
        BatchConfig {
            seed_base: 300,
            replicates: 2,
            population_size: 4,
            generations: 2,
            repeats: 2,
            mutation_rate: 0.5,
            validation_seed: 900_000,
            validation_panel: 2,
            final_test_seed: 950_000,
            final_test_panel: 2,
            threads: 2,
            bootstrap_resamples: 200,
            bootstrap_seed: 7,
        }
    }

    #[test]
    fn stats_are_deterministic_and_count_signs() {
        let v = [0.2, -0.1, 0.3, 0.0, 0.5];
        let a = stats(&v, 500, 7, Some(1));
        let b = stats(&v, 500, 7, Some(1));
        assert_eq!(a, b);
        assert_eq!(a.n, 5);
        assert_eq!((a.positive, a.negative, a.zero), (3, 1, 1));
        assert_eq!(a.matched_expected, Some(3));
        assert_eq!(a.median, 0.2);
        assert!((a.mean - 0.18).abs() < 1e-9);
        assert!(a.ci95.0 <= a.mean && a.mean <= a.ci95.1);
        let other = stats(&v, 500, 8, Some(-1));
        assert_eq!(other.matched_expected, Some(1));
        assert_ne!(other.ci95, a.ci95);
        let single = stats(&[1.5], 500, 7, None);
        assert_eq!(single.ci95, (1.5, 1.5));
        assert_eq!(single.variance, 0.0);
    }

    #[test]
    fn expected_signs_cover_the_paired_comparisons() {
        assert_eq!(
            expected_sign("exploration-favoured", "exploration_probability"),
            Some(1)
        );
        assert_eq!(
            expected_sign("distractor-heavy", "context_records"),
            Some(-1)
        );
        assert_eq!(expected_sign("baseline", "context_records"), None);
    }

    #[test]
    fn batch_is_reproducible_matched_and_thread_independent() {
        let config = tiny();
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let a = run(dir_a.path(), &config).unwrap();
        let b = run(
            dir_b.path(),
            &BatchConfig {
                threads: 1,
                ..config.clone()
            },
        )
        .unwrap();
        assert_eq!(a, b);
        assert_eq!(a.replicates.len(), 2);
        for r in &a.replicates {
            assert_eq!(r.environments.len(), ENVIRONMENTS.len());
            assert_eq!(r.founder_hashes.len(), 4);
            assert_eq!(r.seed, 300 + u64::from(r.replicate));
            assert!(r.paired_validation.contains_key("exploration-favoured"));
            assert!(r.paired_validation.contains_key("exploration-wm-fixed"));
            assert!(
                r.paired_validation
                    .contains_key("exploration-explore-fixed")
            );
            assert!(r.paired_validation.contains_key("distractor-heavy"));
            assert!(!r.paired_validation.contains_key("baseline"));
            // baseline has no probe, so success_rate cannot be paired.
            assert!(!r.paired_validation["exploration-favoured"].contains_key("success_rate"));
            assert!(
                r.paired_validation["exploration-favoured"].contains_key("exploration_probability")
            );
            assert!(r.paired_final_test["distractor-heavy"].contains_key("context_records"));
        }
        // Different replicates have different founders.
        assert_ne!(
            a.replicates[0].founder_hashes,
            a.replicates[1].founder_hashes
        );
        let read = read_batch(dir_a.path()).unwrap();
        assert_eq!(read, a);
        let table = report_markdown(&a);
        assert!(table.contains("paired Δ(env) − Δ(baseline)"));
        assert!(table.contains("final-test panel"));
        assert_eq!(
            a.summary.paired_validation["exploration-favoured"]["exploration_probability"].n,
            2
        );
    }
}
