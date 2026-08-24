//! How hard the text is to read, as one number.
//!
//! `PLAN.md` §8 makes Flesch–Kincaid ≤ grade 6 a shipping gate on *every rendered
//! protocol*, for a stated reason: "the design target is a frightened second-language
//! reader, not a calm fluent one". Someone reading a card while a person in front of
//! them is dying is not reading carefully, and a sentence they have to re-read costs
//! seconds that belong to the patient.
//!
//! # Why this lives in the library and not in the test that first needed it
//!
//! The grade check began as a private helper inside `tests/corpus_integrity.rs`, which
//! was fine while the corpus was the only thing being measured. It stopped being fine
//! when [`crate::eval`] gained the same gate: two implementations of a shipping
//! threshold drift, and the day they disagree is the day the corpus test passes at 5.9
//! and the gate report fails at 6.1, or — much worse — the reverse.
//!
//! So there is one function, and both callers use it. The number in the gate report is
//! the number CI printed.
//!
//! # What the heuristic is, precisely
//!
//! Flesch–Kincaid needs a syllable count, and syllable counting in English is not a
//! solved problem. Syllables here are vowel groups, with a silent trailing `e` removed
//! and a floor of one per word. That undercounts "fire" and overcounts "ideal", and
//! neither error matters at the resolution this gate works at: it exists to catch a card
//! that drifted into clinical prose, and clinical prose fails on sentence length and
//! Latin roots together, by whole grades rather than tenths.
//!
//! The value is therefore comparable across our own text and not comparable with another
//! tool's Flesch–Kincaid. `tests/corpus_integrity.rs` prints every card's margin for
//! exactly that reason: a reviewer watching a card sit at 5.9 learns more than a reviewer
//! reading "passed".

/// Flesch–Kincaid grade level. Higher is harder. Returns `0.0` for text with no words.
///
/// See the module docs for what the syllable count does and does not promise.
#[must_use]
pub fn grade(text: &str) -> f64 {
    let sentences = text
        .chars()
        .filter(|c| matches!(c, '.' | '!' | '?'))
        .count()
        .max(1) as f64;
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return 0.0;
    }
    let word_count = words.len() as f64;
    let syllables: f64 = words.iter().map(|w| syllables_in(w) as f64).sum();
    0.39 * (word_count / sentences) + 11.8 * (syllables / word_count) - 15.59
}

/// The hardest-to-read item in a labelled set, or `None` when the set is empty.
///
/// Labels are carried through so a failing gate can name the card rather than report a
/// number with nowhere to look. Ties break on the label, ascending, so a set with two
/// items at the same grade always reports the same one — a gate whose failure message
/// changes between runs is a gate people learn to re-run instead of fix.
#[must_use]
pub fn hardest<'a>(items: impl IntoIterator<Item = (&'a str, &'a str)>) -> Option<(String, f64)> {
    items
        .into_iter()
        .map(|(label, text)| (label.to_owned(), grade(text)))
        .reduce(|worst, next| {
            let harder = next.1 > worst.1;
            let tied_and_earlier = next.1 == worst.1 && next.0 < worst.0;
            if harder || tied_and_earlier {
                next
            } else {
                worst
            }
        })
}

fn syllables_in(word: &str) -> usize {
    let cleaned: String = word
        .chars()
        .filter(|c| c.is_alphabetic())
        .flat_map(char::to_lowercase)
        .collect();
    if cleaned.is_empty() {
        // A bare number reads as one beat: "10" is "ten".
        return 1;
    }
    let trimmed = if cleaned.len() > 2 && cleaned.ends_with('e') {
        cleaned.get(..cleaned.len() - 1).unwrap_or(&cleaned)
    } else {
        &cleaned
    };
    let mut count = 0;
    let mut previous_was_vowel = false;
    for ch in trimmed.chars() {
        let is_vowel = matches!(ch, 'a' | 'e' | 'i' | 'o' | 'u' | 'y');
        if is_vowel && !previous_was_vowel {
            count += 1;
        }
        previous_was_vowel = is_vowel;
    }
    count.max(1)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn empty_text_has_no_grade_rather_than_a_negative_one() {
        assert_eq!(grade(""), 0.0);
        assert_eq!(grade("   \n  "), 0.0);
    }

    /// The direction is the whole point: plain instructions must score below prose that
    /// has drifted into a clinical register. The absolute values are heuristic; the
    /// ordering is what the gate depends on.
    #[test]
    fn clinical_prose_scores_harder_than_plain_instructions() {
        let plain = "Push hard on the chest. Do not stop. Call for help.";
        let clinical = "Commence uninterrupted external cardiac compressions at an \
                        appropriate anatomical landmark whilst simultaneously arranging \
                        definitive advanced life support intervention.";
        assert!(grade(plain) < 6.0, "plain text measured {}", grade(plain));
        assert!(
            grade(clinical) > 12.0,
            "clinical text measured {}",
            grade(clinical)
        );
    }

    /// Sentence length alone can fail the gate. Splitting one long instruction into two
    /// short ones is the cheapest fix available to an author, so the metric has to reward
    /// it or the advice is useless.
    #[test]
    fn splitting_a_sentence_lowers_the_grade() {
        let one = "Roll them onto their side and tilt the head back and watch the chest \
                   rise and fall and stay with them until help arrives.";
        let two = "Roll them onto their side. Tilt the head back. Watch the chest. Stay \
                   with them.";
        assert!(
            grade(two) < grade(one),
            "{} should be easier than {}",
            grade(two),
            grade(one)
        );
    }

    #[test]
    fn a_bare_number_counts_as_one_beat() {
        assert_eq!(syllables_in("10"), 1);
        assert_eq!(syllables_in("—"), 1);
        assert_eq!(syllables_in("call"), 1);
        assert_eq!(syllables_in("ambulance"), 3);
        // Silent trailing `e`: "chest" and "chose" are both one beat.
        assert_eq!(syllables_in("chose"), 1);
    }

    #[test]
    fn the_hardest_item_is_named_so_a_failure_has_somewhere_to_look() {
        let (label, worst) = hardest([
            ("easy.card", "Sit down. Stay still."),
            (
                "hard.card",
                "Administer supplementary oxygenation via a non-rebreather apparatus.",
            ),
        ])
        .expect("two items");
        assert_eq!(label, "hard.card");
        assert!(worst > 6.0);
    }

    #[test]
    fn an_empty_set_has_no_hardest_item() {
        assert_eq!(hardest([]), None);
    }

    /// A gate whose failure message moves between runs is a gate people re-run.
    #[test]
    fn ties_break_on_the_label_so_the_failure_message_is_stable() {
        let same = "Sit down. Stay still.";
        let forwards = hardest([("b.card", same), ("a.card", same)]).expect("two items");
        let backwards = hardest([("a.card", same), ("b.card", same)]).expect("two items");
        assert_eq!(forwards, backwards);
        assert_eq!(forwards.0, "a.card");
    }
}
