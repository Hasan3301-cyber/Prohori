//! Privacy-minimal tamper-evident audit chain.
//!
//! Events record decisions and provenance, never the user's free-text medical report.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditEvent {
    pub sequence: u64,
    pub at_epoch_seconds: u64,
    pub kind: String,
    pub attributes: BTreeMap<String, String>,
    pub previous_hash: String,
    pub hash: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditLog {
    events: Vec<AuditEvent>,
}

impl AuditLog {
    #[must_use]
    pub fn events(&self) -> &[AuditEvent] {
        &self.events
    }

    pub fn append(
        &mut self,
        at: u64,
        kind: impl Into<String>,
        attributes: BTreeMap<String, String>,
    ) -> Result<&AuditEvent, &'static str> {
        if attributes.keys().any(|key| {
            matches!(
                key.as_str(),
                "message" | "report" | "patient_name" | "phone"
            )
        }) {
            return Err("audit attributes may not contain patient text or identifiers");
        }
        let sequence = self.events.len() as u64;
        let previous_hash = self
            .events
            .last()
            .map_or_else(|| "0".repeat(64), |event| event.hash.clone());
        let kind = kind.into();
        let hash = event_hash(sequence, at, &kind, &attributes, &previous_hash);
        self.events.push(AuditEvent {
            sequence,
            at_epoch_seconds: at,
            kind,
            attributes,
            previous_hash,
            hash,
        });
        self.events.last().ok_or("audit append failed")
    }

    #[must_use]
    pub fn verify(events: &[AuditEvent]) -> bool {
        let mut previous = "0".repeat(64);
        for (index, event) in events.iter().enumerate() {
            if event.sequence != index as u64 || event.previous_hash != previous {
                return false;
            }
            if event.hash
                != event_hash(
                    event.sequence,
                    event.at_epoch_seconds,
                    &event.kind,
                    &event.attributes,
                    &event.previous_hash,
                )
            {
                return false;
            }
            previous.clone_from(&event.hash);
        }
        true
    }
}

fn event_hash(
    sequence: u64,
    at: u64,
    kind: &str,
    attributes: &BTreeMap<String, String>,
    previous: &str,
) -> String {
    let canonical =
        serde_json::to_vec(&(sequence, at, kind, attributes, previous)).unwrap_or_default();
    hex::encode(Sha256::digest(canonical))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn valid_chain_verifies() {
        let mut log = AuditLog::default();
        let _ = log.append(
            1,
            "rule_fired",
            BTreeMap::from([("rule_id".into(), "cardiac_arrest".into())]),
        );
        let _ = log.append(
            2,
            "dial_opened",
            BTreeMap::from([("source".into(), "built_in".into())]),
        );
        assert!(AuditLog::verify(log.events()));
    }
    #[test]
    fn tampering_breaks_chain() {
        let mut log = AuditLog::default();
        let _ = log.append(1, "rule_fired", BTreeMap::new());
        let mut events = log.events().to_vec();
        if let Some(event) = events.first_mut() {
            event.kind = "other".into();
        }
        assert!(!AuditLog::verify(&events));
    }
    #[test]
    fn patient_text_keys_are_refused() {
        let mut log = AuditLog::default();
        assert!(
            log.append(
                1,
                "input",
                BTreeMap::from([("message".into(), "chest pain".into())])
            )
            .is_err()
        );
    }
}
