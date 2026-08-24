//! The uniffi boundary: everything the Kotlin shell can see of the Rust core.
//!
//! # This layer is a contract, not a projection
//!
//! Nothing from [`prohori_core`] is re-exported. Every type crossing the boundary is
//! declared here and built from a core type by hand. That is more code than exposing the
//! core directly, and it buys two things:
//!
//! - Adding a field to a core struct cannot silently change the app's API. A boundary
//!   that tracks internals automatically is a boundary that changes when nobody meant it
//!   to, and this one carries medical instructions.
//! - The boundary can be *shaped* for the caller. [`Triage::recognised_without_guidance`]
//!   is the clearest case: the core exposes it as a filter over hits, and this layer
//!   materialises it as its own field, because a UI that has to remember to filter is a
//!   UI that will eventually forget and turn "we know this is an emergency and have no
//!   card for it" into silence.
//!
//! # Nothing here returns an error
//!
//! There is no `Result` in this file, and that is deliberate rather than lazy:
//!
//! - The corpus is compiled into the library ([`prohori_core::bundled`]), so
//!   construction has no I/O and no failure mode. A build that shipped a broken card
//!   reports it through [`Prohori::corpus_load_errors`] — a field the About screen can
//!   show — instead of an exception at startup, because refusing to start is the one
//!   behaviour this app must never have.
//! - Triage over a message cannot fail. An unrecognised message produces an empty
//!   [`Triage`], which is a different thing from an error and must read differently in
//!   the UI.
//! - [`Prohori::emergency_numbers`] always returns a dialable number, down to the GSM
//!   fallback. See `prohori_core::emergency` for why refusing is the dangerous answer
//!   for a phone number and the safe one for a road segment.
//!
//! An exception the Kotlin side can forget to catch is an exception that shows a blank
//! screen to someone holding a phone next to a person who is not breathing.
//!
//! # Licence note
//!
//! uniffi is MPL-2.0. It does not reach these Apache-2.0 sources, but the generated
//! Kotlin embeds uniffi's runtime templates, so MPL-2.0 belongs in the app's notices.

use prohori_core::city_pack::{HospitalRouteRequest, LoadedCityPack};
use prohori_core::confirmation::{
    Channel as CoreConfirmationChannel, Confirmation as CoreConfirmation,
    ConfirmationState as CoreConfirmationState, Reply as CoreHospitalReply,
    ReplySource as CoreReplySource,
};
use prohori_core::emergency::{self, NumberSource, PackEmergency};
use prohori_core::fallback;
use prohori_core::guidance;
use prohori_core::inference::{self, AgeBand, Specialty};
use prohori_core::protocol::{Corpus, Protocol, StepKind};
use prohori_core::redflag::{self, RedFlagHit, RuleStatus};
use prohori_core::render;
use prohori_core::retrieval::Index;
use prohori_core::severity::Severity;
use prohori_core::verifier;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

uniffi::setup_scaffolding!();

// ---------------------------------------------------------------------------
// Severity
// ---------------------------------------------------------------------------

/// How urgent this is. Mirrors `prohori_core::severity::Severity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Urgency {
    /// Manage at home. No transport needed.
    SelfCare,
    /// See a clinician, but not tonight.
    Standard,
    /// Needs care within hours. Go now.
    Urgent,
    /// Life is in immediate danger. Call, and start the protocol.
    Critical,
}

impl From<Severity> for Urgency {
    fn from(value: Severity) -> Self {
        match value {
            Severity::SelfCare => Self::SelfCare,
            Severity::Standard => Self::Standard,
            Severity::Urgent => Self::Urgent,
            Severity::Critical => Self::Critical,
        }
    }
}

// ---------------------------------------------------------------------------
// Cards
// ---------------------------------------------------------------------------

/// What a step does to the patient, so the UI can style assessment and action
/// differently. See `data/firstaid/SCHEMA.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum StepAction {
    /// Look, listen, ask. Nothing has been done to the patient yet.
    Assessment,
    /// Do this to the patient.
    Action,
    /// Get more help than you are.
    Escalation,
}

impl From<StepKind> for StepAction {
    fn from(value: StepKind) -> Self {
        match value {
            StepKind::Assessment => Self::Assessment,
            StepKind::Action => Self::Action,
            StepKind::Escalation => Self::Escalation,
        }
    }
}

/// One instruction on a card.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CardStep {
    /// 1-based, as printed. Not an index.
    pub number: u32,
    pub kind: StepAction,
    pub text: String,
}

/// A first-aid card, ready to render with no further logic on the Kotlin side.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FirstAidCard {
    pub protocol_id: String,
    pub title: String,
    /// Who this card is for. Shown above the steps so someone who arrived here by
    /// mistake can leave before doing anything.
    pub applies_to: String,
    pub steps: Vec<CardStep>,
    /// Rendered verbatim, never paraphrased. The verifier cannot detect a polarity
    /// inversion, so these lines do not pass through anything that could rewrite them.
    pub do_not: Vec<String>,
    pub escalate_if: Vec<String>,
    /// Human-readable citation lines, for the provenance footer.
    pub sources: Vec<String>,
    /// False for every card in this build. The UI must say so rather than imply a
    /// clinical authority nobody has given (`docs/CONVENTIONS.md` §9).
    pub clinically_reviewed: bool,
    /// Name and credential of the reviewing clinician, when there is one.
    pub reviewed_by: Option<String>,
    /// One sentence on who has checked this card. Never empty, so there is no state in
    /// which a screen can show a card and show nothing about where it came from.
    ///
    /// Authored in Rust rather than in Kotlin on purpose: the same sentence has to appear
    /// on the screen and in `plain_text`, and two copies of a sentence about medical
    /// authority is one copy too many.
    pub provenance: String,
    /// The whole card as one block of text — steps, sources, and review status — straight
    /// from the file. What the app shows when there is no model on the device, and what
    /// leaves the app when someone shares a card. See `prohori_core::render`.
    pub plain_text: String,
}

/// A BM25 result ready for the app to render.
///
/// The score deliberately does not cross the boundary: it is meaningful only while
/// ranking one result list and showing it would turn an information-retrieval value into
/// fake medical confidence. The words that matched do cross, so a user can judge why the
/// card appeared.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SearchResult {
    pub card: FirstAidCard,
    pub matched: Vec<String>,
}

impl From<&Protocol> for FirstAidCard {
    fn from(protocol: &Protocol) -> Self {
        Self {
            protocol_id: protocol.id.clone(),
            title: protocol.title.clone(),
            applies_to: protocol.applies_to.clone(),
            steps: protocol
                .steps
                .iter()
                .map(|step| CardStep {
                    number: step.n,
                    kind: step.kind.into(),
                    text: step.text.clone(),
                })
                .collect(),
            do_not: protocol.do_not.clone(),
            escalate_if: protocol.escalate_if.clone(),
            sources: render::source_lines(protocol),
            clinically_reviewed: protocol.is_clinically_reviewed(),
            reviewed_by: protocol.reviewed_by.clone(),
            provenance: render::provenance(protocol),
            plain_text: render::plain_text(protocol),
        }
    }
}

// ---------------------------------------------------------------------------
// Triage
// ---------------------------------------------------------------------------

/// One red-flag rule that fired, with the phrase that fired it.
///
/// `matched` is here so the UI can show *why* a card appeared. A card that arrives
/// unexplained is a card a frightened person cannot sanity-check.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RecognisedEmergency {
    pub rule_id: String,
    pub matched: String,
    pub severity: Urgency,
    /// True when the rule has no protocol written yet.
    pub guidance_pending: bool,
}

impl From<&RedFlagHit> for RecognisedEmergency {
    fn from(hit: &RedFlagHit) -> Self {
        Self {
            rule_id: hit.rule_id.to_owned(),
            matched: hit.matched.to_owned(),
            severity: hit.severity.into(),
            guidance_pending: hit.status == RuleStatus::Pending,
        }
    }
}

/// The result of running the rule table over one message.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct Triage {
    /// Highest severity across every rule that fired, pending ones included. `None`
    /// means nothing fired — which is not the same as "not urgent", and must not be
    /// rendered as reassurance.
    pub severity: Option<Urgency>,
    /// The card to show, when a rule that fired has one.
    pub card: Option<FirstAidCard>,
    /// Every rule that fired, already in priority order. This is the trace.
    pub hits: Vec<RecognisedEmergency>,
    /// Rules that fired with no card behind them. A subset of `hits`, materialised
    /// rather than left for the UI to filter — see the module docs. When this is
    /// non-empty the UI must say "this looks serious and we have no guidance yet" and
    /// keep the dialer in front of the user.
    pub recognised_without_guidance: Vec<RecognisedEmergency>,
    /// True when the deterministic layer answered on its own and no model should be
    /// consulted. In this build it is the only path that exists.
    pub bypasses_model: bool,
}

// ---------------------------------------------------------------------------
// Emergency numbers
// ---------------------------------------------------------------------------

/// Where a number came from. The UI shows this; a guess must never look like local
/// knowledge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum NumberProvenance {
    /// The user typed it. Trusted above everything else.
    UserOverride,
    /// From a downloaded city pack.
    CityPack,
    /// From the table compiled into this library.
    BuiltIn,
    /// The GSM emergency number, used when nothing else is known.
    GsmFallback,
}

impl From<NumberSource> for NumberProvenance {
    fn from(value: NumberSource) -> Self {
        match value {
            NumberSource::UserOverride => Self::UserOverride,
            NumberSource::CityPack => Self::CityPack,
            NumberSource::BuiltIn => Self::BuiltIn,
            NumberSource::GsmFallback => Self::GsmFallback,
        }
    }
}

/// Numbers to dial, with their provenance.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EmergencyNumbers {
    /// As displayed.
    pub ambulance: String,
    /// As dialled — digits and `+ * #` only, ready for `ACTION_DIAL`.
    pub ambulance_dial: String,
    pub police: Option<String>,
    pub fire: Option<String>,
    pub country: Option<String>,
    pub country_name: Option<String>,
    pub provenance: NumberProvenance,
    /// False for `BuiltIn` and `GsmFallback`. When false the UI must caveat the number
    /// rather than present it as this city's ambulance line.
    pub confirmed_local: bool,
    /// True when 112 is worth offering as a second button.
    pub gsm_112_also_works: bool,
}

