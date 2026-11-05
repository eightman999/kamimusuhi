//! Selection harness: run several genomes through one scenario and measure.
//!
//! The question this answers is narrow: *does a gene difference produce an
//! observable behaviour difference?* It does not rank individuals and has no
//! fitness function; success and cost are stored side by side, never
//! combined. Each individual gets its own runtime directory, the same
//! deterministic ID seed, the same fixed clock, the same fake Persona and
//! reflector, and the same scripted user turns. What differs between
//! individuals is the genome file, so any difference in the metrics is a
//! genome effect.
//!
//! Two scenarios are bundled:
//! - `baseline`: ten ordinary turns; measures recall, history and
//!   reflection volume.
//! - `exploration`: a fact is stated early in wording that shares no
//!   lexical overlap with the question asked about it later, after the fact
//!   has left the history window. The deterministic baseline retrieval
//!   cannot find it; only the exploration branch (admitting an uncertain,
//!   non-baseline recall candidate) can. Success = the fact reached the
//!   workspace on the question turn.
//!
//! Every metric is either read back from the runtime's own records (turn
//! replies, the workspace context, the trace file) or counted at the call
//! site of a documented runtime API. Wall-clock time is recorded but is not
//! reproducible.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use kamimusuhi_core::genome::{Genome, Phenotype};
use kamimusuhi_core::routing::PrivacyConstraint;
use kamimusuhi_core::trace::TraceEventKind;
use serde::{Deserialize, Serialize};

use crate::c0::{chat, reflection};
use crate::config::ResourceImplementation;
use crate::dialogue::DialogueSession;
use crate::{Runtime, RuntimeError, RuntimeOptions, read_trace};

/// Results file inside the harness output directory.
pub const RESULTS_FILE_NAME: &str = "results.jsonl";

/// The ordinary scripted conversation. Long enough to push early utterances
/// out of a default history window and to cross several reflection cadences.
pub const DEFAULT_SCENARIO: [&str; 10] = [
    "こんにちは。私は海の近くに住んでいます。",
    "昨日は雨で、屋内で本を読んで過ごしました。",
    "好きな食べ物は焼き魚です。",
    "先週、友人と山へハイキングに行きました。",
    "最近は睡眠が浅くて、朝が少しつらいです。",
    "私の仕事はソフトウェア開発です。",
    "訂正します。住んでいるのは海ではなく川の近くです。",
    "週末は何をして過ごすのがいいと思いますか。",
    "私の好きな食べ物を覚えていますか。",
    "今日はありがとう。また話しましょう。",
];

/// The information-gap conversation. Turn 2 states the fact; turns 3–8 push
/// it out of a default 12-message window; turn 9 asks about it in words that
/// share no character bigram with the fact, so lexical retrieval misses.
pub const EXPLORATION_SCENARIO: [&str; 10] = [
    "こんにちは。今日は自己紹介をします。",
    "焼き魚が一番のごちそうです。",
    "昨日は雨で、屋内で本を読んで過ごしました。",
    "先週、友人と山へハイキングに行きました。",
    "最近は睡眠が浅くて、朝が少しつらいです。",
    "私の仕事はソフトウェア開発です。",
    "週末は庭の手入れをする予定です。",
    "通勤は自転車で三十分ほどかかります。",
    "夕食に何を出せば喜ぶと思いますか。",
    "今日はありがとう。また話しましょう。",
];

/// The distractor conversation. Same shape as [`EXPLORATION_SCENARIO`], but
/// the question on turn 9 shares bigrams with the fact on turn 2, so the
/// deterministic baseline finds it without exploring. Exploring here only
/// admits unrelated out-of-window records: pure context cost, no benefit.
pub const DISTRACTOR_SCENARIO: [&str; 10] = [
    "こんにちは。私は海の近くに住んでいます。",
    "好きな食べ物は焼き魚です。",
    "昨日は雨で、屋内で本を読んで過ごしました。",
    "先週、友人と山へハイキングに行きました。",
    "最近は睡眠が浅くて、朝が少しつらいです。",
    "私の仕事はソフトウェア開発です。",
    "週末は庭の手入れをする予定です。",
    "通勤は自転車で三十分ほどかかります。",
    "私の好きな食べ物を覚えていますか。",
    "今日はありがとう。また話しましょう。",
];

/// What the exploration scenario checks on its question turn.
pub const EXPLORATION_EXPECTED_TEXT: &str = "焼き魚";
pub const EXPLORATION_PROBE_TURN: usize = 8;

/// A success probe: on `turn_index` (0-based), `expected_text` must appear
/// in the material the Persona was given from beyond the history window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probe {
    pub turn_index: usize,
    pub expected_text: String,
}

