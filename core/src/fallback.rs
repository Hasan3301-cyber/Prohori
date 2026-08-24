//! Model-written guidance for the case the corpus does not cover.
//!
//! `PLAN.md` §1 forbids the model from authoring *reviewed* medical content, and
//! everything in [`crate::verifier`] exists to enforce that against a source of truth.
//! This module handles the one case where there is no source of truth to check against:
//! the red-flag table found nothing, retrieval found nothing, and the person typing is
//! still in front of someone who is hurt. Crush injury under a slab, a wound going bad on
//! day three, an unplanned birth, water that may not be safe — none of those are in
//! `data/firstaid/`, and all of them happen in the disasters this app is for.
//!
//! # What makes this safe enough to ship
//!
//! Not a promise about the model. Four mechanical constraints, in order of strength:
//!
//! 1. **A digit cannot be sampled.** `data/grammar/fallback.gbnf` removes `0-9` from the
//!    character class and deletes the `\uXXXX` escape branch. There is no dose, no depth,
//!    no count of tablets, and no compression rate — not "rejected afterwards", but
//!    unreachable at the sampler. [`validate`] still refuses Unicode digits, because a
//!    negated ASCII class permits `০` and `٣`.
//! 2. **A spelled-out quantity is refused.** A grammar cannot see "three hundred", so
//!    [`NUMBER_WORDS`] does.
//! 3. **Drug, dose, procedure and invasive vocabulary is refused outright.** Reusing
//!    [`crate::verifier::is_medication_vocabulary`] and
//!    [`crate::verifier::is_authoring_stem`] rather than a second copy of the same lists,
//!    plus [`INVASIVE_TERMS`] for the things a bystander must not be told to do to a body.
//! 4. **The reading grade gate is the corpus's own gate.** [`crate::readability::grade`],
//!    the same function `tests/corpus_integrity.rs` uses, at the same threshold.
//!
//! # Why being this strict is affordable
//!
//! Because refusing costs a paragraph, not the answer. The cited safety-net card in
//! [`crate::guidance`] renders first and always — with no model on the device at all — so
//! every refusal here degrades to "the general approach to a casualty, cited", never to a
//! blank screen. That is what lets the checks be blunt instead of clever.
//!
//! Blunt specifically means **polarity-blind**, the same limitation
//! [`crate::verifier`] documents. "Do not try to suck out the venom" is good advice and
//! this module refuses it, because the alternative is trusting the model to have got the
//! polarity right, and the check that distinguishes the two does not exist. A false
//! refusal costs the extra paragraph. The other error costs a life.
//!
//! # What is not checked
//!
//! Stated plainly, in the habit of [`crate::verifier`]:
//!
//! - **Correctness.** Nothing here knows whether the advice is right. It knows the advice
//!   contains no number, no drug and no procedure, and reads at grade six.
//! - **Omission.** A step the model failed to write is invisible.
//! - **Roman numerals.** "III" is not a digit and not in [`NUMBER_WORDS`]. It survives
//!   the number checks and is caught only if it is attached to a banned unit.
//! - **Ordinals.** "first" and "second" are permitted, because "look around first" is
//!   ordinary English and banning it would refuse most safe answers.

use crate::readability;
use crate::redflag::RedFlagAssessment;
use crate::retrieval::Hit;
use crate::verifier::{is_authoring_stem, is_medication_vocabulary, word_set};
use serde::Deserialize;
use std::fmt;

/// The grammar the sampler runs under. Embedded for the reason [`crate::bundled`] gives:
/// the bytes CI checked are the bytes on the phone.
pub const FALLBACK_GBNF: &str = include_str!("../../data/grammar/fallback.gbnf");

/// The system prompt for the unmatched-query turn.
pub const FALLBACK_SYSTEM_PROMPT: &str = include_str!("../../data/prompts/fallback-system.txt");

