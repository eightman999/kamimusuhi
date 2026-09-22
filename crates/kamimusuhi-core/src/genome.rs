//! Genome: long-lived, numeric disposition parameters.
//!
//! A genome is the part of an individual that is *not* learned. It holds no
//! memory, no conversation history, no persona text: only bounded numbers
//! that bias how the runtime behaves (how wide it recalls, how often it
//! reflects, how much it explores). Memory is the record of what happened;
//! the genome is the record of what kind of thing it happened to.
//!
//! Design rules:
//! - Every gene is a `f64` in `[0.0, 1.0]`. Anything else fails validation.
//! - The genome is stable across restarts and across memory rollbacks. It
//!   changes only by an explicit [`Genome::mutate`], which produces a *new*
//!   genome with a new ID, a bumped `version` and a `parent_id` link.
//! - `hash` is a content digest over the genes only. Two genomes with the
//!   same genes have the same hash even if their IDs differ, so a hash
//!   compares dispositions and an ID names an instance.
//! - The runtime never reads genes directly; it reads a [`Phenotype`], the
//!   derived runtime settings. That is the only place gene→behaviour formulas
//!   live.
//!
//! Persistence is JSON (the workspace already depends on `serde_json`; no
//! YAML dependency is added for the MVP).

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::digest::json_digest;

/// Bump when the gene set or its semantics change incompatibly.
pub const GENOME_SCHEMA_VERSION: u32 = 1;

/// Default file name inside a runtime directory.
pub const GENOME_FILE_NAME: &str = "genome.json";

/// Fixed half-width of one mutation step. A mutated gene moves by at most
/// this much in either direction before clamping.
pub const MUTATION_STEP: f64 = 0.15;

/// Genes are stored as multiples of this. Quantizing keeps every value a
/// short decimal that survives JSON text round-trips exactly, so the hash
/// computed in memory is the hash computed from the file.
pub const GENE_RESOLUTION: f64 = 1e-6;

/// Snap any finite value to [`GENE_RESOLUTION`] without clamping. Derived
/// values use this so they, too, survive a JSON text round-trip unchanged.
pub fn round_to_resolution(value: f64) -> f64 {
    (value / GENE_RESOLUTION).round() / (1.0 / GENE_RESOLUTION)
}

/// Clamp into `[0, 1]` and snap to [`GENE_RESOLUTION`].
pub fn quantize_gene(value: f64) -> f64 {
    // Integer micro-units divided by a power of ten yield the double nearest
    // the short decimal, which prints as that decimal and parses back to the
    // same bits. Multiplying by 1e-6 would not: it leaves representation noise.
    let units = (value.clamp(0.0, 1.0) / GENE_RESOLUTION).round();
    (units / (1.0 / GENE_RESOLUTION)).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// Gene groups
// ---------------------------------------------------------------------------

/// Baseline affective style.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Temperament {
    /// Appetite for the unfamiliar. Widens recall and raises exploration.
    pub curiosity: f64,
    /// Preference for the safe path over the fast one.
    pub caution: f64,
    /// Inclination to engage rather than withdraw.
    pub sociability: f64,
    /// Tolerance for slow or repetitive interaction.
    pub patience: f64,
    /// How much internal state shows in output.
    pub expressiveness: f64,
}

impl Default for Temperament {
    fn default() -> Self {
        Self {
            curiosity: 0.5,
            caution: 0.5,
            sociability: 0.5,
            patience: 0.5,
            expressiveness: 0.5,
        }
    }
}

/// Thinking style.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cognition {
    /// How often and how deeply the individual reflects on experience.
    pub reflection_depth: f64,
    /// Preference for general patterns over concrete detail.
    pub abstraction: f64,
    /// Tolerance for imprecision. High = insists on exactness.
    pub precision: f64,
    /// How much recent context is kept in view at once.
    pub working_memory_span: f64,
    /// Willingness to link distant ideas.
    pub associativity: f64,
}

impl Default for Cognition {
    fn default() -> Self {
        Self {
            reflection_depth: 0.5,
            abstraction: 0.5,
            precision: 0.5,
            working_memory_span: 0.5,
            associativity: 0.5,
        }
    }
}

/// What the individual is pulled toward.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Drives {
    /// Reward from new stimuli. Raises exploration.
    pub novelty_seeking: f64,
    /// Reward from getting better at something.
    pub mastery: f64,
    /// Reward from connection with others.
    pub affiliation: f64,
    /// Reward from acting on its own judgement.
    pub autonomy: f64,
    /// Reward from structure and predictability.
    pub order: f64,
}

