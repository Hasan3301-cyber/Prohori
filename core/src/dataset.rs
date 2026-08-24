//! Deterministic synthesis of the P5 fine-tuning corpus.
//!
//! `PLAN.md` §8 sets the boundary this module exists to respect:
//!
//! > we train the model on *format and selection*, never on free-form medical prose.
//! > That is what keeps §1 true.
//!
//! So every example here teaches one of two things — emit the slot schema correctly, and
//! pick the right card — and nothing here contains a word of medical advice. The advice
//! lives in `data/firstaid/`, is rendered verbatim, and is never generated.
//!
//! # Where the text comes from
//!
//! Inputs are built from each card's `also_called` list: 154 lay phrases across
//! 18 cards, already written as the words a frightened person types. They are indexed for
//! retrieval but never rendered, which makes them the one piece of hand-written vocabulary
//! in the repository that can be reused as model input without leaking authored prose into
//! a training target.
//!
//! Each phrase is dropped into a *frame* — a fixed prefix and suffix carrying the
//! surrounding register and, where it has one, an age band. Frames never touch the phrase,
//! which is what lets a symptom span stay a literal substring of the input.
//!
//! # No randomness, by construction
//!
//! `docs/CONVENTIONS.md` §2 requires all randomness to be seeded and injected. This module
//! satisfies it by having none: the generator is a cross product walked in a fixed order,
//! and degradation family selection is `index % FAMILIES.len()`. Building twice produces
//! byte-identical output, which is why the manifest can carry a digest that means
//! something. There is no RNG to seed, and CI's grep for one stays clean.
//!
//! # The holdout is on phrases *and* frames
//!
//! The last two `also_called` phrases of every card are held out, and the four eval frames
//! share no text with the twelve training frames. An eval item is therefore a novel phrase
//! in a novel frame — not a paraphrase of something the adapter has already seen. Holding
//! out only one of the two would let a memorised phrase pass in a new wrapper, or a
//! memorised wrapper pass with a new phrase, and either would make the eval numbers a
//! measurement of recall rather than of generalisation.
//!
//! # Degraded English is generated, not hoped for
//!
//! `PLAN.md` §8 requires ≥95% slot accuracy on a *held-out* misspelled / non-native /
//! ASR-error split, "so a clean-prose average cannot hide it". Five transform families
//! produce it — see [`Degradation`]. Two damage the phrase and therefore move the symptom
//! span; three damage only the frame and leave the span alone.
//!
//! A degraded example carries **its clean twin's label**. That is the entire point of the
//! split: the situation did not change, only the typing did, so the answer must not change
//! either. In particular the severity label is computed from the *clean* input's red-flag
//! floor, never from the degraded one — otherwise the generator would be teaching the
//! model to undertriage precisely the inputs the gate is meant to protect.
//!
//! What that costs is visible rather than hidden: [`DatasetManifest::rule_coverage_lost`]
//! counts the degraded inputs whose red-flag trigger no longer fires, so a reader can see
//! exactly how many cases rest on the model alone with no deterministic floor underneath.
//!
//! # Severity labels, and the part a clinician still owns
//!
//! Ten protocols take their severity from `redflag::RULES`, where a rule already names the
//! card — that is a decision the rule table made and this module reads. The other nine
//! have no rule, so [`PENDING_CLINICAL_SEVERITY`] declares them, uniformly `Urgent`, and
//! says out loud that no clinician has reviewed it.
//!
//! Uniform `Urgent` is deliberate and it is deliberately crude. It overtriages a sunburn,
//! and `docs/CONVENTIONS.md` §7 says that is the acceptable direction to be wrong in. The
//! alternative — this module inventing per-card clinical judgements — would be a worse
//! kind of wrong, because it would look reviewed. The table is the single highest-value
//! thing for the first clinician to touch.
//!
//! # This generator cannot approve its own output
//!
//! [`DatasetManifest::clinical_review`] is `None` and no code path here can set it. The
//! data this module produces is unreviewed synthetic text, and `crate::eval` will refuse
//! to clear a release without an attestation that a person reviewed it. A repository that
//! could sign off on its own training data would not be enforcing anything.

use crate::bundled;
use crate::protocol::Corpus;
use crate::redflag;
use crate::severity::Severity;
use serde::Serialize;
use sha2::{Digest, Sha256};

/// How many `also_called` phrases per card are withheld from training.
pub const PHRASES_HELD_OUT_PER_PROTOCOL: usize = 2;

/// A fixed prefix and suffix wrapped around a phrase.
///
/// The phrase is never modified by a frame, so a symptom span cut from the phrase is a
/// literal substring of the assembled input — which is what
/// `inference::retain_grounded_symptoms` demands before it will show a symptom at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub id: &'static str,
    pub prefix: &'static str,
    pub suffix: &'static str,
    /// The `age_band` slot this frame implies. `"unknown"` where it implies nothing —
    /// `data/prompts/triage-system.txt` says "Use unknown when the age is not stated",
    /// and a frame that does not state one must not invent one.
    pub age_band: &'static str,
}

/// Training frames. Twelve registers a real message arrives in.
///
/// The colon frames ("my father: ") are not stylistic filler — they compose with any
/// fragment, including phrases that are already clauses ("will not wake up"), where
/// "my father has will not wake up" would be ungrammatical garbage to train on.
pub const TRAIN_FRAMES: &[Frame] = &[
    Frame {
        id: "plain",
        prefix: "",
        suffix: "",
        age_band: "unknown",
    },
    Frame {
        id: "help_prefix",
        prefix: "help ",
        suffix: "",
        age_band: "unknown",
    },
    Frame {
        id: "please_now",
        prefix: "please help ",
        suffix: " right now",
        age_band: "unknown",
    },
    Frame {
        id: "emergency_prefix",
        prefix: "emergency ",
        suffix: "",
        age_band: "unknown",
    },
    Frame {
        id: "what_do_i_do",
        prefix: "",
        suffix: " what do i do",
        age_band: "unknown",
    },
    Frame {
        id: "since_morning",
        prefix: "",
        suffix: " since this morning",
        age_band: "unknown",
    },
    Frame {
        id: "at_home",
        prefix: "",
        suffix: " at home",
        age_band: "unknown",
    },
    Frame {
        id: "need_help_fast",
        prefix: "",
        suffix: " need help fast",
        age_band: "unknown",
    },
    Frame {
        id: "father",
        prefix: "my father: ",
        suffix: "",
        age_band: "older_adult",
    },
    Frame {
        id: "wife",
        prefix: "my wife: ",
        suffix: "",
        age_band: "adult",
    },
    Frame {
        id: "child",
        prefix: "my child: ",
        suffix: "",
        age_band: "child",
    },
    Frame {
        id: "worker",
        prefix: "a man at work: ",
        suffix: "",
        age_band: "adult",
    },
];