/// A country the built-in table knows, for the settings picker. Offline, so the user can
/// correct a wrong guess with no network.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CountryChoice {
    /// ISO 3166-1 alpha-2.
    pub code: String,
    pub name: String,
    pub ambulance: String,
}

// ---------------------------------------------------------------------------
// Explicit hospital confirmation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum HospitalContactChannel {
    Online,
    SmsIntent,
    Voice,
}

impl From<HospitalContactChannel> for CoreConfirmationChannel {
    fn from(value: HospitalContactChannel) -> Self {
        match value {
            HospitalContactChannel::Online => Self::Online,
            HospitalContactChannel::SmsIntent => Self::SmsIntent,
            HospitalContactChannel::Voice => Self::Voice,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum HospitalReply {
    Yes,
    No,
}

impl From<HospitalReply> for CoreHospitalReply {
    fn from(value: HospitalReply) -> Self {
        match value {
            HospitalReply::Yes => Self::Yes,
            HospitalReply::No => Self::No,
        }
    }
}

/// Who heard the answer the screen is about to describe.
///
/// Exposed so the UI cannot accidentally attribute a relay-matched message to a person. See
/// `prohori_core::confirmation::ReplySource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum HospitalReplySource {
    Operator,
    OnlineRelay,
}

impl From<CoreReplySource> for HospitalReplySource {
    fn from(value: CoreReplySource) -> Self {
        match value {
            CoreReplySource::Operator => Self::Operator,
            CoreReplySource::OnlineRelay => Self::OnlineRelay,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum HospitalConfirmationStatus {
    Draft,
    Awaiting,
    Confirmed,
    Declined,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HospitalConfirmationRequest {
    pub hospital_id: String,
    pub specialty: String,
    pub eta_minutes: u32,
    pub channel: HospitalContactChannel,
    pub created_at_epoch_millis: u64,
}

/// A complete UI projection of the single active hospital contact attempt.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HospitalConfirmationView {
    pub pack_id: String,
    pub case_id: String,
    pub hospital_id: String,
    pub hospital_name: String,
    pub destination: String,
    /// Normalised by the core (`cardiac`, not `Cardiac`), so the relay's audit line and the
    /// screen name the same thing the message names.
    pub specialty: String,
    pub eta_minutes: u32,
    pub channel: HospitalContactChannel,
    pub status: HospitalConfirmationStatus,
    /// True only for a terminal, explicit YES.
    pub explicit_ready: bool,
    pub sms_body: Option<String>,
    pub voice_script: Option<String>,
    /// The exact text the relay sends. Present only on the online channel.
    pub online_body: Option<String>,
    pub contacted_at_epoch_seconds: Option<u64>,
    pub replied_at_epoch_seconds: Option<u64>,
    pub expired_at_epoch_seconds: Option<u64>,
    pub recorded_by: Option<String>,
    /// Set with `recorded_by` on a terminal answer, so the screen can say how it was heard
    /// instead of implying a person was on the line.
    pub reply_source: Option<HospitalReplySource>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HospitalConfirmationResult {
    pub accepted: bool,
    pub error: Option<String>,
    pub confirmation: Option<HospitalConfirmationView>,
}

#[derive(Debug, Clone)]
struct ActiveHospitalConfirmation {
    pack_id: String,
    hospital_name: String,
    confirmation: CoreConfirmation,
}

fn hospital_confirmation_view(active: &ActiveHospitalConfirmation) -> HospitalConfirmationView {
    let (status, contacted_at, replied_at, expired_at, recorded_by, reply_source) =
        match &active.confirmation.state {
            CoreConfirmationState::Draft => (
                HospitalConfirmationStatus::Draft,
                None,
                None,
                None,
                None,
                None,
            ),
            CoreConfirmationState::Awaiting {
                sent_at_epoch_seconds,
            } => (
                HospitalConfirmationStatus::Awaiting,
                Some(*sent_at_epoch_seconds),
                None,
                None,
                None,
                None,
            ),
            CoreConfirmationState::Confirmed {
                replied_at_epoch_seconds,
                recorded_by,
                source,
            } => (
                HospitalConfirmationStatus::Confirmed,
                None,
                Some(*replied_at_epoch_seconds),
                None,
                Some(recorded_by.clone()),
                Some(HospitalReplySource::from(*source)),
            ),
            CoreConfirmationState::Declined {
                replied_at_epoch_seconds,
                recorded_by,
                source,
            } => (
                HospitalConfirmationStatus::Declined,
                None,
                Some(*replied_at_epoch_seconds),
                None,
                Some(recorded_by.clone()),
                Some(HospitalReplySource::from(*source)),
            ),
            CoreConfirmationState::Expired {
                expired_at_epoch_seconds,
            } => (
                HospitalConfirmationStatus::Expired,
                None,
                None,
                Some(*expired_at_epoch_seconds),
                None,
                None,
            ),
        };
    let channel = match active.confirmation.channel {
        CoreConfirmationChannel::Online => HospitalContactChannel::Online,
        CoreConfirmationChannel::SmsIntent => HospitalContactChannel::SmsIntent,
        CoreConfirmationChannel::Voice => HospitalContactChannel::Voice,
    };
    HospitalConfirmationView {
        pack_id: active.pack_id.clone(),
        case_id: active.confirmation.case_id.clone(),
        hospital_id: active.confirmation.hospital_id.clone(),
        hospital_name: active.hospital_name.clone(),
        destination: active.confirmation.destination.clone(),
        specialty: active.confirmation.specialty.clone(),
        eta_minutes: active.confirmation.eta_minutes,
        channel,
        status,
        explicit_ready: active.confirmation.hospital_is_ready(),
        sms_body: (channel == HospitalContactChannel::SmsIntent)
            .then(|| active.confirmation.sms_body()),
        voice_script: (channel == HospitalContactChannel::Voice)
            .then(|| active.confirmation.voice_script()),
        online_body: (channel == HospitalContactChannel::Online)
            .then(|| active.confirmation.online_body()),
        contacted_at_epoch_seconds: contacted_at,
        replied_at_epoch_seconds: replied_at,
        expired_at_epoch_seconds: expired_at,
        recorded_by,
        reply_source,
    }
}

fn hospital_confirmation_result(
    active: Option<&ActiveHospitalConfirmation>,
    accepted: bool,
    error: Option<String>,
) -> HospitalConfirmationResult {
    HospitalConfirmationResult {
        accepted,
        error,
        confirmation: active.map(hospital_confirmation_view),
    }
}

// ---------------------------------------------------------------------------
// Signed city packs and offline routing
// ---------------------------------------------------------------------------

/// One payload entry read from a city-pack archive by Kotlin.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CityPackFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// Result of attempting to replace the installed pack. A failed pack never replaces a
/// previously verified one.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CityPackInstall {
    pub accepted: bool,
    pub error: Option<String>,
    pub pack_id: Option<String>,
    pub city: Option<String>,
    pub version: Option<u32>,
    pub field_checked: bool,
}

/// Metadata the UI must show beside every route claim.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CityPackInfo {
    pub pack_id: String,
    pub city: String,
    pub country: String,
    pub version: u32,
    pub built_at_epoch_seconds: u64,
    pub attribution: String,
    pub field_checked: bool,
    pub hospital_count: u32,
    pub road_edge_count: u32,
}

/// Inputs that affect passability or hospital selection for one offline route request.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct OfflineRouteRequest {
    pub latitude: f64,
    pub longitude: f64,
    pub specialty: String,
    pub now_epoch_seconds: u64,
    pub vehicle_width_millimetres: u32,
    pub vehicle_height_millimetres: u32,
    pub permit_flooded_origin_zone: bool,
}

/// A route after signature, digest, schema, freshness, hazard, and vehicle checks.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OfflineRouteResult {
    pub accepted: bool,
    pub error: Option<String>,
    pub pack_id: Option<String>,
    pub field_checked: bool,
    pub hospital_id: Option<String>,
    pub hospital_name: Option<String>,
    pub hospital_hotline: Option<String>,
    pub hospital_sms: Option<String>,
    /// Present only when the verified pack binds a Telegram chat for this hospital, so the UI
    /// shows the online button exactly when there is somewhere for it to send.
    pub hospital_telegram: Option<String>,
    pub edge_ids: Vec<u32>,
    pub estimated_seconds: Option<u64>,
    pub condition_age_seconds: Option<u64>,
    pub condition_sources: Vec<String>,
    pub facility_age_seconds: Option<u64>,
    pub attribution: Option<String>,
}

fn offline_route_error(message: &str) -> OfflineRouteResult {
    OfflineRouteResult {
        accepted: false,
        error: Some(message.to_owned()),
        pack_id: None,
        field_checked: false,
        hospital_id: None,
        hospital_name: None,
        hospital_hotline: None,
        hospital_sms: None,
        hospital_telegram: None,
        edge_ids: Vec::new(),
        estimated_seconds: None,
        condition_age_seconds: None,
        condition_sources: Vec::new(),
        facility_age_seconds: None,
        attribution: None,
    }
}

// ---------------------------------------------------------------------------
// Verified renderings
// ---------------------------------------------------------------------------

/// A rendering that has been through the verifier.
///
/// There is no way to get an unverified string across this boundary. When P1 adds a
/// model, the Kotlin side hands its output to [`Prohori::verified_rendering`] and
/// displays `text`, whatever happened — on refusal `text` is the card's own words.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct VerifiedRendering {
    /// Safe to display. Either the rendering, or the protocol verbatim.
    pub text: String,
    /// True when the rendering was refused and `text` is the source card.
    pub fell_back: bool,
    /// Why it was refused, for the trace and the eval harness. Never shown to a user
    /// mid-emergency.
    pub violations: Vec<String>,
}

// ---------------------------------------------------------------------------
// On-device model contract
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ModelAgeBand {
    Infant,
    Child,
    Adult,
    OlderAdult,
    Unknown,
}

impl From<AgeBand> for ModelAgeBand {
    fn from(value: AgeBand) -> Self {
        match value {
            AgeBand::Infant => Self::Infant,
            AgeBand::Child => Self::Child,
            AgeBand::Adult => Self::Adult,
            AgeBand::OlderAdult => Self::OlderAdult,
            AgeBand::Unknown => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ModelSpecialty {
    GeneralEmergency,
    Cardiac,
    Neurology,
    Trauma,
    Toxicology,
    Respiratory,
    Burns,
    Unknown,
}

impl From<Specialty> for ModelSpecialty {
    fn from(value: Specialty) -> Self {
        match value {
            Specialty::GeneralEmergency => Self::GeneralEmergency,
            Specialty::Cardiac => Self::Cardiac,
            Specialty::Neurology => Self::Neurology,
            Specialty::Trauma => Self::Trauma,
            Specialty::Toxicology => Self::Toxicology,
            Specialty::Respiratory => Self::Respiratory,
            Specialty::Burns => Self::Burns,
            Specialty::Unknown => Self::Unknown,
        }
    }
}

/// Prompt and GBNF grammar handed to llama.cpp for one intake turn.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct InferenceContract {
    pub prompt: String,
    pub grammar: String,
}

/// A model result after Rust validation and rule-floor enforcement.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ModelAssessment {
    pub accepted: bool,
    pub error: Option<String>,
    pub severity: Option<Urgency>,
    pub card: Option<FirstAidCard>,
    pub age_band: ModelAgeBand,
    pub specialty: ModelSpecialty,
    pub symptoms: Vec<String>,
    pub needs_emergency_services: bool,
}

// ---------------------------------------------------------------------------
// Model-written guidance, for queries the corpus does not cover
// ---------------------------------------------------------------------------

/// Guidance the model on this phone wrote itself, after every check in
/// [`prohori_core::fallback`] passed.
///
/// Deliberately **not** a [`FirstAidCard`] and deliberately not convertible into one. It
/// carries no `protocol_id`, no `sources`, no `clinically_reviewed` flag and no
/// `plain_text`, because it has none of those things: nobody cited it and nobody reviewed
/// it. A type that *could* be handed to the card renderer by accident would eventually be
/// handed to it, and then the one screen in this app that must never look authoritative
/// would look exactly like the eighteen that are.
///
/// What it does carry is [`Self::disclaimer`], which the UI may not omit.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ModelWrittenGuidance {
    /// One sentence to a frightened person, before the instructions start.
    pub reassurance: String,
    /// What to do, in order. Never empty — an accepted answer with no steps is a refusal.
    pub steps: Vec<String>,
    /// What makes it worse. May be empty.
    pub do_not: Vec<String>,
    /// Always true, and a field rather than an assumption so the screen has something to
    /// bind to. `data/grammar/fallback.gbnf` makes `false` unrepresentable: nothing the
    /// model writes has the authority to tell somebody not to call for help.
    pub call_now: bool,
    /// One sentence saying who wrote this and what it therefore is not.
    ///
    /// Authored in Rust for the same reason [`FirstAidCard::provenance`] is: it has to
    /// appear on the screen and in anything shared out of the app, and two copies of a
    /// sentence about medical authority is one copy too many.
    pub disclaimer: String,
}

/// The outcome of offering one model-written answer to Rust.
///
/// Three distinguishable outcomes, because collapsing them would make the local trace
/// useless: the answer was accepted, the answer was thrown away
/// ([`Self::error`]), or the fallback should never have run at all
/// ([`Self::suppressed`]).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FallbackAssessment {
    pub accepted: bool,
    /// Why the model's answer was thrown away — a named `prohori_core::fallback::FallbackError`
    /// such as a digit, a drug name, or a reading grade above six. For the trace, not for
    /// the user.
    pub error: Option<String>,
    /// Why the fallback had no business running: a red-flag rule fired, or the corpus does
    /// cover this after all. Set when that became true *between* building the contract and
    /// offering the output — a race the deterministic layer wins.
    pub suppressed: Option<String>,
    pub guidance: Option<ModelWrittenGuidance>,
}