impl Default for Drives {
    fn default() -> Self {
        Self {
            novelty_seeking: 0.5,
            mastery: 0.5,
            affiliation: 0.5,
            autonomy: 0.5,
            order: 0.5,
        }
    }
}

/// How the individual changes over time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Adaptation {
    /// How readily new experience reshapes derived state.
    pub plasticity: f64,
    /// Preference for consolidating over discarding.
    pub consolidation_bias: f64,
    /// How fast unused material fades.
    pub forgetting_rate: f64,
    /// Willingness to accept a change that might be wrong.
    pub risk_tolerance: f64,
    /// Resistance of the genome itself to mutation. Scales mutation steps
    /// down; it never blocks them.
    pub mutation_resistance: f64,
}

impl Default for Adaptation {
    fn default() -> Self {
        Self {
            plasticity: 0.5,
            consolidation_bias: 0.5,
            forgetting_rate: 0.5,
            risk_tolerance: 0.5,
            mutation_resistance: 0.5,
        }
    }
}

// ---------------------------------------------------------------------------
// Genome
// ---------------------------------------------------------------------------

/// The complete heritable parameter set.
///
/// Identity (`genome_id`, `version`, `parent_id`) and the `hash` are metadata;
/// the four gene groups are the content. `hash` is recomputed on every
/// construction path and verified on load, so a hand-edited file that forgot
/// to update it is rejected rather than silently trusted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Genome {
    pub schema_version: u32,
    /// Instance name. New on every `clone`/`mutate`.
    pub genome_id: String,
    /// Generation counter: 1 for a fresh genome, +1 per mutation. Cloning
    /// keeps the version (it is the same generation, a different instance).
    pub version: u32,
    /// The genome this one was cloned or mutated from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// `sha256:` digest over the gene groups only.
    pub hash: String,
    pub temperament: Temperament,
    pub cognition: Cognition,
    pub drives: Drives,
    pub adaptation: Adaptation,
}

/// Why a genome was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GenomeError {
    #[error("genome schema version {found} is not supported (this build reads {supported})")]
    SchemaVersion { found: u32, supported: u32 },
    #[error("gene {key} = {value} is outside [0.0, 1.0]")]
    GeneOutOfRange { key: String, value: String },
    #[error("unknown gene {0}")]
    UnknownGene(String),
    #[error("genome hash mismatch: file says {found}, genes hash to {computed}")]
    HashMismatch { found: String, computed: String },
    #[error("genome_id is empty")]
    EmptyId,
    #[error("genome at {path}: {message}")]
    Io { path: String, message: String },
    #[error("genome at {path} is malformed: {message}")]
    Malformed { path: String, message: String },
}

/// Names every gene in a fixed order. The order is the on-disk and `diff`
/// order, so it must not be reshuffled casually.
pub const GENE_KEYS: [&str; 20] = [
    "temperament.curiosity",
    "temperament.caution",
    "temperament.sociability",
    "temperament.patience",
    "temperament.expressiveness",
    "cognition.reflection_depth",
    "cognition.abstraction",
    "cognition.precision",
    "cognition.working_memory_span",
    "cognition.associativity",
    "drives.novelty_seeking",
    "drives.mastery",
    "drives.affiliation",
    "drives.autonomy",
    "drives.order",
    "adaptation.plasticity",
    "adaptation.consolidation_bias",
    "adaptation.forgetting_rate",
    "adaptation.risk_tolerance",
    "adaptation.mutation_resistance",
];

impl Genome {
    pub const FILE_NAME: &'static str = GENOME_FILE_NAME;

    /// A fresh, all-0.5 genome with a random ID.
    pub fn new_default() -> Self {
        Self::with_id(random_genome_id())
    }

    /// A fresh default genome with an explicit ID (deterministic tests, or
    /// an operator-chosen name).
    pub fn with_id(genome_id: impl Into<String>) -> Self {
        let mut genome = Self {
            schema_version: GENOME_SCHEMA_VERSION,
            genome_id: genome_id.into(),
            version: 1,
            parent_id: None,
            hash: String::new(),
            temperament: Temperament::default(),
            cognition: Cognition::default(),
            drives: Drives::default(),
            adaptation: Adaptation::default(),
        };
        genome.refresh_hash();
        genome
    }