/// Evaluation frames. Disjoint from [`TRAIN_FRAMES`] in text, not merely in id.
pub const EVAL_FRAMES: &[Frame] = &[
    Frame {
        id: "eval_bystander",
        prefix: "someone here: ",
        suffix: "",
        age_band: "unknown",
    },
    Frame {
        id: "eval_brother",
        prefix: "my brother: ",
        suffix: "",
        age_band: "adult",
    },
    Frame {
        id: "eval_baby",
        prefix: "my baby: ",
        suffix: "",
        age_band: "infant",
    },
    Frame {
        id: "eval_send_help",
        prefix: "",
        suffix: " please send help",
        age_band: "unknown",
    },
];

/// Severity for the nine cards no red-flag rule names. **No clinician has reviewed this.**
///
/// See the module docs for why every row is `Urgent`. Adding a card to `data/firstaid/`
/// without adding it here fails `tests::every_shipped_protocol_has_a_declared_severity`,
/// which is the point: a new card must not silently acquire a severity.
pub const PENDING_CLINICAL_SEVERITY: &[(&str, Severity)] = &[
    ("burn.thermal", Severity::Urgent),
    ("chest.pain", Severity::Urgent),
    ("dehydration.diarrhoea", Severity::Urgent),
    ("electric.shock", Severity::Urgent),
    ("fracture.suspected", Severity::Urgent),
    ("head.injury", Severity::Urgent),
    ("heat.illness", Severity::Urgent),
    ("poisoning.swallowed", Severity::Urgent),
    ("snake.bite", Severity::Urgent),
];

/// The `specialty` slot each card implies.
///
/// Derived from the card, never from the message. `city_pack.rs` routes on this field, so
/// it decides which hospital a caller is sent to; letting a model infer it from free text
/// would put routing downstream of a guess. The prompt's "use unknown when the specialty
/// is not stated" applies to what the *caller* stated; once a card is chosen, the card
/// states it.
pub const PROTOCOL_SPECIALTY: &[(&str, &str)] = &[
    ("allergy.anaphylaxis", "respiratory"),
    ("bleeding.severe", "trauma"),
    ("breathing.distress", "respiratory"),
    ("burn.thermal", "burns"),
    ("chest.pain", "cardiac"),
    ("choking.adult", "respiratory"),
    ("cpr.adult", "cardiac"),
    ("dehydration.diarrhoea", "general_emergency"),
    ("drowning.rescue", "respiratory"),
    ("electric.shock", "burns"),
    ("fracture.suspected", "trauma"),
    ("head.injury", "trauma"),
    ("heat.illness", "general_emergency"),
    ("poisoning.swallowed", "toxicology"),
    ("seizure.active", "neurology"),
    ("snake.bite", "toxicology"),
    ("stroke.suspected", "neurology"),
    ("unresponsive.breathing", "general_emergency"),
];

/// Messages that are not emergencies, with the span the model may keep.
///
/// Without these the adapter would only ever have seen inputs that deserve a card, and the
/// cheapest way to score well on that data is to always produce one. `crate::eval` scores
/// "no card" as a selection class for the same reason. An empty symptom list is a target
/// in its own right: "where is the nearest pharmacy" contains no symptom, and the grammar
/// permits `[]`.
///
/// `tests::negatives_fire_no_red_flag_rule` proves none of them trips the rule table,
/// which would otherwise make the label a lie.
const NEGATIVES_TRAIN: &[(&str, &[&str])] = &[
    ("i want to book a doctor appointment next week", &[]),
    ("my knee is sore after football", &["knee is sore"]),
    ("where is the nearest pharmacy", &[]),
    ("my son has a runny nose", &["runny nose"]),
    ("i have a mild headache since yesterday", &["mild headache"]),
    ("can i take my tablets with food", &[]),
];

const NEGATIVES_EVAL: &[(&str, &[&str])] = &[
    ("my phone will not charge", &[]),
    ("what are the visiting hours at the hospital", &[]),
    ("my back aches after sitting all day", &["back aches"]),
    ("i need a repeat prescription", &[]),
];

/// How an input was degraded. Two families damage the phrase, three the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Degradation {
    /// Adjacent letters transposed in the longest word. Damages the phrase.
    Misspelling,
    /// `-ing` heard as `-in`, or two words run together. Damages the phrase.
    ///
    /// Both errors survive `normalize::canonical_spelling`, which already folds
    /// `bleedin` and `chokin`. That is visible in `rule_coverage_lost` staying low for
    /// this family rather than being asserted here.
    Asr,
    /// Shouted, unpunctuated, repeated. Damages the frame — and case never moves a span,
    /// because grounding lowercases both sides before comparing.
    Panic,
    /// SMS and transliteration register: `please` → `plz`. Damages the frame.
    Texting,
    /// Dropped articles and copulas, the commonest second-language marker. Damages the
    /// frame.
    DroppedFunctionWords,
}

impl Degradation {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Misspelling => "misspelling",
            Self::Asr => "asr",
            Self::Panic => "panic",
            Self::Texting => "texting",
            Self::DroppedFunctionWords => "dropped_function_words",
        }
    }
}