// ---------------------------------------------------------------------------
// The handle
// ---------------------------------------------------------------------------

/// The core, loaded. Construct one at app startup and keep it.
///
/// Holds the embedded corpus so a keystroke does not re-parse four JSON files. Immutable
/// after construction, which is what makes it safe to share across threads without the
/// Kotlin side thinking about locks.
#[derive(Debug, uniffi::Object)]
pub struct Prohori {
    corpus: Corpus,
    index: Index,
    /// Parsed once, like the corpus, so an unmatched keystroke does not re-parse a JSON
    /// file. `None` only in a build whose safety-net card failed validation, which
    /// [`Prohori::corpus_load_errors`] then names.
    safety_net: Option<Protocol>,
    load_errors: Vec<String>,
    city_pack: RwLock<Option<LoadedCityPack>>,
    hospital_confirmation: RwLock<Option<ActiveHospitalConfirmation>>,
}

#[uniffi::export]
impl Prohori {
    /// Load the corpus compiled into this library. Cannot fail; see the module docs.
    #[uniffi::constructor]
    #[must_use]
    pub fn new() -> Arc<Self> {
        let (corpus, errors) = prohori_core::bundled::corpus();
        let index = Index::build(&corpus);
        let mut load_errors: Vec<String> = errors.iter().map(ToString::to_string).collect();
        // A broken safety-net card surfaces the same way a broken corpus card does: named
        // on the About screen, not thrown at startup.
        let safety_net = match guidance::safety_net() {
            Ok(protocol) => Some(protocol),
            Err(problems) => {
                load_errors.extend(problems.iter().map(ToString::to_string));
                None
            }
        };
        Arc::new(Self {
            corpus,
            index,
            safety_net,
            load_errors,
            city_pack: RwLock::new(None),
            hospital_confirmation: RwLock::new(None),
        })
    }

    /// Run the red-flag rules over a message.
    ///
    /// No model, no network, no allocation the caller has to manage. This is the whole
    /// P0 decision path.
    #[must_use]
    pub fn triage(&self, message: String) -> Triage {
        let assessment = redflag::assess(&message);
        let severity = assessment.severity();
        Triage {
            severity: severity.map(Into::into),
            card: assessment
                .card()
                .and_then(|hit| hit.protocol_id)
                .and_then(|id| self.corpus.get(id))
                .map(FirstAidCard::from),
            hits: assessment.hits.iter().map(Into::into).collect(),
            recognised_without_guidance: assessment
                .unsupported()
                .into_iter()
                .map(Into::into)
                .collect(),
            bypasses_model: severity.is_some_and(Severity::bypasses_model),
        }
    }

    /// A card by id, for deep links and for the browsable list.
    ///
    /// The safety-net card is not reachable here. It has an id, but it is not a corpus
    /// card, and a deep link that resolved to it would put "we have no card for this" in a
    /// list of things this app does have a card for. See [`Prohori::safety_net_card`].
    #[must_use]
    pub fn card(&self, protocol_id: String) -> Option<FirstAidCard> {
        self.corpus.get(&protocol_id).map(FirstAidCard::from)
    }

    /// Every card in this build, in id order. The safety-net card is not one of them.
    #[must_use]
    pub fn cards(&self) -> Vec<FirstAidCard> {
        self.corpus.protocols().map(FirstAidCard::from).collect()
    }

    /// Find up to three cited cards using the deterministic offline BM25 index.
    ///
    /// Retrieval never changes triage or suppresses a red flag. Kotlin receives complete
    /// cards because it must not perform a second lookup and accidentally lose the
    /// citations or review status on the way to the screen.
    #[must_use]
    pub fn search(&self, query: String) -> Vec<SearchResult> {
        self.index
            .search(&query, 3)
            .into_iter()
            .filter_map(|hit| {
                self.corpus
                    .get(&hit.protocol_id)
                    .map(|protocol| SearchResult {
                        card: FirstAidCard::from(protocol),
                        matched: hit.matched,
                    })
            })
            .collect()
    }

    /// Build the exact prompt/grammar pair for llama.cpp.
    #[must_use]
    pub fn inference_contract(&self, message: String) -> InferenceContract {
        // A model prompt is not a place to retain an unbounded medical history. Limit by
        // Unicode scalar values so a cut cannot split UTF-8 and crash the JNI side.
        let report: String = message.chars().take(1_000).collect();
        InferenceContract {
            prompt: format!("{}\n\nUser report:\n{}", inference::SYSTEM_PROMPT, report),
            grammar: inference::TRIAGE_GBNF.to_owned(),
        }
    }

    /// Validate llama.cpp output and merge it with the deterministic rule result.
    ///
    /// An invalid model result never creates an exception or a blank emergency screen.
    /// The return carries the rule card and severity, when present, and names the model
    /// failure for the local trace.
    #[must_use]
    pub fn accept_model_output(&self, message: String, json: String) -> ModelAssessment {
        let rules = redflag::assess(&message);
        let floor = rules.severity();
        let rule_card = rules
            .card()
            .and_then(|hit| hit.protocol_id)
            .and_then(|id| self.corpus.get(id));

        match inference::validate_slots(&json, &self.corpus, floor) {
            Ok(mut slots) => {
                slots.retain_grounded_symptoms(&message);
                // A deterministic red-flag card outranks a model-selected card. Retrieval
                // and the model can add context; neither can replace the safety floor.
                let card = rule_card.or_else(|| slots.protocol(&self.corpus));
                ModelAssessment {
                    accepted: true,
                    error: None,
                    severity: Some(slots.severity.into()),
                    card: card.map(FirstAidCard::from),
                    age_band: slots.age_band.into(),
                    specialty: slots.specialty.into(),
                    symptoms: slots.symptoms,
                    needs_emergency_services: slots.needs_emergency_services,
                }
            }
            Err(error) => ModelAssessment {
                accepted: false,
                error: Some(error.to_string()),
                severity: floor.map(Into::into),
                card: rule_card.map(FirstAidCard::from),
                age_band: ModelAgeBand::Unknown,
                specialty: ModelSpecialty::Unknown,
                symptoms: Vec::new(),
                needs_emergency_services: floor.is_some_and(Severity::bypasses_model),
            },
        }
    }