/// A scripted scenario, optionally with a success probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scenario {
    pub name: String,
    pub turns: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<Probe>,
}

impl Scenario {
    pub fn baseline() -> Self {
        Self {
            name: "baseline".to_owned(),
            turns: DEFAULT_SCENARIO.iter().map(|s| (*s).to_owned()).collect(),
            probe: None,
        }
    }

    pub fn exploration() -> Self {
        Self {
            name: "exploration".to_owned(),
            turns: EXPLORATION_SCENARIO
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            probe: Some(Probe {
                turn_index: EXPLORATION_PROBE_TURN,
                expected_text: EXPLORATION_EXPECTED_TEXT.to_owned(),
            }),
        }
    }

    pub fn distractor() -> Self {
        Self {
            name: "distractor".to_owned(),
            turns: DISTRACTOR_SCENARIO
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            probe: Some(Probe {
                turn_index: EXPLORATION_PROBE_TURN,
                expected_text: EXPLORATION_EXPECTED_TEXT.to_owned(),
            }),
        }
    }

    /// Bundled scenario by name.
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "baseline" => Some(Self::baseline()),
            "exploration" => Some(Self::exploration()),
            "distractor" => Some(Self::distractor()),
            _ => None,
        }
    }

    /// Keep only the first `n` turns. A probe beyond the cut is dropped.
    pub fn truncate(&mut self, n: usize) {
        self.turns.truncate(n);
        if self.probe.as_ref().is_some_and(|p| p.turn_index >= n) {
            self.probe = None;
        }
    }
}

/// What the harness runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessConfig {
    /// Base seed. Run `k` of an individual uses ID seed `seed + k`; the
    /// mutants use mutation seeds `seed + 1 ..= seed + mutants`.
    pub seed: u64,
    /// How many seeds each individual is run with. Success rate and mean cost
    /// are over these runs; every run is reproducible on its own.
    pub repeats: u32,
    /// Mutants derived from the parent.
    pub mutants: u32,
    /// Per-gene mutation probability handed to `Genome::mutate`.
    pub mutation_rate: f64,
    /// Configured automatic reflection cadence (`c0.reflection_interval_turns`)
    /// every individual starts from. Genes scale it from here.
    pub reflection_interval_turns: u32,
    pub scenario: Scenario,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            seed: 1,
            repeats: 1,
            mutants: 2,
            mutation_rate: 1.0,
            reflection_interval_turns: 3,
            scenario: Scenario::baseline(),
        }
    }
}

/// Volume of what the individual did over the scenario.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BehaviorMetrics {
    pub turns_completed: usize,
    pub turns_requested: usize,
    /// Sum over turns of retained records surfaced into the workspace.
    pub episodic_recalled: usize,
    pub relationship_recalled: usize,
    /// Sum over turns of raw evidence recalled from beyond the history window.
    pub evidence_recalled: usize,
    /// Sum over turns of conversation messages replayed to the Persona.
    pub history_messages: usize,
    /// Automatic reflection cycles run by the cadence.
    pub reflections: usize,
    /// Derived-lane activations those cycles applied.
    pub reflection_activations: usize,
}

/// The exploration branch, counted over the scenario.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorationMetrics {
    /// Turns with at least one uncertain, non-baseline recall candidate.
    pub opportunity_turns: usize,
    /// Opportunity turns where the draw cleared the probability.
    pub exploration_turns_attempted: usize,
    /// Opportunity turns where it did not.
    pub exploration_turns_skipped: usize,
    /// Candidate records admitted into the workspace by exploring, summed
    /// over turns.
    pub candidates_promoted: usize,
}

/// What the scenario cost, kept apart from whether it succeeded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostMetrics {
    /// `persona.invoked` trace events: one per language-model turn.
    pub persona_calls: usize,
    /// `resource.completed` trace events: delegated cognitive-resource calls.
    pub resource_calls: usize,
    /// Tool calls the Persona made, summed over turns.
    pub tool_calls: usize,
    /// Records handed to the Persona from beyond the history window (recalled
    /// evidence + retained memories), summed: the context the exploration
    /// branch pays for.
    pub context_records: usize,
}

/// The probe's verdict for one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub probe_turn: usize,
    pub expected_text: String,
    /// The expected text reached the Persona's out-of-window material on the
    /// probe turn.
    pub success: bool,
    /// Which workspace section held it, when found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub found_in: Option<String>,
}

