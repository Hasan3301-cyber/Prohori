//! The red-flag rule layer: deterministic, allocation-light, and in front of the model.
//!
//! `PLAN.md` §1 puts this layer *before* inference for the presentations where
//! latency kills. It runs on raw text in microseconds and never loads a model.
//!
//! # Why there is no negation analysis here
//!
//! EcoGuardian's `intent.py` did careful negation handling: it looked back four
//! words, split on contrast conjunctions, and suppressed a match behind `no`/`not`.
//! That is right for classifying a question and wrong for catching an arrest,
//! because the two directions are not symmetric:
//!
//! - "not breathing" — the negation *is* the emergency
//! - "not bleeding"  — the negation removes it
//!
//! A parser that gets the first case backwards kills someone. So this layer does
//! not analyse negation at all. **Every trigger encodes the emergency in its own
//! surface form.** Bare `breathe` is never a trigger; ` not breathe ` is. A red
//! flag therefore cannot be suppressed by a negation the parser misreads, because
//! nothing here suppresses anything.
//!
//! # Known, deliberate overtriage
//!
//! `" can not breathe "` contains `" not breathe "`, so "I can't breathe" — a
//! *conscious* person in respiratory distress — also fires the CPR rule, and the
//! CPR rule is higher priority than `rf.breathing.severe_distress`, so the CPR
//! card is what shows. That ordering is not laziness: `gasp` is a trigger of the
//! distress rule, and agonal gasping *is* arrest. Ranking distress above arrest
//! would send a real arrest to the wrong card, which is the worse mistake.
//!
//! It is accepted rather than special-cased, because the protocol itself is the
//! guard: `cpr.adult` step 1 is an assessment ("tap the shoulders, shout"), not an
//! action. A reader who is talking will fail that check in two seconds and read on
//! — and `breathing.distress` is one of the search results sitting under the card.
//! The alternative — suppression logic — reintroduces exactly the failure mode this
//! module is built to avoid.
//!
//! That guard is not a hope about the corpus. It is enforced by
//! `tests/corpus_integrity.rs::cpr_card_opens_with_an_assessment_step`, and more
//! broadly by `no_protocol_opens_with_an_action`. If `cpr.adult` step 1 ever became an
//! action, that test fails and this trade-off has to be revisited rather than silently
//! becoming unsafe.
//!
//! The same shape appears once more: `"stroke"` is a trigger of
//! `rf.neuro.stroke_fast`, and "heat stroke" contains it, so a heat emergency shows
//! the stroke card. Retrieval ranks `heat.illness` first for that query — see
//! `tests/retrieval_quality.rs` — so the right card is on screen either way, and
//! `stroke.suspected` carries an `escalate_if` line that names heat and says to start
//! cooling. Both readings end in "call now", which is why this is tolerable.

use crate::normalize::{contains_phrase, normalize};
use crate::severity::Severity;
use serde::{Deserialize, Serialize};

/// Whether a rule can currently show a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleStatus {
    /// The protocol exists in `data/firstaid/` and the card renders.
    Active,
    /// The rule is written and matches, but its protocol has not been authored yet.
    ///
    /// Listed rather than deleted, following EcoGuardian's `roster.py`, which
    /// enumerated the agents the design report claimed and the code did not have.
    /// A pending hit still surfaces the emergency dialer — it renders as "call now,
    /// we do not yet have a card for this" rather than as silence.
    ///
    /// [`RULES`] currently has none: every rule in the table has a card. The variant
    /// stays because the next rule someone writes will be pending before its card is
    /// written, and the path that shows it — [`RedFlagAssessment::unsupported`] and the
    /// FFI's `recognised_without_guidance` — has to keep working on the day that
    /// happens. `tests/redflag_safety.rs::a_rule_with_no_card_is_recognised_out_loud`
    /// exercises it against a rule built in the test, not against the shipped table.
    Pending,
}