    /// A genome with every gene drawn uniformly from `[0, 1]`, deterministic
    /// in `seed`. For seeding a diverse founding population.
    pub fn random_with_id(seed: u64, genome_id: impl Into<String>) -> Self {
        let mut genome = Self::with_id(genome_id);
        let mut rng = SplitMix64::new(seed);
        for key in GENE_KEYS {
            genome
                .set_gene(key, rng.next_unit())
                .expect("GENE_KEYS is exhaustive");
        }
        genome
    }

    /// Digest of the gene groups only: identity metadata is excluded, so a
    /// clone hashes equal to its parent and a mutation hashes differently.
    pub fn compute_hash(&self) -> String {
        json_digest(&serde_json::json!({
            "schema_version": self.schema_version,
            "temperament": self.temperament,
            "cognition": self.cognition,
            "drives": self.drives,
            "adaptation": self.adaptation,
        }))
    }

    /// Recompute `hash` after editing genes.
    pub fn refresh_hash(&mut self) {
        self.hash = self.compute_hash();
    }

    /// Range, schema and hash checks. Called on every load.
    pub fn validate(&self) -> Result<(), GenomeError> {
        if self.schema_version != GENOME_SCHEMA_VERSION {
            return Err(GenomeError::SchemaVersion {
                found: self.schema_version,
                supported: GENOME_SCHEMA_VERSION,
            });
        }
        if self.genome_id.is_empty() {
            return Err(GenomeError::EmptyId);
        }
        for (key, value) in self.genes() {
            if !(0.0..=1.0).contains(&value) || value.is_nan() {
                return Err(GenomeError::GeneOutOfRange {
                    key,
                    value: value.to_string(),
                });
            }
        }
        let computed = self.compute_hash();
        if self.hash != computed {
            return Err(GenomeError::HashMismatch {
                found: self.hash.clone(),
                computed,
            });
        }
        Ok(())
    }

    /// Every gene as `group.name → value`, in [`GENE_KEYS`] order.
    pub fn genes(&self) -> BTreeMap<String, f64> {
        GENE_KEYS
            .iter()
            .map(|key| {
                (
                    (*key).to_owned(),
                    self.gene(key).expect("GENE_KEYS is exhaustive"),
                )
            })
            .collect()
    }

    /// Read one gene by `group.name`.
    pub fn gene(&self, key: &str) -> Result<f64, GenomeError> {
        Ok(match key {
            "temperament.curiosity" => self.temperament.curiosity,
            "temperament.caution" => self.temperament.caution,
            "temperament.sociability" => self.temperament.sociability,
            "temperament.patience" => self.temperament.patience,
            "temperament.expressiveness" => self.temperament.expressiveness,
            "cognition.reflection_depth" => self.cognition.reflection_depth,
            "cognition.abstraction" => self.cognition.abstraction,
            "cognition.precision" => self.cognition.precision,
            "cognition.working_memory_span" => self.cognition.working_memory_span,
            "cognition.associativity" => self.cognition.associativity,
            "drives.novelty_seeking" => self.drives.novelty_seeking,
            "drives.mastery" => self.drives.mastery,
            "drives.affiliation" => self.drives.affiliation,
            "drives.autonomy" => self.drives.autonomy,
            "drives.order" => self.drives.order,
            "adaptation.plasticity" => self.adaptation.plasticity,
            "adaptation.consolidation_bias" => self.adaptation.consolidation_bias,
            "adaptation.forgetting_rate" => self.adaptation.forgetting_rate,
            "adaptation.risk_tolerance" => self.adaptation.risk_tolerance,
            "adaptation.mutation_resistance" => self.adaptation.mutation_resistance,
            other => return Err(GenomeError::UnknownGene(other.to_owned())),
        })
    }