/// One sentence, authored here rather than in Kotlin.
///
/// Same reasoning as `FirstAidCard::provenance`: this sentence has to appear on the screen
/// and in anything shared out of the app, and two copies of a sentence about medical
/// authority is one copy too many.
pub const DISCLAIMER: &str = "The model on this phone wrote this itself. It is not from \
                              the reviewed guidance in this app, no clinician has checked \
                              it, and it contains no doses or measurements because it is \
                              not allowed to write any. Keep calling for help.";

/// Shortest input worth generating for. Below this the fallback would fire on a
/// half-finished word while someone is still typing.
pub const MIN_WORDS: usize = 3;
/// Companion to [`MIN_WORDS`], in characters, so "a b c" does not qualify.
pub const MIN_CHARS: usize = 12;
/// Bounds matching `data/grammar/fallback.gbnf`. Checked again here because [`validate`]
/// is also run against output that did not come from that grammar — the host probe, and
/// any future engine.
pub const MAX_STEPS: usize = 6;
/// Ditto, for the warning list.
pub const MAX_WARNINGS: usize = 3;
/// Ditto, per sentence.
pub const MAX_SENTENCE_CHARS: usize = 140;
/// The corpus's own reading gate (`PLAN.md` §8), applied to model-written text.
pub const MAX_GRADE: f64 = 6.0;

/// Spelled-out quantities. A grammar cannot see these; the digit ban would be theatre
/// without them.
///
/// Ordinals are deliberately absent — see the module docs.
pub const NUMBER_WORDS: &[&str] = &[
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
    "twenty",
    "thirty",
    "forty",
    "fifty",
    "sixty",
    "seventy",
    "eighty",
    "ninety",
    "hundred",
    "thousand",
    "half",
    "quarter",
    "double",
    "triple",
    "twice",
    "thrice",
    "dozen",
    // Units of substance and of length. Without a number these are nearly meaningless,
    // which is the point: text reaching for one is text reaching for a measurement.
    "teaspoon",
    "teaspoons",
    "tablespoon",
    "tablespoons",
    "spoon",
    "spoonful",
    "spoonfuls",
    "cup",
    "cups",
    "litre",
    "litres",
    "liter",
    "liters",
    "gram",
    "grams",
    "kilogram",
    "kilograms",
    "ounce",
    "ounces",
    "inch",
    "inches",
    "cm",
    "centimetre",
    "centimetres",
    "centimeter",
    "centimeters",
    "degrees",
];

/// Things a bystander must never be told to do to a body by a program with no reviewer.
///
/// Matched as prefixes, like [`crate::verifier::is_authoring_stem`]. `syringe`, `needle`,
/// `stitch` and `surgery` are absent because
/// [`crate::verifier::is_medication_vocabulary`] already carries them, and one list per
/// concept is the rule.
pub const INVASIVE_TERMS: &[&str] = &[
    "tourniquet",
    "incis",
    "cauter",
    "scalpel",
    "lanc",
    "punctur",
    "pierc",
    "suck",
    "venom",
];

/// Whether the fallback may run at all, and why not when it may not.
///
/// Every refusal is named so the local trace can say which layer answered instead of
/// leaving a silent screen to be explained by guesswork.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Permission {
    /// Nothing deterministic had an answer. The model may write one.
    Allowed,
    /// A red-flag rule fired. `docs/CONVENTIONS.md` §7 and §10: the rule layer always
    /// wins, and nothing the model writes may appear beside a critical card.
    RedFlagFired { rule_id: String },
    /// Retrieval found a card, so the corpus does cover this.
    CorpusMatched { protocol_id: String },
    /// Too little text to be a report. Guards the automatic trigger against firing on a
    /// half-typed word.
    TooShort,
}

impl Permission {
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }

    #[must_use]
    pub fn reason(&self) -> Option<String> {
        match self {
            Self::Allowed => None,
            Self::RedFlagFired { rule_id } => {
                Some(format!("rule {rule_id} fired; the rule layer answers"))
            }
            Self::CorpusMatched { protocol_id } => {
                Some(format!("the corpus covers this: {protocol_id}"))
            }
            Self::TooShort => Some("not enough text yet".to_owned()),
        }
    }
}

