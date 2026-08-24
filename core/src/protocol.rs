//! The first-aid corpus: types, validation, and loading.
//!
//! `data/firstaid/SCHEMA.md` is the human-facing description of this format. This
//! module is the enforcement. Everything the user ever reads as medical guidance
//! originates in one of these files, because `PLAN.md` §1 forbids the model from
//! authoring medical content and [`crate::verifier`] can only check that claim
//! against a source of truth.
//!
//! # Why one bad file does not brick the app
//!
//! `docs/CONVENTIONS.md` §4 says an unparseable protocol is not loaded and the loader
//! says which file and why. It does **not** say the whole corpus fails. EcoGuardian
//! could be bricked at boot by a corrupt audit database, and repeating that here would
//! mean a typo in the drowning protocol takes the CPR card down with it.
//!
//! So [`Corpus::from_entries`] returns the protocols that validated *and* the errors
//! for the ones that did not. On a phone the good cards still render and the broken
//! ones report themselves as missing. In CI,
//! `tests/corpus_integrity.rs::the_shipped_corpus_loads_with_no_errors` asserts the
//! error list is empty, so a broken file can never actually reach a phone.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Longest `also_called` entry that is still plausibly a thing someone types into a
/// search box rather than a sentence someone wrote.
const MAX_SEARCH_PHRASE_WORDS: usize = 6;

/// What a step does to the patient. See `data/firstaid/SCHEMA.md`.
///
/// Not documentation: `tests/corpus_integrity.rs` asserts that no protocol opens with
/// an [`StepKind::Action`], which is what makes the red-flag layer's deliberate
/// overtriage onto `cpr.adult` safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// Look, listen, ask. Nothing has been done to the patient yet.
    Assessment,
    /// Do this to the patient.
    Action,
    /// Get more help than you are.
    Escalation,
}

/// One instruction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// 1-based position. Contiguous and ascending across a protocol.
    pub n: u32,
    pub kind: StepKind,
    pub text: String,
}

/// Where the content came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Citation {
    pub source: String,
    #[serde(default)]
    pub section: String,
    #[serde(default)]
    pub url: String,
}

/// One first-aid protocol, as loaded from `data/firstaid/<id>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Protocol {
    pub id: String,
    pub version: String,
    pub title: String,
    pub applies_to: String,
    /// Lay words for this emergency, for [`crate::retrieval`] only.
    ///
    /// A card titled "Not breathing — push on the chest" is not what a frightened
    /// person types. They type "heart attack", "collapsed", "he is not waking up". This
    /// field is where that vocabulary lives, and it exists because the alternative is
    /// worse: folding `heart attack → cpr` into [`crate::normalize`] would break
    /// `docs/CONVENTIONS.md` §6, which forbids folding one word onto a different word.
    ///
    /// **Indexed, never rendered.** It is not part of [`Protocol::renderable_text`], so
    /// no rendering may draw on it, and it never reaches the screen. That is also why
    /// [`Protocol::validate`] refuses prose here: content nobody sees is content nobody
    /// reviews, and an instruction hidden in a search field would escape every check
    /// this module makes.
    #[serde(default)]
    pub also_called: Vec<String>,
    /// Flesch–Kincaid grade of the step text. `PLAN.md` §8 gates this at ≤ 6.
    pub reading_grade: u8,
    /// Name and credential of the clinician who signed off, or `None`.
    ///
    /// `docs/CONVENTIONS.md` §9: `None` renders in the UI as "no clinician has
    /// reviewed this". It does not render as nothing.
    pub reviewed_by: Option<String>,
    /// ISO 8601 date of sign-off. `None` whenever `reviewed_by` is `None`.
    pub reviewed_at: Option<String>,
    pub citations: Vec<Citation>,
    pub steps: Vec<Step>,
    #[serde(default)]
    pub do_not: Vec<String>,
    #[serde(default)]
    pub escalate_if: Vec<String>,
}

