//! Cognitive resource routing.
//!
//! Phase 1 had one resource in one slot, chosen by a human editing a config
//! file. There was no way to say "this must not leave the machine", or "this
//! is trivial, do not spend a remote call on it". This module is where that
//! decision lives, and it is a *policy* boundary in the same sense as
//! [`crate::mutation::MutationPolicyV0`]: deterministic, giving a reason for
//! every outcome, and refusing rather than guessing.
//!
//! Three properties are load-bearing.
//!
//! **Deterministic.** Same request, same candidates, same decision. No
//! scoring model, no learning, no randomness, no dependence on wall time.
//! A routing bug has to be reproducible, and a route that varies run to run
//! makes every downstream difference unattributable.
//!
//! **Refusing, not downgrading.** A [`PrivacyConstraint::LocalOnly`] request
//! with no local candidate is an error. It is never quietly satisfied by an
//! external resource — sending data off the machine because the preferred
//! option was busy is precisely the failure this type exists to prevent.
//!
//! **Declared, not discovered.** [`ResourceCapabilities`] is what an operator
//! asserts about a resource, on the same trust footing as the endpoint URL
//! they configured. A resource claiming to be local does not make it local.
//! Nothing here probes or measures; nothing here believes a provider about
//! itself.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ids::ResourceId;
use crate::mutation::UnknownVocabulary;
use crate::resources::{ResourceDescriptor, ResourceSlot};

/// Where a resource physically runs, and therefore where data sent to it goes.
///
/// Ordered least to most exposed, so "at most this exposed" is a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalityClass {
    /// Runs inside this process. Nothing leaves it.
    InProcess,
    /// A separate process on this machine.
    LocalHost,
    /// A machine the operator controls, reachable without the public internet.
    LocalNetwork,
    /// A third-party service. Data sent here has left the operator's control.
    External,
}

impl LocalityClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InProcess => "in_process",
            Self::LocalHost => "local_host",
            Self::LocalNetwork => "local_network",
            Self::External => "external",
        }
    }

    /// Whether data sent here stays under the operator's control.
    pub const fn is_local(self) -> bool {
        matches!(self, Self::InProcess | Self::LocalHost | Self::LocalNetwork)
    }
}

impl fmt::Display for LocalityClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LocalityClass {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "in_process" => Self::InProcess,
            "local_host" => Self::LocalHost,
            "local_network" => Self::LocalNetwork,
            "external" => Self::External,
            other => return Err(UnknownVocabulary::new("locality_class", other)),
        })
    }
}

/// What kind of material a resource handles.
///
/// One variant today. The field exists so that adding audio or vision later is
/// a new variant rather than a schema change to everything that routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    Text,
}

impl Modality {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
        }
    }
}

impl fmt::Display for Modality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How quickly a resource is expected to answer. Ordered fastest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LatencyClass {
    /// Returns without waiting on anything outside the process.
    Instant,
    /// Fast enough for an interactive turn.
    Fast,
    /// Acceptable only when nothing is waiting on it.
    Slow,
}

impl LatencyClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Instant => "instant",
            Self::Fast => "fast",
            Self::Slow => "slow",
        }
    }
}

impl fmt::Display for LatencyClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What using a resource costs. Ordered cheapest first, so a budget is a
/// comparison rather than a lookup table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostClass {
    Free,
    Low,
    High,
}

impl CostClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Low => "low",
            Self::High => "high",
        }
    }
}

impl fmt::Display for CostClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How capable a resource is claimed to be. Ordered weakest first.
///
/// Declared by configuration. Kamimusuhi does not benchmark providers, and a
/// tier here is an operator's assertion, not a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityTier {
    /// Fixtures and trivial transforms.
    Basic,
    Standard,
    High,
}

impl QualityTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Basic => "basic",
            Self::Standard => "standard",
            Self::High => "high",
        }
    }
}

impl fmt::Display for QualityTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a resource is usable right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Healthy,
    /// Usable, but only when nothing better qualifies.
    Degraded,
    Unavailable,
}

impl HealthState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unavailable => "unavailable",
        }
    }

    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Healthy | Self::Degraded)
    }
}

impl fmt::Display for HealthState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a resource is declared to be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceCapabilities {
    pub locality: LocalityClass,
    pub modalities: BTreeSet<Modality>,
    /// Largest input this resource accepts, in the same unit a
    /// [`RoutingRequest::context_size`] is expressed in.
    pub context_capacity: u32,
    pub latency: LatencyClass,
    pub cost: CostClass,
    pub quality: QualityTier,
    pub health: HealthState,
}