/// Tried in this order starting at `index % 5`, wrapping. The first family that actually
/// changes the text wins; a family that cannot apply to a given frame is skipped rather
/// than silently producing a "degraded" example identical to its clean twin.
const FAMILIES: &[Degradation] = &[
    Degradation::Misspelling,
    Degradation::Asr,
    Degradation::Panic,
    Degradation::Texting,
    Degradation::DroppedFunctionWords,
];

/// One training or evaluation item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Example {
    /// The message as the pipeline would receive it.
    pub input: String,
    /// The completion the model must emit, in `data/grammar/triage.gbnf` property order.
    pub slots_json: String,
    /// The card the *model* must select, or `None` for "nothing applies".
    pub protocol_id: Option<String>,
    /// The card the *rule table* shows for this exact input, if any. Recorded rather than
    /// re-derived so a gate runner builds the app's ranking — rule card first, model pick
    /// second — without a second implementation of that order.
    pub rule_card: Option<String>,
    pub severity: Severity,
    pub age_band: &'static str,
    pub specialty: &'static str,
    /// Literal spans of [`Example::input`], case-insensitively.
    pub symptoms: Vec<String>,
    pub needs_emergency_services: bool,
    pub degraded: bool,
    pub degradation: Option<Degradation>,
    pub frame_id: &'static str,
    pub source_phrase: String,
}

impl Example {
    /// One JSONL row. `input`/`output` are what a trainer consumes; the rest is provenance
    /// for a human reading a failure, and a trainer ignores it.
    ///
    /// The system prompt is deliberately absent: it is a constant
    /// (`inference::SYSTEM_PROMPT`) and duplicating it into thousands of rows would let a
    /// dataset drift out of step with the prompt the app actually ships.
    #[must_use]
    pub fn to_jsonl(&self) -> String {
        serde_json::json!({
            "input": self.input,
            "output": self.slots_json,
            "protocol_id": self.protocol_id,
            "rule_card": self.rule_card,
            "severity": self.severity.as_str(),
            "degraded": self.degraded,
            "degradation": self.degradation.map(Degradation::as_str),
            "frame": self.frame_id,
            "source_phrase": self.source_phrase,
        })
        .to_string()
    }

    /// The gate case this example expects, in [`crate::eval`]'s terms.
    ///
    /// `expected_protocol` follows the app's own ranking — the rule card if one fired, the
    /// model's pick otherwise, `""` for neither — because that is the card a caller
    /// actually sees. `core/examples/validate_slots.rs` computes the same order, and the
    /// two must not drift apart: a gate scoring a ranking the app does not build would be
    /// measuring a program nobody runs.
    ///
    /// Note that when a rule fires for a *different* card than this example trains toward,
    /// the rule card is what is expected at rank one. The model's target then sits at rank
    /// two, where the top-3 gate still counts it. That is the deterministic layer winning,
    /// which is the arrangement `PLAN.md` §1 asks for.
    #[must_use]
    pub fn to_eval_case(&self) -> crate::eval::EvalCase {
        crate::eval::EvalCase {
            message: self.input.clone(),
            critical: self.severity == Severity::Critical,
            expected_severity: self.severity,
            expected_protocol: self
                .rule_card
                .clone()
                .or_else(|| self.protocol_id.clone())
                .unwrap_or_default(),
            expected_symptoms: self.symptoms.clone(),
            degraded_input: self.degraded,
            requires_handoff: self.needs_emergency_services,
        }
    }
}

/// Convert a split into gate cases, preserving order so a predictions file can be a
/// line-for-line companion.
#[must_use]
pub fn eval_cases(examples: &[Example]) -> Vec<crate::eval::EvalCase> {
    examples.iter().map(Example::to_eval_case).collect()
}

/// What was built, and from what.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatasetManifest {
    pub schema_version: &'static str,
    /// SHA-256 over the labelling surface actually consumed: every `(id, phrase)` pair
    /// plus the severity and specialty assigned to each card. Changing a lay phrase or a
    /// label changes this; changing a protocol's *steps* does not, because steps are never
    /// trained on.
    pub label_source_sha256: String,
    /// SHA-256 over `bundled::FILES`, answering the separate question "was this the
    /// shipped corpus".
    pub bundled_corpus_sha256: String,
    pub train_sha256: String,
    pub eval_sha256: String,
    pub train_count: usize,
    pub eval_count: usize,
    pub train_degraded_count: usize,
    pub eval_degraded_count: usize,
    pub negative_count: usize,
    pub phrases_held_out_per_protocol: usize,
    pub train_frame_count: usize,
    pub eval_frame_count: usize,
    /// Degraded inputs whose clean twin fired a red-flag rule and which no longer do.
    ///
    /// These are the cases with no deterministic floor left underneath them: if the
    /// adapter gets one wrong, nothing catches it. Reported so the number is argued about
    /// rather than discovered.
    pub rule_coverage_lost: usize,
    /// Always `None`. No code path in this module can set it. See the module docs.
    pub clinical_review: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dataset {
    pub train: Vec<Example>,
    pub eval: Vec<Example>,
    pub manifest: DatasetManifest,
}

/// Render a split as JSONL, one example per line, trailing newline.
#[must_use]
pub fn jsonl(examples: &[Example]) -> String {
    let mut out = String::new();
    for example in examples {
        out.push_str(&example.to_jsonl());
        out.push('\n');
    }
    out
}

