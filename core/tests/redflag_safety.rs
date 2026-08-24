//! Safety suite for the red-flag layer.
//!
//! `docs/CONVENTIONS.md` §1: a safety invariant lands with its test in the same
//! commit. These are those tests.
//!
//! The negative cases matter more than the positive ones. A rule that fires when it
//! should is easy to write; a table that stays quiet on "he is breathing normally"
//! is the thing that proves the design in `redflag`'s module docs actually holds.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use prohori_core::normalize::normalize;
use prohori_core::redflag::{RULES, RedFlagAssessment, RedFlagHit, RuleStatus, assess};
use prohori_core::severity::Severity;
use proptest::prelude::*;
use std::collections::HashSet;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Table integrity — the rule table checks itself
// ---------------------------------------------------------------------------

/// Catches a trigger written in prose form. `"bleeding badly"` normalizes to
/// `"bleed bad"`, so writing the prose version would silently never match.
#[test]
fn every_trigger_is_already_in_canonical_form() {
    for rule in RULES {
        for trigger in rule.triggers {
            let canonical = normalize(trigger);
            assert_eq!(
                canonical.trim(),
                *trigger,
                "rule {} trigger {:?} is not canonical — write it as {:?}",
                rule.id,
                trigger,
                canonical.trim()
            );
        }
    }
}

#[test]
fn rule_ids_are_unique() {
    let mut seen = HashSet::new();
    for rule in RULES {
        assert!(seen.insert(rule.id), "duplicate rule id {}", rule.id);
    }
}

#[test]
fn no_rule_has_an_empty_trigger_list() {
    for rule in RULES {
        assert!(
            !rule.triggers.is_empty(),
            "rule {} matches nothing",
            rule.id
        );
        for trigger in rule.triggers {
            assert!(!trigger.is_empty(), "rule {} has an empty trigger", rule.id);
        }
    }
}

/// The honesty invariant. An active rule must be able to show a card; a pending one
/// must not pretend it can.
#[test]
fn protocol_is_present_exactly_when_the_rule_is_active() {
    for rule in RULES {
        match rule.status {
            RuleStatus::Active => assert!(
                rule.protocol_id.is_some(),
                "active rule {} has no protocol to render",
                rule.id
            ),
            RuleStatus::Pending => assert!(
                rule.protocol_id.is_none(),
                "pending rule {} points at a protocol; mark it Active",
                rule.id
            ),
        }
    }
}

#[test]
fn every_red_flag_is_critical() {
    for rule in RULES {
        assert_eq!(
            rule.severity,
            Severity::Critical,
            "rule {} is in the red-flag table but is not Critical",
            rule.id
        );
        assert!(rule.severity.bypasses_model());
    }
}

// ---------------------------------------------------------------------------
// Positive cases, in the phrasing the §8 eval gate actually targets
// ---------------------------------------------------------------------------

#[test]
fn recognises_arrest_across_spelling_and_second_language_phrasing() {
    for message in [
        "He is not breathing",
        "he not breathing",
        "my father is not breathin",
        "She stopped breathing",
        "no breath at all",
        "there is no sign of breathing",
    ] {
        let hit = assess(message);
        assert_eq!(
            hit.card().map(|h| h.rule_id),
            Some("rf.airway.not_breathing"),
            "{message:?} should show the CPR card"
        );
    }
}

#[test]
fn recognises_absent_circulation() {
    for message in [
        "no pulse",
        "I cannot find his pulse",
        "cant feel a pulse",
        "no heartbeat",
        "his heart stopped",
        "cardiac arrest",
    ] {
        let assessment = assess(message);
        assert!(
            assessment
                .hits
                .iter()
                .any(|h| h.rule_id == "rf.circulation.no_pulse"),
            "{message:?} should fire the no-pulse rule"
        );
    }
}

#[test]
fn recognises_catastrophic_bleeding() {
    for message in [
        "bleeding badly",
        "he is bleeding a lot",
        "there is a lot of blood",
        "blood is spurting out",
        "the bleeding will not stop",
        "blood soaked through the cloth",
        "deep cut on his arm",
    ] {
        let assessment = assess(message);
        assert_eq!(
            assessment.card().and_then(|h| h.protocol_id),
            Some("bleeding.severe"),
            "{message:?} should show the bleeding card"
        );
    }
}