impl Protocol {
    /// True only when a named clinician has signed off.
    #[must_use]
    pub fn is_clinically_reviewed(&self) -> bool {
        self.reviewed_by.is_some()
    }

    #[must_use]
    pub fn first_step(&self) -> Option<&Step> {
        self.steps.first()
    }

    /// Look a step up by its `n`, not by its index.
    #[must_use]
    pub fn step(&self, n: u32) -> Option<&Step> {
        self.steps.iter().find(|s| s.n == n)
    }

    /// The text a rendering is allowed to draw from — title, `applies_to`, and steps.
    ///
    /// This is the allowed-content set [`crate::verifier`] checks a rendering against.
    /// Step numbers are included as text so a rendering may legitimately say "step 3".
    ///
    /// Two categories are deliberately **excluded**, and both exclusions make the
    /// check stricter rather than looser:
    ///
    /// - `do_not` and `escalate_if` are rendered verbatim by the UI, never paraphrased,
    ///   because the verifier cannot detect a polarity inversion — "do not give
    ///   medicine" and "give medicine" share every token. Keeping them out of the
    ///   allowed set means their vocabulary cannot be borrowed to launder an invented
    ///   instruction into a step.
    /// - Citations are rendered from a fixed template and never pass through a model.
    ///   Admitting them would whitelist four arbitrary four-digit publication years.
    /// - `also_called` is search vocabulary, not content. Admitting it would let a
    ///   rendering use a word the card itself never says.
    #[must_use]
    pub fn renderable_text(&self) -> String {
        let mut out = String::with_capacity(1024);
        out.push_str(&self.title);
        out.push('\n');
        out.push_str(&self.applies_to);
        out.push('\n');
        for step in &self.steps {
            out.push_str(&step.n.to_string());
            out.push(' ');
            out.push_str(&step.text);
            out.push('\n');
        }
        out
    }

    /// The protocol as a plain card, straight from the file with nothing generated.
    ///
    /// This is what P0 ships: the whole app works with no model present at all. It is
    /// also the fallback whenever [`crate::verifier::verify_rendering`] rejects a model
    /// rendering, so the worst case is prose that reads like a manual, never a blank
    /// screen and never an unverified instruction.
    ///
    /// The instructions only. For the whole document — sources and review status included
    /// — use [`crate::render::plain_text`], and read that module's docs for why the two
    /// forms are kept apart.
    #[must_use]
    pub fn render_verbatim(&self) -> String {
        crate::render::instructions(self)
    }