/// One line of `results.jsonl`: one individual, one run seed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndividualResult {
    /// Position in the population: 0 is the parent, then mutants, then
    /// explorer presets, then extra genomes.
    pub index: usize,
    pub label: String,
    pub genome_id: String,
    pub genome_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub genome_version: u32,
    pub phenotype: Phenotype,
    /// Genes that differ from the parent, as `key: value`. Empty for the
    /// parent itself.
    pub genes_changed_from_parent: BTreeMap<String, f64>,
    pub effective_reflection_interval: u32,
    pub scenario: String,
    /// Base seed of the harness run.
    pub seed: u64,
    /// ID seed this particular run used.
    pub run_seed: u64,
    pub repeat_index: u32,
    pub completed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub metrics: BehaviorMetrics,
    pub exploration: ExplorationMetrics,
    pub cost: CostMetrics,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    pub wall_ms: u64,
    pub runtime_dir: String,
}

/// One individual to run: a genome plus a label for the report.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub label: String,
    pub genome: Genome,
}

/// The two exploration extremes, derived from the parent so everything but
/// the exploration genes stays identical: `low-exploration` (curiosity and
/// novelty_seeking 0 → p = 0.05) and `high-exploration` (both 1 → p = 0.65).
pub fn explorer_presets(parent: &Genome) -> Vec<Candidate> {
    let preset = |label: &str, value: f64| {
        let mut genome = parent.clone_genome_with_id(format!("{}-{label}", parent.genome_id));
        genome.set_gene("temperament.curiosity", value).unwrap();
        genome.set_gene("drives.novelty_seeking", value).unwrap();
        Candidate {
            label: label.to_owned(),
            genome,
        }
    };
    vec![
        preset("low-exploration", 0.0),
        preset("high-exploration", 1.0),
    ]
}

/// Build the population: parent, then `mutants` deterministic mutants, then
/// any explicitly supplied genomes.
pub fn population(
    parent: &Genome,
    config: &HarnessConfig,
    extra: Vec<Candidate>,
) -> Vec<Candidate> {
    let mut candidates = vec![Candidate {
        label: "parent".to_owned(),
        genome: parent.clone(),
    }];
    for i in 1..=config.mutants {
        let seed = config.seed + u64::from(i);
        let id = format!("gen-sel-{}-m{i}", config.seed);
        candidates.push(Candidate {
            label: format!("mutant-{i}"),
            genome: parent.mutate_with_id(seed, config.mutation_rate, id),
        });
    }
    candidates.extend(extra);
    candidates
}

/// Run every candidate `repeats` times and write one JSONL line per
/// (individual, run) to `<out_dir>/results.jsonl`. Runtime directories live
/// under `<out_dir>/individuals/<index>-<label>/seed-<run_seed>/`.
pub fn run(
    out_dir: &Path,
    config: &HarnessConfig,
    candidates: &[Candidate],
) -> Result<Vec<IndividualResult>, RuntimeError> {
    if config.repeats == 0 {
        return Err(RuntimeError::Usage("repeats must be at least 1".to_owned()));
    }
    std::fs::create_dir_all(out_dir).map_err(|source| RuntimeError::DirectoryIo {
        path: out_dir.display().to_string(),
        message: source.to_string(),
    })?;
    let parent = candidates.first().map(|c| &c.genome);
    let mut results = Vec::with_capacity(candidates.len() * config.repeats as usize);
    for (index, candidate) in candidates.iter().enumerate() {
        for repeat in 0..config.repeats {
            let run_seed = config.seed + u64::from(repeat);
            let dir = out_dir
                .join("individuals")
                .join(format!("{index}-{}", candidate.label))
                .join(format!("seed-{run_seed}"));
            results.push(run_individual(
                index, candidate, parent, &dir, config, repeat, run_seed,
            ));
        }
    }
    let path = out_dir.join(RESULTS_FILE_NAME);
    let mut text = String::new();
    for result in &results {
        text.push_str(&serde_json::to_string(result).expect("result serializes"));
        text.push('\n');
    }
    std::fs::write(&path, text).map_err(|source| RuntimeError::DirectoryIo {
        path: path.display().to_string(),
        message: source.to_string(),
    })?;
    Ok(results)
}

