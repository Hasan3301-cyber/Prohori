//! Severity levels and the only sanctioned way to combine them.
//!
//! See `docs/CONVENTIONS.md` §5. The escalate-only rule is carried over from
//! EcoGuardian, where the LLM triage path was allowed to raise a severity but
//! never to lower one. The combinator is `max` underneath; it exists as a named
//! function so that a call site cannot express a downgrade by accident.

use serde::{Deserialize, Serialize};

/// Four levels, ordered. `Ord` follows declaration order, so `Critical` is greatest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Manage at home. No transport needed.
    SelfCare,
    /// See a clinician, but not tonight.
    Standard,
    /// Needs care within hours. Go now.
    Urgent,
    /// Life is in immediate danger. Call, and start the protocol.
    Critical,
}

impl Severity {
    /// Combine two severities. The result is never lower than `baseline`.
    ///
    /// This is the *only* supported way to fold a new assessment into an existing
    /// one. Assigning a severity directly from a model result is a bug.
    #[must_use]
    pub fn escalate(baseline: Self, candidate: Self) -> Self {
        if candidate > baseline {
            candidate
        } else {
            baseline
        }
    }

    /// Stable machine-readable name, used in traces, audit entries, and FFI.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SelfCare => "self_care",
            Self::Standard => "standard",
            Self::Urgent => "urgent",
            Self::Critical => "critical",
        }
    }

    /// True when the case must not wait for model inference.
    #[must_use]
    pub fn bypasses_model(self) -> bool {
        self >= Self::Urgent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_is_least_to_most_dangerous() {
        assert!(Severity::SelfCare < Severity::Standard);
        assert!(Severity::Standard < Severity::Urgent);
        assert!(Severity::Urgent < Severity::Critical);
    }

    #[test]
    fn escalate_never_downgrades() {
        assert_eq!(
            Severity::escalate(Severity::Critical, Severity::SelfCare),
            Severity::Critical,
            "a self-care assessment must not pull a critical case down"
        );
    }

    #[test]
    fn escalate_does_raise() {
        assert_eq!(
            Severity::escalate(Severity::Standard, Severity::Critical),
            Severity::Critical
        );
    }

    #[test]
    fn only_urgent_and_above_bypass_the_model() {
        assert!(Severity::Critical.bypasses_model());
        assert!(Severity::Urgent.bypasses_model());
        assert!(!Severity::Standard.bypasses_model());
        assert!(!Severity::SelfCare.bypasses_model());
    }
}