    /// Whether the model may write guidance of its own for this message.
    ///
    /// True only when the red-flag table found nothing, no complete lay phrase declared by
    /// a corpus card matched, and there is enough text to be a report rather than a
    /// half-typed word. `Index::template_search(..).is_empty()` is structural and never a
    /// score threshold; loose BM25 suggestions remain useful for browsing but cannot claim
    /// that the template covers the situation.
    ///
    /// The Kotlin side calls this before spending two to six seconds of CPU on a decode.
    /// It is not the enforcement point: [`Prohori::accept_fallback_output`] asks again.
    #[must_use]
    pub fn fallback_permitted(&self, message: String) -> bool {
        let rules = redflag::assess(&message);
        let hits = self.index.template_search(&message, 3);
        fallback::permitted(&message, &rules, &hits)
    }

    /// Why the fallback will not run, or `None` when it will.
    ///
    /// Exists so a trace can say which layer answered. A screen that shows nothing and
    /// explains nothing is a screen someone has to guess about later.
    #[must_use]
    pub fn fallback_suppression(&self, message: String) -> Option<String> {
        let rules = redflag::assess(&message);
        let hits = self.index.template_search(&message, 3);
        fallback::permission(&message, &rules, &hits).reason()
    }

    /// Build the prompt/grammar pair for a query the corpus does not cover.
    ///
    /// Reuses [`InferenceContract`] because the JNI signature is already
    /// `generate(modelPath, prompt, grammar)` and a second record with the same two fields
    /// would only invite the two paths to drift. The grammar is the one whose character
    /// class has no digits in it.
    ///
    /// Does **not** check [`Prohori::fallback_permitted`]. Building a contract is harmless;
    /// accepting output is not, and that is where the check is enforced.
    #[must_use]
    pub fn fallback_contract(&self, message: String) -> InferenceContract {
        // Same cap and the same reason as `inference_contract`: bounded by Unicode scalar
        // values so a cut cannot split UTF-8 on the way into the JNI.
        let report: String = message.chars().take(1_000).collect();
        InferenceContract {
            prompt: format!(
                "{}\n\nUser report:\n{}",
                fallback::FALLBACK_SYSTEM_PROMPT,
                report
            ),
            grammar: fallback::FALLBACK_GBNF.to_owned(),
        }
    }

    /// Check one model-written answer and return it only if it survives every check.
    ///
    /// Re-runs the red-flag rules and the index first, so this cannot be bypassed by a
    /// caller that skipped [`Prohori::fallback_permitted`] or by a message that became an
    /// emergency while the model was still decoding. A rule that fires during a decode
    /// discards the answer: `docs/CONVENTIONS.md` §10 and the rule floor in
    /// [`Prohori::accept_model_output`] are the same principle applied twice.
    ///
    /// Never returns an error and never panics on garbage. Unparseable output is a refusal
    /// with a reason, which the UI renders as nothing at all beneath a card that is already
    /// on the screen.
    #[must_use]
    pub fn accept_fallback_output(&self, message: String, json: String) -> FallbackAssessment {
        let rules = redflag::assess(&message);
        let hits = self.index.template_search(&message, 3);
        let permission = fallback::permission(&message, &rules, &hits);
        if !permission.is_allowed() {
            return FallbackAssessment {
                accepted: false,
                error: None,
                // `Permission::reason` is `Some` for every variant except `Allowed`, which
                // this branch has already excluded. The default is here so a future
                // variant cannot turn a suppression into a silent one.
                suppressed: Some(
                    permission
                        .reason()
                        .unwrap_or_else(|| "the deterministic layer answered".to_owned()),
                ),
                guidance: None,
            };
        }

        match fallback::validate(&json) {
            Ok(written) => FallbackAssessment {
                accepted: true,
                error: None,
                suppressed: None,
                guidance: Some(ModelWrittenGuidance {
                    reassurance: written.reassurance,
                    steps: written.steps,
                    do_not: written.do_not,
                    // Not read back from the model's JSON: the grammar permits only the
                    // literal `true`, and a validator that echoed the field would be
                    // trusting the thing it is checking.
                    call_now: true,
                    disclaimer: fallback::DISCLAIMER.to_owned(),
                }),
            },
            Err(error) => FallbackAssessment {
                accepted: false,
                error: Some(error.to_string()),
                suppressed: None,
                guidance: None,
            },
        }
    }

    /// The cited card to show when the corpus has nothing: the general approach to a
    /// casualty.
    ///
    /// Deterministic, instant, and correct with no model on the device at all — which is
    /// what makes the strictness of [`Prohori::accept_fallback_output`] affordable. A
    /// refusal there costs a paragraph, never the whole screen.
    ///
    /// Absent from [`Prohori::cards`] and from [`Prohori::card`] by construction: it is not
    /// in the corpus, so the browse list stays an honest answer to "what does this app
    /// know". Asserted in both directions by test.
    #[must_use]
    pub fn safety_net_card(&self) -> Option<FirstAidCard> {
        self.safety_net.as_ref().map(FirstAidCard::from)
    }

    /// Empty in a healthy build. Non-empty means this build shipped a card that failed
    /// validation, and the About screen should say which one rather than leaving a hole
    /// where a card should be.
    #[must_use]
    pub fn corpus_load_errors(&self) -> Vec<String> {
        self.load_errors.clone()
    }

    /// Resolve the numbers to dial.
    ///
    /// `country` is ISO 3166-1 alpha-2 — from the SIM, the locale, or the user's own
    /// setting. `user_override` wins over everything. `pack_ambulance` is a city pack's
    /// number when one is installed. Always returns something dialable.
    #[must_use]
    pub fn install_city_pack(
        &self,
        envelope_json: Vec<u8>,
        files: Vec<CityPackFile>,
        public_key: Vec<u8>,
    ) -> CityPackInstall {
        let mut payloads = HashMap::new();
        for file in files {
            if payloads.insert(file.path.clone(), file.bytes).is_some() {
                return CityPackInstall {
                    accepted: false,
                    error: Some(format!("duplicate city-pack path {:?}", file.path)),
                    pack_id: None,
                    city: None,
                    version: None,
                    field_checked: false,
                };
            }
        }
        let pack = match LoadedCityPack::load(&envelope_json, &payloads, &public_key) {
            Ok(pack) => pack,
            Err(error) => {
                return CityPackInstall {
                    accepted: false,
                    error: Some(error.to_string()),
                    pack_id: None,
                    city: None,
                    version: None,
                    field_checked: false,
                };
            }
        };
        let result = CityPackInstall {
            accepted: true,
            error: None,
            pack_id: Some(pack.manifest.pack_id.clone()),
            city: Some(pack.manifest.city.clone()),
            version: Some(pack.manifest.version),
            field_checked: pack.manifest.field_checked,
        };
        let Ok(mut installed) = self.city_pack.write() else {
            return CityPackInstall {
                accepted: false,
                error: Some("city-pack state lock is unavailable".to_owned()),
                pack_id: None,
                city: None,
                version: None,
                field_checked: false,
            };
        };
        *installed = Some(pack);
        result
    }

    #[must_use]
    pub fn city_pack_info(&self) -> Option<CityPackInfo> {
        let installed = self.city_pack.read().ok()?;
        let pack = installed.as_ref()?;
        Some(CityPackInfo {
            pack_id: pack.manifest.pack_id.clone(),
            city: pack.manifest.city.clone(),
            country: pack.manifest.country.clone(),
            version: pack.manifest.version,
            built_at_epoch_seconds: pack.manifest.built_at_epoch_seconds,
            attribution: pack.manifest.attribution.clone(),
            field_checked: pack.manifest.field_checked,
            hospital_count: u32::try_from(pack.hospitals.len()).unwrap_or(u32::MAX),
            road_edge_count: u32::try_from(pack.graph.edges.len()).unwrap_or(u32::MAX),
        })
    }

    #[must_use]
    pub fn offline_route(&self, request: OfflineRouteRequest) -> OfflineRouteResult {
        let Ok(installed) = self.city_pack.read() else {
            return offline_route_error("city-pack state lock is unavailable");
        };
        let Some(pack) = installed.as_ref() else {
            return offline_route_error("no verified city pack is installed");
        };
        match pack.route_to_hospital(HospitalRouteRequest {
            latitude: request.latitude,
            longitude: request.longitude,
            specialty: &request.specialty,
            now_epoch_seconds: request.now_epoch_seconds,
            vehicle_width_millimetres: request.vehicle_width_millimetres,
            vehicle_height_millimetres: request.vehicle_height_millimetres,
            permit_flooded_origin_zone: request.permit_flooded_origin_zone,
        }) {
            Ok(route) => OfflineRouteResult {
                accepted: true,
                error: None,
                pack_id: Some(pack.manifest.pack_id.clone()),
                field_checked: pack.manifest.field_checked,
                hospital_id: Some(route.hospital.id),
                hospital_name: Some(route.hospital.name),
                hospital_hotline: Some(route.hospital.hotline),
                hospital_sms: route.hospital.sms_number,
                hospital_telegram: route.hospital.telegram_chat_id,
                edge_ids: route.route.edge_ids,
                estimated_seconds: Some(route.route.estimated_seconds),
                condition_age_seconds: Some(route.condition_age_seconds),
                condition_sources: route.condition_sources,
                facility_age_seconds: Some(route.facility_age_seconds),
                attribution: Some(pack.manifest.attribution.clone()),
            },
            Err(error) => OfflineRouteResult {
                accepted: false,
                error: Some(error.to_string()),
                pack_id: Some(pack.manifest.pack_id.clone()),
                field_checked: pack.manifest.field_checked,
                hospital_id: None,
                hospital_name: None,
                hospital_hotline: None,
                hospital_sms: None,
                hospital_telegram: None,
                edge_ids: Vec::new(),
                estimated_seconds: None,
                condition_age_seconds: None,
                condition_sources: Vec::new(),
                facility_age_seconds: None,
                attribution: Some(pack.manifest.attribution.clone()),
            },
        }
    }