    /// Write one gene by `group.name`. The value is clamped into `[0, 1]`
    /// and the hash refreshed.
    pub fn set_gene(&mut self, key: &str, value: f64) -> Result<(), GenomeError> {
        if value.is_nan() {
            return Err(GenomeError::GeneOutOfRange {
                key: key.to_owned(),
                value: "NaN".to_owned(),
            });
        }
        let value = quantize_gene(value);
        let slot = match key {
            "temperament.curiosity" => &mut self.temperament.curiosity,
            "temperament.caution" => &mut self.temperament.caution,
            "temperament.sociability" => &mut self.temperament.sociability,
            "temperament.patience" => &mut self.temperament.patience,
            "temperament.expressiveness" => &mut self.temperament.expressiveness,
            "cognition.reflection_depth" => &mut self.cognition.reflection_depth,
            "cognition.abstraction" => &mut self.cognition.abstraction,
            "cognition.precision" => &mut self.cognition.precision,
            "cognition.working_memory_span" => &mut self.cognition.working_memory_span,
            "cognition.associativity" => &mut self.cognition.associativity,
            "drives.novelty_seeking" => &mut self.drives.novelty_seeking,
            "drives.mastery" => &mut self.drives.mastery,
            "drives.affiliation" => &mut self.drives.affiliation,
            "drives.autonomy" => &mut self.drives.autonomy,
            "drives.order" => &mut self.drives.order,
            "adaptation.plasticity" => &mut self.adaptation.plasticity,
            "adaptation.consolidation_bias" => &mut self.adaptation.consolidation_bias,
            "adaptation.forgetting_rate" => &mut self.adaptation.forgetting_rate,
            "adaptation.risk_tolerance" => &mut self.adaptation.risk_tolerance,
            "adaptation.mutation_resistance" => &mut self.adaptation.mutation_resistance,
            other => return Err(GenomeError::UnknownGene(other.to_owned())),
        };
        *slot = value;
        self.refresh_hash();
        Ok(())
    }

    /// Same genes, same version, new instance. The clone records this genome
    /// as its parent and keeps the same hash.
    pub fn clone_genome(&self) -> Self {
        self.clone_genome_with_id(random_genome_id())
    }

    /// [`Self::clone_genome`] with a chosen ID.
    pub fn clone_genome_with_id(&self, genome_id: impl Into<String>) -> Self {
        let mut child = self.clone();
        child.genome_id = genome_id.into();
        child.parent_id = Some(self.genome_id.clone());
        child.refresh_hash();
        child
    }

    /// Deterministic mutation: each gene independently mutates with
    /// probability `rate`, moving by a uniform step in
    /// `±MUTATION_STEP × (1 − mutation_resistance)`, then clamped.
    ///
    /// The same `(genome, seed, rate)` always yields the same child. The
    /// child has a new ID, `version + 1`, and `parent_id` set.
    pub fn mutate(&self, seed: u64, rate: f64) -> Self {
        self.mutate_with_id(seed, rate, random_genome_id())
    }

    /// [`Self::mutate`] with a chosen child ID.
    pub fn mutate_with_id(&self, seed: u64, rate: f64, genome_id: impl Into<String>) -> Self {
        let rate = if rate.is_nan() {
            0.0
        } else {
            rate.clamp(0.0, 1.0)
        };
        let step = MUTATION_STEP * (1.0 - self.adaptation.mutation_resistance);
        let mut rng = SplitMix64::new(seed);
        let mut child = self.clone();
        for key in GENE_KEYS {
            // Draw both numbers every time so a gene's outcome does not depend
            // on whether earlier genes mutated.
            let roll = rng.next_unit();
            let offset = (rng.next_unit() * 2.0 - 1.0) * step;
            if roll < rate {
                let current = child.gene(key).expect("GENE_KEYS is exhaustive");
                child
                    .set_gene(key, current + offset)
                    .expect("GENE_KEYS is exhaustive");
            }
        }
        child.genome_id = genome_id.into();
        child.version = self.version.saturating_add(1);
        child.parent_id = Some(self.genome_id.clone());
        child.refresh_hash();
        child
    }

    /// Runtime settings derived from the genes. See [`Phenotype`].
    pub fn phenotype(&self) -> Phenotype {
        Phenotype::from_genome(self)
    }

    // -- persistence --------------------------------------------------------

    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("genome serializes");
        text.push('\n');
        text
    }

    /// Parse and validate. A `hash` that does not match the genes is an
    /// error, not a warning.
    pub fn from_json(text: &str) -> Result<Self, GenomeError> {
        let genome: Self = serde_json::from_str(text).map_err(|source| GenomeError::Malformed {
            path: "<string>".to_owned(),
            message: source.to_string(),
        })?;
        genome.validate()?;
        Ok(genome)
    }

    pub fn load(path: &Path) -> Result<Self, GenomeError> {
        let text = std::fs::read_to_string(path).map_err(|source| GenomeError::Io {
            path: path.display().to_string(),
            message: source.to_string(),
        })?;
        Self::from_json(&text).map_err(|error| match error {
            GenomeError::Malformed { message, .. } => GenomeError::Malformed {
                path: path.display().to_string(),
                message,
            },
            other => other,
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), GenomeError> {
        self.validate()?;
        std::fs::write(path, self.to_json()).map_err(|source| GenomeError::Io {
            path: path.display().to_string(),
            message: source.to_string(),
        })
    }
}

/// `gen-` plus 32 lowercase hex characters from a random UUID.
fn random_genome_id() -> String {
    format!("gen-{}", uuid::Uuid::new_v4().simple())
}

// ---------------------------------------------------------------------------
// Diff
// ---------------------------------------------------------------------------

/// One gene's values on each side of a comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneDelta {
    pub key: String,
    pub a: f64,
    pub b: f64,
    /// `b − a`.
    pub delta: f64,
}