/// Decide whether model-written guidance is permitted for this message.
///
/// Takes the real assessment and the strict [`crate::retrieval::Index::template_search`]
/// hit list rather than two booleans, so a caller cannot claim "nothing matched" without
/// having asked. `hits` emptiness is the test — **not** a score threshold. Ordinary BM25
/// suggestions are intentionally insufficient: a complete lay phrase declared by a card
/// must be present before generated guidance is suppressed.
#[must_use]
pub fn permission(message: &str, rules: &RedFlagAssessment, hits: &[Hit]) -> Permission {
    if let Some(hit) = rules.hits.first() {
        return Permission::RedFlagFired {
            rule_id: hit.rule_id.to_owned(),
        };
    }
    if let Some(hit) = hits.first() {
        return Permission::CorpusMatched {
            protocol_id: hit.protocol_id.clone(),
        };
    }
    let trimmed = message.trim();
    if trimmed.chars().count() < MIN_CHARS
        || crate::normalize::normalize(trimmed)
            .split_whitespace()
            .count()
            < MIN_WORDS
    {
        return Permission::TooShort;
    }
    Permission::Allowed
}

/// [`permission`] as a bool, for call sites that only gate on it.
#[must_use]
pub fn permitted(message: &str, rules: &RedFlagAssessment, hits: &[Hit]) -> bool {
    permission(message, rules, hits).is_allowed()
}

/// Guidance the model wrote, after every check in this module passed.
///
/// Deliberately not a [`crate::protocol::Protocol`] and deliberately not convertible into
/// one. A card carries citations and a review status; this carries neither, and a type that
/// could be handed to the card renderer by mistake would eventually be handed to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelWrittenGuidance {
    /// One sentence to a frightened person before the instructions start.
    pub reassurance: String,
    /// What to do, in order. Never empty.
    pub steps: Vec<String>,
    /// What makes it worse. May be empty.
    pub do_not: Vec<String>,
}

impl ModelWrittenGuidance {
    /// Every sentence the model wrote, in display order. What the checks run over.
    pub fn sentences(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.reassurance.as_str())
            .chain(self.steps.iter().map(String::as_str))
            .chain(self.do_not.iter().map(String::as_str))
    }
}

/// Why model-written guidance was thrown away.
///
/// One variant per check so a trace, the host probe, and a future red-team report all name
/// the same thing.
#[derive(Debug, Clone, PartialEq)]
pub enum FallbackError {
    /// Not the JSON the grammar describes.
    Malformed { message: String },
    /// Zero steps. Nothing to show is a failure, as in
    /// [`crate::verifier::Violation::Empty`].
    NoSteps,
    /// More list entries than the grammar permits.
    TooMany { field: &'static str, count: usize },
    /// A sentence over [`MAX_SENTENCE_CHARS`], or an empty one.
    BadSentence { chars: usize },
    /// A digit, a Unicode digit, or a spelled-out quantity.
    Number { token: String },
    /// A drug, a dose, or a dosing unit.
    Medication { term: String },
    /// A clinical procedure verb.
    ClinicalProcedure { term: String },
    /// An instruction to cut, pierce, tie, or suck.
    Invasive { term: String },
    /// Above [`MAX_GRADE`] on the corpus's own reading metric.
    TooHardToRead { grade: f64 },
    /// `call_now` was not `true`. Nothing here has the authority to say do not call.
    WouldNotCallForHelp,
}

impl fmt::Display for FallbackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { message } => write!(f, "not the required JSON — {message}"),
            Self::NoSteps => write!(f, "no steps, so there is nothing to show"),
            Self::TooMany { field, count } => write!(f, "{count} entries in {field} is too many"),
            Self::BadSentence { chars } => write!(f, "a sentence of {chars} characters"),
            Self::Number { token } => {
                write!(f, "wrote a quantity ({token:?}), which it may never do")
            }
            Self::Medication { term } => write!(f, "named {term:?}, which is prescribing"),
            Self::ClinicalProcedure { term } => write!(f, "named the procedure {term:?}"),
            Self::Invasive { term } => write!(f, "told someone to {term:?}"),
            Self::TooHardToRead { grade } => {
                write!(f, "reads at grade {grade:.1}, above the grade-six gate")
            }
            Self::WouldNotCallForHelp => write!(f, "did not keep calling for help"),
        }
    }
}