/// Build the whole dataset from a corpus. Deterministic: same corpus, same bytes.
#[must_use]
pub fn build(corpus: &Corpus) -> Dataset {
    let mut train = Vec::new();
    let mut eval = Vec::new();
    let mut label_source = String::new();
    let mut index = 0usize;
    let mut rule_coverage_lost = 0usize;

    for protocol in corpus.protocols() {
        let id = protocol.id.as_str();
        let Some(severity) = declared_severity(id) else {
            // A card with no declared severity is skipped rather than guessed at. The test
            // suite fails on it, so this branch is unreachable in a green tree.
            continue;
        };
        let specialty = specialty_for(id).unwrap_or("unknown");
        label_source.push_str(&format!("{id}\t{}\t{specialty}\n", severity.as_str()));

        let total = protocol.also_called.len();
        let split_at = total.saturating_sub(PHRASES_HELD_OUT_PER_PROTOCOL);
        for (position, phrase) in protocol.also_called.iter().enumerate() {
            label_source.push_str(&format!("{id}\t{phrase}\n"));
            let held_out = position >= split_at;
            let frames = if held_out { EVAL_FRAMES } else { TRAIN_FRAMES };
            let sink = if held_out { &mut eval } else { &mut train };

            for frame in frames {
                if !frame_fits(frame, id) {
                    continue;
                }
                let clean_input = format!("{}{phrase}{}", frame.prefix, frame.suffix);
                // The label is taken from the clean input and reused by the degraded twin.
                let floor = redflag::assess(&clean_input).severity();
                let labelled = floor.map_or(severity, |floor| Severity::escalate(floor, severity));
                let clean_card = rule_card(&clean_input);

                sink.push(make_example(
                    clean_input.clone(),
                    std::slice::from_ref(phrase),
                    Some(id),
                    clean_card.clone(),
                    labelled,
                    frame,
                    specialty,
                    None,
                    phrase,
                ));

                if let Some((input, symptom, family)) =
                    degrade(frame.prefix, phrase, frame.suffix, index)
                {
                    let degraded_card = rule_card(&input);
                    if clean_card.is_some() && degraded_card.is_none() {
                        rule_coverage_lost += 1;
                    }
                    sink.push(make_example(
                        input,
                        &[symptom],
                        Some(id),
                        degraded_card,
                        labelled,
                        frame,
                        specialty,
                        Some(family),
                        phrase,
                    ));
                }
                index += 1;
            }
        }
    }

    for (source, sink) in [(NEGATIVES_TRAIN, &mut train), (NEGATIVES_EVAL, &mut eval)] {
        for (message, symptoms) in source {
            let spans: Vec<String> = symptoms.iter().map(|s| (*s).to_owned()).collect();
            sink.push(make_example(
                (*message).to_owned(),
                &spans,
                None,
                rule_card(message),
                Severity::SelfCare,
                &Frame {
                    id: "negative",
                    prefix: "",
                    suffix: "",
                    age_band: "unknown",
                },
                "unknown",
                None,
                message,
            ));
        }
    }

    let train_jsonl = jsonl(&train);
    let eval_jsonl = jsonl(&eval);
    let bundled_bytes: String = bundled::FILES
        .iter()
        .map(|(name, body)| format!("{name}\n{body}\n"))
        .collect();

    let manifest = DatasetManifest {
        schema_version: "1",
        label_source_sha256: digest(&label_source),
        bundled_corpus_sha256: digest(&bundled_bytes),
        train_sha256: digest(&train_jsonl),
        eval_sha256: digest(&eval_jsonl),
        train_count: train.len(),
        eval_count: eval.len(),
        train_degraded_count: train.iter().filter(|e| e.degraded).count(),
        eval_degraded_count: eval.iter().filter(|e| e.degraded).count(),
        negative_count: NEGATIVES_TRAIN.len() + NEGATIVES_EVAL.len(),
        phrases_held_out_per_protocol: PHRASES_HELD_OUT_PER_PROTOCOL,
        train_frame_count: TRAIN_FRAMES.len(),
        eval_frame_count: EVAL_FRAMES.len(),
        rule_coverage_lost,
        clinical_review: None,
    };
    Dataset {
        train,
        eval,
        manifest,
    }
}

#[allow(clippy::too_many_arguments)]
fn make_example(
    input: String,
    symptoms: &[String],
    protocol_id: Option<&str>,
    rule_card: Option<String>,
    severity: Severity,
    frame: &Frame,
    specialty: &'static str,
    degradation: Option<Degradation>,
    source_phrase: &str,
) -> Example {
    let needs_emergency_services = severity.bypasses_model();
    let slots_json = slots_json(
        severity,
        protocol_id,
        frame.age_band,
        specialty,
        symptoms,
        needs_emergency_services,
    );
    Example {
        input,
        slots_json,
        protocol_id: protocol_id.map(ToOwned::to_owned),
        rule_card,
        severity,
        age_band: frame.age_band,
        specialty,
        symptoms: symptoms.to_vec(),
        needs_emergency_services,
        degraded: degradation.is_some(),
        degradation,
        frame_id: frame.id,
        source_phrase: source_phrase.to_owned(),
    }
}

/// Assemble the completion in grammar order.
///
/// Hand-assembled rather than serialised from a struct because `serde_json` orders map
/// keys alphabetically and `data/grammar/triage.gbnf` fixes a different order. A model
/// trained on alphabetical JSON would be fighting the grammar on every token.
#[must_use]
pub fn slots_json(
    severity: Severity,
    protocol_id: Option<&str>,
    age_band: &str,
    specialty: &str,
    symptoms: &[String],
    needs_emergency_services: bool,
) -> String {
    let protocol = protocol_id.map_or_else(|| "null".to_owned(), |id| format!("\"{id}\""));
    let symptoms: Vec<String> = symptoms
        .iter()
        .map(|s| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_owned()))
        .collect();
    format!(
        "{{\"schema_version\":\"1\",\"severity\":\"{}\",\"protocol_id\":{protocol},\
         \"age_band\":\"{age_band}\",\"specialty\":\"{specialty}\",\"symptoms\":[{}],\
         \"needs_emergency_services\":{needs_emergency_services}}}",
        severity.as_str(),
        symptoms.join(",")
    )
}