/// Result of [`diff`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenomeDiff {
    pub a_id: String,
    pub b_id: String,
    pub same_hash: bool,
    /// Genes whose values differ, in [`GENE_KEYS`] order.
    pub changed: Vec<GeneDelta>,
    /// Euclidean distance over all genes.
    pub distance: f64,
}

/// Compare two genomes gene by gene.
pub fn diff(a: &Genome, b: &Genome) -> GenomeDiff {
    let mut changed = Vec::new();
    let mut sum_sq = 0.0;
    for key in GENE_KEYS {
        let va = a.gene(key).expect("GENE_KEYS is exhaustive");
        let vb = b.gene(key).expect("GENE_KEYS is exhaustive");
        let delta = vb - va;
        sum_sq += delta * delta;
        if delta != 0.0 {
            changed.push(GeneDelta {
                key: key.to_owned(),
                a: va,
                b: vb,
                delta,
            });
        }
    }
    GenomeDiff {
        a_id: a.genome_id.clone(),
        b_id: b.genome_id.clone(),
        same_hash: a.hash == b.hash,
        changed,
        distance: sum_sq.sqrt(),
    }
}

// ---------------------------------------------------------------------------
// Phenotype
// ---------------------------------------------------------------------------

/// Runtime settings derived from a genome.
///
/// This is the only place genes are turned into behaviour. Every field is
/// documented with its formula so a reader can predict the effect of a
/// gene edit without running anything. The runtime consumes these values;
/// it never reads genes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Phenotype {
    pub genome_id: String,
    pub genome_hash: String,
    /// Chance, per opportunity, of preferring an unfamiliar option over the
    /// known-good one. `0.05 + 0.6 × mean(curiosity, novelty_seeking)`.
    pub exploration_probability: f64,
    /// Multiplier on recall breadth (episodic / evidence top-k).
    /// `0.5 + curiosity`, so a default genome leaves top-k unchanged.
    pub recall_breadth_scale: f64,
    /// Multiplier on the configured reflection interval. `1.5 − reflection_depth`,
    /// so a default genome leaves the interval unchanged, depth 1.0 halves
    /// it and depth 0.0 makes it 1.5× longer.
    pub reflection_interval_scale: f64,
    /// Willingness to accept an uncertain change. `mean(risk_tolerance, plasticity)`.
    pub change_acceptance: f64,
    /// How strongly to prefer consolidation over forgetting.
    /// `consolidation_bias − forgetting_rate`, in `[-1, 1]`.
    pub retention_bias: f64,
    /// Multiplier on conversation history kept in view. `0.5 + working_memory_span`.
    pub context_span_scale: f64,
}

/// One exploration draw: the probability in force, the number drawn and
/// what it decided. Recorded so a trace can show *why* a branch was taken.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ExplorationDecision {
    pub probability: f64,
    /// Uniform in `[0, 1)`, derived from `key` alone.
    pub draw: f64,
    pub explore: bool,
}

impl Phenotype {
    /// Decide whether to explore at one opportunity.
    ///
    /// The draw is a pure function of `key` (for a dialogue turn, the turn
    /// ID), never of the genome: two individuals facing the same opportunity
    /// with the same key see the same number, and differ only in whether
    /// their `exploration_probability` clears it. That is what makes an
    /// exploration difference between genomes attributable to the genome.
    pub fn exploration_decision(&self, key: u64) -> ExplorationDecision {
        let draw = round_to_resolution(SplitMix64::new(key).next_unit());
        ExplorationDecision {
            probability: self.exploration_probability,
            draw,
            explore: draw < self.exploration_probability,
        }
    }