#[test]
fn recognises_choking_and_unresponsiveness() {
    assert_eq!(
        assess("he is choking").card().map(|h| h.rule_id),
        Some("rf.airway.choking")
    );
    assert_eq!(
        assess("food stuck in his throat").card().map(|h| h.rule_id),
        Some("rf.airway.choking")
    );
    for message in [
        "she is unconscious",
        "he passed out",
        "he not waking",
        "she is not responding",
        "he wont wake up",
    ] {
        assert_eq!(
            assess(message).card().map(|h| h.rule_id),
            Some("rf.consciousness.unresponsive"),
            "{message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Negative cases — the design proof
// ---------------------------------------------------------------------------

/// The single most important test in the file. Bare `breathe` is not a trigger, so
/// a *present* vital sign must not fire the arrest rule. If this fails, the module's
/// "no negation analysis needed" claim is false and the design has to change.
#[test]
fn present_vital_signs_never_fire_the_arrest_rules() {
    for message in [
        "he is breathing normally",
        "she is breathing",
        "he has a pulse",
        "she is conscious and talking",
        "the bleeding stopped",
        "he is responding to me",
    ] {
        let assessment = assess(message);
        assert!(
            assessment.is_empty(),
            "{message:?} must fire nothing, got {:?}",
            assessment
                .hits
                .iter()
                .map(|h| h.rule_id)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn ordinary_text_fires_nothing() {
    for message in [
        "what is the weather today",
        "he is fit and healthy",
        "i have a headache",
        "my throat hurts a little",
        "there is no emergency",
        "",
        "   ",
        "?!?!",
    ] {
        let assessment = assess(message);
        assert!(
            assessment.is_empty(),
            "{message:?} must fire nothing, got {:?}",
            assessment
                .hits
                .iter()
                .map(|h| h.rule_id)
                .collect::<Vec<_>>()
        );
    }
}

/// `fit` must not have been folded onto `seizure` in `normalize`.
/// See `docs/CONVENTIONS.md` §6.
#[test]
fn being_fit_is_not_a_seizure() {
    assert!(assess("he is fit and healthy").is_empty());
    // But the idiomatic phrasing still works, because it lives in the trigger list.
    assert!(
        assess("he is having a fit")
            .hits
            .iter()
            .any(|h| h.rule_id == "rf.neuro.seizure")
    );
}

// ---------------------------------------------------------------------------
// Documented, deliberate overtriage
// ---------------------------------------------------------------------------

/// "I can't breathe" fires the arrest rule as well as the distress rule, because
/// `" can not breathe "` contains `" not breathe "`. Accepted rather than
/// special-cased — see the `redflag` module docs. The protocol's own first step is
/// the guard, asserted in `corpus_integrity.rs`.
#[test]
fn conscious_breathlessness_overtriages_to_cpr_by_design() {
    let assessment = assess("I cant breathe");
    let fired: Vec<_> = assessment.hits.iter().map(|h| h.rule_id).collect();
    assert!(fired.contains(&"rf.airway.not_breathing"));
    assert!(fired.contains(&"rf.breathing.severe_distress"));
}

/// Negation is not analysed, so an intensity word behind a negation still fires.
/// This is the accepted direction of error: a card too many, never one too few.
#[test]
fn negated_intensity_still_fires_rather_than_risking_silence() {
    assert!(
        !assess("he is not bleeding badly").is_empty(),
        "overtriage here is the intended behaviour"
    );
    // Without an intensity word there is nothing to match, so this stays quiet.
    assert!(assess("he is not bleeding").is_empty());
}

// ---------------------------------------------------------------------------
// Assessment semantics
// ---------------------------------------------------------------------------

#[test]
fn card_is_the_highest_priority_active_hit() {
    let assessment = assess("my father is not breathing and there is a lot of blood");
    let fired: Vec<_> = assessment.hits.iter().map(|h| h.rule_id).collect();
    assert!(fired.contains(&"rf.airway.not_breathing"));
    assert!(fired.contains(&"rf.bleeding.severe"));
    assert_eq!(
        assessment.card().map(|h| h.rule_id),
        Some("rf.airway.not_breathing"),
        "CPR outranks bleeding in the provisional ordering"
    );
    // Hits arrive in priority order and are never re-sorted.
    let priorities: Vec<_> = assessment.hits.iter().map(|h| h.priority).collect();
    let mut sorted = priorities.clone();
    sorted.sort_unstable();
    assert_eq!(priorities, sorted);
}

/// Every rule in the shipped table must end somewhere a reader can act. Not "the rule
/// fires" — that is easy — but "something renders", which is the claim a user cares
/// about.
///
/// The card that comes back is deliberately not asserted to belong to the rule whose
/// trigger was typed. Several rules share triggers by design ("can not breathe" fires
/// both the arrest rule and the distress rule), and priority decides between them. What
/// must never happen is a trigger that fires and leaves the screen empty.
#[test]
fn no_rule_in_the_table_leads_to_silence() {
    for rule in RULES {
        for trigger in rule.triggers {
            let assessment = assess(trigger);
            assert!(
                !assessment.is_empty(),
                "rule {} trigger {trigger:?} fires nothing at all",
                rule.id
            );
            let has_card = assessment.card().is_some();
            let has_admission = !assessment.unsupported().is_empty();
            assert!(
                has_card || has_admission,
                "rule {} trigger {trigger:?} is recognised as {:?} and then shows the \
                 reader nothing — neither a card nor an admission that we have none",
                rule.id,
                assessment.severity()
            );
        }
    }
}

/// A recognised emergency with no authored protocol must report itself as such.
/// "We know this is critical and have no guidance" and "we found nothing" are very
/// different messages, and collapsing them is the failure this test prevents.
///
/// Every rule in [`RULES`] now has a card, so this runs against a hit built here rather
/// than against the shipped table. That is on purpose: the pending path is the one that
/// gets exercised only on the day someone writes a rule ahead of its card, which is
/// exactly when nobody wants to discover it silently drops the hit.
#[test]
fn a_rule_with_no_card_is_recognised_out_loud() {
    let assessment = RedFlagAssessment {
        hits: vec![RedFlagHit {
            rule_id: "rf.test.unwritten",
            protocol_id: None,
            severity: Severity::Critical,
            status: RuleStatus::Pending,
            matched: "unwritten",
            priority: 99,
        }],
    };

    assert!(!assessment.is_empty());
    assert_eq!(assessment.severity(), Some(Severity::Critical));
    assert!(
        assessment.card().is_none(),
        "no protocol exists, so no card may be claimed"
    );
    let unsupported = assessment.unsupported();
    assert_eq!(unsupported.len(), 1);
    assert_eq!(unsupported[0].rule_id, "rf.test.unwritten");
    assert!(unsupported[0].protocol_id.is_none());
}

/// The seizure case the old version of the test above used, now that it has a card.
/// Kept because it is the phrasing a real caller types, and because it pins the
/// promotion: if `seizure.active` were ever removed, the rule would go back to Pending
/// and this would fail rather than degrading quietly to an admission.
#[test]
fn a_reported_seizure_gets_the_seizure_card() {
    let assessment = assess("he is having a seizure");
    assert_eq!(assessment.severity(), Some(Severity::Critical));
    let card = assessment.card().expect("the seizure rule is active");
    assert_eq!(card.rule_id, "rf.neuro.seizure");
    assert_eq!(card.protocol_id, Some("seizure.active"));
    assert!(
        assessment.unsupported().is_empty(),
        "nothing about a seizure is unsupported any more"
    );
}

#[test]
fn a_rule_fires_at_most_once_even_when_several_triggers_match() {
    // "food stuck in throat" matches both "food stuck" and "stuck in throat".
    let assessment = assess("there is food stuck in his throat");
    let choking = assessment
        .hits
        .iter()
        .filter(|h| h.rule_id == "rf.airway.choking")
        .count();
    assert_eq!(choking, 1);
}

#[test]
fn the_matched_trigger_is_reported_for_the_trace() {
    let assessment = assess("he is not breathing");
    let hit = assessment.card().unwrap();
    assert_eq!(hit.matched, "not breathe");
}

#[test]
fn assessment_is_deterministic() {
    let message = "she is unconscious and bleeding badly";
    assert_eq!(assess(message), assess(message));
}

// ---------------------------------------------------------------------------
// Budget and robustness
// ---------------------------------------------------------------------------

/// `PLAN.md` §3 budgets 100 ms for the red-flag card end to end. The matcher is a
/// small fraction of that. 1000 debug-build passes inside the whole budget leaves
/// roughly three orders of magnitude of headroom for UI and FFI.
///
/// The measurement is the *fastest* of several rounds, not the average and not a single
/// run. `cargo test` runs this file's two dozen tests in parallel, so a wall-clock timing
/// taken once measures how busy the machine was as much as how fast the matcher is — this
/// test failed at 104 ms in a full-suite run and passed at 30 ms alone, which says nothing
/// about the code. Contention can only make a round slower, never faster, so the minimum
/// is the tightest honest upper bound on the real cost. A regression that matters shows up
/// in every round, including the best one.
#[test]
fn matching_is_far_inside_the_latency_budget() {
    let message = "my father is not breathing and bleeding badly from a deep cut";
    let mut fastest = Duration::MAX;
    for _ in 0..5 {
        let start = Instant::now();
        for _ in 0..1000 {
            let _ = assess(message);
        }
        fastest = fastest.min(start.elapsed());
    }
    assert!(
        fastest.as_millis() < 100,
        "1000 assessments took {fastest:?} at best; the whole card budget is 100 ms"
    );
}

proptest! {
    /// No input crashes the layer. `core/` forbids unwrap, expect, panic, and slice
    /// indexing precisely so this holds for text nobody thought to write a case for.
    #[test]
    fn assess_never_panics(input in ".*") {
        let _ = assess(&input);
    }

    #[test]
    fn normalize_always_returns_padded_output(input in ".*") {
        let out = normalize(&input);
        prop_assert!(out.starts_with(' '));
        prop_assert!(out.ends_with(' '));
        prop_assert!(!out.contains("  "), "no double spaces: {out:?}");
    }
}