impl ResourceCapabilities {
    /// A deterministic in-process fixture: local, free, instant, basic.
    pub fn in_process_fixture() -> Self {
        Self {
            locality: LocalityClass::InProcess,
            modalities: [Modality::Text].into_iter().collect(),
            context_capacity: 8_192,
            latency: LatencyClass::Instant,
            cost: CostClass::Free,
            quality: QualityTier::Basic,
            health: HealthState::Healthy,
        }
    }

    pub fn supports(&self, modality: Modality) -> bool {
        self.modalities.contains(&modality)
    }
}

/// What kind of work is being delegated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskClass {
    Summarize,
    Generate,
    Classify,
}

impl TaskClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Summarize => "summarize",
            Self::Generate => "generate",
            Self::Classify => "classify",
        }
    }
}

impl fmt::Display for TaskClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TaskClass {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "summarize" => Self::Summarize,
            "generate" => Self::Generate,
            "classify" => Self::Classify,
            other => return Err(UnknownVocabulary::new("task_class", other)),
        })
    }
}

/// How far the material for this task may travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyConstraint {
    /// Must not leave this machine.
    LocalOnly,
    /// May use operator-controlled infrastructure, but no third party.
    NoExternalService,
    /// The default, because a constraint has to be asked for. Making
    /// `LocalOnly` the default would look safer and be worse: every caller
    /// would hit refusals it did not mean, and the first fix anyone reached
    /// for would be to widen it everywhere.
    #[default]
    Unconstrained,
}

impl PrivacyConstraint {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalOnly => "local_only",
            Self::NoExternalService => "no_external_service",
            Self::Unconstrained => "unconstrained",
        }
    }

    /// The most exposed locality this constraint tolerates.
    pub const fn max_locality(self) -> LocalityClass {
        match self {
            Self::LocalOnly => LocalityClass::LocalHost,
            Self::NoExternalService => LocalityClass::LocalNetwork,
            Self::Unconstrained => LocalityClass::External,
        }
    }

    pub fn admits(self, locality: LocalityClass) -> bool {
        locality <= self.max_locality()
    }
}

impl fmt::Display for PrivacyConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PrivacyConstraint {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "local_only" => Self::LocalOnly,
            "no_external_service" => Self::NoExternalService,
            "unconstrained" => Self::Unconstrained,
            other => return Err(UnknownVocabulary::new("privacy_constraint", other)),
        })
    }
}

/// Whether something is waiting on this task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    /// A person is waiting. Slow resources are excluded.
    Interactive,
    /// Nothing is waiting; any latency class qualifies.
    Background,
}

impl Urgency {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::Background => "background",
        }
    }

    /// The slowest latency class this urgency tolerates.
    pub const fn max_latency(self) -> LatencyClass {
        match self {
            Self::Interactive => LatencyClass::Fast,
            Self::Background => LatencyClass::Slow,
        }
    }
}

impl fmt::Display for Urgency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How much capability the task needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequiredDepth {
    Shallow,
    Standard,
    Deep,
}

impl RequiredDepth {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shallow => "shallow",
            Self::Standard => "standard",
            Self::Deep => "deep",
        }
    }

    /// The weakest quality tier that can serve this depth.
    pub const fn min_quality(self) -> QualityTier {
        match self {
            Self::Shallow => QualityTier::Basic,
            Self::Standard => QualityTier::Standard,
            Self::Deep => QualityTier::High,
        }
    }
}

impl fmt::Display for RequiredDepth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a task needs from whatever answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingRequest {
    pub task_class: TaskClass,
    pub privacy: PrivacyConstraint,
    pub urgency: Urgency,
    pub required_depth: RequiredDepth,
    /// Size of the material to be sent, in the unit
    /// [`ResourceCapabilities::context_capacity`] uses.
    pub context_size: u32,
    /// The most this task may cost.
    pub cost_budget: CostClass,
    pub modality: Modality,
}

impl RoutingRequest {
    /// An ordinary interactive turn with no special constraint.
    pub const fn interactive(task_class: TaskClass, context_size: u32) -> Self {
        Self {
            task_class,
            privacy: PrivacyConstraint::Unconstrained,
            urgency: Urgency::Interactive,
            required_depth: RequiredDepth::Shallow,
            context_size,
            cost_budget: CostClass::High,
            modality: Modality::Text,
        }
    }