impl std::error::Error for FallbackError {}

/// The wire shape, exactly as `data/grammar/fallback.gbnf` produces it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGuidance {
    schema_version: String,
    reassurance: String,
    steps: Vec<String>,
    #[serde(default)]
    do_not: Vec<String>,
    call_now: bool,
}

/// Check model-written guidance and return it only if every check passes.
///
/// Checks run in a fixed order and the first failure is returned, so the same output always
/// produces the same refusal — `docs/CONVENTIONS.md` §2 in spirit.
pub fn validate(json: &str) -> Result<ModelWrittenGuidance, FallbackError> {
    let raw: RawGuidance = serde_json::from_str(json).map_err(|err| FallbackError::Malformed {
        message: err.to_string(),
    })?;

    if raw.schema_version != "1" {
        return Err(FallbackError::Malformed {
            message: format!("schema_version {:?}", raw.schema_version),
        });
    }
    if !raw.call_now {
        return Err(FallbackError::WouldNotCallForHelp);
    }

    let guidance = ModelWrittenGuidance {
        reassurance: raw.reassurance.trim().to_owned(),
        steps: raw
            .steps
            .iter()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect(),
        do_not: raw
            .do_not
            .iter()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect(),
    };

    if guidance.steps.is_empty() {
        return Err(FallbackError::NoSteps);
    }
    if guidance.steps.len() > MAX_STEPS {
        return Err(FallbackError::TooMany {
            field: "steps",
            count: guidance.steps.len(),
        });
    }
    if guidance.do_not.len() > MAX_WARNINGS {
        return Err(FallbackError::TooMany {
            field: "do_not",
            count: guidance.do_not.len(),
        });
    }

    if guidance.reassurance.is_empty() {
        return Err(FallbackError::BadSentence { chars: 0 });
    }
    for sentence in guidance.sentences() {
        let chars = sentence.chars().count();
        if chars > MAX_SENTENCE_CHARS {
            return Err(FallbackError::BadSentence { chars });
        }
        // Any Unicode digit, not just ASCII: the grammar's negated class permits `০` and
        // `٣`, and a Bengali numeral is a dose too.
        if let Some(digit) = sentence.chars().find(|c| c.is_numeric()) {
            return Err(FallbackError::Number {
                token: digit.to_string(),
            });
        }
    }

    // One tokenizer for all of it, and the verifier's tokenizer rather than a new one, so
    // a word is split here exactly as it is split when a corpus rendering is checked.
    let all: Vec<String> = guidance.sentences().map(str::to_owned).collect();
    for word in word_set(&all.join(" ")) {
        if NUMBER_WORDS.contains(&word.as_str()) {
            return Err(FallbackError::Number { token: word });
        }
        if is_medication_vocabulary(&word) {
            return Err(FallbackError::Medication { term: word });
        }
        if is_authoring_stem(&word) {
            return Err(FallbackError::ClinicalProcedure { term: word });
        }
        if INVASIVE_TERMS.iter().any(|term| word.starts_with(term)) {
            return Err(FallbackError::Invasive { term: word });
        }
    }

    let grade = readability::grade(&graded_text(&guidance));
    if grade > MAX_GRADE {
        return Err(FallbackError::TooHardToRead { grade });
    }

    Ok(guidance)
}

