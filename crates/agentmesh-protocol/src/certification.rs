//! Evidence-backed capability conformance and certification levels.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::CapabilityPackage;

/// Capability assurance level computed from a certification report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CertificationLevel {
    /// Package is syntactically present but has no review claim.
    Draft,
    /// A named reviewer accepted the package contract.
    Reviewed,
    /// Contract, policy, implementation, and recovery tests passed.
    Tested,
    /// Success evidence was exercised and matched the declared contract.
    Verified,
    /// Security, sandbox, and publisher integrity checks also passed.
    ProductionCertified,
}

/// One named conformance check and its retained evidence reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificationCheck {
    /// Whether the check passed.
    pub passed: bool,
    /// Opaque test run, audit record, or evidence artifact reference.
    pub evidence_reference: String,
}

/// Publisher-submitted certification evidence for one exact package digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificationReport {
    /// Capability identity.
    pub capability_id: String,
    /// Capability version.
    pub capability_version: String,
    /// Digest of the package that was tested and reviewed.
    pub package_digest: String,
    /// Named reviewer for the Reviewed level and above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_by: Option<String>,
    /// Results keyed by stable conformance check name.
    pub checks: BTreeMap<String, CertificationCheck>,
}

/// Computed assurance level and unmet requirements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificationDecision {
    /// Highest level supported by the report.
    pub level: CertificationLevel,
    /// Required checks absent or failed for the next level.
    pub missing_requirements: BTreeSet<String>,
    /// Whether report identity and package digest match.
    pub report_matches_package: bool,
}

/// Computes the highest certification level supported by retained evidence.
///
/// Check names are stable protocol identifiers: `contract`, `policy`,
/// `implementation`, `recovery`, `evidence`, `security`, `sandbox`, and
/// `publisher_integrity`. This validates report claims; it does not rerun tests
/// or verify signatures on external artifacts.
pub fn evaluate_certification(
    package: &CapabilityPackage,
    report: &CertificationReport,
) -> CertificationDecision {
    let digest = package.content_digest().ok();
    let report_matches_package = package.validate().is_ok()
        && report.capability_id == package.capability.id
        && report.capability_version == package.capability.version
        && digest.as_deref() == Some(report.package_digest.as_str());
    if !report_matches_package {
        return CertificationDecision {
            level: CertificationLevel::Draft,
            missing_requirements: BTreeSet::from(["package_identity_or_digest".into()]),
            report_matches_package: false,
        };
    }

    let reviewed = report
        .reviewed_by
        .as_deref()
        .is_some_and(|reviewer| !reviewer.trim().is_empty());
    if !reviewed {
        return CertificationDecision {
            level: CertificationLevel::Draft,
            missing_requirements: BTreeSet::from(["reviewed_by".into()]),
            report_matches_package: true,
        };
    }

    let missing_tested = missing_checks(
        report,
        &["contract", "policy", "implementation", "recovery"],
    );
    if !missing_tested.is_empty() {
        return CertificationDecision {
            level: CertificationLevel::Reviewed,
            missing_requirements: missing_tested,
            report_matches_package: true,
        };
    }
    let missing_verified = missing_checks(report, &["evidence"]);
    if !missing_verified.is_empty() {
        return CertificationDecision {
            level: CertificationLevel::Tested,
            missing_requirements: missing_verified,
            report_matches_package: true,
        };
    }
    let missing_production =
        missing_checks(report, &["security", "sandbox", "publisher_integrity"]);
    if !missing_production.is_empty() {
        return CertificationDecision {
            level: CertificationLevel::Verified,
            missing_requirements: missing_production,
            report_matches_package: true,
        };
    }
    CertificationDecision {
        level: CertificationLevel::ProductionCertified,
        missing_requirements: BTreeSet::new(),
        report_matches_package: true,
    }
}

fn missing_checks(report: &CertificationReport, required: &[&str]) -> BTreeSet<String> {
    required
        .iter()
        .filter(|name| {
            report
                .checks
                .get(**name)
                .is_none_or(|check| !check.passed || check.evidence_reference.trim().is_empty())
        })
        .map(|name| (*name).to_owned())
        .collect()
}