    #[must_use]
    pub const fn with_privacy(mut self, privacy: PrivacyConstraint) -> Self {
        self.privacy = privacy;
        self
    }

    #[must_use]
    pub const fn with_depth(mut self, required_depth: RequiredDepth) -> Self {
        self.required_depth = required_depth;
        self
    }

    #[must_use]
    pub const fn with_urgency(mut self, urgency: Urgency) -> Self {
        self.urgency = urgency;
        self
    }

    #[must_use]
    pub const fn with_cost_budget(mut self, cost_budget: CostClass) -> Self {
        self.cost_budget = cost_budget;
        self
    }
}

/// Why a candidate was chosen or excluded. Stable codes for trace and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RoutingReason {
    Selected,
    /// Eligible, but another candidate ranked ahead of it.
    NotPreferred,
    /// Sending here would exceed the privacy constraint.
    PrivacyExcluded,
    ModalityUnsupported,
    ContextTooLarge,
    CostOverBudget,
    TooSlowForUrgency,
    QualityBelowDepth,
    Unhealthy,
    /// No candidate survived the constraints.
    NoEligibleResource,
}

impl RoutingReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Selected => "SELECTED",
            Self::NotPreferred => "NOT_PREFERRED",
            Self::PrivacyExcluded => "PRIVACY_EXCLUDED",
            Self::ModalityUnsupported => "MODALITY_UNSUPPORTED",
            Self::ContextTooLarge => "CONTEXT_TOO_LARGE",
            Self::CostOverBudget => "COST_OVER_BUDGET",
            Self::TooSlowForUrgency => "TOO_SLOW_FOR_URGENCY",
            Self::QualityBelowDepth => "QUALITY_BELOW_DEPTH",
            Self::Unhealthy => "UNHEALTHY",
            Self::NoEligibleResource => "NO_ELIGIBLE_RESOURCE",
        }
    }

    pub const fn is_exclusion(self) -> bool {
        !matches!(self, Self::Selected | Self::NotPreferred)
    }
}

impl fmt::Display for RoutingReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RoutingReason {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "SELECTED" => Self::Selected,
            "NOT_PREFERRED" => Self::NotPreferred,
            "PRIVACY_EXCLUDED" => Self::PrivacyExcluded,
            "MODALITY_UNSUPPORTED" => Self::ModalityUnsupported,
            "CONTEXT_TOO_LARGE" => Self::ContextTooLarge,
            "COST_OVER_BUDGET" => Self::CostOverBudget,
            "TOO_SLOW_FOR_URGENCY" => Self::TooSlowForUrgency,
            "QUALITY_BELOW_DEPTH" => Self::QualityBelowDepth,
            "UNHEALTHY" => Self::Unhealthy,
            "NO_ELIGIBLE_RESOURCE" => Self::NoEligibleResource,
            other => return Err(UnknownVocabulary::new("routing_reason", other)),
        })
    }
}

/// One candidate and what the router made of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateVerdict {
    pub slot: ResourceSlot,
    pub resource_id: ResourceId,
    pub reason: RoutingReason,
}

/// A resource offered to the router.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingCandidate {
    pub slot: ResourceSlot,
    pub descriptor: ResourceDescriptor,
}

/// What the router decided, and why — including what it turned down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub slot: ResourceSlot,
    pub resource_id: ResourceId,
    pub reason: RoutingReason,
    /// Every candidate with its verdict, in a stable order. A decision that
    /// cannot say what it turned down is not auditable.
    pub considered: Vec<CandidateVerdict>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RoutingError {
    /// Nothing qualified. Deliberately not "fall back to whatever is left":
    /// satisfying a `LocalOnly` request with an external resource because the
    /// local one was unavailable would be exactly the wrong repair.
    #[error("no resource satisfies the request ({considered} candidate(s) considered)")]
    NoEligibleResource {
        considered: usize,
        verdicts: Vec<CandidateVerdict>,
    },
}

impl RoutingError {
    pub fn verdicts(&self) -> &[CandidateVerdict] {
        match self {
            Self::NoEligibleResource { verdicts, .. } => verdicts,
        }
    }
}

/// Chooses which resource answers a request.
pub trait Router: Send + Sync {
    fn route(
        &self,
        request: &RoutingRequest,
        candidates: &[RoutingCandidate],
    ) -> Result<RoutingDecision, RoutingError>;
}

