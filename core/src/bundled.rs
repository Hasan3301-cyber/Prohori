//! The shipped first-aid corpus, compiled into the binary.
//!
//! # Why the corpus is embedded and not read from disk
//!
//! CI proves that the files in `data/firstaid/` parse, validate, escalate somewhere,
//! read at grade six, and name no drugs. That guarantee is about *those bytes*. If the
//! app then read the same content from an asset directory, the guarantee would apply to
//! the repository and not to the thing installed on the phone: an asset can be renamed
//! in a build script, dropped from a packaging rule, or fail to open on a device with a
//! full filesystem, and none of those failures are visible to any test here.
//!
//! Embedding closes that gap. The bytes CI validated are the bytes in the binary, and
//! [`corpus`] cannot fail for an environmental reason — there is no path to be wrong, no
//! permission to be missing, and no I/O to time out while someone is bleeding.
//!
//! # The cost, stated
//!
//! Updating a protocol means rebuilding the library, so a corpus fix ships on the app's
//! release cadence rather than as a data push. For a corpus this size that is still the
//! right trade: eighteen cards is a set one person can hold in their head, and medical
//! content that changes without a rebuild is medical content that ships without CI.
//!
//! It also means adding a protocol requires editing [`FILES`], which is easy to forget.
//! That is why the list is explicit rather than a directory walk generated at build
//! time: `tests/corpus_integrity.rs::the_binary_ships_every_protocol_in_the_repository`
//! compares this list against `data/firstaid/` in both directions, so a protocol that
//! was authored but not bundled fails CI instead of being quietly absent from the app.
//!
//! [`Corpus::from_entries`] remains the general entry point for content that genuinely
//! does arrive at runtime — city packs, and the pack builder on the desktop side.

use crate::protocol::{Corpus, ProtocolError};

/// Every protocol shipped in the binary, as `(filename, contents)`.
///
/// Filenames are kept because [`Corpus::from_entries`] compares the filename stem
/// against the protocol's declared `id`, which is the check that catches a file copied
/// from another protocol and half-edited.
pub static FILES: &[(&str, &str)] = &[
    (
        "allergy.anaphylaxis.json",
        include_str!("../../data/firstaid/allergy.anaphylaxis.json"),
    ),
    (
        "bleeding.severe.json",
        include_str!("../../data/firstaid/bleeding.severe.json"),
    ),
    (
        "breathing.distress.json",
        include_str!("../../data/firstaid/breathing.distress.json"),
    ),
    (
        "burn.thermal.json",
        include_str!("../../data/firstaid/burn.thermal.json"),
    ),
    (
        "chest.pain.json",
        include_str!("../../data/firstaid/chest.pain.json"),
    ),
    (
        "choking.adult.json",
        include_str!("../../data/firstaid/choking.adult.json"),
    ),
    (
        "cpr.adult.json",
        include_str!("../../data/firstaid/cpr.adult.json"),
    ),
    (
        "dehydration.diarrhoea.json",
        include_str!("../../data/firstaid/dehydration.diarrhoea.json"),
    ),
    (
        "drowning.rescue.json",
        include_str!("../../data/firstaid/drowning.rescue.json"),
    ),
    (
        "electric.shock.json",
        include_str!("../../data/firstaid/electric.shock.json"),
    ),
    (
        "fracture.suspected.json",
        include_str!("../../data/firstaid/fracture.suspected.json"),
    ),
    (
        "head.injury.json",
        include_str!("../../data/firstaid/head.injury.json"),
    ),
    (
        "heat.illness.json",
        include_str!("../../data/firstaid/heat.illness.json"),
    ),
    (
        "poisoning.swallowed.json",
        include_str!("../../data/firstaid/poisoning.swallowed.json"),
    ),
    (
        "seizure.active.json",
        include_str!("../../data/firstaid/seizure.active.json"),
    ),
    (
        "snake.bite.json",
        include_str!("../../data/firstaid/snake.bite.json"),
    ),
    (
        "stroke.suspected.json",
        include_str!("../../data/firstaid/stroke.suspected.json"),
    ),
    (
        "unresponsive.breathing.json",
        include_str!("../../data/firstaid/unresponsive.breathing.json"),
    ),
];

/// Parse and validate the embedded corpus.
///
/// Returns errors the same way [`Corpus::from_entries`] does. On a device that list is
/// expected to be empty — CI asserts it — but it is still returned rather than
/// swallowed, so the FFI layer can surface "this build shipped a broken card" instead
/// of a screen that is merely missing one.
#[must_use]
pub fn corpus() -> (Corpus, Vec<ProtocolError>) {
    Corpus::from_entries(FILES.iter().copied())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// Duplicated in spirit by the integration suite, and kept here on purpose: this one
    /// runs as part of the library's own tests, so it fires even when someone runs
    /// `cargo test --lib` or filters the integration tests out.
    #[test]
    fn the_embedded_corpus_loads_clean() {
        let (corpus, errors) = corpus();
        assert!(
            errors.is_empty(),
            "embedded corpus has errors: {}",
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        );
        assert_eq!(corpus.len(), FILES.len());
    }

    #[test]
    fn every_active_red_flag_rule_can_be_served_from_the_binary_alone() {
        let (corpus, _) = corpus();
        for rule in crate::redflag::RULES {
            if rule.status != crate::redflag::RuleStatus::Active {
                continue;
            }
            let id = rule.protocol_id.expect("active rules carry an id");
            assert!(
                corpus.get(id).is_some(),
                "rule {} needs protocol {id:?}, which is not embedded in the binary",
                rule.id
            );
        }
    }
}
