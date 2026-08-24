//! Mechanical enforcement of "the model never authors medical content".
//!
//! `PLAN.md` §1 gives the model three jobs, and none of them is writing medical text:
//! read the message, pick the protocol, render the protocol that was picked. This
//! module is the part that makes the third job checkable instead of merely intended.
//!
//! # How it works
//!
//! A rendering is compared against [`Protocol::renderable_text`], and anything that
//! appears in the rendering but not in the source is a violation:
//!
//! 1. **Numbers.** A digit sequence in the output must exist in the source. This is the
//!    check that stops "press 15 cm deep" and "give 300 of it".
//! 2. **Number–unit pairs.** `10 seconds` in the source does not license `10 minutes`
//!    in the output. Unit swapping is a real generation failure and a dangerous one,
//!    and a bare-number check alone would wave it through.
//! 3. **Clinical authoring vocabulary.** Words like `dose`, `mg`, `prescribe`,
//!    `inject`, `diagnose` are refused unless the source protocol already uses them.
//!    The list is derived from the forbidden-token check in EcoGuardian's
//!    `handover.py`, which blocked `diagnos|administer|prescrib|dosage|dose|mg|ml` on
//!    outbound clinical handovers.
//!
//! Note that check 3 is scoped per protocol, not globally. `cpr.adult` mentions a
//! defibrillator, so rendering that word is fine there and refused everywhere else —
//! which incidentally catches cross-protocol contamination, where a model blends the
//! card it was given with one it remembers.
//!
//! # What happens on failure
//!
//! Not "show nothing", and emphatically not "show it anyway". The caller falls back to
//! [`Protocol::render_verbatim`] — the protocol straight from the file. Use
//! [`rendering_or_source`] so that fallback is the default path rather than something a
//! caller has to remember. The worst case is prose that reads like a manual.
//!
//! # What this does not check
//!
//! Stated plainly, because a verifier that is trusted beyond its reach is worse than
//! none:
//!
//! - **Polarity.** "Do not loosen the band" and "loosen the band" share every token.
//!   This is handled structurally instead: `do_not` and `escalate_if` are rendered
//!   verbatim and never paraphrased, so a polarity inversion has no path into the UI.
//! - **Omission.** A rendering that quietly drops step 6 passes every check here.
//!   Coverage is a separate concern, gated by the faithfulness eval in `PLAN.md` §8.
//! - **Paraphrase drift** that invents no numbers and no clinical vocabulary. A model
//!   could still make a step vaguer than the source. That is what the eval set is for.
//!
//! The gate this module *does* close is the one where a plausible-sounding number or
//! drug name appears out of nowhere, which is the failure mode that hurts someone.

use crate::protocol::Protocol;
use std::collections::BTreeSet;
use std::fmt;

/// Word stems that mean the text has begun practising medicine rather than relaying
/// first aid. Matched as prefixes, so `inject` covers `injection` and `injector`.
const AUTHORING_STEMS: &[&str] = &[
    "diagnos",
    "prescrib",
    "administ",
    "inject",
    "dosag",
    "intubat",
    "defibrillat",
    "anaesthe",
    "anesthe",
    "sutur",
    "amputat",
    "cathet",
    "antibiot",
    "analges",
    "sedat",
];

/// Whole words that mean the same thing. Matched exactly, because these are short
/// enough that prefix matching would catch unrelated words (`ml` inside `mliterally`
/// is nonsense, but `cc` inside `according` is not).
const AUTHORING_WORDS: &[&str] = &[
    "dose",
    "doses",
    "mg",
    "ml",
    "mcg",
    "cc",
    "iu",
    "milligram",
    "milligrams",
    "millilitre",
    "millilitres",
    "milliliter",
    "milliliters",
    "tablet",
    "tablets",
    "pill",
    "pills",
    "capsule",
    "capsules",
    "syringe",
    "needle",
    "iv",
    "drip",
    "surgery",
    "operate",
    "stitch",
    "stitches",
    "medicine",
    "medication",
    "drug",
    "drugs",
    "aspirin",
    "adrenaline",
    "epinephrine",
    "paracetamol",
    "ibuprofen",
    "morphine",
    "insulin",
    "glucose",
    "oxygen",
];

/// One thing a rendering did that its source protocol does not license.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Violation {
    /// The rendering was blank or whitespace. Nothing to show is still a failure.
    Empty,
    /// A number that is not in the source protocol.
    InventedNumber { number: String },
    /// The number is in the source but attached to a different unit there.
    MismatchedUnit { number: String, unit: String },
    /// Clinical authoring vocabulary the source protocol does not use.
    ClinicalAuthoring { term: String },
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "rendering is empty"),
            Self::InventedNumber { number } => {
                write!(f, "number {number:?} does not appear in the protocol")
            }
            Self::MismatchedUnit { number, unit } => write!(
                f,
                "{number:?} {unit:?} does not appear in the protocol with that unit"
            ),
            Self::ClinicalAuthoring { term } => {
                write!(
                    f,
                    "term {term:?} is clinical authoring, not in the protocol"
                )
            }
        }
    }
}