fn run_individual(
    index: usize,
    candidate: &Candidate,
    parent: Option<&Genome>,
    dir: &Path,
    config: &HarnessConfig,
    repeat_index: u32,
    run_seed: u64,
) -> IndividualResult {
    let started = Instant::now();
    let genome = &candidate.genome;
    let genes_changed_from_parent = parent
        .map(|p| {
            kamimusuhi_core::genome::diff(p, genome)
                .changed
                .into_iter()
                .map(|d| (d.key, d.b))
                .collect()
        })
        .unwrap_or_default();
    let mut result = IndividualResult {
        index,
        label: candidate.label.clone(),
        genome_id: genome.genome_id.clone(),
        genome_hash: genome.hash.clone(),
        parent_id: genome.parent_id.clone(),
        genome_version: genome.version,
        phenotype: genome.phenotype(),
        genes_changed_from_parent,
        effective_reflection_interval: genome
            .phenotype()
            .reflection_interval(config.reflection_interval_turns),
        scenario: config.scenario.name.clone(),
        seed: config.seed,
        run_seed,
        repeat_index,
        completed: false,
        error: None,
        metrics: BehaviorMetrics {
            turns_requested: config.scenario.turns.len(),
            ..BehaviorMetrics::default()
        },
        exploration: ExplorationMetrics::default(),
        cost: CostMetrics::default(),
        outcome: None,
        wall_ms: 0,
        runtime_dir: dir.display().to_string(),
    };
    match execute(genome, dir, config, run_seed, &mut result) {
        Ok(()) => {
            result.completed = result.metrics.turns_completed == config.scenario.turns.len();
        }
        Err(error) => result.error = Some(error.to_string()),
    }
    result.wall_ms = started.elapsed().as_millis() as u64;
    result
}

/// The scenario itself. Metrics are filled in as it goes, so a failure
/// mid-way still leaves what was measured.
fn execute(
    genome: &Genome,
    dir: &Path,
    config: &HarnessConfig,
    run_seed: u64,
    result: &mut IndividualResult,
) -> Result<(), RuntimeError> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|source| RuntimeError::DirectoryIo {
            path: dir.display().to_string(),
            message: source.to_string(),
        })?;
    }
    // Identical seed for every individual: IDs, timestamps and therefore the
    // exploration draws line up, so the only free variable is the genome.
    let options = RuntimeOptions::deterministic(run_seed);
    {
        let runtime = Runtime::init(dir, options, ResourceImplementation::FakeA)?;
        let mut runtime_config = runtime.config().clone();
        runtime_config.c0.reflection_interval_turns = config.reflection_interval_turns;
        // The harness measures the exploration branch, so it turns it on; a
        // production runtime keeps the legacy default (off).
        runtime_config.genome_exploration = true;
        runtime_config.save(&runtime.paths().config())?;
        genome.save(&runtime.paths().genome())?;
        runtime.stopping();
    }
    let mut runtime = Runtime::open(dir, options)?;
    let mut session =
        DialogueSession::start(&mut runtime, "harness", PrivacyConstraint::LocalOnly)?;
    for (turn_index, message) in config.scenario.turns.iter().enumerate() {
        let reply = session.turn(&mut runtime, message, |_| Ok(()))?;
        let metrics = &mut result.metrics;
        metrics.turns_completed += 1;
        metrics.episodic_recalled += reply.episodic_recalled;
        metrics.relationship_recalled += reply.relationship_recalled;
        metrics.evidence_recalled += reply.evidence_recalled;
        metrics.history_messages += reply.history_messages;
        result.cost.tool_calls += reply.tool_calls.len();
        result.cost.context_records +=
            reply.evidence_recalled + reply.episodic_recalled + reply.relationship_recalled;
        if let Some(exploration) = &reply.exploration
            && exploration.candidates > 0
        {
            result.exploration.opportunity_turns += 1;
            if exploration.explored {
                result.exploration.exploration_turns_attempted += 1;
                result.exploration.candidates_promoted += exploration.promoted;
            } else {
                result.exploration.exploration_turns_skipped += 1;
            }
        }
        if let Some(probe) = &config.scenario.probe
            && probe.turn_index == turn_index
        {
            result.outcome = Some(probe_outcome(probe, session.last_context()));
        }
        if chat::reflection_due(&runtime, &session) {
            let subject = session.subject().to_owned();
            let report = reflection::run(
                &mut runtime,
                &subject,
                Some(session.session_id()),
                None,
                session.writer_cache(),
            )?;
            result.metrics.reflections += 1;
            result.metrics.reflection_activations += report.activations.len();
        }
    }
    runtime.stopping();
    let trace_path = runtime.paths().trace();
    drop(runtime);
    let events = read_trace(&trace_path).map_err(|error| RuntimeError::DirectoryIo {
        path: trace_path.display().to_string(),
        message: error.to_string(),
    })?;
    for event in events {
        match event.event_kind {
            TraceEventKind::PersonaInvoked => result.cost.persona_calls += 1,
            TraceEventKind::ResourceCompleted => result.cost.resource_calls += 1,
            _ => {}
        }
    }
    Ok(())
}

/// Sections of the workspace context that hold out-of-window material. The
/// replayed history (`recent_context`) is deliberately excluded: finding the
/// fact there would mean the window was wide, not that anything was recalled.
const PROBE_SECTIONS: [&str; 3] = [
    "recalled_evidence",
    "episodic_memories",
    "relationship_memories",
];

