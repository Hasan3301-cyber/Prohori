//! Integrity suite for the shipped corpus in `data/firstaid/`.
//!
//! The unit tests in `protocol.rs` prove the *validator* works on synthetic input.
//! These tests run it against the real files, and they are the ones that fail when
//! someone edits a protocol at 2am.
//!
//! Two invariants here are load-bearing for decisions taken elsewhere:
//!
//! - Every `Active` red-flag rule points at a protocol that actually exists. Without
//!   this, `RuleStatus::Active` is a claim rather than a fact, and the honesty pattern
//!   the whole rule table is built on collapses.
//! - `cpr.adult` opens with an assessment. `core/src/redflag.rs` accepts a known
//!   overtriage — "I can't breathe" fires the CPR rule — specifically *because* the
//!   card's first instruction is to check for a response. If step 1 ever became an
//!   action, that overtriage would stop being acceptable and the rule table would have
//!   to change.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use prohori_core::bundled;
use prohori_core::protocol::{Corpus, StepKind};
use prohori_core::readability;
use prohori_core::redflag::{RULES, RuleStatus};
use prohori_core::retrieval::{Index, is_search_noise};
use prohori_core::verifier::{is_medication_vocabulary, verify_rendering, word_set};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("data")
        .join("firstaid")
}

fn corpus() -> Corpus {
    let (corpus, errors) = Corpus::load_dir(&corpus_dir());
    assert!(
        errors.is_empty(),
        "shipped corpus has errors:\n{}",
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
    corpus
}

// ---------------------------------------------------------------------------
// The corpus loads, and loads cleanly
// ---------------------------------------------------------------------------

/// `Corpus::from_entries` tolerates a bad file so a phone degrades instead of bricking.
/// This test is the other half of that bargain: a bad file never reaches a phone.
#[test]
fn the_shipped_corpus_loads_with_no_errors() {
    let corpus = corpus();
    assert!(
        !corpus.is_empty(),
        "no protocols found in {}",
        corpus_dir().display()
    );
}

// ---------------------------------------------------------------------------
// The binary and the repository hold the same corpus
// ---------------------------------------------------------------------------

/// `core/src/bundled.rs` compiles the corpus into the library so the bytes CI validates
/// are the bytes the app runs. That only holds if the embedded list is complete, and the
/// list is hand-written. This is the test that turns "authored a protocol but forgot to
/// bundle it" into a CI failure instead of a card that silently does not exist on the
/// phone — which is the worst possible way for this particular mistake to show up.
#[test]
fn the_binary_ships_every_protocol_in_the_repository() {
    let mut on_disk: BTreeSet<String> = BTreeSet::new();
    for entry in std::fs::read_dir(corpus_dir()).expect("corpus directory is readable") {
        let path = entry.expect("directory entry").path();
        if path.extension().is_some_and(|ext| ext == "json") {
            on_disk.insert(
                path.file_name()
                    .expect("a file")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    let embedded: BTreeSet<String> = bundled::FILES
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect();

    assert_eq!(
        embedded,
        on_disk,
        "core/src/bundled.rs::FILES and data/firstaid/ disagree.\n  \
         in the repository but not in the binary: {:?}\n  \
         in the binary but not in the repository: {:?}",
        on_disk.difference(&embedded).collect::<Vec<_>>(),
        embedded.difference(&on_disk).collect::<Vec<_>>(),
    );
}

/// Same filenames is not the same content. Every test in this file reads from disk, so
/// without this one they would all pass while the shipped binary carried something else.
#[test]
fn the_embedded_corpus_is_byte_for_byte_the_corpus_these_tests_check() {
    let (embedded, errors) = bundled::corpus();
    assert!(errors.is_empty(), "embedded corpus has errors: {errors:?}");

    let disk = corpus();
    assert_eq!(embedded.len(), disk.len());
    for protocol in disk.protocols() {
        assert_eq!(
            embedded.get(&protocol.id),
            Some(protocol),
            "protocol {:?} differs between the binary and data/firstaid/",
            protocol.id
        );
    }
}

// ---------------------------------------------------------------------------
// The rule table and the corpus agree
// ---------------------------------------------------------------------------

/// `redflag_safety.rs` proves an active rule *declares* a protocol id. This proves the
/// id resolves. Together they mean an active rule can always render something.
#[test]
fn every_active_rule_resolves_to_a_real_protocol() {
    let corpus = corpus();
    for rule in RULES {
        if rule.status != RuleStatus::Active {
            continue;
        }
        let id = rule
            .protocol_id
            .unwrap_or_else(|| panic!("active rule {} has no protocol id", rule.id));
        assert!(
            corpus.get(id).is_some(),
            "rule {} points at protocol {id:?}, which is not in {}",
            rule.id,
            corpus_dir().display()
        );
    }
}

/// A protocol nothing can reach cannot be read by a user, and is far more likely to be a
/// typo in an id than a deliberate spare.
///
/// "Reachable" widened when the corpus did. Through P0 the only way to a card was a
/// red-flag rule, so this test demanded a rule per protocol. From P1 there are two
/// doors, and most cards only have the second one: nine of the eighteen protocols
/// describe situations that are urgent but not red-flag-critical — a burn, a broken
/// bone, watery diarrhoea — and adding a Critical rule for each of them would turn the
/// red-flag layer into a search engine that also shouts.
///
/// So the invariant is now "a rule points at it, **or** its own title finds it". The
/// second half is the stronger claim of the two: it is checked by searching for the
/// title text and requiring the protocol to come back *first*, which fails not only
/// when a card is unreachable but also when two cards are close enough to shadow each
/// other. `tests/retrieval_quality.rs` then does the same job against phrasing a
/// frightened person would actually type.
#[test]
fn every_protocol_is_reachable_by_a_rule_or_by_search() {
    let corpus = corpus();
    let index = Index::build(&corpus);
    for protocol in corpus.protocols() {
        if RULES
            .iter()
            .any(|rule| rule.protocol_id == Some(protocol.id.as_str()))
        {
            continue;
        }
        let hits = index.search(&protocol.title, 1);
        let first = hits.first().map(|hit| hit.protocol_id.as_str());
        assert_eq!(
            first,
            Some(protocol.id.as_str()),
            "protocol {:?} has no rule, and searching its own title {:?} returns {:?} \
             instead of itself; either the id is misspelled or another card shadows it",
            protocol.id,
            protocol.title,
            first
        );
    }
}

/// Pending rules must stay pending until their file lands. If a protocol exists, the
/// rule should be promoted to `Active` rather than quietly leaving a card unshown.
#[test]
fn no_pending_rule_has_a_protocol_sitting_unused() {
    let corpus = corpus();
    for rule in RULES {
        if rule.status != RuleStatus::Pending {
            continue;
        }
        // Pending rules carry no id, so infer the file that would serve them from the
        // rule id's last segment — enough to catch the common oversight.
        let candidate = rule.id.rsplit('.').next().unwrap_or(rule.id);
        for protocol in corpus.protocols() {
            assert!(
                !protocol.id.starts_with(candidate),
                "rule {} is Pending but protocol {:?} exists; promote the rule to Active",
                rule.id,
                protocol.id
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The corpus is searchable, not just loadable
// ---------------------------------------------------------------------------

/// `retrieval.rs::is_search_noise` drops closed-class function words at index time. A
/// title or a search phrase made only of those words indexes to nothing at all: it
/// cannot be matched, and — worse — it looks like working content in a diff.
///
/// This catches the plausible authoring slip, which is an `also_called` entry written as
/// a question ("what do i do", "is it about me") rather than as the words someone
/// searches for.
#[test]
fn no_title_or_search_phrase_is_made_only_of_noise_words() {
    for protocol in corpus().protocols() {
        let mut phrases = vec![("title", protocol.title.as_str())];
        phrases.extend(
            protocol
                .also_called
                .iter()
                .map(|phrase| ("also_called", phrase.as_str())),
        );
        for (field, phrase) in phrases {
            let survives = word_set(phrase)
                .into_iter()
                .any(|word| !is_search_noise(&word));
            assert!(
                survives,
                "protocol {:?} has a {field} of {phrase:?}, which is entirely function \
                 words and therefore indexes to nothing searchable",
                protocol.id
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Step semantics
// ---------------------------------------------------------------------------

/// `data/firstaid/SCHEMA.md` invariant 1. A card must not open by telling a frightened
/// person to put hands on a patient they may have reached by mistake.
#[test]
fn no_protocol_opens_with_an_action() {
    for protocol in corpus().protocols() {
        let first = protocol.first_step().expect("validated to have steps");
        assert_ne!(
            first.kind,
            StepKind::Action,
            "protocol {:?} opens with an action: {:?}",
            protocol.id,
            first.text
        );
    }
}

/// The specific case the red-flag layer's accepted overtriage depends on.
/// See `core/src/redflag.rs`, "Known, deliberate overtriage".
#[test]
fn cpr_card_opens_with_an_assessment_step() {
    let corpus = corpus();
    let cpr = corpus.get("cpr.adult").expect("cpr.adult must exist");
    let first = cpr.step(1).expect("step 1");
    assert_eq!(
        first.kind,
        StepKind::Assessment,
        "cpr.adult step 1 must be an assessment, or the 'I cant breathe' overtriage in \
         redflag.rs stops being safe: {:?}",
        first.text
    );
}

/// Every protocol has to tell the reader to get help at some point. A card that never
/// escalates is a card that quietly encourages someone to handle it alone.
#[test]
fn every_protocol_escalates_somewhere() {
    for protocol in corpus().protocols() {
        let escalates = protocol
            .steps
            .iter()
            .any(|step| step.kind == StepKind::Escalation);
        assert!(
            escalates,
            "protocol {:?} never tells anyone to call for help",
            protocol.id
        );
    }
}

// ---------------------------------------------------------------------------
// Content boundaries
// ---------------------------------------------------------------------------

/// The corpus is held to the same line as a rendering. A protocol that names a drug or
/// a dose has stopped being first aid, and the verifier would then pass a rendering
/// that repeats it, because the verifier's check is scoped per protocol.
#[test]
fn no_step_names_a_drug_or_a_dose() {
    for protocol in corpus().protocols() {
        for step in &protocol.steps {
            for word in word_set(&step.text) {
                assert!(
                    !is_medication_vocabulary(&word),
                    "protocol {:?} step {} names {word:?}, which is prescribing, not first aid",
                    protocol.id,
                    step.n
                );
            }
        }
    }
}

/// Provenance is a field, not a vibe (`docs/CONVENTIONS.md` §9). Citations must be
/// real strings, and every protocol must be honest about review status.
#[test]
fn every_protocol_declares_where_it_came_from() {
    for protocol in corpus().protocols() {
        assert!(
            !protocol.citations.is_empty(),
            "protocol {:?} has no citations",
            protocol.id
        );
        for citation in &protocol.citations {
            assert!(
                citation.source.len() > 10,
                "protocol {:?} has a citation that says almost nothing: {:?}",
                protocol.id,
                citation.source
            );
        }
        // Not asserting `reviewed_by.is_none()` — that would fail the day a clinician
        // signs off, which is the wrong incentive. The invariant is the pairing.
        assert_eq!(
            protocol.reviewed_by.is_some(),
            protocol.reviewed_at.is_some(),
            "protocol {:?} has half a review record",
            protocol.id
        );
    }
}

/// Nothing in this repository has been signed off by a named clinician. This test is
/// here so that fact is visible in test output rather than buried in a JSON field, and
/// so promoting a protocol to reviewed is a deliberate act that touches this file.
#[test]
fn the_review_backlog_is_stated_out_loud() {
    let corpus = corpus();
    let unreviewed: Vec<&str> = corpus
        .protocols()
        .filter(|p| !p.is_clinically_reviewed())
        .map(|p| p.id.as_str())
        .collect();
    assert_eq!(
        unreviewed.len(),
        corpus.len(),
        "a protocol claims clinical review; if that is real, update this test and \
         PLAN.md §10, and put the reviewer's name in the file"
    );
}

// ---------------------------------------------------------------------------
// Reading level — PLAN.md §8 gates this at grade 6
// ---------------------------------------------------------------------------

/// `PLAN.md` §8: Flesch–Kincaid ≤ grade 6. Someone reading a card while a person in
/// front of them is dying is not reading carefully.
///
/// The grade itself comes from [`prohori_core::readability::grade`] rather than from a
/// helper in this file. It used to live here, which was fine until the shipping gate in
/// `prohori_core::eval` needed the same number: two implementations of one threshold
/// drift, and the day they disagree is the day this test passes at 5.9 while the gate
/// report fails at 6.1.
#[test]
fn every_protocol_reads_at_grade_six_or_below() {
    for protocol in corpus().protocols() {
        let text = protocol
            .steps
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let grade = readability::grade(&text);
        // Printed so `cargo test -- --nocapture` shows the margin, not just a pass.
        // A protocol sitting at 5.9 is one edit away from failing a shipping gate.
        eprintln!("{:<24} grade {grade:.1}", protocol.id);
        assert!(
            grade <= 6.0,
            "protocol {:?} reads at grade {grade:.1}; PLAN.md §8 gates this at 6",
            protocol.id
        );
    }
}

/// A declared field nobody checks is provenance theatre. The tolerance is wide because
/// the syllable heuristic is approximate; the point is to catch a file claiming
/// grade 3 for prose that reads at grade 9.
#[test]
fn the_declared_reading_grade_is_not_wishful() {
    for protocol in corpus().protocols() {
        let text = protocol
            .steps
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let measured = readability::grade(&text);
        let declared = f64::from(protocol.reading_grade);
        assert!(
            (declared - measured).abs() <= 2.0,
            "protocol {:?} declares grade {declared} but measures {measured:.1}",
            protocol.id
        );
    }
}

// ---------------------------------------------------------------------------
// End to end: the card a user sees with no model on the device at all
// ---------------------------------------------------------------------------

/// P0 ships with no model. This is the whole user-visible output path for a red flag,
/// exercised without one.
#[test]
fn a_red_flag_renders_a_complete_card_with_no_model_present() {
    let corpus = corpus();
    let assessment = prohori_core::redflag::assess("my father is not breathing, help");
    let card = assessment.card().expect("an active rule fired");
    let protocol = corpus
        .get(card.protocol_id.expect("active rules carry an id"))
        .expect("resolves");

    let rendered = protocol.render_verbatim();
    assert!(rendered.contains(&protocol.title));
    assert!(
        rendered.contains("Do not"),
        "the warnings block must survive into the card"
    );
    for step in &protocol.steps {
        assert!(
            rendered.contains(step.text.as_str()),
            "step {} missing from the verbatim card",
            step.n
        );
    }
}

/// The verifier and the corpus have to agree about tokenization, or the first real
/// model rendering will be rejected for a reason nobody can find. A protocol's own
/// renderable text is the trivial case, and it must pass.
#[test]
fn a_protocol_passes_its_own_verifier() {
    for protocol in corpus().protocols() {
        let text = protocol.renderable_text();
        assert!(
            verify_rendering(protocol, &text).is_ok(),
            "protocol {:?} fails its own verifier",
            protocol.id
        );
    }
}