    /// Start a contact attempt using an endpoint from the currently verified city pack.
    /// The caller supplies only the hospital ID; it cannot substitute an unsigned number.
    #[must_use]
    pub fn start_hospital_confirmation(
        &self,
        request: HospitalConfirmationRequest,
    ) -> HospitalConfirmationResult {
        let endpoint = {
            let Ok(installed) = self.city_pack.read() else {
                return hospital_confirmation_result(
                    None,
                    false,
                    Some("city-pack state lock is unavailable".to_owned()),
                );
            };
            let Some(pack) = installed.as_ref() else {
                return hospital_confirmation_result(
                    None,
                    false,
                    Some("no verified city pack is installed".to_owned()),
                );
            };
            let Some(hospital) = pack
                .hospitals
                .iter()
                .find(|hospital| hospital.id == request.hospital_id)
            else {
                return hospital_confirmation_result(
                    None,
                    false,
                    Some("hospital is not in the installed city pack".to_owned()),
                );
            };
            let destination = match request.channel {
                HospitalContactChannel::Online => hospital.telegram_chat_id.clone(),
                HospitalContactChannel::SmsIntent => hospital.sms_number.clone(),
                HospitalContactChannel::Voice => Some(hospital.hotline.clone()),
            };
            let Some(destination) = destination else {
                // Named per channel: "no registered endpoint" on the online path would send
                // an operator hunting for a phone number that was never the problem.
                return hospital_confirmation_result(
                    None,
                    false,
                    Some(
                        match request.channel {
                            HospitalContactChannel::Online => {
                                "this hospital has no registered Telegram chat"
                            }
                            HospitalContactChannel::SmsIntent => {
                                "this hospital has no registered SMS endpoint"
                            }
                            HospitalContactChannel::Voice => {
                                "this hospital has no registered hotline"
                            }
                        }
                        .to_owned(),
                    ),
                );
            };
            (
                pack.manifest.pack_id.clone(),
                hospital.name.clone(),
                destination,
            )
        };

        let Ok(mut active) = self.hospital_confirmation.write() else {
            return hospital_confirmation_result(
                None,
                false,
                Some("hospital-confirmation state lock is unavailable".to_owned()),
            );
        };
        if let Some(current) = active.as_ref()
            && matches!(
                current.confirmation.state,
                CoreConfirmationState::Awaiting { .. }
            )
        {
            return hospital_confirmation_result(
                Some(current),
                false,
                Some("finish or expire the current hospital request first".to_owned()),
            );
        }
        let confirmation = match CoreConfirmation::new(
            request.hospital_id,
            endpoint.2,
            request.specialty,
            request.eta_minutes,
            request.channel.into(),
            request.created_at_epoch_millis,
        ) {
            Ok(confirmation) => confirmation,
            Err(error) => {
                return hospital_confirmation_result(None, false, Some(error.to_string()));
            }
        };
        if let Some(current) = active.as_ref()
            && current.confirmation.case_id == confirmation.case_id
        {
            return hospital_confirmation_result(
                Some(current),
                false,
                Some("new hospital request would reuse the previous case ID".to_owned()),
            );
        }
        *active = Some(ActiveHospitalConfirmation {
            pack_id: endpoint.0,
            hospital_name: endpoint.1,
            confirmation,
        });
        hospital_confirmation_result(active.as_ref(), true, None)
    }

    #[must_use]
    pub fn hospital_confirmation(&self) -> Option<HospitalConfirmationView> {
        self.hospital_confirmation
            .read()
            .ok()?
            .as_ref()
            .map(hospital_confirmation_view)
    }

    /// The operator calls this only after they actually sent the SMS or asked the voice
    /// question. Opening another app is deliberately not treated as delivery.
    #[must_use]
    pub fn mark_hospital_contacted(&self, at_epoch_seconds: u64) -> HospitalConfirmationResult {
        let Ok(mut active) = self.hospital_confirmation.write() else {
            return hospital_confirmation_result(
                None,
                false,
                Some("hospital-confirmation state lock is unavailable".to_owned()),
            );
        };
        let Some(current) = active.as_mut() else {
            return hospital_confirmation_result(
                None,
                false,
                Some("no hospital request is active".to_owned()),
            );
        };
        let accepted = current.confirmation.mark_sent(at_epoch_seconds);
        hospital_confirmation_result(
            Some(current),
            accepted,
            (!accepted).then(|| "hospital request is not a draft".to_owned()),
        )
    }

    /// Record what a person on this device heard a hospital operator say.
    ///
    /// The relay must not reach this entry point — see [`Self::ingest_online_reply`]. Keeping
    /// them separate is what makes [`HospitalReplySource`] trustworthy: neither caller can
    /// choose the label it is recorded under.
    #[must_use]
    pub fn record_hospital_reply(
        &self,
        reply: HospitalReply,
        at_epoch_seconds: u64,
        recorded_by: String,
    ) -> HospitalConfirmationResult {
        let Ok(mut active) = self.hospital_confirmation.write() else {
            return hospital_confirmation_result(
                None,
                false,
                Some("hospital-confirmation state lock is unavailable".to_owned()),
            );
        };
        let Some(current) = active.as_mut() else {
            return hospital_confirmation_result(
                None,
                false,
                Some("no hospital request is active".to_owned()),
            );
        };
        let accepted = current.confirmation.record_reply(
            reply.into(),
            at_epoch_seconds,
            recorded_by,
            CoreReplySource::Operator,
        );
        hospital_confirmation_result(
            Some(current),
            accepted,
            (!accepted).then(|| {
                "reply requires an awaiting request, a valid time, and a named recorder".to_owned()
            }),
        )
    }

    /// Record a YES/NO the relay matched to a case, addressed by case id.
    ///
    /// `relay_case_id` must equal the active request's case id. Without that equality check a
    /// reply meant for one dispatch could confirm another — the failure mode that made a
    /// shared bot token unusable, since Telegram permits a single `getUpdates` consumer and
    /// the winner's committed offset consumes everyone else's answers. The check lives here,
    /// in Rust, where it is tested, rather than in the transport that happens to fetch it.
    ///
    /// A relay reply is refused on the SMS and voice channels by
    /// `prohori_core::confirmation::Confirmation::record_reply`: a server cannot have heard a
    /// phone call.
    #[must_use]
    pub fn ingest_online_reply(
        &self,
        reply: HospitalReply,
        at_epoch_seconds: u64,
        relay_case_id: String,
    ) -> HospitalConfirmationResult {
        let Ok(mut active) = self.hospital_confirmation.write() else {
            return hospital_confirmation_result(
                None,
                false,
                Some("hospital-confirmation state lock is unavailable".to_owned()),
            );
        };
        let Some(current) = active.as_mut() else {
            return hospital_confirmation_result(
                None,
                false,
                Some("no hospital request is active".to_owned()),
            );
        };
        if current.confirmation.case_id != relay_case_id.trim() {
            return hospital_confirmation_result(
                Some(current),
                false,
                Some("relay reply is for a different case".to_owned()),
            );
        }
        let accepted = current.confirmation.record_reply(
            reply.into(),
            at_epoch_seconds,
            "prohori relay".to_owned(),
            CoreReplySource::OnlineRelay,
        );
        hospital_confirmation_result(
            Some(current),
            accepted,
            (!accepted).then(|| {
                "relay reply requires an awaiting online request and a valid time".to_owned()
            }),
        )
    }

    #[must_use]
    pub fn expire_hospital_confirmation(
        &self,
        at_epoch_seconds: u64,
    ) -> HospitalConfirmationResult {
        let Ok(mut active) = self.hospital_confirmation.write() else {
            return hospital_confirmation_result(
                None,
                false,
                Some("hospital-confirmation state lock is unavailable".to_owned()),
            );
        };
        let Some(current) = active.as_mut() else {
            return hospital_confirmation_result(
                None,
                false,
                Some("no hospital request is active".to_owned()),
            );
        };
        let accepted = current.confirmation.expire(at_epoch_seconds);
        hospital_confirmation_result(
            Some(current),
            accepted,
            (!accepted).then(|| "only an awaiting request can expire".to_owned()),
        )
    }

    #[must_use]
    pub fn emergency_numbers(
        &self,
        country: Option<String>,
        user_override: Option<String>,
        pack_ambulance: Option<String>,
    ) -> EmergencyNumbers {
        let installed_pack = self.city_pack.read().ok().and_then(|installed| {
            installed
                .as_ref()
                .filter(|pack| pack.manifest.field_checked)
                .map(|pack| pack.emergency.clone())
        });
        let pack = installed_pack.or_else(|| {
            pack_ambulance.map(|ambulance| PackEmergency {
                country: country.clone().unwrap_or_default(),
                ambulance,
                police: None,
                fire: None,
            })
        });
        let resolved =
            emergency::resolve(user_override.as_deref(), pack.as_ref(), country.as_deref());
        EmergencyNumbers {
            ambulance: resolved.ambulance.clone(),
            ambulance_dial: resolved.dial_string(),
            police: resolved.police.clone(),
            fire: resolved.fire.clone(),
            country: resolved.country.clone(),
            country_name: resolved.country_name.clone(),
            provenance: resolved.source.into(),
            confirmed_local: resolved.source.is_confirmed_local(),
            gsm_112_also_works: resolved.gsm_112_also_works,
        }
    }

    /// Every country the built-in table knows, for the settings picker.
    #[must_use]
    pub fn known_countries(&self) -> Vec<CountryChoice> {
        emergency::COUNTRIES
            .iter()
            .map(|entry| CountryChoice {
                code: entry.country.to_owned(),
                name: entry.name.to_owned(),
                ambulance: entry.ambulance.to_owned(),
            })
            .collect()
    }