/// The severity a card is labelled with, from the rule table where one names it.
#[must_use]
pub fn declared_severity(protocol_id: &str) -> Option<Severity> {
    let from_rules = redflag::RULES
        .iter()
        .filter(|rule| rule.protocol_id == Some(protocol_id))
        .map(|rule| rule.severity)
        .max();
    from_rules.or_else(|| {
        PENDING_CLINICAL_SEVERITY
            .iter()
            .find(|(id, _)| *id == protocol_id)
            .map(|(_, severity)| *severity)
    })
}

#[must_use]
pub fn specialty_for(protocol_id: &str) -> Option<&'static str> {
    PROTOCOL_SPECIALTY
        .iter()
        .find(|(id, _)| *id == protocol_id)
        .map(|(_, specialty)| *specialty)
}

/// The card the deterministic layer shows for this input, if any.
fn rule_card(input: &str) -> Option<String> {
    redflag::assess(input)
        .card()
        .and_then(|hit| hit.protocol_id)
        .map(ToOwned::to_owned)
}

/// Reject an age band the corpus cannot serve.
///
/// Every card is written for an adult; `cpr.adult` and `choking.adult` say so in their
/// ids. Labelling one of them for an infant or a child would train the adapter to hand a
/// parent adult compression depths for a baby, which is the exact shape of harm
/// `PLAN.md` §1 puts the deterministic layer in front of the model to prevent.
///
/// `choking.adult` lists "child choking" among its search phrases, and that stays: being
/// *findable* by a worried parent is not the same as being *labelled* as paediatric.
fn frame_fits(frame: &Frame, protocol_id: &str) -> bool {
    !(protocol_id.ends_with(".adult") && matches!(frame.age_band, "infant" | "child"))
}

/// Produce a degraded twin: `(input, symptom span, family)`.
///
/// Families are tried starting at `index % FAMILIES.len()` and wrapping, so selection is
/// deterministic and spread evenly, and a family that cannot apply to a frame yields to
/// the next one instead of emitting a "degraded" example that is not degraded.
fn degrade(
    prefix: &str,
    phrase: &str,
    suffix: &str,
    index: usize,
) -> Option<(String, String, Degradation)> {
    let start = index % FAMILIES.len();
    for offset in 0..FAMILIES.len() {
        let Some(family) = FAMILIES.get((start + offset) % FAMILIES.len()) else {
            continue;
        };
        let attempt = match family {
            Degradation::Misspelling => misspell(phrase)
                .map(|damaged| (format!("{prefix}{damaged}{suffix}"), damaged.clone())),
            Degradation::Asr => {
                asr(phrase).map(|damaged| (format!("{prefix}{damaged}{suffix}"), damaged.clone()))
            }
            Degradation::Panic => {
                let input = format!("{prefix}{phrase}{suffix}").to_uppercase();
                Some((format!("{input} PLEASE HELP"), phrase.to_owned()))
            }
            Degradation::Texting => reframe(prefix, phrase, suffix, texting),
            Degradation::DroppedFunctionWords => {
                reframe(prefix, phrase, suffix, drop_function_words)
            }
        };
        if let Some((input, symptom)) = attempt
            && input != format!("{prefix}{phrase}{suffix}")
        {
            return Some((input, symptom, *family));
        }
    }
    None
}

/// Apply a frame-only transform to prefix and suffix, preserving their edge spacing so the
/// phrase stays exactly where it was.
fn reframe(
    prefix: &str,
    phrase: &str,
    suffix: &str,
    transform: fn(&str) -> Option<String>,
) -> Option<(String, String)> {
    let new_prefix = transform_part(prefix, transform);
    let new_suffix = transform_part(suffix, transform);
    if new_prefix.is_none() && new_suffix.is_none() {
        return None;
    }
    let prefix = new_prefix.unwrap_or_else(|| prefix.to_owned());
    let suffix = new_suffix.unwrap_or_else(|| suffix.to_owned());
    Some((format!("{prefix}{phrase}{suffix}"), phrase.to_owned()))
}

fn transform_part(part: &str, transform: fn(&str) -> Option<String>) -> Option<String> {
    let core = part.trim();
    if core.is_empty() {
        return None;
    }
    let changed = transform(core)?;
    let lead = if part.starts_with(' ') { " " } else { "" };
    let trail = if part.ends_with(' ') { " " } else { "" };
    Some(format!("{lead}{changed}{trail}"))
}

/// Transpose the second and third letters of the longest word. First word wins a tie, so
/// the same phrase always degrades the same way.
fn misspell(phrase: &str) -> Option<String> {
    let mut target: Option<&str> = None;
    for word in phrase.split(' ') {
        let long_enough = word.chars().count() >= 4;
        let all_letters = word.chars().all(|c| c.is_ascii_alphabetic());
        if long_enough && all_letters && target.is_none_or(|t| word.len() > t.len()) {
            target = Some(word);
        }
    }
    let target = target?;
    let mut letters: Vec<char> = target.chars().collect();
    letters.swap(1, 2);
    let damaged: String = letters.into_iter().collect();
    let mut done = false;
    let words: Vec<String> = phrase
        .split(' ')
        .map(|word| {
            if !done && word == target {
                done = true;
                damaged.clone()
            } else {
                word.to_owned()
            }
        })
        .collect();
    Some(words.join(" "))
}

/// `-ing` heard as `-in`, or — where there is no `-ing` — the last two words run together.
fn asr(phrase: &str) -> Option<String> {
    let words: Vec<&str> = phrase.split(' ').collect();
    let has_ing = words
        .iter()
        .any(|w| w.chars().count() >= 5 && w.ends_with("ing"));
    if has_ing {
        let clipped: Vec<String> = words
            .iter()
            .map(|w| match w.strip_suffix("ing") {
                Some(stem) if w.chars().count() >= 5 => format!("{stem}in"),
                _ => (*w).to_owned(),
            })
            .collect();
        return Some(clipped.join(" "));
    }
    if words.len() < 2 {
        return None;
    }
    let split = words.len().saturating_sub(2);
    let (head, tail) = words.split_at(split);
    let merged = tail.concat();
    let mut out: Vec<String> = head.iter().map(|w| (*w).to_owned()).collect();
    out.push(merged);
    Some(out.join(" "))
}

