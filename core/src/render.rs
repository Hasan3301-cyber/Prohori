//! Turning a protocol into text a person can read, with its provenance attached to it.
//!
//! # Why provenance is part of the rendering and not a separate field the UI may forget
//!
//! `docs/CONVENTIONS.md` §9 says any value that could be unverified carries a field saying
//! so, and the UI shows it. That works while the text stays inside the UI. It stops working
//! the moment the text leaves — copied into a message, read aloud down a phone line, pasted
//! into a group chat where six people will act on it.
//!
//! So [`plain_text`] emits one document with the steps, the sources, and a sentence saying
//! who has and has not checked them. Whatever happens to that string afterwards, the reader
//! can tell where the words came from. A first-aid instruction with no attribution is
//! indistinguishable from something someone made up, and the app has no way to earn back
//! trust it spends by looking more authoritative than it is.
//!
//! Every card in this build renders "No clinician has reviewed this card." That is not a
//! placeholder to be quietly dropped before release — it is the true statement, and it stays
//! until a named clinician signs the file.
//!
//! # Two forms, deliberately
//!
//! [`instructions`] is the card's own words and nothing else: title, who it is for, the
//! steps, the warnings. It is what [`crate::verifier::rendering_or_source`] falls back to
//! when it refuses a generated rendering, and it is a *fragment* — prose to be slotted into
//! a screen that already draws the sources and the review status around it.
//!
//! [`plain_text`] is the whole document, and stands alone.
//!
//! Keeping them apart means the fragment cannot grow a duplicate sources block inside a
//! screen that already has one, and the document cannot lose its sources by being wired to
//! the wrong function. If they are ever merged, the one to keep is the document.

use crate::protocol::Protocol;

