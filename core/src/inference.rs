//! The contract between an untrusted on-device model and the deterministic core.
//!
//! llama.cpp applies [`TRIAGE_GBNF`] while sampling, making malformed JSON and unknown
//! enum values unreachable. The result is still untrusted: this module checks the schema
//! version, sizes, protocol id, and — most importantly — folds model severity through
//! [`Severity::escalate`] so a model can never downgrade a rule-table result.

use crate::protocol::{Corpus, Protocol};
use crate::severity::Severity;
use serde::Deserialize;
use std::fmt;

pub const TRIAGE_GBNF: &str = include_str!("../../data/grammar/triage.gbnf");

pub const SYSTEM_PROMPT: &str = include_str!("../../data/prompts/triage-system.txt");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgeBand {
    Infant,
    Child,
    Adult,
    OlderAdult,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Specialty {
    GeneralEmergency,
    Cardiac,
    Neurology,
    Trauma,
    Toxicology,
    Respiratory,
    Burns,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSlots {
    schema_version: String,
    severity: Severity,
    protocol_id: Option<String>,
    age_band: AgeBand,
    specialty: Specialty,
    symptoms: Vec<String>,
    needs_emergency_services: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedSlots {
    pub severity: Severity,
    pub protocol_id: Option<String>,
    pub age_band: AgeBand,
    pub specialty: Specialty,
    pub symptoms: Vec<String>,
    pub needs_emergency_services: bool,
}

impl ValidatedSlots {
    #[must_use]
    pub fn protocol<'a>(&self, corpus: &'a Corpus) -> Option<&'a Protocol> {
        self.protocol_id.as_deref().and_then(|id| corpus.get(id))
    }

    /// Remove model phrases that are not literal spans of the user's report.
    ///
    /// This deliberately prefers an empty UI field over a plausible paraphrase. A model
    /// may classify a report, but it may not put words into a caller's mouth.
    pub fn retain_grounded_symptoms(&mut self, report: &str) {
        let report = report.to_lowercase();
        self.symptoms.retain(|symptom| {
            let symptom = symptom.trim().to_lowercase();
            !symptom.is_empty() && report.contains(&symptom)
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotError {
    InvalidJson,
    UnsupportedSchema(String),
    UnknownProtocol(String),
    TooManySymptoms,
    EmptySymptom,
    SymptomTooLong,
}

impl fmt::Display for SlotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson => write!(f, "model output is not the constrained intake object"),
            Self::UnsupportedSchema(value) => {
                write!(f, "unsupported intake schema version {value:?}")
            }
            Self::UnknownProtocol(value) => write!(f, "unknown protocol id {value:?}"),
            Self::TooManySymptoms => write!(f, "model returned more than eight symptoms"),
            Self::EmptySymptom => write!(f, "model returned an empty symptom"),
            Self::SymptomTooLong => write!(f, "model returned a symptom longer than 80 characters"),
        }
    }
}

/// Validate one grammar-constrained model response and apply the rule-table floor.
pub fn validate_slots(
    json: &str,
    corpus: &Corpus,
    rule_floor: Option<Severity>,
) -> Result<ValidatedSlots, SlotError> {
    let raw: RawSlots = serde_json::from_str(json).map_err(|_| SlotError::InvalidJson)?;
    if raw.schema_version != "1" {
        return Err(SlotError::UnsupportedSchema(raw.schema_version));
    }
    if raw.symptoms.len() > 8 {
        return Err(SlotError::TooManySymptoms);
    }
    for symptom in &raw.symptoms {
        if symptom.trim().is_empty() {
            return Err(SlotError::EmptySymptom);
        }
        if symptom.chars().count() > 80 {
            return Err(SlotError::SymptomTooLong);
        }
    }
    if let Some(id) = raw.protocol_id.as_deref()
        && corpus.get(id).is_none()
    {
        return Err(SlotError::UnknownProtocol(id.to_owned()));
    }

    let severity = rule_floor
        .map(|floor| Severity::escalate(floor, raw.severity))
        .unwrap_or(raw.severity);
    Ok(ValidatedSlots {
        severity,
        protocol_id: raw.protocol_id,
        age_band: raw.age_band,
        specialty: raw.specialty,
        symptoms: raw.symptoms,
        // Urgent/critical is a code-owned reason to call even if the model emitted false.
        needs_emergency_services: raw.needs_emergency_services || severity.bypasses_model(),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use crate::bundled;

    fn corpus() -> Corpus {
        bundled::corpus().0
    }

    fn output(severity: &str, protocol: &str) -> String {
        format!(
            "{{\"schema_version\":\"1\",\"severity\":\"{severity}\",\"protocol_id\":{protocol},\"age_band\":\"adult\",\"specialty\":\"general_emergency\",\"symptoms\":[\"pain\"],\"needs_emergency_services\":false}}"
        )
    }

    #[test]
    fn a_model_cannot_downgrade_a_rule_floor() {
        let slots = validate_slots(
            &output("self_care", "\"chest.pain\""),
            &corpus(),
            Some(Severity::Critical),
        )
        .expect("valid slots");
        assert_eq!(slots.severity, Severity::Critical);
        assert!(slots.needs_emergency_services);
    }

    #[test]
    fn a_model_may_escalate() {
        let slots = validate_slots(
            &output("critical", "\"chest.pain\""),
            &corpus(),
            Some(Severity::Standard),
        )
        .expect("valid slots");
        assert_eq!(slots.severity, Severity::Critical);
    }

    #[test]
    fn an_unknown_protocol_is_refused() {
        let error = validate_slots(&output("urgent", "\"made.up\""), &corpus(), None)
            .expect_err("unknown ids are not model creativity");
        assert_eq!(error, SlotError::UnknownProtocol("made.up".to_owned()));
    }

    #[test]
    fn extra_fields_are_refused() {
        let text = output("standard", "null").replace(
            "\"needs_emergency_services\":false",
            "\"surprise\":true,\"needs_emergency_services\":false",
        );
        assert_eq!(
            validate_slots(&text, &corpus(), None),
            Err(SlotError::InvalidJson)
        );
    }

    #[test]
    fn grammar_enumerates_every_shipped_protocol() {
        for protocol in corpus().protocols() {
            assert!(
                TRIAGE_GBNF.contains(&format!("\\\"{}\\\"", protocol.id)),
                "{} is selectable in the corpus but absent from the grammar",
                protocol.id
            );
        }
    }

    #[test]
    fn ungrounded_model_symptoms_are_removed() {
        let mut slots = validate_slots(
            &output("urgent", "\"breathing.distress\"").replace(
                "[\"pain\"]",
                "[\"cant breath\",\"/no_think\",\"invented fever\"]",
            ),
            &corpus(),
            None,
        )
        .expect("valid slots");
        slots.retain_grounded_symptoms("I cant breath properly");
        assert_eq!(slots.symptoms, vec!["cant breath"]);
    }
}