/// Deterministic rule-based router.
///
/// Hard constraints exclude; survivors are ordered by a fixed total order.
/// Nothing is scored, weighted or learned, and the last tie-break is the slot
/// name, so there is always exactly one answer.
///
/// Preference among eligible candidates, in order:
///
/// 1. cheapest — a task should not cost more than it needs to;
/// 2. healthy before degraded — a usable-but-struggling resource is a
///    fallback, not a peer;
/// 3. highest declared quality;
/// 4. fastest;
/// 5. slot name, so the order is total.
#[derive(Debug, Default, Clone, Copy)]
pub struct RuleRouter;

impl RuleRouter {
    /// Why this candidate cannot serve the request, if it cannot.
    ///
    /// Order matters for the reason reported: privacy is checked first so a
    /// privacy refusal is never masked by a cheaper complaint about cost.
    fn exclusion(
        request: &RoutingRequest,
        capabilities: &ResourceCapabilities,
    ) -> Option<RoutingReason> {
        if !request.privacy.admits(capabilities.locality) {
            return Some(RoutingReason::PrivacyExcluded);
        }
        if !capabilities.supports(request.modality) {
            return Some(RoutingReason::ModalityUnsupported);
        }
        if !capabilities.health.is_usable() {
            return Some(RoutingReason::Unhealthy);
        }
        if capabilities.context_capacity < request.context_size {
            return Some(RoutingReason::ContextTooLarge);
        }
        if capabilities.cost > request.cost_budget {
            return Some(RoutingReason::CostOverBudget);
        }
        if capabilities.latency > request.urgency.max_latency() {
            return Some(RoutingReason::TooSlowForUrgency);
        }
        if capabilities.quality < request.required_depth.min_quality() {
            return Some(RoutingReason::QualityBelowDepth);
        }
        None
    }

    /// Total preference key. Lower sorts first.
    fn rank(capabilities: &ResourceCapabilities, slot: &ResourceSlot) -> impl Ord + use<> {
        (
            capabilities.cost,
            capabilities.health,
            // Higher quality first, so invert.
            std::cmp::Reverse(capabilities.quality),
            capabilities.latency,
            slot.clone(),
        )
    }
}