/// The card's own words: title, who it is for, the numbered steps, the warnings.
///
/// Nothing here is generated, inferred, or reworded — this is the file, laid out. Warnings
/// render verbatim because the verifier cannot detect a polarity inversion ("do not give
/// medicine" and "give medicine" share every token), so nothing is allowed to rewrite them.
///
/// No labels are invented beyond the two the lists need. `applies_to` is a sentence that
/// reads on its own and gets no heading, because a rendering that adds words is a rendering
/// that can add the wrong ones.
#[must_use]
pub fn instructions(protocol: &Protocol) -> String {
    let mut out = String::with_capacity(2048);
    out.push_str(&protocol.title);
    out.push_str("\n\n");
    out.push_str(&protocol.applies_to);
    out.push_str("\n\n");
    for step in &protocol.steps {
        out.push_str(&format!("{}. {}\n", step.n, step.text));
    }
    if !protocol.do_not.is_empty() {
        out.push_str("\nDo not:\n");
        for line in &protocol.do_not {
            out.push_str("- ");
            out.push_str(line);
            out.push('\n');
        }
    }
    if !protocol.escalate_if.is_empty() {
        out.push_str("\nIf this happens:\n");
        for line in &protocol.escalate_if {
            out.push_str("- ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// One line per citation, for a list the UI draws itself.
///
/// The single formatter for a citation, called by the FFI card as well, so a source cannot
/// be written one way on a screen and another way in the text that gets shared from it.
/// URLs are left out: they belong in [`plain_text`], where the reader has no app to tap.
#[must_use]
pub fn source_lines(protocol: &Protocol) -> Vec<String> {
    protocol
        .citations
        .iter()
        .map(|citation| {
            if citation.section.trim().is_empty() {
                citation.source.clone()
            } else {
                format!("{} — {}", citation.source, citation.section)
            }
        })
        .collect()
}

/// One sentence on who has checked this card, safe to show anywhere.
///
/// Never empty. An unreviewed card says so and then says where its words came from, because
/// "nobody has checked this" on its own reads as "this might be anything", and the truthful
/// position is narrower than that: the steps are copied from published guidance, and what is
/// missing is a clinician's signature on this particular file.
///
/// [`Protocol::validate`] rejects a card with no citations, so the second half of that
/// sentence is always true.
#[must_use]
pub fn provenance(protocol: &Protocol) -> String {
    match (&protocol.reviewed_by, &protocol.reviewed_at) {
        (Some(who), Some(when)) => format!("Reviewed by {who} on {when}."),
        (Some(who), None) => format!("Reviewed by {who}."),
        (None, _) => "No clinician has reviewed this card. Every step above is taken from \
                      the sources listed, unchanged."
            .to_owned(),
    }
}

/// The complete card as one block of text: the words, where they came from, who checked them.
///
/// This is what the app shows when there is no model on the device, and what leaves the app
/// when someone copies or shares a card. See the module docs for why the provenance travels
/// with it.
#[must_use]
pub fn plain_text(protocol: &Protocol) -> String {
    let mut out = instructions(protocol);
    out.push_str("\nSources:\n");
    for (line, citation) in source_lines(protocol).iter().zip(&protocol.citations) {
        out.push_str("- ");
        out.push_str(line);
        out.push('\n');
        if !citation.url.trim().is_empty() {
            out.push_str("  ");
            out.push_str(citation.url.trim());
            out.push('\n');
        }
    }
    out.push('\n');
    out.push_str(&provenance(protocol));
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::bundled;
    use crate::protocol::{Citation, Step, StepKind};

    fn protocol() -> Protocol {
        Protocol {
            id: "test.card".to_owned(),
            version: "1.0.0".to_owned(),
            title: "Choking".to_owned(),
            applies_to: "Someone who cannot cough or speak.".to_owned(),
            also_called: vec!["food stuck".to_owned()],
            reading_grade: 4,
            reviewed_by: None,
            reviewed_at: None,
            citations: vec![
                Citation {
                    source: "WHO First Aid Guidelines 2020".to_owned(),
                    section: "Airway obstruction".to_owned(),
                    url: "https://www.who.int/".to_owned(),
                },
                Citation {
                    source: "A source with no section and no link".to_owned(),
                    section: String::new(),
                    url: String::new(),
                },
            ],
            steps: vec![
                Step {
                    n: 1,
                    kind: StepKind::Assessment,
                    text: "Ask if they are choking.".to_owned(),
                },
                Step {
                    n: 2,
                    kind: StepKind::Action,
                    text: "Hit them between the shoulder blades.".to_owned(),
                },
            ],
            do_not: vec!["Do not reach into the mouth.".to_owned()],
            escalate_if: vec!["They go limp — start chest compressions.".to_owned()],
        }
    }

    #[test]
    fn the_document_carries_every_word_of_the_card() {
        let text = plain_text(&protocol());
        for expected in [
            "Choking",
            "Someone who cannot cough or speak.",
            "1. Ask if they are choking.",
            "2. Hit them between the shoulder blades.",
            "Do not reach into the mouth.",
            "They go limp — start chest compressions.",
        ] {
            assert!(
                text.contains(expected),
                "{expected:?} missing from:\n{text}"
            );
        }
    }

    /// The point of the task. A card without its sources is an anonymous instruction.
    #[test]
    fn the_document_names_its_sources_and_links_them() {
        let text = plain_text(&protocol());
        assert!(text.contains("Sources:"));
        assert!(text.contains("WHO First Aid Guidelines 2020 — Airway obstruction"));
        assert!(text.contains("A source with no section and no link"));
        assert!(
            text.contains("https://www.who.int/"),
            "a citation the reader cannot check is a citation on trust alone"
        );
    }

    /// `docs/CONVENTIONS.md` §9: unreviewed renders as unreviewed, not as nothing.
    #[test]
    fn an_unreviewed_card_says_so_in_words() {
        let text = plain_text(&protocol());
        assert!(text.contains("No clinician has reviewed this card."));
        assert!(
            text.trim_end().ends_with("unchanged."),
            "provenance is the last thing in the document:\n{text}"
        );
    }

    #[test]
    fn a_reviewed_card_names_the_clinician_and_the_date() {
        let mut reviewed = protocol();
        reviewed.reviewed_by = Some("Dr A. Rahman, MBBS".to_owned());
        reviewed.reviewed_at = Some("2026-03-01".to_owned());
        let text = plain_text(&reviewed);
        assert!(text.contains("Reviewed by Dr A. Rahman, MBBS on 2026-03-01."));
        assert!(!text.contains("No clinician"));

        reviewed.reviewed_at = None;
        assert_eq!(provenance(&reviewed), "Reviewed by Dr A. Rahman, MBBS.");
    }

    /// There is no state in which the UI can show a card and show nothing about its
    /// provenance, because there is no state in which this returns an empty string.
    #[test]
    fn provenance_is_never_empty_for_any_card_in_the_build() {
        let (corpus, errors) = bundled::corpus();
        assert!(errors.is_empty(), "{errors:?}");
        for card in corpus.protocols() {
            assert!(
                !provenance(card).trim().is_empty(),
                "{} renders no provenance",
                card.id
            );
            assert!(
                plain_text(card).contains(&provenance(card)),
                "{} renders provenance the document then drops",
                card.id
            );
        }
    }

    /// The em dash is a separator, not decoration: with no section there is nothing to
    /// separate, and a trailing dash reads as a missing value rather than an absent one.
    #[test]
    fn a_citation_with_no_section_is_not_rendered_with_a_dangling_dash() {
        let lines = source_lines(&protocol());
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1], "A source with no section and no link");
        assert!(!lines[1].contains('—'));
    }

    /// The fragment is the verifier's fallback and must stay a fragment. A sources block
    /// inside it would be drawn twice on a screen that already lists them.
    #[test]
    fn the_fragment_carries_no_sources_and_no_provenance() {
        let text = instructions(&protocol());
        assert!(!text.contains("Sources:"));
        assert!(!text.contains("No clinician"));
        assert!(text.contains("1. Ask if they are choking."));
    }
}