/// SMS and transliteration register.
fn texting(text: &str) -> Option<String> {
    map_words(text, |word| {
        Some(match word {
            "please" => "plz",
            "help" => "hlp",
            "what" => "wat",
            "someone" => "sum1",
            "need" => "nd",
            "fast" => "fst",
            "morning" => "mrng",
            "emergency" => "emrgncy",
            "send" => "snd",
            _ => return None,
        })
    })
}

/// Articles and copulas dropped — the commonest second-language marker in the wild, and
/// the one `PLAN.md` §8 names first ("he not waking").
fn drop_function_words(text: &str) -> Option<String> {
    let kept: Vec<&str> = text
        .split(' ')
        .filter(|word| {
            !matches!(
                *word,
                "a" | "an" | "the" | "is" | "are" | "am" | "do" | "does" | "at"
            )
        })
        .collect();
    if kept.len() == text.split(' ').count() || kept.is_empty() {
        return None;
    }
    Some(kept.join(" "))
}

fn map_words(text: &str, lookup: fn(&str) -> Option<&'static str>) -> Option<String> {
    let mut changed = false;
    let words: Vec<String> = text
        .split(' ')
        .map(|word| match lookup(word) {
            Some(replacement) => {
                changed = true;
                replacement.to_owned()
            }
            None => word.to_owned(),
        })
        .collect();
    if changed { Some(words.join(" ")) } else { None }
}