    /// Check a generated rendering against its source card, falling back to the card.
    ///
    /// Unused in P0 — there is no model yet — and exported now so the model can only
    /// ever arrive behind it. An unknown `protocol_id` returns a refusal with no text
    /// rather than passing the rendering through, because a rendering whose source
    /// cannot be found is a rendering nothing has checked.
    #[must_use]
    pub fn verified_rendering(&self, protocol_id: String, rendering: String) -> VerifiedRendering {
        let Some(protocol) = self.corpus.get(&protocol_id) else {
            return VerifiedRendering {
                text: String::new(),
                fell_back: true,
                violations: vec![format!("unknown protocol {protocol_id:?}")],
            };
        };
        let (text, violations) = verifier::rendering_or_source(protocol, &rendering);
        VerifiedRendering {
            fell_back: !violations.is_empty(),
            text,
            violations: violations.iter().map(ToString::to_string).collect(),
        }
    }
}

/// Version of the Rust core, for the About screen and for traces.
///
/// Also the cheapest possible proof that the `.so` in the APK is the one that was built:
/// if this string is wrong, nothing below it can be trusted either.
#[uniffi::export]
#[must_use]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use prohori_core::city_pack::SignedManifest;
    use sha2::{Digest, Sha256};

    fn decode_test_key(value: &str) -> Vec<u8> {
        value
            .trim()
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let text = std::str::from_utf8(pair).expect("test key is UTF-8");
                u8::from_str_radix(text, 16).expect("test key is hex")
            })
            .collect()
    }

    fn core_with_demo_pack() -> Arc<Prohori> {
        core_with_pack(None)
    }

    /// The committed demo pack, optionally with a Telegram chat bound to RMCH.
    ///
    /// `None` installs the shipped bytes and the shipped key verbatim, so this fixture keeps
    /// proving that the asset in the APK verifies. `Some(chat)` rewrites `hospitals.json`,
    /// recomputes that one digest, and re-signs the manifest with a throwaway key — because
    /// the shipped pack deliberately binds no online endpoint (`core/examples/build_p3_demo.rs`
    /// explains why inventing RMCH's chat id would ship a button that appears to work). A
    /// fixture may invent what an asset may not.
    fn core_with_pack(telegram_chat: Option<&str>) -> Arc<Prohori> {
        let core = Prohori::new();
        let root = "../../app/src/main/assets/city-pack/ruet-demo/";
        let mut files = vec![
            CityPackFile {
                path: "conditions.snap".to_owned(),
                bytes: include_bytes!(concat!(
                    "../../app/src/main/assets/city-pack/ruet-demo/",
                    "conditions.snap"
                ))
                .to_vec(),
            },
            CityPackFile {
                path: "emergency.json".to_owned(),
                bytes: include_bytes!(concat!(
                    "../../app/src/main/assets/city-pack/ruet-demo/",
                    "emergency.json"
                ))
                .to_vec(),
            },
            CityPackFile {
                path: "hospitals.json".to_owned(),
                bytes: include_bytes!(concat!(
                    "../../app/src/main/assets/city-pack/ruet-demo/",
                    "hospitals.json"
                ))
                .to_vec(),
            },
            CityPackFile {
                path: "roads.graph".to_owned(),
                bytes: include_bytes!(concat!(
                    "../../app/src/main/assets/city-pack/ruet-demo/",
                    "roads.graph"
                ))
                .to_vec(),
            },
            CityPackFile {
                path: "shelters.json".to_owned(),
                bytes: include_bytes!(concat!(
                    "../../app/src/main/assets/city-pack/ruet-demo/",
                    "shelters.json"
                ))
                .to_vec(),
            },
            CityPackFile {
                path: "zones.geojson".to_owned(),
                bytes: include_bytes!(concat!(
                    "../../app/src/main/assets/city-pack/ruet-demo/",
                    "zones.geojson"
                ))
                .to_vec(),
            },
        ];
        let mut envelope_json = include_bytes!(concat!(
            "../../app/src/main/assets/city-pack/ruet-demo/",
            "manifest.json"
        ))
        .to_vec();
        let mut key = decode_test_key(include_str!(concat!(
            "../../app/src/main/assets/city-pack/ruet-demo/",
            "verification-key.hex"
        )));
        if let Some(chat) = telegram_chat {
            let rewritten = hospitals_with_telegram(chat);
            let hospitals = files
                .iter_mut()
                .find(|file| file.path == "hospitals.json")
                .expect("the fixture list contains hospitals.json");
            let digest: [u8; 32] = Sha256::digest(&rewritten).into();
            hospitals.bytes = rewritten;

            let signed: SignedManifest =
                serde_json::from_slice(&envelope_json).expect("committed manifest parses");
            let mut manifest = signed.manifest;
            manifest
                .files
                .insert("hospitals.json".to_owned(), hex_encode(&digest));
            // The signature covers `serde_json::to_vec(&manifest)`, which is what
            // `city_pack::verify` recomputes. Signing anything else here would produce a
            // fixture that fails for a reason unrelated to what the test is about.
            let canonical = serde_json::to_vec(&manifest).expect("manifest re-serialises");
            let signing_key = SigningKey::from_bytes(&[7u8; 32]);
            let resigned = SignedManifest {
                signature: hex_encode(&signing_key.sign(&canonical).to_bytes()),
                manifest,
            };
            envelope_json = serde_json::to_vec(&resigned).expect("envelope re-serialises");
            key = signing_key.verifying_key().to_bytes().to_vec();
        }
        let install = core.install_city_pack(envelope_json, files, key);
        assert!(install.accepted, "{root}: {:?}", install.error);
        core
    }

    /// The committed `hospitals.json`, with a Telegram chat added to the one hospital.
    fn hospitals_with_telegram(chat: &str) -> Vec<u8> {
        let mut document: serde_json::Value = serde_json::from_slice(include_bytes!(concat!(
            "../../app/src/main/assets/city-pack/ruet-demo/",
            "hospitals.json"
        )))
        .expect("committed hospitals.json parses");
        let hospital = document
            .get_mut("hospitals")
            .and_then(|list| list.get_mut(0))
            .expect("the demo pack describes one hospital");
        hospital["telegram_chat_id"] = serde_json::Value::String(chat.to_owned());
        serde_json::to_vec(&document).expect("hospitals.json re-serialises")
    }

    /// Lowercase hex, for the same reason [`decode_test_key`] is hand-written: the `hex` crate
    /// is not a dependency of this crate and two loops do not justify making it one.
    fn hex_encode(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn voice_request(at: u64) -> HospitalConfirmationRequest {
        HospitalConfirmationRequest {
            hospital_id: "rmch".to_owned(),
            specialty: "general_emergency".to_owned(),
            eta_minutes: 13,
            channel: HospitalContactChannel::Voice,
            created_at_epoch_millis: at,
        }
    }

    #[test]
    fn confirmation_uses_only_the_signed_hospital_endpoint() {
        let core = core_with_demo_pack();
        let started = core.start_hospital_confirmation(voice_request(100));
        assert!(started.accepted, "{:?}", started.error);
        let confirmation = started.confirmation.expect("confirmation");
        assert_eq!(confirmation.destination, "+880721760254");
        assert_eq!(confirmation.status, HospitalConfirmationStatus::Draft);
        assert!(!confirmation.explicit_ready);
        assert!(confirmation.voice_script.is_some());

        let sms = core.start_hospital_confirmation(HospitalConfirmationRequest {
            channel: HospitalContactChannel::SmsIntent,
            ..voice_request(101)
        });
        assert!(!sms.accepted);
        assert!(sms.error.expect("reason").contains("no registered SMS"));

        // The shipped pack binds no Telegram chat either, and the refusal has to name the
        // channel the operator actually pressed.
        let online = core.start_hospital_confirmation(HospitalConfirmationRequest {
            channel: HospitalContactChannel::Online,
            ..voice_request(102)
        });
        assert!(!online.accepted);
        assert!(
            online
                .error
                .expect("reason")
                .contains("no registered Telegram chat")
        );
    }

    /// The online channel, end to end across the boundary, on a pack that binds a chat.
    ///
    /// The endpoint is only reachable because a signature stands behind it — the fixture
    /// re-signs rather than bypassing verification.
    #[test]
    fn a_signed_telegram_endpoint_reaches_a_draft_the_relay_can_send() {
        let core = core_with_pack(Some("-1001234567890"));
        let route = core.offline_route(OfflineRouteRequest {
            latitude: 24.3630,
            longitude: 88.6280,
            specialty: "general_emergency".to_owned(),
            // A minute after the committed snapshot, so the route is refused for a missing
            // endpoint if it is refused at all — never for staleness.
            now_epoch_seconds: 1_787_284_413,
            vehicle_width_millimetres: 2_100,
            vehicle_height_millimetres: 2_600,
            permit_flooded_origin_zone: false,
        });
        assert!(route.accepted, "{:?}", route.error);
        assert_eq!(
            route.hospital_telegram.as_deref(),
            Some("-1001234567890"),
            "the UI gates the Telegram button on this field"
        );

        let started = core.start_hospital_confirmation(HospitalConfirmationRequest {
            channel: HospitalContactChannel::Online,
            ..voice_request(200)
        });
        assert!(started.accepted, "{:?}", started.error);
        let draft = started.confirmation.expect("confirmation");
        assert_eq!(draft.destination, "-1001234567890");
        assert_eq!(draft.status, HospitalConfirmationStatus::Draft);
        assert!(!draft.explicit_ready, "a draft is not a readiness claim");

        // Exactly one rendering is offered per channel, so the screen cannot show the
        // operator a script for a call the app is not placing.
        let body = draft
            .online_body
            .expect("the online channel carries a body");
        assert!(body.contains(&draft.case_id));
        assert!(body.contains("Reply YES to confirm readiness"));
        assert!(draft.sms_body.is_none());
        assert!(draft.voice_script.is_none());
        assert!(draft.reply_source.is_none(), "nobody has answered yet");
    }

    /// The check whose absence made a shared bot token unusable.
    ///
    /// Telegram permits one `getUpdates` consumer, so N phones sharing a token means the
    /// winner commits the offset and delivers case A's YES to phone B. A relay makes that
    /// impossible by being the single consumer — and this assertion makes it impossible
    /// *here* too, so a future "simplification" back into the APK fails a test rather than
    /// confirming the wrong dispatch.
    #[test]
    fn a_relay_reply_for_another_case_never_confirms_this_one() {
        let core = core_with_pack(Some("@rmch_emergency"));
        assert!(
            core.start_hospital_confirmation(HospitalConfirmationRequest {
                channel: HospitalContactChannel::Online,
                ..voice_request(200)
            })
            .accepted
        );
        assert!(core.mark_hospital_contacted(210).accepted);

        let wrong = core.ingest_online_reply(HospitalReply::Yes, 211, "PRO-DEADBEEF".to_owned());
        assert!(!wrong.accepted);
        assert!(wrong.error.expect("reason").contains("different case"));
        let still_waiting = wrong.confirmation.expect("state");
        assert_eq!(still_waiting.status, HospitalConfirmationStatus::Awaiting);
        assert!(!still_waiting.explicit_ready);

        let case_id = still_waiting.case_id;
        let right = core.ingest_online_reply(HospitalReply::Yes, 212, case_id);
        assert!(right.accepted, "{:?}", right.error);
        let confirmed = right.confirmation.expect("state");
        assert_eq!(confirmed.status, HospitalConfirmationStatus::Confirmed);
        assert!(confirmed.explicit_ready);
        assert_eq!(
            confirmed.reply_source,
            Some(HospitalReplySource::OnlineRelay),
            "the screen must not describe a matched message as something a person heard"
        );
    }

    /// A relay cannot have overheard a phone call.
    #[test]
    fn a_relay_reply_cannot_answer_for_the_voice_channel() {
        let core = core_with_demo_pack();
        assert!(
            core.start_hospital_confirmation(voice_request(300))
                .accepted
        );
        assert!(core.mark_hospital_contacted(310).accepted);
        let case_id = core
            .hospital_confirmation()
            .expect("an active request")
            .case_id;

        let injected = core.ingest_online_reply(HospitalReply::Yes, 311, case_id);
        assert!(
            !injected.accepted,
            "a correct case id must not be enough to answer a channel the relay does not run"
        );
        let state = injected.confirmation.expect("state");
        assert_eq!(state.status, HospitalConfirmationStatus::Awaiting);
        assert!(!state.explicit_ready);

        // The operator who is actually on the call can still answer.
        let heard = core.record_hospital_reply(HospitalReply::Yes, 312, "operator".to_owned());
        assert!(heard.accepted, "{:?}", heard.error);
        assert_eq!(
            heard.confirmation.expect("state").reply_source,
            Some(HospitalReplySource::Operator)
        );
    }

    #[test]
    fn boundary_never_turns_contact_or_silence_into_readiness() {
        let core = core_with_demo_pack();
        assert!(
            core.start_hospital_confirmation(voice_request(100))
                .accepted
        );
        let contacted = core.mark_hospital_contacted(110);
        assert!(contacted.accepted);
        assert_eq!(
            contacted.confirmation.expect("state").status,
            HospitalConfirmationStatus::Awaiting
        );

        let overlapping = core.start_hospital_confirmation(voice_request(111));
        assert!(!overlapping.accepted);
        assert!(!overlapping.confirmation.expect("existing").explicit_ready);

        let expired = core.expire_hospital_confirmation(120);
        assert!(expired.accepted);
        assert_eq!(
            expired.confirmation.expect("state").status,
            HospitalConfirmationStatus::Expired
        );
        assert!(
            !core
                .record_hospital_reply(HospitalReply::Yes, 121, "operator".to_owned())
                .accepted
        );

        assert!(
            core.start_hospital_confirmation(voice_request(130))
                .accepted
        );
        assert!(core.mark_hospital_contacted(131).accepted);
        let yes = core.record_hospital_reply(HospitalReply::Yes, 132, "operator".to_owned());
        assert!(yes.accepted);
        let confirmation = yes.confirmation.expect("state");
        assert_eq!(confirmation.status, HospitalConfirmationStatus::Confirmed);
        assert!(confirmation.explicit_ready);
        assert!(
            !core
                .start_hospital_confirmation(voice_request(130))
                .accepted
        );
        assert!(
            !core
                .record_hospital_reply(HospitalReply::No, 133, "other".to_owned())
                .accepted
        );
    }

    /// The path a user takes in the worst case, across the boundary, with no model.
    #[test]
    fn a_red_flag_crosses_the_boundary_as_a_complete_card() {
        let core = Prohori::new();
        let triage = core.triage("my father is not breathing".to_owned());

        assert_eq!(triage.severity, Some(Urgency::Critical));
        assert!(triage.bypasses_model);
        let card = triage.card.expect("an active rule fired");
        assert_eq!(card.protocol_id, "cpr.adult");
        assert!(!card.steps.is_empty());
        assert!(
            !card.do_not.is_empty(),
            "warnings must survive the boundary"
        );
        assert!(!card.sources.is_empty(), "provenance must survive too");
        assert!(!card.plain_text.is_empty());
        assert!(
            !card.clinically_reviewed,
            "no card in this build is reviewed; the UI must be able to say so"
        );
    }

    /// Provenance is not a footnote the UI may choose to draw.
    ///
    /// Both fields are checked because they answer different questions.
    /// `clinically_reviewed` is what a screen branches on; `provenance` is what travels with
    /// the text once it has left the screen, and `plain_text` is the thing that leaves. A
    /// card shared into a group chat with its steps and without "no clinician has reviewed
    /// this" is the failure this test exists to prevent.
    #[test]
    fn a_card_cannot_cross_the_boundary_without_saying_where_it_came_from() {
        let core = Prohori::new();
        for card in core.cards() {
            assert!(
                !card.provenance.trim().is_empty(),
                "card {:?} crossed the boundary with no provenance sentence",
                card.protocol_id
            );
            assert!(
                card.plain_text.contains(&card.provenance),
                "card {:?} has a provenance sentence its shareable text drops",
                card.protocol_id
            );
            assert!(
                card.plain_text.contains("Sources:"),
                "card {:?} is shareable without its sources",
                card.protocol_id
            );
            for source in &card.sources {
                assert!(
                    card.plain_text.contains(source),
                    "card {:?} lists {source:?} on screen but not in its text",
                    card.protocol_id
                );
            }
            assert!(
                card.provenance.contains("No clinician has reviewed"),
                "card {:?} claims a review nobody has done: {:?}",
                card.protocol_id,
                card.provenance
            );
            assert!(!card.clinically_reviewed);
        }
    }

    /// The invariant the red-flag layer's accepted overtriage rests on, checked at the
    /// boundary as well as in the corpus: the first thing the UI can render is a look,
    /// not a push on someone's chest.
    #[test]
    fn the_first_step_the_ui_can_show_is_never_an_action() {
        let core = Prohori::new();
        for card in core.cards() {
            let first = card.steps.first().expect("cards have steps");
            assert_ne!(
                first.kind,
                StepAction::Action,
                "card {:?} opens with an action: {:?}",
                card.protocol_id,
                first.text
            );
        }
    }

    /// The boundary's version of "a recognised emergency never reaches an empty screen".
    ///
    /// Every rule in the shipped table has a card now, so `recognised_without_guidance`
    /// is empty on every real input — which is the point of the assertion, not a reason
    /// to delete it. What this pins is that the *disjunction* holds: for every trigger,
    /// either a card crossed the boundary or the UI was handed an explicit admission.
    /// The day someone adds a rule ahead of its card, the second branch starts carrying
    /// traffic, and it has to already work.
    #[test]
    fn a_rule_that_fires_never_reaches_the_ui_with_nothing_to_show() {
        let core = Prohori::new();
        for rule in prohori_core::redflag::RULES {
            for trigger in rule.triggers {
                let triage = core.triage((*trigger).to_owned());
                assert!(
                    triage.severity.is_some(),
                    "rule {} trigger {trigger:?} crossed the boundary with no severity",
                    rule.id
                );
                assert!(
                    triage.card.is_some() || !triage.recognised_without_guidance.is_empty(),
                    "rule {} trigger {trigger:?} gives the UI neither a card nor an \
                     admission: {triage:?}",
                    rule.id
                );
                assert!(
                    triage
                        .recognised_without_guidance
                        .iter()
                        .all(|hit| hit.guidance_pending),
                    "an admission must be flagged as pending: {triage:?}"
                );
            }
        }
    }

    /// The stroke phrasing the previous version of the test above used, now that the rule
    /// is active. Kept because it is what a caller types, and because it pins the
    /// promotion at the boundary rather than only in core.
    #[test]
    fn a_reported_stroke_reaches_the_ui_as_a_card() {
        let core = Prohori::new();
        let triage = core.triage("i think he is having a stroke".to_owned());
        assert_eq!(triage.severity, Some(Urgency::Critical));
        assert!(triage.bypasses_model);
        assert!(
            triage.recognised_without_guidance.is_empty(),
            "there is guidance for this now: {triage:?}"
        );
        let card = triage.card.expect("the stroke rule is active");
        assert_eq!(card.protocol_id, "stroke.suspected");
    }

    #[test]
    fn an_unrecognised_message_is_empty_and_not_reassuring() {
        let core = Prohori::new();
        let triage = core.triage("what are your opening hours".to_owned());
        assert_eq!(triage.severity, None);
        assert!(triage.card.is_none());
        assert!(triage.hits.is_empty());
        assert!(!triage.bypasses_model);
    }

    #[test]
    fn search_crosses_the_boundary_with_complete_cited_cards() {
        let core = Prohori::new();
        let results = core.search("burned hand with hot water".to_owned());
        let first = results.first().expect("the burn card is in the corpus");
        assert_eq!(first.card.protocol_id, "burn.thermal");
        assert!(!first.matched.is_empty());
        assert!(!first.card.steps.is_empty());
        assert!(!first.card.sources.is_empty());
        assert!(!first.card.provenance.is_empty());
    }

    #[test]
    fn search_does_not_turn_unrelated_text_into_medical_advice() {
        let core = Prohori::new();
        assert!(
            core.search("my phone will not charge".to_owned())
                .is_empty()
        );
    }

    #[test]
    fn invalid_model_output_falls_back_to_the_rule_card() {
        let core = Prohori::new();
        let result =
            core.accept_model_output("he is not breathing".to_owned(), "not json".to_owned());
        assert!(!result.accepted);
        assert_eq!(result.severity, Some(Urgency::Critical));
        assert_eq!(
            result.card.as_ref().map(|card| card.protocol_id.as_str()),
            Some("cpr.adult")
        );
        assert!(result.needs_emergency_services);
    }

    #[test]
    fn model_output_cannot_replace_a_rule_card_or_downgrade_it() {
        let core = Prohori::new();
        let json = "{\"schema_version\":\"1\",\"severity\":\"self_care\",\"protocol_id\":\"burn.thermal\",\"age_band\":\"adult\",\"specialty\":\"burns\",\"symptoms\":[\"not breathing\"],\"needs_emergency_services\":false}";
        let result = core.accept_model_output("he is not breathing".to_owned(), json.to_owned());
        assert!(result.accepted);
        assert_eq!(result.severity, Some(Urgency::Critical));
        assert_eq!(
            result.card.as_ref().map(|card| card.protocol_id.as_str()),
            Some("cpr.adult")
        );
    }

    #[test]
    fn the_corpus_in_this_build_is_clean() {
        assert!(
            Prohori::new().corpus_load_errors().is_empty(),
            "this build ships a card that failed validation"
        );
    }

    // -----------------------------------------------------------------------
    // Model-written guidance
    // -----------------------------------------------------------------------

    /// A real report of a real emergency that none of the eighteen cards covers and no
    /// red-flag rule names — a collapsed building, which is the disaster case this feature
    /// was asked for. One constant, so if the corpus ever grows to cover it the tests below
    /// fail together and in one place.
    const UNMATCHED: &str = "my neighbour is trapped under a concrete slab";

    /// The one message shape this whole feature exists for.
    #[test]
    fn a_query_the_corpus_cannot_answer_lets_the_model_write() {
        let core = Prohori::new();
        let message = UNMATCHED.to_owned();
        assert!(core.triage(message.clone()).hits.is_empty());
        assert!(
            core.search(message.clone()).is_empty(),
            "the corpus now covers this; pick another unmatched message"
        );
        assert!(core.fallback_permitted(message.clone()));
        assert!(core.fallback_suppression(message).is_none());
    }

    /// Browsing search is intentionally forgiving, so a shared word may offer a related card.
    /// That loose suggestion must not claim the report is covered or block model generation.
    #[test]
    fn a_loose_search_suggestion_does_not_suppress_an_uncovered_report() {
        let core = Prohori::new();
        let message = "he is very cold and cannot stop shivering".to_owned();
        assert!(
            !core.search(message.clone()).is_empty(),
            "keep a real loose-search collision in this boundary regression"
        );
        assert!(core.fallback_permitted(message.clone()));
        assert!(core.fallback_suppression(message).is_none());
    }

    #[test]
    fn a_red_flag_takes_the_answer_away_from_the_model() {
        let core = Prohori::new();
        let message = "he is not breathing".to_owned();
        assert!(!core.fallback_permitted(message.clone()));
        assert!(
            core.fallback_suppression(message)
                .is_some_and(|why| why.contains("rule")),
            "a suppression must name the layer that answered instead"
        );
    }

    #[test]
    fn a_corpus_match_takes_the_answer_away_from_the_model() {
        let core = Prohori::new();
        let message = "cool the burn with water".to_owned();
        assert!(!core.search(message.clone()).is_empty());
        assert!(!core.fallback_permitted(message.clone()));
        assert!(
            core.fallback_suppression(message)
                .is_some_and(|why| why.contains("burn.thermal"))
        );
    }

    /// The suppression is re-checked at acceptance, so a caller that skipped
    /// `fallback_permitted` — or a message that turned into an emergency mid-decode —
    /// still cannot get model-written text onto the screen.
    #[test]
    fn output_offered_for_a_red_flag_message_is_discarded_not_shown() {
        let core = Prohori::new();
        let json = r#"{"schema_version":"1","reassurance":"Stay with them.",
                       "steps":["Keep them warm."],"do_not":[],"call_now":true}"#;
        let result = core.accept_fallback_output("he is not breathing".to_owned(), json.to_owned());
        assert!(!result.accepted);
        assert!(result.guidance.is_none());
        assert!(result.suppressed.is_some(), "and it says why");
        assert!(result.error.is_none(), "the output was never the problem");
    }

    #[test]
    fn accepted_guidance_crosses_the_boundary_with_its_disclaimer_attached() {
        let core = Prohori::new();
        let json = r#"{"schema_version":"1",
                       "reassurance":"Stay with them. Help is coming.",
                       "steps":["Keep pressing on the cut until the bleeding stops.",
                                "Keep them warm and keep talking to them."],
                       "do_not":["Do not let anyone move them."],
                       "call_now":true}"#;
        let result = core.accept_fallback_output(UNMATCHED.to_owned(), json.to_owned());
        assert!(
            result.accepted,
            "error {:?}, suppressed {:?}",
            result.error, result.suppressed
        );
        let guidance = result.guidance.expect("accepted output carries guidance");
        assert_eq!(guidance.steps.len(), 2);
        assert!(
            guidance.call_now,
            "the grammar cannot express anything else"
        );
        assert!(
            !guidance.disclaimer.trim().is_empty(),
            "no screen may show this text without saying who wrote it"
        );
    }

    #[test]
    fn a_dose_in_model_output_never_reaches_the_boundary() {
        let core = Prohori::new();
        for bad in [
            r#"{"schema_version":"1","reassurance":"Stay calm.","steps":["Give 2 tablets."],"do_not":[],"call_now":true}"#,
            r#"{"schema_version":"1","reassurance":"Stay calm.","steps":["Give two spoons of medicine."],"do_not":[],"call_now":true}"#,
            r#"{"schema_version":"1","reassurance":"Stay calm.","steps":["Cut it open and suck it out."],"do_not":[],"call_now":true}"#,
            "not json at all",
        ] {
            let result = core.accept_fallback_output(UNMATCHED.to_owned(), bad.to_owned());
            assert!(!result.accepted, "{bad}");
            assert!(result.guidance.is_none(), "{bad}");
            assert!(result.error.is_some(), "a refusal must name itself: {bad}");
        }
    }

    #[test]
    fn the_fallback_grammar_cannot_express_a_digit() {
        let core = Prohori::new();
        let contract = core.fallback_contract(UNMATCHED.to_owned());
        let rule = contract
            .grammar
            .lines()
            .find(|line| line.trim_start().starts_with("char ::="))
            .expect("the grammar must define the character class it is named for");
        assert!(rule.contains("0-9"), "the exclusion is the point: {rule}");
        assert!(
            !rule.contains("\\u"),
            "an escape branch would reopen the hole the character class closes: {rule}"
        );
        assert!(contract.prompt.contains(UNMATCHED));
        assert_ne!(
            contract.grammar,
            core.inference_contract(UNMATCHED.to_owned()).grammar,
            "the two paths must not share a grammar"
        );
    }

    /// The safety net is what makes refusing affordable, so it must be there with no model
    /// installed and must never be mistaken for one of the eighteen reviewed cards.
    #[test]
    fn the_safety_net_card_is_cited_unreviewed_and_not_in_the_browse_list() {
        let core = Prohori::new();
        let card = core.safety_net_card().expect("this build ships one");
        assert!(!card.sources.is_empty(), "it has to say where it came from");
        assert!(!card.clinically_reviewed);
        assert!(card.reviewed_by.is_none());
        assert!(!card.provenance.trim().is_empty());
        assert!(!card.steps.is_empty());
        assert_ne!(
            card.steps.first().map(|step| step.kind),
            Some(StepAction::Action),
            "no card may open by telling someone to do something"
        );

        assert!(core.card(card.protocol_id.clone()).is_none());
        assert!(
            !core
                .cards()
                .iter()
                .any(|other| other.protocol_id == card.protocol_id),
            "the browse list must stay an honest answer to what the corpus knows"
        );
    }

    #[test]
    fn there_is_always_a_number_to_dial() {
        let core = Prohori::new();

        let known = core.emergency_numbers(Some("BD".to_owned()), None, None);
        assert_eq!(known.ambulance, "999");
        assert_eq!(known.provenance, NumberProvenance::BuiltIn);
        assert!(
            !known.confirmed_local,
            "a built-in table entry is not confirmation that this city answers it"
        );

        let nothing = core.emergency_numbers(None, None, None);
        assert_eq!(nothing.provenance, NumberProvenance::GsmFallback);
        assert!(!nothing.ambulance_dial.is_empty());

        let typed = core.emergency_numbers(Some("BD".to_owned()), Some("10921".to_owned()), None);
        assert_eq!(typed.ambulance, "10921");
        assert!(typed.confirmed_local);
    }

    #[test]
    fn a_dial_string_is_safe_to_hand_to_an_intent() {
        let core = Prohori::new();
        let numbers = core.emergency_numbers(Some("us".to_owned()), None, None);
        assert!(
            numbers
                .ambulance_dial
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, '+' | '*' | '#'))
        );
    }

    #[test]
    fn the_country_picker_is_not_empty_and_names_real_numbers() {
        let choices = Prohori::new().known_countries();
        assert!(choices.len() > 40);
        for choice in &choices {
            assert_eq!(choice.code.len(), 2);
            assert!(!choice.name.is_empty());
            assert!(!choice.ambulance.is_empty());
        }
    }

    #[test]
    fn a_bad_rendering_cannot_cross_the_boundary() {
        let core = Prohori::new();
        let result = core.verified_rendering(
            "cpr.adult".to_owned(),
            "Give 300 mg of aspirin, then press 15 cm deep.".to_owned(),
        );
        assert!(result.fell_back);
        assert!(!result.violations.is_empty());
        assert!(!result.text.contains("aspirin"));
        assert!(!result.text.contains("300"));
    }

    #[test]
    fn an_unknown_protocol_is_refused_rather_than_waved_through() {
        let core = Prohori::new();
        let result = core.verified_rendering("no.such.card".to_owned(), "Do this.".to_owned());
        assert!(result.fell_back);
        assert!(result.text.is_empty());
    }

    #[test]
    fn the_version_string_is_the_crate_version() {
        assert_eq!(core_version(), env!("CARGO_PKG_VERSION"));
    }
}