impl std::error::Error for Violation {}

/// True for words that name a drug, a dose, or a dosing unit.
///
/// Exposed so the corpus itself can be held to the same line as a rendering:
/// `tests/corpus_integrity.rs` asserts no authored step text contains one of these.
/// A protocol that names a drug has stopped being first aid, and the verifier would
/// then happily pass a rendering that repeats it.
///
/// Scoped to the drug-and-dose list on purpose. The procedure stems are not included,
/// because `cpr.adult` legitimately tells a bystander to switch a defibrillator on.
#[must_use]
pub fn is_medication_vocabulary(word: &str) -> bool {
    AUTHORING_WORDS.contains(&word.to_lowercase().as_str())
}

/// True for words that name a clinical procedure — the stems, not the drug list.
///
/// Kept separate from [`is_medication_vocabulary`] because the two callers need
/// different lines drawn, and the difference is not an oversight:
///
/// - A **corpus rendering** may say "defibrillator", because a cited card told it to.
///   That is why the stems are excluded from `is_medication_vocabulary` and why
///   [`verify_rendering`] only refuses a stem the source protocol does not itself use.
/// - **Model-written guidance** ([`crate::fallback`]) has no source protocol, so there
///   is nothing that could have licensed the word. Every stem is refused outright.
///
/// Matched as a prefix, the same way [`verify_rendering`] matches them, so `inject`
/// covers `injection` and `injector`.
#[must_use]
pub fn is_authoring_stem(word: &str) -> bool {
    let lowered = word.to_lowercase();
    AUTHORING_STEMS.iter().any(|stem| lowered.starts_with(stem))
}

/// Split text the way [`verify_rendering`] does, returning lowercase words only.
///
/// Exposed for the corpus checks so they tokenize identically to the verifier rather
/// than approximating it with their own splitter.
#[must_use]
pub fn word_set(text: &str) -> BTreeSet<String> {
    words(&tokenize(text))
}

/// Check a rendering against its source protocol.
///
/// Violations come back sorted, so the same rendering always produces the same report
/// (`docs/CONVENTIONS.md` §2 in spirit — no incidental nondeterminism anywhere a
/// failure needs to be reproducible).
pub fn verify_rendering(protocol: &Protocol, rendered: &str) -> Result<(), Vec<Violation>> {
    if rendered.trim().is_empty() {
        return Err(vec![Violation::Empty]);
    }

    let source = tokenize(&protocol.renderable_text());
    let output = tokenize(rendered);

    let source_numbers = numbers(&source);
    let source_pairs = number_units(&source);
    let source_words = words(&source);

    let mut violations = BTreeSet::new();

    for (number, unit) in number_units(&output) {
        if !source_numbers.contains(&number) {
            violations.insert(Violation::InventedNumber { number });
            continue;
        }
        // A bare number at the end of a clause has no unit to check.
        if !unit.is_empty() && !source_pairs.contains(&(number.clone(), unit.clone())) {
            violations.insert(Violation::MismatchedUnit { number, unit });
        }
    }

    for word in words(&output) {
        if source_words.contains(&word) {
            continue;
        }
        let is_authoring = AUTHORING_WORDS.contains(&word.as_str())
            || AUTHORING_STEMS.iter().any(|stem| {
                word.starts_with(stem) && !source_words.iter().any(|src| src.starts_with(stem))
            });
        if is_authoring {
            violations.insert(Violation::ClinicalAuthoring { term: word });
        }
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations.into_iter().collect())
    }
}

/// Verify, and fall back to the protocol's own text when verification fails.
///
/// The safe path by construction: a caller cannot display an unverified rendering by
/// forgetting to check a `Result`. The returned violations are for the trace and for
/// the eval harness, not for the user.
#[must_use]
pub fn rendering_or_source(protocol: &Protocol, rendered: &str) -> (String, Vec<Violation>) {
    match verify_rendering(protocol, rendered) {
        Ok(()) => (rendered.to_owned(), Vec::new()),
        Err(violations) => (protocol.render_verbatim(), violations),
    }
}

// ---------------------------------------------------------------------------
// Tokenizing
// ---------------------------------------------------------------------------

/// A number or a word. Digits and letters never share a token, so `5cm` becomes
/// `5` followed by `cm` and the unit check still sees the pair.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Number(String),
    Word(String),
}