fn digest(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::inference::validate_slots;

    fn corpus() -> Corpus {
        bundled::corpus().0
    }

    fn dataset() -> Dataset {
        build(&corpus())
    }

    fn all(dataset: &Dataset) -> Vec<&Example> {
        dataset.train.iter().chain(dataset.eval.iter()).collect()
    }

    // -----------------------------------------------------------------------
    // Labels exist, come from one place, and cover the corpus
    // -----------------------------------------------------------------------

    /// A card added without a severity decision must fail here rather than be skipped
    /// silently by `build`.
    #[test]
    fn every_shipped_protocol_has_a_declared_severity() {
        for protocol in corpus().protocols() {
            assert!(
                declared_severity(&protocol.id).is_some(),
                "protocol {:?} has no red-flag rule and no row in \
                 PENDING_CLINICAL_SEVERITY; decide its severity rather than shipping it \
                 unlabelled",
                protocol.id
            );
            assert!(
                specialty_for(&protocol.id).is_some(),
                "protocol {:?} has no row in PROTOCOL_SPECIALTY, and city_pack routes on \
                 that field",
                protocol.id
            );
        }
    }

    /// Two sources for one label is two answers waiting to disagree.
    #[test]
    fn no_protocol_takes_its_severity_from_two_places() {
        for (id, _) in PENDING_CLINICAL_SEVERITY {
            assert!(
                !redflag::RULES.iter().any(|r| r.protocol_id == Some(*id)),
                "protocol {id:?} is named by a red-flag rule and also declared in \
                 PENDING_CLINICAL_SEVERITY; delete the pending row and let the rule decide"
            );
        }
    }

    #[test]
    fn the_specialty_table_names_no_card_the_corpus_does_not_have() {
        let corpus = corpus();
        for (id, _) in PROTOCOL_SPECIALTY {
            assert!(corpus.get(id).is_some(), "{id:?} is not in the corpus");
        }
        for (id, _) in PENDING_CLINICAL_SEVERITY {
            assert!(corpus.get(id).is_some(), "{id:?} is not in the corpus");
        }
    }

    /// The refusal that makes the rest of P5 mean anything.
    #[test]
    fn the_generator_cannot_attest_to_its_own_clinical_review() {
        assert_eq!(
            dataset().manifest.clinical_review,
            None,
            "synthetic data generated in this repository has not been reviewed by anyone; \
             a manifest that claimed otherwise would defeat the P5 gate"
        );
    }

    // -----------------------------------------------------------------------
    // The examples are usable by the shipped pipeline
    // -----------------------------------------------------------------------

    /// Every completion must survive the validator the app actually runs. A dataset the
    /// shipped code would reject is a dataset that trains a model to be rejected.
    #[test]
    fn every_example_validates_through_the_shipped_validator() {
        let corpus = corpus();
        let dataset = dataset();
        for example in all(&dataset) {
            let floor = redflag::assess(&example.input).severity();
            let slots = validate_slots(&example.slots_json, &corpus, floor)
                .unwrap_or_else(|e| panic!("{:?} rejected: {e}", example.input));
            assert_eq!(
                slots.severity, example.severity,
                "the rule floor moved the label for {:?}; the dataset and the pipeline \
                 disagree about the answer",
                example.input
            );
            assert_eq!(slots.protocol_id, example.protocol_id);
            assert_eq!(
                slots.needs_emergency_services,
                example.needs_emergency_services
            );
        }
    }

    /// `retain_grounded_symptoms` deletes any phrase that is not a literal span of the
    /// report. A training target it would delete teaches the model to produce output the
    /// app throws away.
    #[test]
    fn every_symptom_survives_grounding_against_its_own_input() {
        let corpus = corpus();
        let dataset = dataset();
        for example in all(&dataset) {
            let floor = redflag::assess(&example.input).severity();
            let mut slots = validate_slots(&example.slots_json, &corpus, floor).expect("valid");
            slots.retain_grounded_symptoms(&example.input);
            assert_eq!(
                slots.symptoms, example.symptoms,
                "a symptom was dropped by grounding for input {:?}",
                example.input
            );
        }
    }

    #[test]
    fn the_completion_is_in_grammar_order_not_alphabetical_order() {
        let example = dataset()
            .train
            .first()
            .cloned()
            .expect("a training example");
        let order = [
            "schema_version",
            "severity",
            "protocol_id",
            "age_band",
            "specialty",
            "symptoms",
            "needs_emergency_services",
        ];
        let mut cursor = 0usize;
        for key in order {
            let found = example
                .slots_json
                .find(key)
                .unwrap_or_else(|| panic!("{key} missing from {}", example.slots_json));
            assert!(
                found >= cursor,
                "{key} is out of grammar order in {}",
                example.slots_json
            );
            cursor = found;
        }
    }

    // -----------------------------------------------------------------------
    // The holdout is real
    // -----------------------------------------------------------------------

    #[test]
    fn train_and_eval_share_no_phrase() {
        let dataset = dataset();
        let train: std::collections::BTreeSet<&str> = dataset
            .train
            .iter()
            .filter(|e| e.protocol_id.is_some())
            .map(|e| e.source_phrase.as_str())
            .collect();
        for example in dataset.eval.iter().filter(|e| e.protocol_id.is_some()) {
            assert!(
                !train.contains(example.source_phrase.as_str()),
                "phrase {:?} appears in both splits",
                example.source_phrase
            );
        }
        assert!(!train.is_empty(), "no training phrases at all");
    }

    /// Frames must be disjoint in *text*, not merely in id, or an eval item is a training
    /// item wearing a different label.
    #[test]
    fn train_and_eval_share_no_frame_text() {
        for eval in EVAL_FRAMES {
            for train in TRAIN_FRAMES {
                assert!(
                    !(eval.prefix == train.prefix && eval.suffix == train.suffix),
                    "eval frame {:?} is textually identical to train frame {:?}",
                    eval.id,
                    train.id
                );
            }
        }
    }

    #[test]
    fn no_adult_only_card_is_ever_labelled_for_a_child() {
        let dataset = dataset();
        for example in all(&dataset) {
            let Some(id) = example.protocol_id.as_deref() else {
                continue;
            };
            if id.ends_with(".adult") {
                assert!(
                    !matches!(example.age_band, "infant" | "child"),
                    "{id} was labelled {} for input {:?}",
                    example.age_band,
                    example.input
                );
            }
        }
        // And the guard has to actually be doing something, or it is decoration.
        assert!(
            TRAIN_FRAMES
                .iter()
                .chain(EVAL_FRAMES)
                .any(|f| matches!(f.age_band, "infant" | "child")),
            "no paediatric frame exists, so frame_fits proves nothing"
        );
    }

    // -----------------------------------------------------------------------
    // Degradation
    // -----------------------------------------------------------------------

    /// A degraded twin that equals its clean twin would be counted in the degraded split
    /// while measuring nothing, which is exactly the "clean-prose average hiding it" that
    /// `PLAN.md` §8 holds the split out to prevent.
    #[test]
    fn a_degraded_example_is_actually_degraded() {
        let dataset = dataset();
        let mut clean: std::collections::BTreeMap<(&str, &str), &str> =
            std::collections::BTreeMap::new();
        for example in all(&dataset).into_iter().filter(|e| !e.degraded) {
            clean.insert(
                (example.source_phrase.as_str(), example.frame_id),
                example.input.as_str(),
            );
        }
        let mut checked = 0usize;
        for example in all(&dataset).into_iter().filter(|e| e.degraded) {
            let twin = clean
                .get(&(example.source_phrase.as_str(), example.frame_id))
                .unwrap_or_else(|| {
                    panic!(
                        "degraded example {:?} has no clean twin; its label was copied from \
                         something that does not exist",
                        example.input
                    )
                });
            assert_ne!(
                *twin, example.input,
                "a degraded example is identical to its clean twin"
            );
            checked += 1;
        }
        assert!(checked > 0, "nothing in the dataset is degraded");
    }

    /// Every family must be reachable, or the split is narrower than it claims.
    #[test]
    fn every_degradation_family_appears_in_the_data() {
        let dataset = dataset();
        for family in FAMILIES {
            assert!(
                all(&dataset).iter().any(|e| e.degradation == Some(*family)),
                "no example uses the {:?} family",
                family.as_str()
            );
        }
    }

    #[test]
    fn a_degraded_twin_keeps_its_clean_twins_severity() {
        // "knocked out" trips the unresponsive rule, so this input is labelled Critical,
        // not the Urgent that head.injury declares. Shouting it must not change that.
        let clean = "my child: knocked out";
        assert_eq!(redflag::assess(clean).severity(), Some(Severity::Critical));
        let (degraded, _, family) =
            degrade("my child: ", "knocked out", "", 2).expect("panic always applies");
        assert_eq!(family, Degradation::Panic);
        assert_eq!(
            redflag::assess(&degraded).severity(),
            Some(Severity::Critical),
            "panic degradation must not cost the rule floor"
        );
    }

    #[test]
    fn misspelling_and_asr_move_the_span_and_frame_families_do_not() {
        let (_, span, family) = degrade("", "bleeding", "", 0).expect("misspells");
        assert_eq!(family, Degradation::Misspelling);
        assert_eq!(span, "beleding");

        let (_, span, family) = degrade("", "bleeding", "", 1).expect("asr");
        assert_eq!(family, Degradation::Asr);
        assert_eq!(span, "bleedin");

        let (input, span, family) = degrade("please help ", "chest pain", " right now", 3)
            .expect("texting applies to this frame");
        assert_eq!(family, Degradation::Texting);
        assert_eq!(span, "chest pain", "a frame family must not move the span");
        assert_eq!(input, "plz hlp chest pain right now");
    }

    /// `normalize` already folds `breathin` and `chokin` onto their lemmas, so this family
    /// should cost no rule coverage at all. That is two modules agreeing, and it is worth
    /// failing on if someone deletes a fold.
    #[test]
    fn asr_degradation_does_not_cost_the_red_flag_layer() {
        for clean in ["not breathing", "choking"] {
            let before = redflag::assess(clean).card().map(|hit| hit.rule_id);
            assert!(
                before.is_some(),
                "{clean:?} must fire a rule or this test proves nothing"
            );
            let (damaged, _, family) = degrade("", clean, "", 1).expect("asr applies");
            assert_eq!(family, Degradation::Asr);
            let after = redflag::assess(&damaged).card().map(|hit| hit.rule_id);
            assert_eq!(
                before, after,
                "{clean:?} degraded to {damaged:?} and lost its rule"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Negatives
    // -----------------------------------------------------------------------

    /// A negative that trips a rule is mislabelled, and would train the model to argue
    /// with the deterministic layer.
    #[test]
    fn negatives_fire_no_red_flag_rule() {
        for (message, _) in NEGATIVES_TRAIN.iter().chain(NEGATIVES_EVAL) {
            let assessment = redflag::assess(message);
            assert!(
                assessment.is_empty(),
                "negative {message:?} fired {:?}",
                assessment
                    .hits
                    .iter()
                    .map(|h| h.rule_id)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn both_splits_contain_negatives_and_they_select_no_card() {
        let dataset = dataset();
        for split in [&dataset.train, &dataset.eval] {
            let negatives: Vec<&Example> =
                split.iter().filter(|e| e.protocol_id.is_none()).collect();
            assert!(!negatives.is_empty(), "a split has no negatives");
            for example in negatives {
                assert_eq!(example.severity, Severity::SelfCare);
                assert!(!example.needs_emergency_services);
                assert!(example.slots_json.contains("\"protocol_id\":null"));
            }
        }
    }

    // -----------------------------------------------------------------------
    // Shape, size, determinism
    // -----------------------------------------------------------------------

    /// `PLAN.md` §8 calls for thousands of slot-extraction examples: "highest training
    /// value per token".
    #[test]
    fn the_training_split_has_thousands_of_examples() {
        let manifest = dataset().manifest;
        assert!(
            manifest.train_count >= 2000,
            "only {} training examples",
            manifest.train_count
        );
        assert!(
            manifest.eval_count >= 200,
            "only {} eval examples",
            manifest.eval_count
        );
        assert!(
            manifest.eval_degraded_count * 3 >= manifest.eval_count,
            "the degraded split is {} of {} eval examples, too thin to gate on",
            manifest.eval_degraded_count,
            manifest.eval_count
        );
    }

    /// `docs/CONVENTIONS.md` §2. There is no RNG here, so this must hold exactly — and the
    /// manifest digests are worthless if it does not.
    #[test]
    fn building_twice_produces_identical_bytes() {
        let first = dataset();
        let second = dataset();
        assert_eq!(first.manifest, second.manifest);
        assert_eq!(jsonl(&first.train), jsonl(&second.train));
        assert_eq!(jsonl(&first.eval), jsonl(&second.eval));
    }

    /// Provenance is a field, not a vibe (`docs/CONVENTIONS.md` §9).
    #[test]
    fn the_manifest_digests_change_when_the_data_changes() {
        let manifest = dataset().manifest;
        assert_eq!(manifest.train_sha256.len(), 64);
        assert_ne!(manifest.train_sha256, manifest.eval_sha256);
        assert_ne!(manifest.label_source_sha256, manifest.bundled_corpus_sha256);
        assert!(
            manifest.rule_coverage_lost
                <= manifest.train_degraded_count + manifest.eval_degraded_count,
            "more coverage lost than there are degraded examples"
        );
    }

    // -----------------------------------------------------------------------
    // Handing the eval split to the gates
    // -----------------------------------------------------------------------

    /// `eval::evaluate` fails closed on an absent critical, degraded, or handoff subset.
    /// If this split stopped populating one of them, the gate would report a named failure
    /// rather than a pass — but the dataset would still be quietly broken, so check here.
    #[test]
    fn the_eval_split_populates_every_subset_the_gates_require() {
        let cases = eval_cases(&dataset().eval);
        assert!(cases.iter().any(|c| c.critical), "no critical cases");
        assert!(cases.iter().any(|c| c.degraded_input), "no degraded cases");
        assert!(cases.iter().any(|c| c.requires_handoff), "no handoff cases");
        assert!(
            cases.iter().any(|c| !c.requires_handoff),
            "every case requires handoff, so the handoff gate cannot catch a false positive"
        );
        assert!(
            cases.iter().any(|c| c.expected_protocol.is_empty()),
            "no case expects an empty ranking, so 'nothing applies' is untested"
        );
    }

    /// The rule card outranks the model's pick, so that is what the gate must expect.
    #[test]
    fn a_rule_card_outranks_the_models_own_target_in_the_expectation() {
        let dataset = dataset();
        let escalated = dataset
            .eval
            .iter()
            .chain(&dataset.train)
            .find(|e| e.rule_card.is_some() && e.rule_card.as_deref() != e.protocol_id.as_deref())
            .expect("some phrase trips a rule for a different card than it trains toward");
        let case = escalated.to_eval_case();
        assert_eq!(case.expected_protocol, escalated.rule_card.clone().unwrap());
        assert_ne!(
            case.expected_protocol,
            escalated.protocol_id.clone().unwrap()
        );
    }

    #[test]
    fn a_negative_expects_no_card_and_no_handoff() {
        let case = dataset()
            .eval
            .iter()
            .find(|e| e.protocol_id.is_none())
            .expect("a negative in the eval split")
            .to_eval_case();
        assert_eq!(case.expected_protocol, "");
        assert!(!case.requires_handoff);
        assert!(!case.critical);
    }

    #[test]
    fn a_jsonl_row_is_one_line_of_parseable_json() {
        let example = dataset().eval.first().cloned().expect("an eval example");
        let line = example.to_jsonl();
        assert!(!line.contains('\n'));
        let parsed: serde_json::Value = serde_json::from_str(&line).expect("valid json");
        assert_eq!(
            parsed.get("input").and_then(|v| v.as_str()),
            Some(example.input.as_str())
        );
        // The completion is carried as a string: it is text the model must produce.
        let output = parsed
            .get("output")
            .and_then(|v| v.as_str())
            .expect("output is a string");
        assert_eq!(output, example.slots_json);
    }
}