/// One red-flag rule.
#[derive(Debug, Clone, Copy)]
pub struct RedFlagRule {
    /// Stable id. Appears in traces and audit entries; never reused or renumbered.
    pub id: &'static str,
    /// Protocol in `data/firstaid/`. `None` exactly when `status` is `Pending`.
    pub protocol_id: Option<&'static str>,
    pub severity: Severity,
    pub status: RuleStatus,
    /// Phrases in canonical (post-`normalize`) form. Self-validated by
    /// `tests/redflag_safety.rs::every_trigger_is_already_in_canonical_form`.
    pub triggers: &'static [&'static str],
}

/// The rule table, in priority order. Index 0 is shown first when several fire.
///
/// **Ordering is provisional and pending clinician review** (`PLAN.md` §10, open
/// question 3). It currently follows `<C>ABC` with cardiac arrest ahead of
/// catastrophic haemorrhage, on the grounds that compressions are the action that
/// cannot wait. That is a clinical judgement this repository is not qualified to
/// make alone, and it is written down here so a reviewer can change one number.
pub static RULES: &[RedFlagRule] = &[
    RedFlagRule {
        id: "rf.airway.not_breathing",
        protocol_id: Some("cpr.adult"),
        severity: Severity::Critical,
        status: RuleStatus::Active,
        triggers: &[
            "not breathe",
            "no breathe",
            "stop breathe",
            "no sign of breathe",
        ],
    },
    RedFlagRule {
        id: "rf.circulation.no_pulse",
        protocol_id: Some("cpr.adult"),
        severity: Severity::Critical,
        status: RuleStatus::Active,
        triggers: &[
            "no pulse",
            "not pulse",
            "pulseless",
            "no heartbeat",
            "no heart beat",
            "can not find pulse",
            "can not feel pulse",
            "heart stop",
            "cardiac arrest",
        ],
    },
    RedFlagRule {
        id: "rf.bleeding.severe",
        protocol_id: Some("bleeding.severe"),
        severity: Severity::Critical,
        status: RuleStatus::Active,
        triggers: &[
            "severe bleed",
            "bad bleed",
            "bleed bad",
            "heavy bleed",
            "bleed heavy",
            "bleed lot",
            "lot of blood",
            "blood everywhere",
            "blood soak",
            "soak through",
            "not stop bleed",
            "bleed will not stop",
            "bleed does not stop",
            "spurt",
            "gush",
            "bleed out",
            "deep cut",
            "cut artery",
            "artery",
        ],
    },
    RedFlagRule {
        id: "rf.airway.choking",
        protocol_id: Some("choking.adult"),
        severity: Severity::Critical,
        status: RuleStatus::Active,
        triggers: &[
            "choke",
            "food stuck",
            "stuck in throat",
            "stuck in windpipe",
            "something in throat",
            "can not swallow",
            "grab throat",
            "hand on throat",
        ],
    },
    RedFlagRule {
        id: "rf.consciousness.unresponsive",
        protocol_id: Some("unresponsive.breathing"),
        severity: Severity::Critical,
        status: RuleStatus::Active,
        triggers: &[
            "unresponsive",
            "unconscious",
            "not conscious",
            "not respond",
            "no respond",
            "not wake",
            "pass out",
            "out cold",
            "black out",
            "knock out",
        ],
    },
    // ---------------------------------------------------------------------
    // Below here the rules are lower in priority, not lower in severity.
    // Every one has a card; the ordering above is about which card is shown
    // first when several fire, and is still pending clinician review.
    // ---------------------------------------------------------------------
    RedFlagRule {
        id: "rf.allergy.anaphylaxis",
        protocol_id: Some("allergy.anaphylaxis"),
        severity: Severity::Critical,
        status: RuleStatus::Active,
        triggers: &[
            "anaphylaxis",
            "throat swell",
            "swell throat",
            "tongue swell",
            "face swell",
            "allergy reaction",
            "epipen",
            "epi pen",
        ],
    },
    RedFlagRule {
        id: "rf.environment.drowning",
        protocol_id: Some("drowning.rescue"),
        severity: Severity::Critical,
        status: RuleStatus::Active,
        triggers: &[
            "drown",
            "under water",
            "pull out of water",
            "fall in water",
            "fall in pool",
            "fall in river",
        ],
    },
    RedFlagRule {
        id: "rf.breathing.severe_distress",
        protocol_id: Some("breathing.distress"),
        severity: Severity::Critical,
        status: RuleStatus::Active,
        triggers: &[
            "can not breathe",
            "struggle to breathe",
            "barely breathe",
            "hardly breathe",
            "fight for breathe",
            "short of breathe",
            "gasp",
            "turn blue",
            "go blue",
            "lip are blue",
            "lip blue",
        ],
    },
    RedFlagRule {
        id: "rf.neuro.stroke_fast",
        protocol_id: Some("stroke.suspected"),
        severity: Severity::Critical,
        status: RuleStatus::Active,
        triggers: &[
            "stroke",
            "face droop",
            "droop face",
            "mouth droop",
            "slur",
            "weak on one side",
            "numb on one side",
            "can not lift arm",
            "can not raise arm",
            "one side of face",
            "one side of body",
        ],
    },
    RedFlagRule {
        id: "rf.neuro.seizure",
        protocol_id: Some("seizure.active"),
        severity: Severity::Critical,
        status: RuleStatus::Active,
        triggers: &[
            "seizure",
            "convulsion",
            "have fit",
            "take fit",
            "shake uncontrollably",
            "jerk uncontrollably",
            "body jerk",
            "foam at mouth",
        ],
    },
];