/// Split text into lowercase word and number tokens.
///
/// Deliberately *not* [`crate::normalize::normalize`]: that folds misspellings onto
/// lemmas, which is right for matching a frightened person's input and wrong here.
/// Folding on this side of the check could make an invented term resolve onto a word
/// the protocol happens to contain, which would hide exactly what we are looking for.
fn tokenize(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut numeric = false;
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch.is_ascii_digit() {
            if !current.is_empty() && !numeric {
                flush(&mut out, &mut current, numeric);
            }
            numeric = true;
            current.push(ch);
        } else if ch.is_alphanumeric() {
            if !current.is_empty() && numeric {
                flush(&mut out, &mut current, numeric);
            }
            numeric = false;
            for lower in ch.to_lowercase() {
                current.push(lower);
            }
        } else if (ch == '.' || ch == ',')
            && numeric
            && !current.is_empty()
            && chars.peek().is_some_and(char::is_ascii_digit)
        {
            // Keep decimals and thousands together, normalized to one separator so
            // both sides of the comparison agree.
            current.push('.');
        } else if !current.is_empty() {
            flush(&mut out, &mut current, numeric);
        }
    }
    if !current.is_empty() {
        flush(&mut out, &mut current, numeric);
    }
    out
}

fn flush(out: &mut Vec<Token>, current: &mut String, numeric: bool) {
    let text = std::mem::take(current);
    out.push(if numeric {
        Token::Number(text)
    } else {
        Token::Word(text)
    });
}

fn numbers(tokens: &[Token]) -> BTreeSet<String> {
    tokens
        .iter()
        .filter_map(|t| match t {
            Token::Number(n) => Some(n.clone()),
            Token::Word(_) => None,
        })
        .collect()
}

fn words(tokens: &[Token]) -> BTreeSet<String> {
    tokens
        .iter()
        .filter_map(|t| match t {
            Token::Word(w) => Some(w.clone()),
            Token::Number(_) => None,
        })
        .collect()
}