    /// Validate every invariant in `data/firstaid/SCHEMA.md`.
    ///
    /// Returns *all* problems rather than the first, so an author fixing a file sees
    /// the whole list in one run.
    pub fn validate(&self) -> Result<(), Vec<ProtocolError>> {
        let mut errors = Vec::new();
        let id = || self.id.clone();

        for (field, value) in [
            ("id", self.id.as_str()),
            ("version", self.version.as_str()),
            ("title", self.title.as_str()),
            ("applies_to", self.applies_to.as_str()),
        ] {
            if value.trim().is_empty() {
                errors.push(ProtocolError::EmptyField { id: id(), field });
            }
        }

        if self.citations.is_empty() {
            errors.push(ProtocolError::NoCitations { id: id() });
        }
        for citation in &self.citations {
            if citation.source.trim().is_empty() {
                errors.push(ProtocolError::EmptyField {
                    id: id(),
                    field: "citations[].source",
                });
            }
        }

        // A protocol with no steps has nothing to render, so it would fail silently at
        // exactly the moment it was needed.
        if self.steps.is_empty() {
            errors.push(ProtocolError::NoSteps { id: id() });
        }

        for (index, step) in self.steps.iter().enumerate() {
            let expected = u32::try_from(index).unwrap_or(u32::MAX).saturating_add(1);
            if step.n != expected {
                errors.push(ProtocolError::StepNumbering {
                    id: id(),
                    expected,
                    found: step.n,
                });
            }
            if step.text.trim().is_empty() {
                errors.push(ProtocolError::EmptyStepText {
                    id: id(),
                    n: step.n,
                });
            }
        }

        // SCHEMA.md invariant 1. Step 1 tells the reader to check something or to get
        // help — never to put hands on a patient they may have arrived at by mistake.
        if let Some(first) = self.first_step()
            && first.kind == StepKind::Action
        {
            errors.push(ProtocolError::OpensWithAction { id: id() });
        }

        if self.reviewed_by.is_none() && self.reviewed_at.is_some() {
            errors.push(ProtocolError::ReviewDateWithoutReviewer { id: id() });
        }

        for line in self.do_not.iter().chain(self.escalate_if.iter()) {
            if line.trim().is_empty() {
                errors.push(ProtocolError::EmptyField {
                    id: id(),
                    field: "do_not / escalate_if entry",
                });
            }
        }

        // `also_called` is indexed and never displayed, so nothing downstream will ever
        // read it as content. That is exactly why it has to stay a list of search
        // phrases: a sentence here would be authored medical text that no reviewer,
        // reading-grade check, or verifier ever looks at.
        for term in &self.also_called {
            if term.trim().is_empty() {
                errors.push(ProtocolError::EmptyField {
                    id: id(),
                    field: "also_called entry",
                });
            } else if term.split_whitespace().count() > MAX_SEARCH_PHRASE_WORDS
                || term.contains(['.', '!', '?', ';'])
            {
                errors.push(ProtocolError::NotASearchPhrase {
                    id: id(),
                    term: term.clone(),
                });
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Why a protocol was refused. Every variant names the file it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// The JSON did not parse, or a required field was missing.
    Malformed {
        file: String,
        message: String,
    },
    /// `id` does not match the filename stem.
    IdMismatch {
        file: String,
        id: String,
    },
    EmptyField {
        id: String,
        field: &'static str,
    },
    NoCitations {
        id: String,
    },
    NoSteps {
        id: String,
    },
    StepNumbering {
        id: String,
        expected: u32,
        found: u32,
    },
    EmptyStepText {
        id: String,
        n: u32,
    },
    /// Step 1 is an action. See `data/firstaid/SCHEMA.md`.
    OpensWithAction {
        id: String,
    },
    /// An `also_called` entry is prose rather than a search phrase.
    NotASearchPhrase {
        id: String,
        term: String,
    },
    /// A review date with nobody attached to it — provenance theatre.
    ReviewDateWithoutReviewer {
        id: String,
    },
    /// Two files claim the same `id`.
    DuplicateId {
        id: String,
    },
    /// The file could not be read at all.
    Unreadable {
        file: String,
        message: String,
    },
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { file, message } => write!(f, "{file}: malformed — {message}"),
            Self::IdMismatch { file, id } => {
                write!(f, "{file}: declares id {id:?}, which is not the filename")
            }
            Self::EmptyField { id, field } => write!(f, "{id}: field {field} is empty"),
            Self::NoCitations { id } => write!(f, "{id}: no citations; where is this from?"),
            Self::NoSteps { id } => write!(f, "{id}: no steps, so nothing would render"),
            Self::StepNumbering {
                id,
                expected,
                found,
            } => {
                write!(
                    f,
                    "{id}: steps must run 1..n; expected {expected}, found {found}"
                )
            }
            Self::EmptyStepText { id, n } => write!(f, "{id}: step {n} has no text"),
            Self::OpensWithAction { id } => write!(
                f,
                "{id}: step 1 is an action; a card must open with an assessment or an escalation"
            ),
            Self::NotASearchPhrase { id, term } => write!(
                f,
                "{id}: also_called entry {term:?} is a sentence; that field is for search \
                 words, and nothing in it is ever shown or reviewed"
            ),
            Self::ReviewDateWithoutReviewer { id } => {
                write!(f, "{id}: reviewed_at is set but reviewed_by is null")
            }
            Self::DuplicateId { id } => write!(f, "{id}: declared by more than one file"),
            Self::Unreadable { file, message } => write!(f, "{file}: unreadable — {message}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

/// The loaded corpus, keyed by protocol id.
#[derive(Debug, Clone, Default)]
pub struct Corpus {
    by_id: BTreeMap<String, Protocol>,
}

impl Corpus {
    /// Parse and validate `(filename, json)` pairs.
    ///
    /// Returns the corpus of everything that validated, plus one error per rejection.
    /// The corpus is usable even when the error list is not empty — see the module
    /// docs for why that is deliberate rather than sloppy.
    ///
    /// `filename` may be a bare stem or a path; only the stem is compared to `id`.
    #[must_use]
    pub fn from_entries<I, N, J>(entries: I) -> (Self, Vec<ProtocolError>)
    where
        I: IntoIterator<Item = (N, J)>,
        N: AsRef<str>,
        J: AsRef<str>,
    {
        let mut by_id: BTreeMap<String, Protocol> = BTreeMap::new();
        let mut errors = Vec::new();

        for (name, json) in entries {
            let file = name.as_ref();
            let stem = filename_stem(file);

            let protocol: Protocol = match serde_json::from_str(json.as_ref()) {
                Ok(parsed) => parsed,
                Err(err) => {
                    errors.push(ProtocolError::Malformed {
                        file: file.to_owned(),
                        message: err.to_string(),
                    });
                    continue;
                }
            };

            if protocol.id != stem {
                errors.push(ProtocolError::IdMismatch {
                    file: file.to_owned(),
                    id: protocol.id.clone(),
                });
                continue;
            }

            if let Err(mut problems) = protocol.validate() {
                errors.append(&mut problems);
                continue;
            }

            if by_id.contains_key(&protocol.id) {
                errors.push(ProtocolError::DuplicateId {
                    id: protocol.id.clone(),
                });
                continue;
            }

            by_id.insert(protocol.id.clone(), protocol);
        }

        (Self { by_id }, errors)
    }

    /// Read every `*.json` file in a directory.
    ///
    /// Used by tests, the desktop harness, and the pack build. The device never calls
    /// this: the shipped corpus is compiled into the binary by [`crate::bundled`], so
    /// there is no asset path to be wrong and no I/O to fail while someone is bleeding.
    #[must_use]
    pub fn load_dir(dir: &std::path::Path) -> (Self, Vec<ProtocolError>) {
        let mut entries: Vec<(String, String)> = Vec::new();
        let mut errors = Vec::new();

        let read = match std::fs::read_dir(dir) {
            Ok(read) => read,
            Err(err) => {
                return (
                    Self::default(),
                    vec![ProtocolError::Unreadable {
                        file: dir.display().to_string(),
                        message: err.to_string(),
                    }],
                );
            }
        };

        // Sorted, so a load is reproducible and a duplicate-id error always names the
        // same file. `docs/CONVENTIONS.md` §2 in spirit: no incidental nondeterminism.
        let mut paths: Vec<std::path::PathBuf> = Vec::new();
        for entry in read {
            match entry {
                Ok(entry) => {
                    let path = entry.path();
                    if path.extension().is_some_and(|ext| ext == "json") {
                        paths.push(path);
                    }
                }
                Err(err) => errors.push(ProtocolError::Unreadable {
                    file: dir.display().to_string(),
                    message: err.to_string(),
                }),
            }
        }
        paths.sort();

        for path in paths {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("<non-utf8 filename>")
                .to_owned();
            match std::fs::read_to_string(&path) {
                Ok(json) => entries.push((name, json)),
                Err(err) => errors.push(ProtocolError::Unreadable {
                    file: name,
                    message: err.to_string(),
                }),
            }
        }

        let (corpus, mut load_errors) = Self::from_entries(entries);
        errors.append(&mut load_errors);
        (corpus, errors)
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Protocol> {
        self.by_id.get(id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Ids in sorted order.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.by_id.keys().map(String::as_str)
    }

    pub fn protocols(&self) -> impl Iterator<Item = &Protocol> {
        self.by_id.values()
    }
}

/// The part of a filename before the last `.json`, ignoring any directory.
///
/// Protocol ids contain dots (`cpr.adult`), so `Path::file_stem` is wrong here — it
/// would return `cpr` and every id would look mismatched.
fn filename_stem(name: &str) -> &str {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    base.strip_suffix(".json").unwrap_or(base)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn minimal(id: &str) -> String {
        format!(
            r#"{{
              "id": "{id}",
              "version": "1.0.0",
              "title": "T",
              "applies_to": "A",
              "reading_grade": 4,
              "reviewed_by": null,
              "reviewed_at": null,
              "citations": [{{ "source": "S" }}],
              "steps": [{{ "n": 1, "kind": "assessment", "text": "Look." }}]
            }}"#
        )
    }

    #[test]
    fn a_minimal_protocol_round_trips() {
        let (corpus, errors) = Corpus::from_entries([("x.y.json", minimal("x.y"))]);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(corpus.len(), 1);
        let protocol = corpus.get("x.y").expect("loaded");
        assert!(!protocol.is_clinically_reviewed());
    }

    /// Ids contain dots, which `Path::file_stem` would truncate.
    #[test]
    fn dotted_ids_match_their_filename() {
        assert_eq!(filename_stem("cpr.adult.json"), "cpr.adult");
        assert_eq!(filename_stem("data/firstaid/cpr.adult.json"), "cpr.adult");
        assert_eq!(filename_stem(r"data\firstaid\cpr.adult.json"), "cpr.adult");
        assert_eq!(filename_stem("cpr.adult"), "cpr.adult");
    }

    #[test]
    fn id_must_match_the_filename() {
        let (corpus, errors) = Corpus::from_entries([("wrong.json", minimal("cpr.adult"))]);
        assert!(corpus.is_empty());
        assert_eq!(
            errors,
            vec![ProtocolError::IdMismatch {
                file: "wrong.json".to_owned(),
                id: "cpr.adult".to_owned(),
            }]
        );
    }

    #[test]
    fn malformed_json_is_reported_by_filename_and_not_swallowed() {
        let (corpus, errors) = Corpus::from_entries([("broken.json", "{ not json")]);
        assert!(corpus.is_empty());
        assert!(matches!(
            errors.as_slice(),
            [ProtocolError::Malformed { file, .. }] if file == "broken.json"
        ));
    }

    /// The point of the module docs: a bad file loses its own card, nothing else.
    #[test]
    fn one_bad_file_does_not_take_the_good_ones_with_it() {
        let (corpus, errors) = Corpus::from_entries([
            ("good.one.json", minimal("good.one")),
            ("broken.json", "{{{".to_owned()),
            ("good.two.json", minimal("good.two")),
        ]);
        assert_eq!(corpus.len(), 2);
        assert!(corpus.get("good.one").is_some());
        assert!(corpus.get("good.two").is_some());
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn step_numbers_must_be_contiguous_from_one() {
        let json = minimal("x.y").replace(r#""n": 1"#, r#""n": 2"#);
        let (_, errors) = Corpus::from_entries([("x.y.json", json)]);
        assert_eq!(
            errors,
            vec![ProtocolError::StepNumbering {
                id: "x.y".to_owned(),
                expected: 1,
                found: 2,
            }]
        );
    }

    #[test]
    fn a_protocol_may_not_open_with_an_action() {
        let json = minimal("x.y").replace("assessment", "action");
        let (_, errors) = Corpus::from_entries([("x.y.json", json)]);
        assert_eq!(
            errors,
            vec![ProtocolError::OpensWithAction {
                id: "x.y".to_owned()
            }]
        );
    }

    #[test]
    fn a_citation_is_required() {
        let json = minimal("x.y").replace(r#"[{ "source": "S" }]"#, "[]");
        let (_, errors) = Corpus::from_entries([("x.y.json", json)]);
        assert_eq!(
            errors,
            vec![ProtocolError::NoCitations {
                id: "x.y".to_owned()
            }]
        );
    }

    #[test]
    fn a_review_date_without_a_reviewer_is_rejected() {
        let json =
            minimal("x.y").replace(r#""reviewed_at": null"#, r#""reviewed_at": "2026-01-01""#);
        let (_, errors) = Corpus::from_entries([("x.y.json", json)]);
        assert_eq!(
            errors,
            vec![ProtocolError::ReviewDateWithoutReviewer {
                id: "x.y".to_owned()
            }]
        );
    }

    #[test]
    fn duplicate_ids_are_refused_rather_than_silently_overwriting() {
        let (corpus, errors) =
            Corpus::from_entries([("x.y.json", minimal("x.y")), ("x.y.json", minimal("x.y"))]);
        assert_eq!(corpus.len(), 1);
        assert_eq!(
            errors,
            vec![ProtocolError::DuplicateId {
                id: "x.y".to_owned()
            }]
        );
    }

    #[test]
    fn renderable_text_carries_step_numbers_so_a_rendering_may_cite_them() {
        let (corpus, _) = Corpus::from_entries([("x.y.json", minimal("x.y"))]);
        let text = corpus.get("x.y").expect("loaded").renderable_text();
        assert!(text.contains("Look."));
        assert!(text.contains('1'));
    }

    /// The verifier cannot see polarity, so warnings never go through it. Keeping them
    /// out of the allowed set is what stops their vocabulary being borrowed.
    #[test]
    fn renderable_text_excludes_warnings_but_the_verbatim_card_shows_them() {
        let json = minimal("x.y").replace(
            r#""steps":"#,
            r#""do_not": ["Do not give medicine."], "steps":"#,
        );
        let (corpus, errors) = Corpus::from_entries([("x.y.json", json)]);
        assert!(errors.is_empty(), "{errors:?}");
        let protocol = corpus.get("x.y").expect("loaded");
        assert!(!protocol.renderable_text().contains("medicine"));
        assert!(protocol.render_verbatim().contains("Do not give medicine."));
    }

    /// `also_called` is search vocabulary. Prose in it would be medical text that no
    /// reviewer and no reading-grade check ever sees.
    #[test]
    fn a_sentence_in_the_search_field_is_refused() {
        let json = minimal("x.y").replace(
            r#""steps":"#,
            r#""also_called": ["Press hard on the middle of the chest."], "steps":"#,
        );
        let (_, errors) = Corpus::from_entries([("x.y.json", json)]);
        assert!(
            matches!(errors.as_slice(), [ProtocolError::NotASearchPhrase { id, .. }] if id == "x.y"),
            "{errors:?}"
        );
    }

    #[test]
    fn short_search_phrases_are_accepted_and_stay_off_the_screen() {
        let json = minimal("x.y").replace(
            r#""steps":"#,
            r#""also_called": ["heart attack", "collapsed"], "steps":"#,
        );
        let (corpus, errors) = Corpus::from_entries([("x.y.json", json)]);
        assert!(errors.is_empty(), "{errors:?}");
        let protocol = corpus.get("x.y").expect("loaded");
        assert_eq!(protocol.also_called.len(), 2);
        assert!(
            !protocol.renderable_text().contains("heart attack"),
            "search vocabulary must not become renderable content"
        );
        assert!(
            !protocol.render_verbatim().contains("heart attack"),
            "search vocabulary must never reach the screen"
        );
    }

    #[test]
    fn a_missing_directory_reports_itself_instead_of_looking_empty() {
        let (corpus, errors) = Corpus::load_dir(std::path::Path::new("no/such/dir"));
        assert!(corpus.is_empty());
        assert!(matches!(
            errors.as_slice(),
            [ProtocolError::Unreadable { .. }]
        ));
    }
}