fn probe_outcome(probe: &Probe, context: Option<&serde_json::Value>) -> Outcome {
    let found_in = context.and_then(|context| {
        PROBE_SECTIONS
            .iter()
            .find(|section| {
                context
                    .get(section)
                    .is_some_and(|v| v.to_string().contains(&probe.expected_text))
            })
            .map(|s| (*s).to_owned())
    });
    Outcome {
        probe_turn: probe.turn_index,
        expected_text: probe.expected_text.clone(),
        success: found_in.is_some(),
        found_in,
    }
}

/// Read a results file back.
pub fn read_results(path: &Path) -> Result<Vec<IndividualResult>, RuntimeError> {
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

/// Per-individual aggregate over its runs. Success rate and cost are
/// separate numbers; nothing here combines them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Aggregate {
    pub label: String,
    pub genome_id: String,
    pub genome_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub phenotype: Phenotype,
    pub genes_changed_from_parent: BTreeMap<String, f64>,
    pub effective_reflection_interval: u32,
    pub runs: usize,
    pub completed_runs: usize,
    /// `None` when the scenario has no probe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success_rate: Option<f64>,
    pub successes: usize,
    pub mean: MeanMetrics,
}

/// Means over runs of every counted quantity.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MeanMetrics {
    pub history_messages: f64,
    pub episodic_recalled: f64,
    pub relationship_recalled: f64,
    pub evidence_recalled: f64,
    pub reflections: f64,
    pub reflection_activations: f64,
    pub opportunity_turns: f64,
    pub exploration_turns_attempted: f64,
    pub exploration_turns_skipped: f64,
    pub candidates_promoted: f64,
    pub persona_calls: f64,
    pub resource_calls: f64,
    pub tool_calls: f64,
    pub context_records: f64,
    pub wall_ms: f64,
}

/// Group results by individual (in first-seen order) and average.
pub fn aggregate(results: &[IndividualResult]) -> Vec<Aggregate> {
    let mut order: Vec<&str> = Vec::new();
    let mut groups: BTreeMap<&str, Vec<&IndividualResult>> = BTreeMap::new();
    for r in results {
        if !groups.contains_key(r.label.as_str()) {
            order.push(&r.label);
        }
        groups.entry(&r.label).or_default().push(r);
    }
    order
        .into_iter()
        .map(|label| {
            let runs = &groups[label];
            let n = runs.len() as f64;
            let first = runs[0];
            let mean = |f: &dyn Fn(&IndividualResult) -> usize| -> f64 {
                kamimusuhi_core::genome::round_to_resolution(
                    runs.iter().map(|r| f(r) as f64).sum::<f64>() / n,
                )
            };
            let probed = runs.iter().filter(|r| r.outcome.is_some()).count();
            let successes = runs
                .iter()
                .filter(|r| r.outcome.as_ref().is_some_and(|o| o.success))
                .count();
            Aggregate {
                label: label.to_owned(),
                genome_id: first.genome_id.clone(),
                genome_hash: first.genome_hash.clone(),
                parent_id: first.parent_id.clone(),
                phenotype: first.phenotype.clone(),
                genes_changed_from_parent: first.genes_changed_from_parent.clone(),
                effective_reflection_interval: first.effective_reflection_interval,
                runs: runs.len(),
                completed_runs: runs.iter().filter(|r| r.completed).count(),
                success_rate: (probed > 0).then(|| {
                    kamimusuhi_core::genome::round_to_resolution(successes as f64 / probed as f64)
                }),
                successes,
                mean: MeanMetrics {
                    history_messages: mean(&|r| r.metrics.history_messages),
                    episodic_recalled: mean(&|r| r.metrics.episodic_recalled),
                    relationship_recalled: mean(&|r| r.metrics.relationship_recalled),
                    evidence_recalled: mean(&|r| r.metrics.evidence_recalled),
                    reflections: mean(&|r| r.metrics.reflections),
                    reflection_activations: mean(&|r| r.metrics.reflection_activations),
                    opportunity_turns: mean(&|r| r.exploration.opportunity_turns),
                    exploration_turns_attempted: mean(&|r| {
                        r.exploration.exploration_turns_attempted
                    }),
                    exploration_turns_skipped: mean(&|r| r.exploration.exploration_turns_skipped),
                    candidates_promoted: mean(&|r| r.exploration.candidates_promoted),
                    persona_calls: mean(&|r| r.cost.persona_calls),
                    resource_calls: mean(&|r| r.cost.resource_calls),
                    tool_calls: mean(&|r| r.cost.tool_calls),
                    context_records: mean(&|r| r.cost.context_records),
                    wall_ms: mean(&|r| r.wall_ms as usize),
                },
            }
        })
        .collect()
}