/// Every number paired with the word that follows it, or `""` at end of text.
fn number_units(tokens: &[Token]) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for (index, token) in tokens.iter().enumerate() {
        if let Token::Number(number) = token {
            let unit = match tokens.get(index + 1) {
                Some(Token::Word(word)) => word.clone(),
                _ => String::new(),
            };
            out.insert((number.clone(), unit));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::protocol::{Citation, Step, StepKind};

    fn protocol_with(steps: &[&str]) -> Protocol {
        Protocol {
            id: "test.protocol".to_owned(),
            version: "1.0.0".to_owned(),
            title: "Test card".to_owned(),
            applies_to: "Anyone testing.".to_owned(),
            also_called: vec!["test".to_owned()],
            reading_grade: 4,
            reviewed_by: None,
            reviewed_at: None,
            citations: vec![Citation {
                source: "Test".to_owned(),
                section: String::new(),
                url: String::new(),
            }],
            steps: steps
                .iter()
                .enumerate()
                .map(|(index, text)| Step {
                    n: u32::try_from(index).unwrap_or(0) + 1,
                    kind: StepKind::Assessment,
                    text: (*text).to_owned(),
                })
                .collect(),
            do_not: vec!["Do not give medicine.".to_owned()],
            escalate_if: vec!["They stop breathing.".to_owned()],
        }
    }

    #[test]
    fn a_faithful_paraphrase_passes() {
        let protocol = protocol_with(&["Press down about 5 cm deep.", "Wait 10 seconds."]);
        assert!(
            verify_rendering(&protocol, "Press about 5 cm deep, then wait 10 seconds.").is_ok()
        );
    }

    #[test]
    fn an_invented_number_is_refused() {
        let protocol = protocol_with(&["Press down about 5 cm deep."]);
        let errors = verify_rendering(&protocol, "Press down about 15 cm deep.")
            .expect_err("15 is not in the protocol");
        assert_eq!(
            errors,
            vec![Violation::InventedNumber {
                number: "15".to_owned()
            }]
        );
    }

    /// The check a bare-number comparison would miss, and the one most likely to hurt.
    #[test]
    fn a_swapped_unit_is_refused_even_though_the_number_is_present() {
        let protocol = protocol_with(&["Watch the chest for up to 10 seconds."]);
        let errors = verify_rendering(&protocol, "Watch the chest for up to 10 minutes.")
            .expect_err("seconds became minutes");
        assert_eq!(
            errors,
            vec![Violation::MismatchedUnit {
                number: "10".to_owned(),
                unit: "minutes".to_owned(),
            }]
        );
    }

    #[test]
    fn a_dose_invented_from_nowhere_is_refused() {
        let protocol = protocol_with(&["Keep them warm and wait."]);
        let errors = verify_rendering(&protocol, "Give them 300 mg of aspirin to chew.")
            .expect_err("this is prescribing");
        assert!(errors.contains(&Violation::InventedNumber {
            number: "300".to_owned()
        }));
        assert!(errors.contains(&Violation::ClinicalAuthoring {
            term: "mg".to_owned()
        }));
        assert!(errors.contains(&Violation::ClinicalAuthoring {
            term: "aspirin".to_owned()
        }));
    }

    #[test]
    fn authoring_stems_catch_every_inflection() {
        let protocol = protocol_with(&["Keep them warm and wait."]);
        for rendering in [
            "You should diagnose the cause first.",
            "The diagnosis is likely a heart attack.",
            "Administer the treatment now.",
            "Inject it into the thigh.",
            "Use an auto-injector.",
            "Follow the dosage on the box.",
        ] {
            assert!(
                verify_rendering(&protocol, rendering).is_err(),
                "{rendering:?} should be refused"
            );
        }
    }

    /// Scoped per protocol, so a word the card legitimately uses is not blocked.
    #[test]
    fn vocabulary_the_protocol_itself_uses_is_allowed() {
        let protocol = protocol_with(&["Switch the defibrillator on and follow it."]);
        assert!(verify_rendering(&protocol, "Switch the defibrillator on.").is_ok());

        // ...and the same word is refused for a card that does not mention it.
        let other = protocol_with(&["Keep pressing on the chest."]);
        assert!(verify_rendering(&other, "Fetch a defibrillator.").is_err());
    }

    /// `do_not` is not in the allowed set, so its vocabulary cannot launder a step.
    #[test]
    fn warning_vocabulary_cannot_be_borrowed_into_a_step() {
        let protocol = protocol_with(&["Keep them warm and wait."]);
        let errors = verify_rendering(&protocol, "Give them medicine.")
            .expect_err("do_not says the opposite");
        assert_eq!(
            errors,
            vec![Violation::ClinicalAuthoring {
                term: "medicine".to_owned()
            }]
        );
    }

    #[test]
    fn an_empty_rendering_is_a_violation_not_a_pass() {
        let protocol = protocol_with(&["Keep them warm."]);
        assert_eq!(
            verify_rendering(&protocol, "   ").expect_err("blank"),
            vec![Violation::Empty]
        );
    }

    #[test]
    fn a_rendering_may_refer_to_step_numbers() {
        let protocol = protocol_with(&["Look.", "Listen.", "Wait."]);
        assert!(verify_rendering(&protocol, "Go back to step 2.").is_ok());
        assert!(verify_rendering(&protocol, "Go back to step 9.").is_err());
    }

    #[test]
    fn the_fallback_is_the_protocol_itself_never_the_bad_rendering() {
        let protocol = protocol_with(&["Press about 5 cm deep."]);
        let (shown, violations) = rendering_or_source(&protocol, "Press about 50 cm deep.");
        assert!(!violations.is_empty());
        assert!(!shown.contains("50"));
        assert!(shown.contains("Press about 5 cm deep."));
        assert!(
            shown.contains("Do not give medicine."),
            "the verbatim card keeps its warnings"
        );
    }

    #[test]
    fn a_clean_rendering_passes_through_unchanged() {
        let protocol = protocol_with(&["Press about 5 cm deep."]);
        let (shown, violations) = rendering_or_source(&protocol, "Press about 5 cm deep.");
        assert!(violations.is_empty());
        assert_eq!(shown, "Press about 5 cm deep.");
    }

    #[test]
    fn tokenizing_separates_digits_from_letters_so_units_are_visible() {
        assert_eq!(
            tokenize("5cm"),
            vec![Token::Number("5".to_owned()), Token::Word("cm".to_owned())]
        );
        assert_eq!(tokenize("1,000"), vec![Token::Number("1.000".to_owned())]);
        assert_eq!(tokenize("0.5"), vec![Token::Number("0.5".to_owned())]);
        // A trailing full stop is punctuation, not a decimal point.
        assert_eq!(
            tokenize("wait 10."),
            vec![
                Token::Word("wait".to_owned()),
                Token::Number("10".to_owned())
            ]
        );
    }

    #[test]
    fn verification_is_deterministic_and_sorted() {
        let protocol = protocol_with(&["Keep them warm."]);
        let bad = "Give 300 mg of aspirin and 20 ml of insulin.";
        let first = verify_rendering(&protocol, bad).expect_err("many violations");
        let second = verify_rendering(&protocol, bad).expect_err("many violations");
        assert_eq!(first, second);
        let mut sorted = first.clone();
        sorted.sort();
        assert_eq!(first, sorted);
    }
}