/// Join the sentences so [`crate::readability::grade`] counts them as sentences.
///
/// The grade formula divides words by terminal punctuation, with a floor of one sentence.
/// Corpus text always ends an instruction with a full stop; a model's list entry often does
/// not, and six unpunctuated entries joined with spaces would measure as one 150-word
/// sentence and fail a gate the text did not actually fail. So each entry that does not end
/// in terminal punctuation gets one, which is what the list already means.
///
/// Shaping the input rather than forking the function is deliberate: `readability`'s own
/// docs say the reason it lives in the library is that two implementations of a shipping
/// threshold drift.
fn graded_text(guidance: &ModelWrittenGuidance) -> String {
    let mut out = String::with_capacity(1024);
    for sentence in guidance.sentences() {
        out.push_str(sentence);
        if !sentence.ends_with(['.', '!', '?']) {
            out.push('.');
        }
        out.push(' ');
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::protocol::Corpus;
    use crate::redflag;
    use crate::retrieval::Index;

    fn json(steps: &[&str]) -> String {
        let steps = steps
            .iter()
            .map(|s| format!("{s:?}"))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"schema_version":"1","reassurance":"Stay with them. Help is coming.",
                "steps":[{steps}],"do_not":[],"call_now":true}}"#
        )
    }

    /// No cards at all, so "retrieval found nothing" is a fact about the fixture rather
    /// than a guess about how BM25F will score a sentence.
    fn empty_index() -> Index {
        Index::build(&Corpus::default())
    }

    fn bundled_index() -> Index {
        let (corpus, errors) = crate::bundled::corpus();
        assert!(errors.is_empty(), "the bundled corpus must load clean");
        Index::build(&corpus)
    }

    #[test]
    fn a_plain_answer_passes() {
        let guidance = validate(&json(&[
            "Keep pressing on the wound until the bleeding stops.",
            "Keep them warm and keep talking to them.",
        ]))
        .expect("plain, number-free text must pass");
        assert_eq!(guidance.steps.len(), 2);
        assert!(guidance.do_not.is_empty());
    }

    #[test]
    fn a_digit_is_refused_even_though_the_grammar_should_have_stopped_it() {
        let error = validate(&json(&["Press hard for 10 minutes."])).expect_err("refused");
        assert_eq!(
            error,
            FallbackError::Number {
                token: "1".to_owned()
            }
        );
    }

    /// The negated ASCII class in the grammar permits these, so the validator must not.
    #[test]
    fn a_non_ascii_digit_is_still_a_digit() {
        let error = validate(&json(&["Give them ৩ spoons of water."])).expect_err("refused");
        assert!(matches!(error, FallbackError::Number { .. }), "{error:?}");
    }

    #[test]
    fn a_spelled_out_quantity_is_refused() {
        for text in [
            "Press hard for three minutes.",
            "Give half a cup of water.",
            "Repeat this twice.",
        ] {
            let error = validate(&json(&[text])).expect_err("refused");
            assert!(matches!(error, FallbackError::Number { .. }), "{text}");
        }
    }

    #[test]
    fn a_drug_is_refused() {
        let error = validate(&json(&["Give them paracetamol for the pain."])).expect_err("refused");
        assert_eq!(
            error,
            FallbackError::Medication {
                term: "paracetamol".to_owned()
            }
        );
    }

    /// The case `is_medication_vocabulary` alone would have passed — the whole reason
    /// `verifier::is_authoring_stem` was added.
    #[test]
    fn a_procedure_verb_is_refused() {
        let error = validate(&json(&["Administer air into the mouth."])).expect_err("refused");
        assert_eq!(
            error,
            FallbackError::ClinicalProcedure {
                term: "administer".to_owned()
            }
        );
    }

    #[test]
    fn an_invasive_instruction_is_refused() {
        let error = validate(&json(&["Cut the bite open and suck it out."])).expect_err("refused");
        assert_eq!(
            error,
            FallbackError::Invasive {
                term: "suck".to_owned()
            }
        );
    }

    /// Polarity-blind, and documented as such: this is good advice and it is refused.
    #[test]
    fn a_safe_warning_about_a_banned_thing_is_refused_too() {
        let error = validate(&json(&["Do not tie a tourniquet on the neck."]))
            .expect_err("refused, deliberately");
        assert!(matches!(error, FallbackError::Invasive { .. }), "{error:?}");
    }

    #[test]
    fn clinical_prose_is_refused_on_reading_grade() {
        let error = validate(&json(&[
            "Commence uninterrupted external compressions whilst simultaneously arranging \
             definitive advanced intervention.",
        ]))
        .expect_err("refused");
        assert!(
            matches!(error, FallbackError::TooHardToRead { .. }),
            "{error:?}"
        );
    }

    /// Six unpunctuated entries must not fail a gate the text did not fail.
    #[test]
    fn unpunctuated_entries_are_graded_as_sentences() {
        let guidance = validate(&json(&[
            "Look around and make sure it is safe",
            "Call for help and say what you can see",
            "Keep them still and keep them warm",
            "Stay with them until help comes",
        ]))
        .expect("short plain entries must pass without full stops");
        assert_eq!(guidance.steps.len(), 4);
    }

    #[test]
    fn guidance_that_would_not_call_for_help_is_refused() {
        let text = json(&["Keep them warm."]).replace("\"call_now\":true", "\"call_now\":false");
        assert_eq!(validate(&text), Err(FallbackError::WouldNotCallForHelp));
    }

    #[test]
    fn no_steps_is_a_failure_not_an_empty_card() {
        assert_eq!(validate(&json(&[])), Err(FallbackError::NoSteps));
    }

    #[test]
    fn a_red_flag_takes_the_answer_away_from_the_model() {
        let index = empty_index();
        let message = "he is not breathing at all";
        let rules = redflag::assess(message);
        let hits = index.search(message, 3);
        assert!(matches!(
            permission(message, &rules, &hits),
            Permission::RedFlagFired { .. }
        ));
    }

    #[test]
    fn a_corpus_match_takes_the_answer_away_from_the_model() {
        let index = bundled_index();
        let message = "cool the burn with water";
        let rules = redflag::assess(message);
        let hits = index.search(message, 3);
        assert!(
            !hits.is_empty(),
            "this message is supposed to match the burn card"
        );
        assert_eq!(
            permission(message, &rules, &hits),
            Permission::CorpusMatched {
                protocol_id: "burn.thermal".to_owned()
            }
        );
    }

    #[test]
    fn a_half_typed_word_does_not_start_a_generation() {
        let index = empty_index();
        for message in ["", "  ", "he", "cru"] {
            let rules = redflag::assess(message);
            let hits = index.search(message, 3);
            assert_eq!(
                permission(message, &rules, &hits),
                Permission::TooShort,
                "{message:?}"
            );
        }
    }

    /// The whole point of the module: a real report that nothing deterministic answers.
    /// Asserted against an empty corpus here so the claim is structural; the same claim
    /// against the eighteen shipped cards lives in `tests/fallback_safety.rs`.
    #[test]
    fn an_unmatched_report_is_allowed() {
        let index = empty_index();
        let message = "my neighbour is trapped under a concrete slab";
        let rules = redflag::assess(message);
        let hits = index.search(message, 3);
        assert_eq!(permission(message, &rules, &hits), Permission::Allowed);
        assert!(permitted(message, &rules, &hits));
    }

    #[test]
    fn every_refusal_says_why() {
        assert!(Permission::Allowed.reason().is_none());
        for permission in [
            Permission::RedFlagFired {
                rule_id: "rf.airway.not_breathing".to_owned(),
            },
            Permission::CorpusMatched {
                protocol_id: "burn.thermal".to_owned(),
            },
            Permission::TooShort,
        ] {
            assert!(permission.reason().is_some_and(|r| !r.is_empty()));
        }
    }
}