/// The gene → phenotype → branch → resource usage → outcome table, as
/// Markdown. One column per individual; rows are grouped by layer. Rows
/// identical across all individuals are listed separately with their value,
/// so "no difference" is visible too.
pub fn compare_markdown(results: &[IndividualResult]) -> String {
    let aggregates = aggregate(results);
    let header: Vec<String> = aggregates.iter().map(|a| a.label.clone()).collect();
    let mut rows: Vec<(&str, String, Vec<String>)> = Vec::new();
    let fmt = |v: f64| {
        if v.fract() == 0.0 {
            format!("{v:.0}")
        } else {
            format!("{v:.2}")
        }
    };

    rows.push((
        "genome",
        "genome_id".to_owned(),
        aggregates.iter().map(|a| a.genome_id.clone()).collect(),
    ));
    rows.push((
        "genome",
        "hash".to_owned(),
        aggregates
            .iter()
            .map(|a| a.genome_hash.chars().take(19).collect())
            .collect(),
    ));
    rows.push((
        "genome",
        "parent_id".to_owned(),
        aggregates
            .iter()
            .map(|a| a.parent_id.clone().unwrap_or_else(|| "-".to_owned()))
            .collect(),
    ));
    rows.push((
        "genome",
        "runs".to_owned(),
        aggregates.iter().map(|a| a.runs.to_string()).collect(),
    ));

    let mut gene_keys: Vec<String> = aggregates
        .iter()
        .flat_map(|a| a.genes_changed_from_parent.keys().cloned())
        .collect();
    gene_keys.sort();
    gene_keys.dedup();
    let parent_genes = results
        .first()
        .and_then(|r| read_parent_genes(&r.runtime_dir))
        .unwrap_or_default();
    for key in gene_keys {
        rows.push((
            "gene",
            key.clone(),
            aggregates
                .iter()
                .map(|a| {
                    a.genes_changed_from_parent
                        .get(&key)
                        .or_else(|| parent_genes.get(&key))
                        .map_or_else(|| "-".to_owned(), |v| format!("{v:.3}"))
                })
                .collect(),
        ));
    }

    let ph = |f: fn(&Phenotype) -> f64| -> Vec<String> {
        aggregates
            .iter()
            .map(|a| format!("{:.3}", f(&a.phenotype)))
            .collect()
    };
    rows.push((
        "phenotype",
        "exploration_probability".to_owned(),
        ph(|p| p.exploration_probability),
    ));
    rows.push((
        "phenotype",
        "recall_breadth_scale".to_owned(),
        ph(|p| p.recall_breadth_scale),
    ));
    rows.push((
        "phenotype",
        "reflection_interval_scale".to_owned(),
        ph(|p| p.reflection_interval_scale),
    ));
    rows.push((
        "phenotype",
        "context_span_scale".to_owned(),
        ph(|p| p.context_span_scale),
    ));
    rows.push((
        "phenotype",
        "effective_reflection_interval".to_owned(),
        aggregates
            .iter()
            .map(|a| a.effective_reflection_interval.to_string())
            .collect(),
    ));

    let m = |f: fn(&MeanMetrics) -> f64| -> Vec<String> {
        aggregates.iter().map(|a| fmt(f(&a.mean))).collect()
    };
    rows.push((
        "branch",
        "opportunity_turns".to_owned(),
        m(|x| x.opportunity_turns),
    ));
    rows.push((
        "branch",
        "exploration_turns_attempted".to_owned(),
        m(|x| x.exploration_turns_attempted),
    ));
    rows.push((
        "branch",
        "exploration_turns_skipped".to_owned(),
        m(|x| x.exploration_turns_skipped),
    ));
    rows.push((
        "branch",
        "candidates_promoted".to_owned(),
        m(|x| x.candidates_promoted),
    ));
    rows.push((
        "behavior",
        "history_messages".to_owned(),
        m(|x| x.history_messages),
    ));
    rows.push((
        "behavior",
        "episodic_recalled".to_owned(),
        m(|x| x.episodic_recalled),
    ));
    rows.push((
        "behavior",
        "relationship_recalled".to_owned(),
        m(|x| x.relationship_recalled),
    ));
    rows.push((
        "behavior",
        "evidence_recalled".to_owned(),
        m(|x| x.evidence_recalled),
    ));
    rows.push(("behavior", "reflections".to_owned(), m(|x| x.reflections)));
    rows.push((
        "behavior",
        "reflection_activations".to_owned(),
        m(|x| x.reflection_activations),
    ));
    rows.push(("cost", "persona_calls".to_owned(), m(|x| x.persona_calls)));
    rows.push(("cost", "resource_calls".to_owned(), m(|x| x.resource_calls)));
    rows.push(("cost", "tool_calls".to_owned(), m(|x| x.tool_calls)));
    rows.push((
        "cost",
        "context_records".to_owned(),
        m(|x| x.context_records),
    ));
    rows.push((
        "cost",
        "wall_ms (not reproducible)".to_owned(),
        m(|x| x.wall_ms),
    ));
    rows.push((
        "outcome",
        "completed_runs".to_owned(),
        aggregates
            .iter()
            .map(|a| format!("{}/{}", a.completed_runs, a.runs))
            .collect(),
    ));
    rows.push((
        "outcome",
        "successes".to_owned(),
        aggregates
            .iter()
            .map(|a| match a.success_rate {
                Some(_) => format!("{}/{}", a.successes, a.runs),
                None => "-".to_owned(),
            })
            .collect(),
    ));
    rows.push((
        "outcome",
        "success_rate".to_owned(),
        aggregates
            .iter()
            .map(|a| {
                a.success_rate
                    .map_or_else(|| "-".to_owned(), |r| format!("{r:.2}"))
            })
            .collect(),
    ));

    let mut out = String::new();
    out.push_str(&format!("| layer | quantity | {} |\n", header.join(" | ")));
    out.push_str(&format!("|---|---|{}\n", "---|".repeat(header.len())));
    let mut identical = Vec::new();
    for (layer, name, values) in rows {
        let differs = values.windows(2).any(|w| w[0] != w[1]);
        if differs || layer == "genome" || layer == "outcome" {
            out.push_str(&format!("| {layer} | {name} | {} |\n", values.join(" | ")));
        } else {
            identical.push(format!("{name}={}", values[0]));
        }
    }
    if !identical.is_empty() {
        out.push_str(&format!(
            "\nidentical across all individuals: {}\n",
            identical.join(", ")
        ));
    }
    out
}

