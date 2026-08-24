//! The safety-net card: what to say when the corpus has nothing to say.
//!
//! A message that fires no red-flag rule and matches no card used to end at a notice
//! saying this app knows a small number of emergencies. In a flood or an earthquake that
//! is the app failing at the moment it exists for, so there is now always something
//! cited on the screen: the general approach to a casualty, which is true of nearly
//! every emergency and is exactly the part a frightened bystander does not know.
//!
//! # Why this is not the nineteenth protocol
//!
//! It uses [`Protocol`] and passes [`Protocol::validate`], so it is held to every rule in
//! `data/firstaid/SCHEMA.md` — one citation minimum, contiguous steps, no opening action,
//! honest `reviewed_by`, grade six. It is nevertheless **not** inserted into the
//! [`crate::protocol::Corpus`], and that is deliberate:
//!
//! - [`crate::retrieval`] must not rank it. A card that matches everything would win
//!   queries that belong to a real protocol.
//! - `data/grammar/triage.gbnf` enumerates every protocol id as a literal alternative, so
//!   a nineteenth id means a new grammar, a new line in `data/prompts/triage-system.txt`,
//!   and new rows in the P5 dataset — which changes `train_sha256`, `eval_sha256` and
//!   `bundled_corpus_sha256` in `model/datasets/p5/manifest.json` and invalidates a
//!   training run. An adapter cannot emit an id it never saw during training.
//! - It is not a protocol in the sense the corpus means. It is what is left when no
//!   protocol applies, and keeping it in its own namespace makes that structural rather
//!   than a comment.
//!
//! Embedded with `include_str!` for the reason [`crate::bundled`] gives at length: the
//! bytes CI validated are the bytes in the binary, and there is no path to be wrong while
//! someone is bleeding.

use crate::protocol::{Protocol, ProtocolError};

/// The card, as shipped. Filename kept so an error can name it.
pub const FILE: (&str, &str) = (
    "unknown-emergency.json",
    include_str!("../../data/guidance/unknown-emergency.json"),
);

/// The id this card declares. Reserved: no protocol in `data/firstaid/` may use it, which
/// `tests/fallback_safety.rs` asserts in both directions.
pub const SAFETY_NET_ID: &str = "unknown.emergency";

/// Parse and validate the embedded safety-net card.
///
/// Returns the same error type the corpus loader returns, so a broken card surfaces
/// through the FFI's existing `corpus_load_errors` path and shows up as "this build is
/// incomplete" rather than as a screen that is quietly missing a section.
pub fn safety_net() -> Result<Protocol, Vec<ProtocolError>> {
    let (file, json) = FILE;
    let protocol: Protocol = match serde_json::from_str(json) {
        Ok(parsed) => parsed,
        Err(err) => {
            return Err(vec![ProtocolError::Malformed {
                file: file.to_owned(),
                message: err.to_string(),
            }]);
        }
    };
    if protocol.id != SAFETY_NET_ID {
        return Err(vec![ProtocolError::IdMismatch {
            file: file.to_owned(),
            id: protocol.id,
        }]);
    }
    protocol.validate()?;
    Ok(protocol)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// Runs under `cargo test --lib` too, so the card cannot break without something
    /// failing even when the integration suite is filtered out.
    #[test]
    fn the_embedded_safety_net_loads_clean() {
        let card = safety_net().expect("the safety-net card must validate");
        assert_eq!(card.id, SAFETY_NET_ID);
        assert!(!card.citations.is_empty());
        assert!(card.reviewed_by.is_none(), "nobody has signed this off");
    }

    /// It must never be reachable as a corpus card, or the browse list and the grammar
    /// would disagree about what this app knows.
    #[test]
    fn the_safety_net_id_is_not_a_bundled_protocol() {
        let (corpus, _) = crate::bundled::corpus();
        assert!(corpus.get(SAFETY_NET_ID).is_none());
    }
}
