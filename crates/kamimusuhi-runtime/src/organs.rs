//! Runtime promotion map for experimentally validated cognitive organs.
//!
//! This module does not pretend that an experiment checkpoint is already a
//! production adapter. It fixes the *admission decision* and the role each
//! result is allowed to occupy. Actual implementations register through
//! [`kamimusuhi_core::organs::OrganSupervisor`] and must report the exact
//! descriptor they were admitted under.

use kamimusuhi_core::organs::{
    ExperimentVerdict, OrganDescriptor, OrganEvidence, OrganKey, OrganRole, PromotionMode,
};

fn descriptor(
    key: &str,
    role: OrganRole,
    experiment: &str,
    verdict: ExperimentVerdict,
    promotion: PromotionMode,
    revision: &str,
    report_path: &str,
) -> OrganDescriptor {
    OrganDescriptor {
        key: OrganKey::new(key).expect("static organ key is valid"),
        role,
        // The promotion boundary is real now; the learned checkpoint adapter
        // is deliberately named as pending rather than pretending the Python
        // experiment itself is a stable runtime implementation.
        implementation: "checkpoint-adapter-pending".to_owned(),
        version: "0".to_owned(),
        promotion,
        evidence: OrganEvidence {
            experiment: experiment.to_owned(),
            verdict,
            source_revision: Some(revision.to_owned()),
            report_path: Some(report_path.to_owned()),
        },
    }
}

/// Promotion decisions backed by the completed experiment battery.
///
/// Active means "eligible to influence cognition once a conforming runtime
/// adapter and pinned checkpoint are supplied". It does not mean a checkpoint
/// is bundled in the repository. O0 stays shadow-only because its comparative
/// criterion remained PARTIAL.
pub fn validated_experiment_manifest() -> Vec<OrganDescriptor> {
    vec![
        descriptor(
            "h0-regulation",
            OrganRole::Regulation,
            "H0",
            ExperimentVerdict::Pass,
            PromotionMode::Active,
            "c50c294fd843fa5613fdcacdc741dd2e123175f5",
            "experiments/h0/reports/H0_RESULTS.md",
        ),
        descriptor(
            "r0-memory-gate",
            OrganRole::MemoryGate,
            "R0",
            ExperimentVerdict::Pass,
            PromotionMode::Active,
            "4f95df640c5fb0dba42b1c8bd5f664094b976e47",
            "experiments/r0/reports/R0_RESULTS.md",
        ),
        descriptor(
            "s0-agency-attribution",
            OrganRole::AgencyAttribution,
            "S0",
            ExperimentVerdict::Pass,
            PromotionMode::Active,
            "f15aea6bbd8b1a53f020a5924e3ffcb153a96d37",
            "experiments/s0/reports/S0_RESULTS.md",
        ),
        descriptor(
            "t0-temporal-state",
            OrganRole::TemporalState,
            "T0",
            ExperimentVerdict::Pass,
            PromotionMode::Active,
            "72b5011bb629a2992e876e5fc9ec3fd8a430b04c",
            "experiments/t0/reports/T0_RESULTS.md",
        ),
        descriptor(
            "o0-object-state",
            OrganRole::ObjectState,
            "O0",
            ExperimentVerdict::Partial,
            PromotionMode::Shadow,
            "d18ee4316196f474ff9274d864b0d21ecfdf073c",
            "experiments/o0/reports/O0_RESULTS.md",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pass_results_are_active_and_o0_is_shadow() {
        let manifest = validated_experiment_manifest();
        assert_eq!(manifest.len(), 5);
        for descriptor in &manifest {
            descriptor.validate().unwrap();
        }
        assert_eq!(
            manifest
                .iter()
                .filter(|d| d.promotion == PromotionMode::Active)
                .count(),
            4
        );
        let o0 = manifest
            .iter()
            .find(|d| d.evidence.experiment == "O0")
            .unwrap();
        assert_eq!(o0.promotion, PromotionMode::Shadow);
        assert_eq!(o0.evidence.verdict, ExperimentVerdict::Partial);
    }

    #[test]
    fn negative_results_are_not_promoted() {
        let experiments: Vec<&str> = validated_experiment_manifest()
            .iter()
            .map(|d| d.evidence.experiment.as_str())
            .collect();
        assert!(!experiments.contains(&"G0"));
        assert!(!experiments.contains(&"P0"));
    }
}