    pub fn from_genome(genome: &Genome) -> Self {
        let t = &genome.temperament;
        let c = &genome.cognition;
        let d = &genome.drives;
        let a = &genome.adaptation;
        let r = round_to_resolution;
        Self {
            genome_id: genome.genome_id.clone(),
            genome_hash: genome.hash.clone(),
            exploration_probability: r(
                (0.05 + 0.6 * ((t.curiosity + d.novelty_seeking) / 2.0)).clamp(0.0, 1.0)
            ),
            recall_breadth_scale: r(0.5 + t.curiosity),
            reflection_interval_scale: r(1.5 - c.reflection_depth),
            change_acceptance: r((a.risk_tolerance + a.plasticity) / 2.0),
            retention_bias: r(a.consolidation_bias - a.forgetting_rate),
            context_span_scale: r(0.5 + c.working_memory_span),
        }
    }

    /// Apply `reflection_interval_scale` to a configured interval.
    ///
    /// `0` stays `0`: an operator who disabled automatic reflection keeps it
    /// disabled. Any positive interval stays at least `1`.
    pub fn reflection_interval(&self, configured_turns: u32) -> u32 {
        if configured_turns == 0 {
            return 0;
        }
        scale_count(configured_turns as usize, self.reflection_interval_scale).max(1) as u32
    }

    /// Apply `context_span_scale` to the configured conversation-history
    /// window. Never below 1: a turn always sees at least its predecessor.
    pub fn history_messages(&self, configured: usize) -> usize {
        scale_count(configured.max(1), self.context_span_scale).max(1)
    }

    /// Apply `recall_breadth_scale` to a top-k. `0` stays `0` (that domain
    /// is switched off); any positive count stays at least `1`.
    pub fn recall_top_k(&self, configured: usize) -> usize {
        if configured == 0 {
            return 0;
        }
        scale_count(configured, self.recall_breadth_scale).max(1)
    }
}

/// Round-half-up scaling of a count, never below zero.
fn scale_count(count: usize, scale: f64) -> usize {
    let scaled = (count as f64) * scale;
    if scaled <= 0.0 {
        0
    } else {
        (scaled + 0.5).floor() as usize
    }
}

// ---------------------------------------------------------------------------
// PRNG
// ---------------------------------------------------------------------------

/// SplitMix64: tiny, deterministic, and good enough for mutation draws. No
/// external crate, so `kamimusuhi-core`'s dependency boundary is untouched.
struct SplitMix64(u64);