/// One rule that fired, with the trigger that fired it.
///
/// `Serialize` only: hits are produced for traces and the FFI boundary, never read
/// back from JSON, and `&'static str` fields cannot be deserialized into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RedFlagHit {
    pub rule_id: &'static str,
    pub protocol_id: Option<&'static str>,
    pub severity: Severity,
    pub status: RuleStatus,
    /// The exact trigger phrase that matched. Goes into the trace so a reviewer can
    /// see *why* a card appeared, not just that it did.
    pub matched: &'static str,
    /// Index into [`RULES`]. Lower is more urgent.
    pub priority: usize,
}

/// The result of running the whole table over one message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RedFlagAssessment {
    /// Every rule that fired, already in priority order. Never sorted afterwards.
    pub hits: Vec<RedFlagHit>,
}

impl RedFlagAssessment {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }

    /// Highest severity across all hits, including pending ones.
    #[must_use]
    pub fn severity(&self) -> Option<Severity> {
        self.hits.iter().map(|h| h.severity).max()
    }

    /// The card to render: the highest-priority hit that has a protocol.
    #[must_use]
    pub fn card(&self) -> Option<&RedFlagHit> {
        self.hits.iter().find(|h| h.status == RuleStatus::Active)
    }

    /// Hits we recognised but cannot yet give a card for. The UI must show these —
    /// dropping them silently would turn "we know this is an emergency and have no
    /// guidance" into "we found nothing", which are very different messages.
    #[must_use]
    pub fn unsupported(&self) -> Vec<&RedFlagHit> {
        self.hits
            .iter()
            .filter(|h| h.status == RuleStatus::Pending)
            .collect()
    }
}

/// Run every rule over `raw`. Normalizes once, then scans.
///
/// Hits come back in [`RULES`] order, which is priority order, so no sort happens
/// and the result is deterministic for a given input.
#[must_use]
pub fn assess(raw: &str) -> RedFlagAssessment {
    let text = normalize(raw);
    let mut hits = Vec::new();
    for (priority, rule) in RULES.iter().enumerate() {
        // First matching trigger wins; a rule fires at most once.
        if let Some(matched) = rule
            .triggers
            .iter()
            .find(|trigger| contains_phrase(&text, trigger))
        {
            hits.push(RedFlagHit {
                rule_id: rule.id,
                protocol_id: rule.protocol_id,
                severity: rule.severity,
                status: rule.status,
                matched,
                priority,
            });
        }
    }
    RedFlagAssessment { hits }
}