/// The parent's own gene values, read from its runtime directory so the
/// table can show the baseline next to each changed gene. Best-effort: a
/// missing directory just leaves the baseline column blank.
fn read_parent_genes(runtime_dir: &str) -> Option<BTreeMap<String, f64>> {
    let path = PathBuf::from(runtime_dir).join(Genome::FILE_NAME);
    Genome::load(&path).ok().map(|g| g.genes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn short_config(seed: u64) -> HarnessConfig {
        let mut scenario = Scenario::baseline();
        scenario.truncate(6);
        HarnessConfig {
            seed,
            repeats: 1,
            mutants: 1,
            mutation_rate: 1.0,
            reflection_interval_turns: 2,
            scenario,
        }
    }

    fn extreme(label: &str, genes: &[(&str, f64)]) -> Candidate {
        let mut genome = Genome::with_id(format!("gen-{label}"));
        for (key, value) in genes {
            genome.set_gene(key, *value).unwrap();
        }
        Candidate {
            label: label.to_owned(),
            genome,
        }
    }

    #[test]
    fn population_is_deterministic_and_labelled() {
        let parent = Genome::with_id("gen-p");
        let config = short_config(5);
        let a = population(&parent, &config, explorer_presets(&parent));
        let b = population(&parent, &config, explorer_presets(&parent));
        assert_eq!(a.len(), 4);
        assert_eq!(a[0].label, "parent");
        assert_eq!(a[1].label, "mutant-1");
        assert_eq!(a[1].genome.hash, b[1].genome.hash);
        assert_eq!(a[1].genome.parent_id.as_deref(), Some("gen-p"));
        assert_eq!(a[2].genome.phenotype().exploration_probability, 0.05);
        assert_eq!(a[3].genome.phenotype().exploration_probability, 0.65);
        assert_eq!(a[3].genome.hash, b[3].genome.hash);
    }

    #[test]
    fn same_seed_reproduces_behaviour_metrics() {
        let parent = Genome::with_id("gen-p");
        let mut config = short_config(11);
        config.repeats = 2;
        let candidates = population(&parent, &config, Vec::new());
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let a = run(dir_a.path(), &config, &candidates).unwrap();
        let b = run(dir_b.path(), &config, &candidates).unwrap();
        assert_eq!(a.len(), 4);
        for (x, y) in a.iter().zip(&b) {
            assert!(x.completed, "{x:?}");
            assert_eq!(x.metrics, y.metrics);
            assert_eq!(x.exploration, y.exploration);
            assert_eq!(x.cost, y.cost);
            assert_eq!(x.outcome, y.outcome);
            assert_eq!(x.genome_hash, y.genome_hash);
        }
        assert_eq!(a[0].run_seed, 11);
        assert_eq!(a[1].run_seed, 12);
        let read = read_results(&dir_a.path().join(RESULTS_FILE_NAME)).unwrap();
        assert_eq!(read, a);
    }

    #[test]
    fn gene_differences_produce_behaviour_differences() {
        let parent = Genome::with_id("gen-p");
        let config = short_config(3);
        let candidates = population(
            &parent,
            &config,
            vec![
                extreme(
                    "reflective-narrow",
                    &[
                        ("cognition.reflection_depth", 1.0),
                        ("cognition.working_memory_span", 0.0),
                    ],
                ),
                extreme("shallow", &[("cognition.reflection_depth", 0.0)]),
            ],
        );
        let dir = tempfile::tempdir().unwrap();
        let results = run(dir.path(), &config, &candidates).unwrap();
        assert!(results.iter().all(|r| r.completed), "{results:#?}");
        let by_label = |l: &str| results.iter().find(|r| r.label == l).unwrap();
        let parent = by_label("parent");
        let reflective = by_label("reflective-narrow");
        let shallow = by_label("shallow");
        assert_eq!(parent.effective_reflection_interval, 2);
        assert_eq!(reflective.effective_reflection_interval, 1);
        assert_eq!(shallow.effective_reflection_interval, 3);
        assert!(reflective.metrics.reflections > parent.metrics.reflections);
        assert!(shallow.metrics.reflections < parent.metrics.reflections);
        assert!(reflective.metrics.history_messages < parent.metrics.history_messages);
        assert_eq!(parent.cost.persona_calls, shallow.cost.persona_calls);
        let table = compare_markdown(&results);
        assert!(table.contains("| behavior | reflections |"));
        assert!(table.contains("cognition.reflection_depth"));
    }

    #[test]
    fn exploration_scenario_links_probability_to_branch_and_outcome() {
        let parent = Genome::with_id("gen-p");
        let config = HarnessConfig {
            seed: 100,
            repeats: 6,
            mutants: 0,
            mutation_rate: 1.0,
            reflection_interval_turns: 0,
            scenario: Scenario::exploration(),
        };
        let candidates = population(&parent, &config, explorer_presets(&parent));
        let dir = tempfile::tempdir().unwrap();
        let results = run(dir.path(), &config, &candidates).unwrap();
        assert!(results.iter().all(|r| r.completed), "{results:#?}");
        let aggregates = aggregate(&results);
        let by_label = |l: &str| aggregates.iter().find(|a| a.label == l).unwrap();
        let low = by_label("low-exploration");
        let default = by_label("parent");
        let high = by_label("high-exploration");
        // Every run had opportunities, and they were identical across
        // individuals: the scenario, not the genome, creates them.
        assert!(low.mean.opportunity_turns > 0.0);
        assert_eq!(low.mean.opportunity_turns, high.mean.opportunity_turns);
        // Branch choice follows the probability (common random numbers, so
        // higher p can only add attempts).
        assert!(low.mean.exploration_turns_attempted <= default.mean.exploration_turns_attempted);
        assert!(default.mean.exploration_turns_attempted <= high.mean.exploration_turns_attempted);
        assert!(high.mean.exploration_turns_attempted > low.mean.exploration_turns_attempted);
        // The outcome is decided by the branch on the probe turn, and only by it.
        for r in &results {
            let outcome = r.outcome.as_ref().unwrap();
            let promoted_on_probe = r.exploration.exploration_turns_attempted > 0;
            if outcome.success {
                assert!(promoted_on_probe, "{r:?}");
                assert_eq!(outcome.found_in.as_deref(), Some("recalled_evidence"));
            }
        }
        assert!(high.success_rate.unwrap() > low.success_rate.unwrap());
        // Cost is stored separately: exploring widens the context but never
        // changes how often the individual speaks.
        assert_eq!(low.mean.persona_calls, high.mean.persona_calls);
        assert!(high.mean.context_records > low.mean.context_records);
        let table = compare_markdown(&results);
        assert!(table.contains("| branch | exploration_turns_attempted |"));
        assert!(table.contains("| outcome | success_rate |"));
    }

    #[test]
    fn probe_ignores_the_history_window() {
        let probe = Probe {
            turn_index: 0,
            expected_text: "焼き魚".to_owned(),
        };
        let only_in_history = serde_json::json!({
            "recent_context": [{"text": "焼き魚が一番のごちそうです。"}],
            "recalled_evidence": [],
        });
        assert!(!probe_outcome(&probe, Some(&only_in_history)).success);
        let recalled = serde_json::json!({
            "recalled_evidence": [{"content": {"text": "焼き魚が一番のごちそうです。"}}],
        });
        let outcome = probe_outcome(&probe, Some(&recalled));
        assert!(outcome.success);
        assert_eq!(outcome.found_in.as_deref(), Some("recalled_evidence"));
        assert!(!probe_outcome(&probe, None).success);
    }
}