impl Router for RuleRouter {
    fn route(
        &self,
        request: &RoutingRequest,
        candidates: &[RoutingCandidate],
    ) -> Result<RoutingDecision, RoutingError> {
        // Sorted up front so the verdict list — and every tie-break below — is
        // independent of the order the caller happened to supply.
        let mut ordered: Vec<&RoutingCandidate> = candidates.iter().collect();
        ordered.sort_by(|a, b| a.slot.cmp(&b.slot));

        let mut verdicts = Vec::with_capacity(ordered.len());
        let mut eligible: Vec<&RoutingCandidate> = Vec::new();
        for candidate in &ordered {
            match Self::exclusion(request, &candidate.descriptor.capabilities) {
                Some(reason) => verdicts.push(CandidateVerdict {
                    slot: candidate.slot.clone(),
                    resource_id: candidate.descriptor.resource_id,
                    reason,
                }),
                None => eligible.push(candidate),
            }
        }

        let Some(chosen) = eligible
            .iter()
            .min_by_key(|c| Self::rank(&c.descriptor.capabilities, &c.slot))
            .copied()
        else {
            return Err(RoutingError::NoEligibleResource {
                considered: ordered.len(),
                verdicts,
            });
        };

        for candidate in eligible {
            verdicts.push(CandidateVerdict {
                slot: candidate.slot.clone(),
                resource_id: candidate.descriptor.resource_id,
                reason: if candidate.slot == chosen.slot {
                    RoutingReason::Selected
                } else {
                    RoutingReason::NotPreferred
                },
            });
        }
        verdicts.sort_by(|a, b| a.slot.cmp(&b.slot));

        Ok(RoutingDecision {
            slot: chosen.slot.clone(),
            resource_id: chosen.descriptor.resource_id,
            reason: RoutingReason::Selected,
            considered: verdicts,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::resources::ResourceKind;

    use super::*;

    fn candidate(
        slot: &str,
        id: u128,
        locality: LocalityClass,
        cost: CostClass,
        quality: QualityTier,
        latency: LatencyClass,
    ) -> RoutingCandidate {
        RoutingCandidate {
            slot: ResourceSlot::new(slot),
            descriptor: ResourceDescriptor {
                resource_id: ResourceId::from_u128(id),
                name: slot.to_owned(),
                kind: ResourceKind::Generation,
                adapter: "test".to_owned(),
                version: "1".to_owned(),
                read_only: true,
                capabilities: ResourceCapabilities {
                    locality,
                    modalities: [Modality::Text].into_iter().collect(),
                    context_capacity: 8_192,
                    latency,
                    cost,
                    quality,
                    health: HealthState::Healthy,
                },
            },
        }
    }

    fn local() -> RoutingCandidate {
        candidate(
            "local",
            0x1,
            LocalityClass::LocalHost,
            CostClass::Free,
            QualityTier::Standard,
            LatencyClass::Fast,
        )
    }

    fn remote() -> RoutingCandidate {
        candidate(
            "remote",
            0x2,
            LocalityClass::External,
            CostClass::High,
            QualityTier::High,
            LatencyClass::Fast,
        )
    }

    fn request() -> RoutingRequest {
        RoutingRequest::interactive(TaskClass::Summarize, 100)
    }

    fn verdict<'a>(decision: &'a RoutingDecision, slot: &str) -> &'a CandidateVerdict {
        decision
            .considered
            .iter()
            .find(|v| v.slot.as_str() == slot)
            .unwrap_or_else(|| panic!("{slot} is missing from the verdicts"))
    }

    #[test]
    fn routing_is_a_pure_function_of_request_and_candidates() {
        let candidates = vec![remote(), local()];
        let first = RuleRouter.route(&request(), &candidates).unwrap();
        let second = RuleRouter.route(&request(), &candidates).unwrap();
        assert_eq!(first, second);

        // And independent of the order the caller supplied them in.
        let reversed = vec![local(), remote()];
        assert_eq!(RuleRouter.route(&request(), &reversed).unwrap(), first);
    }

    #[test]
    fn the_cheapest_sufficient_resource_wins() {
        let decision = RuleRouter.route(&request(), &[local(), remote()]).unwrap();
        assert_eq!(decision.slot.as_str(), "local");
        assert_eq!(decision.reason, RoutingReason::Selected);
        assert_eq!(
            verdict(&decision, "remote").reason,
            RoutingReason::NotPreferred
        );
    }

    #[test]
    fn local_only_never_selects_an_external_resource() {
        let request = request().with_privacy(PrivacyConstraint::LocalOnly);
        let decision = RuleRouter.route(&request, &[local(), remote()]).unwrap();
        assert_eq!(decision.slot.as_str(), "local");
        assert_eq!(
            verdict(&decision, "remote").reason,
            RoutingReason::PrivacyExcluded
        );
    }

    #[test]
    fn local_only_refuses_rather_than_falling_back_to_a_remote_resource() {
        // The failure mode this whole type exists to prevent: the local option
        // is gone, and sending the data off the machine anyway.
        let request = request().with_privacy(PrivacyConstraint::LocalOnly);
        let error = RuleRouter
            .route(&request, &[remote()])
            .expect_err("a local-only request must not be served remotely");
        let RoutingError::NoEligibleResource {
            considered,
            verdicts,
        } = error;
        assert_eq!(considered, 1);
        assert_eq!(verdicts[0].reason, RoutingReason::PrivacyExcluded);
    }

    #[test]
    fn no_external_service_still_admits_the_operators_own_network() {
        let network = candidate(
            "network",
            0x3,
            LocalityClass::LocalNetwork,
            CostClass::Low,
            QualityTier::High,
            LatencyClass::Fast,
        );
        let request = request().with_privacy(PrivacyConstraint::NoExternalService);
        let decision = RuleRouter.route(&request, &[network, remote()]).unwrap();
        assert_eq!(decision.slot.as_str(), "network");
    }

    #[test]
    fn each_hard_constraint_excludes_with_its_own_reason() {
        let weak = candidate(
            "weak",
            0x4,
            LocalityClass::External,
            CostClass::High,
            QualityTier::Basic,
            LatencyClass::Fast,
        );
        let cases: Vec<(&str, RoutingRequest, RoutingCandidate, RoutingReason)> = vec![
            (
                "context",
                RoutingRequest {
                    context_size: 1_000_000,
                    ..request()
                },
                remote(),
                RoutingReason::ContextTooLarge,
            ),
            (
                "cost",
                request().with_cost_budget(CostClass::Free),
                remote(),
                RoutingReason::CostOverBudget,
            ),
            (
                "depth",
                request().with_depth(RequiredDepth::Deep),
                weak,
                RoutingReason::QualityBelowDepth,
            ),
        ];
        for (name, request, candidate, expected) in cases {
            let error = RuleRouter
                .route(&request, std::slice::from_ref(&candidate))
                .expect_err(&format!("{name} must exclude the only candidate"));
            assert_eq!(error.verdicts().len(), 1, "{name}");
            assert_eq!(error.verdicts()[0].reason, expected, "{name}");
        }
    }

    #[test]
    fn a_slow_resource_is_excluded_from_an_interactive_turn_but_not_a_background_one() {
        let slow = candidate(
            "slow",
            0x5,
            LocalityClass::External,
            CostClass::Low,
            QualityTier::High,
            LatencyClass::Slow,
        );
        let interactive = RuleRouter.route(&request(), std::slice::from_ref(&slow));
        assert!(matches!(
            interactive,
            Err(RoutingError::NoEligibleResource { .. })
        ));
        assert_eq!(
            interactive.unwrap_err().verdicts()[0].reason,
            RoutingReason::TooSlowForUrgency
        );

        let background = request().with_urgency(Urgency::Background);
        assert_eq!(
            RuleRouter
                .route(&background, &[slow])
                .unwrap()
                .slot
                .as_str(),
            "slow"
        );
    }

    #[test]
    fn an_unavailable_resource_is_excluded_and_a_degraded_one_is_a_fallback() {
        let mut down = local();
        down.descriptor.capabilities.health = HealthState::Unavailable;
        let error = RuleRouter.route(&request(), &[down.clone()]).unwrap_err();
        assert_eq!(error.verdicts()[0].reason, RoutingReason::Unhealthy);

        // Degraded is usable, but loses to a healthy peer at the same cost.
        let mut degraded = local();
        degraded.slot = ResourceSlot::new("degraded");
        degraded.descriptor.capabilities.health = HealthState::Degraded;
        let mut healthy = local();
        healthy.slot = ResourceSlot::new("healthy");
        let decision = RuleRouter.route(&request(), &[degraded, healthy]).unwrap();
        assert_eq!(decision.slot.as_str(), "healthy");
    }

    #[test]
    fn a_context_larger_than_capacity_is_excluded_by_size_not_by_guesswork() {
        let mut small = local();
        small.descriptor.capabilities.context_capacity = 10;
        let request = RoutingRequest {
            context_size: 11,
            ..request()
        };
        let error = RuleRouter.route(&request, &[small]).unwrap_err();
        assert_eq!(error.verdicts()[0].reason, RoutingReason::ContextTooLarge);
    }

    #[test]
    fn privacy_is_reported_ahead_of_cheaper_complaints() {
        // An external resource that is also over budget must still be reported
        // as a privacy exclusion: that is the reason an operator must see.
        let mut expensive = remote();
        expensive.descriptor.capabilities.cost = CostClass::High;
        let request = request()
            .with_privacy(PrivacyConstraint::LocalOnly)
            .with_cost_budget(CostClass::Free);
        let error = RuleRouter.route(&request, &[expensive]).unwrap_err();
        assert_eq!(error.verdicts()[0].reason, RoutingReason::PrivacyExcluded);
    }

    #[test]
    fn the_decision_accounts_for_every_candidate() {
        let decision = RuleRouter
            .route(
                &request().with_privacy(PrivacyConstraint::LocalOnly),
                &[local(), remote()],
            )
            .unwrap();
        assert_eq!(decision.considered.len(), 2);
        assert!(
            decision
                .considered
                .windows(2)
                .all(|w| w[0].slot <= w[1].slot),
            "verdicts must be in a stable order"
        );
    }

    #[test]
    fn vocabulary_round_trips() {
        for reason in [
            RoutingReason::Selected,
            RoutingReason::NotPreferred,
            RoutingReason::PrivacyExcluded,
            RoutingReason::ModalityUnsupported,
            RoutingReason::ContextTooLarge,
            RoutingReason::CostOverBudget,
            RoutingReason::TooSlowForUrgency,
            RoutingReason::QualityBelowDepth,
            RoutingReason::Unhealthy,
            RoutingReason::NoEligibleResource,
        ] {
            assert_eq!(reason.as_str().parse::<RoutingReason>().unwrap(), reason);
        }
        for locality in [
            LocalityClass::InProcess,
            LocalityClass::LocalHost,
            LocalityClass::LocalNetwork,
            LocalityClass::External,
        ] {
            assert_eq!(
                locality.as_str().parse::<LocalityClass>().unwrap(),
                locality
            );
        }
        assert!(LocalityClass::LocalHost.is_local());
        assert!(!LocalityClass::External.is_local());
    }
}