impl SplitMix64 {
    const fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    fn next_unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

impl fmt::Display for Genome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} v{} {}",
            self.genome_id,
            self.version,
            &self.hash[..self.hash.len().min(19)]
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed() -> Genome {
        Genome::with_id("gen-test")
    }

    #[test]
    fn default_is_valid_and_all_half() {
        let g = fixed();
        g.validate().unwrap();
        assert_eq!(g.version, 1);
        assert!(g.genes().values().all(|v| (*v - 0.5).abs() < f64::EPSILON));
        assert_eq!(g.genes().len(), GENE_KEYS.len());
    }

    #[test]
    fn hash_covers_genes_not_identity() {
        let a = fixed();
        let b = Genome::with_id("gen-other");
        assert_eq!(a.hash, b.hash);
        let mut c = fixed();
        c.set_gene("temperament.curiosity", 0.9).unwrap();
        assert_ne!(a.hash, c.hash);
        assert!(c.hash.starts_with("sha256:"));
    }

    #[test]
    fn set_gene_clamps_and_rejects_unknown() {
        let mut g = fixed();
        g.set_gene("drives.order", 7.0).unwrap();
        assert_eq!(g.gene("drives.order").unwrap(), 1.0);
        g.set_gene("drives.order", -3.0).unwrap();
        assert_eq!(g.gene("drives.order").unwrap(), 0.0);
        assert!(matches!(
            g.set_gene("drives.nope", 0.5),
            Err(GenomeError::UnknownGene(_))
        ));
        assert!(g.set_gene("drives.order", f64::NAN).is_err());
    }

    #[test]
    fn clone_keeps_genes_version_and_hash_but_not_id() {
        let parent = fixed();
        let child = parent.clone_genome_with_id("gen-child");
        assert_eq!(child.genome_id, "gen-child");
        assert_eq!(child.parent_id.as_deref(), Some("gen-test"));
        assert_eq!(child.version, parent.version);
        assert_eq!(child.hash, parent.hash);
        assert!(diff(&parent, &child).changed.is_empty());
        let random = parent.clone_genome();
        assert_ne!(random.genome_id, parent.genome_id);
        assert!(random.genome_id.starts_with("gen-"));
    }

    #[test]
    fn mutate_is_deterministic_and_bounded() {
        let parent = fixed();
        let a = parent.mutate_with_id(42, 1.0, "gen-a");
        let b = parent.mutate_with_id(42, 1.0, "gen-b");
        assert_eq!(a.genes(), b.genes());
        assert_eq!(a.hash, b.hash);
        assert_eq!(a.version, 2);
        assert_eq!(a.parent_id.as_deref(), Some("gen-test"));
        a.validate().unwrap();
        let d = diff(&parent, &a);
        assert!(!d.changed.is_empty());
        assert!(!d.same_hash);
        for delta in &d.changed {
            // step = 0.15 × (1 − 0.5)
            assert!(
                delta.delta.abs() <= MUTATION_STEP * 0.5 + 1e-12,
                "{delta:?}"
            );
        }
        let other_seed = parent.mutate_with_id(43, 1.0, "gen-c");
        assert_ne!(other_seed.genes(), a.genes());
    }

    #[test]
    fn mutate_rate_zero_changes_nothing_but_lineage() {
        let parent = fixed();
        let child = parent.mutate_with_id(1, 0.0, "gen-z");
        assert_eq!(child.genes(), parent.genes());
        assert_eq!(child.hash, parent.hash);
        assert_eq!(child.version, 2);
    }

    #[test]
    fn mutation_resistance_shrinks_steps() {
        let mut stiff = fixed();
        stiff
            .set_gene("adaptation.mutation_resistance", 1.0)
            .unwrap();
        let child = stiff.mutate_with_id(7, 1.0, "gen-s");
        assert_eq!(child.genes(), stiff.genes());
    }

    #[test]
    fn diff_reports_changed_genes_and_distance() {
        let a = fixed();
        let mut b = fixed();
        b.set_gene("temperament.curiosity", 0.8).unwrap();
        b.set_gene("cognition.reflection_depth", 0.1).unwrap();
        let d = diff(&a, &b);
        assert_eq!(d.changed.len(), 2);
        assert_eq!(d.changed[0].key, "temperament.curiosity");
        assert!((d.changed[0].delta - 0.3).abs() < 1e-9);
        assert_eq!(d.changed[1].key, "cognition.reflection_depth");
        assert!((d.distance - (0.09f64 + 0.16).sqrt()).abs() < 1e-9);
        assert!(!d.same_hash);
    }

    #[test]
    fn json_round_trip_and_hash_verification() {
        let g = fixed();
        let text = g.to_json();
        let back = Genome::from_json(&text).unwrap();
        assert_eq!(back, g);

        let tampered = text.replace("\"curiosity\": 0.5", "\"curiosity\": 0.9");
        assert!(matches!(
            Genome::from_json(&tampered),
            Err(GenomeError::HashMismatch { .. })
        ));
        let out_of_range = text.replace("\"curiosity\": 0.5", "\"curiosity\": 1.5");
        assert!(matches!(
            Genome::from_json(&out_of_range),
            Err(GenomeError::GeneOutOfRange { .. })
        ));
        let wrong_schema = text.replace("\"schema_version\": 1", "\"schema_version\": 99");
        assert!(matches!(
            Genome::from_json(&wrong_schema),
            Err(GenomeError::SchemaVersion { .. })
        ));
    }

    #[test]
    fn mutated_genomes_survive_text_round_trip() {
        let parent = fixed();
        for seed in 0..200u64 {
            let child = parent.mutate_with_id(seed, 1.0, "gen-rt");
            let back =
                Genome::from_json(&child.to_json()).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
            assert_eq!(back, child);
        }
    }

    #[test]
    fn genes_are_quantized() {
        let mut g = fixed();
        g.set_gene("temperament.curiosity", 0.123_456_789).unwrap();
        assert_eq!(g.gene("temperament.curiosity").unwrap(), 0.123_457);
        assert_eq!(quantize_gene(2.0), 1.0);
        assert_eq!(quantize_gene(-1.0), 0.0);
    }

    #[test]
    fn random_genome_is_seeded_and_valid() {
        let a = Genome::random_with_id(9, "gen-r");
        let b = Genome::random_with_id(9, "gen-r");
        let c = Genome::random_with_id(10, "gen-r");
        a.validate().unwrap();
        assert_eq!(a, b);
        assert_ne!(a.hash, c.hash);
        assert!(a.genes().values().any(|v| (*v - 0.5).abs() > 0.1));
    }

    #[test]
    fn genome_holds_no_memory_fields() {
        // The serialized shape is exactly metadata + the four gene groups.
        let value: serde_json::Value = serde_json::from_str(&fixed().to_json()).unwrap();
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "adaptation",
                "cognition",
                "drives",
                "genome_id",
                "hash",
                "schema_version",
                "temperament",
                "version",
            ]
        );
    }

    #[test]
    fn phenotype_default_is_neutral() {
        let p = fixed().phenotype();
        assert!((p.recall_breadth_scale - 1.0).abs() < 1e-12);
        assert!((p.reflection_interval_scale - 1.0).abs() < 1e-12);
        assert!((p.exploration_probability - 0.35).abs() < 1e-12);
        assert_eq!(p.reflection_interval(0), 0);
        assert_eq!(p.reflection_interval(4), 4);
        assert_eq!(p.recall_top_k(0), 0);
        assert_eq!(p.recall_top_k(2), 2);
        assert_eq!(p.history_messages(12), 12);
    }

    #[test]
    fn phenotype_survives_text_round_trip() {
        let mut g = fixed();
        g.set_gene("temperament.curiosity", 0.333_333).unwrap();
        g.set_gene("drives.novelty_seeking", 0.123_457).unwrap();
        let p = g.phenotype();
        let text = serde_json::to_string(&p).unwrap();
        let back: Phenotype = serde_json::from_str(&text).unwrap();
        assert_eq!(back, p);
        assert_eq!(p.exploration_probability, 0.187_037);
    }

    #[test]
    fn exploration_decision_is_keyed_not_genome_seeded() {
        let low = {
            let mut g = fixed();
            g.set_gene("temperament.curiosity", 0.0).unwrap();
            g.set_gene("drives.novelty_seeking", 0.0).unwrap();
            g.phenotype()
        };
        let high = {
            let mut g = fixed();
            g.set_gene("temperament.curiosity", 1.0).unwrap();
            g.set_gene("drives.novelty_seeking", 1.0).unwrap();
            g.phenotype()
        };
        assert_eq!(low.exploration_probability, 0.05);
        assert_eq!(high.exploration_probability, 0.65);
        let mut explored_low = 0;
        let mut explored_high = 0;
        for key in 0..1000u64 {
            let a = low.exploration_decision(key);
            let b = high.exploration_decision(key);
            assert_eq!(a.draw, b.draw, "same key, same draw");
            assert_eq!(a, low.exploration_decision(key), "deterministic");
            assert!((0.0..1.0).contains(&a.draw));
            explored_low += usize::from(a.explore);
            explored_high += usize::from(b.explore);
            // A higher probability never explores less at the same key.
            assert!(!a.explore || b.explore);
        }
        assert!((30..80).contains(&explored_low), "{explored_low}");
        assert!((600..700).contains(&explored_high), "{explored_high}");
    }

    #[test]
    fn phenotype_tracks_working_memory_span() {
        let mut g = fixed();
        g.set_gene("cognition.working_memory_span", 0.0).unwrap();
        assert_eq!(g.phenotype().history_messages(12), 6);
        assert_eq!(g.phenotype().history_messages(1), 1);
        g.set_gene("cognition.working_memory_span", 1.0).unwrap();
        assert_eq!(g.phenotype().history_messages(12), 18);
    }

    #[test]
    fn phenotype_tracks_curiosity_and_reflection_depth() {
        let mut g = fixed();
        g.set_gene("temperament.curiosity", 1.0).unwrap();
        g.set_gene("cognition.reflection_depth", 1.0).unwrap();
        let p = g.phenotype();
        assert_eq!(p.recall_top_k(2), 3); // 2 × 1.5
        assert_eq!(p.reflection_interval(4), 2); // 4 × 0.5
        g.set_gene("temperament.curiosity", 0.0).unwrap();
        g.set_gene("cognition.reflection_depth", 0.0).unwrap();
        let p = g.phenotype();
        assert_eq!(p.recall_top_k(2), 1); // 2 × 0.5
        assert_eq!(p.recall_top_k(1), 1); // never below 1 when enabled
        assert_eq!(p.reflection_interval(4), 6); // 4 × 1.5
        assert_eq!(p.reflection_interval(1), 2);
    }
}
