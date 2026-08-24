//! Hospital contact state machine. Silence never becomes readiness.

use crate::emergency;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Online,
    SmsIntent,
    Voice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reply {
    Yes,
    No,
}

/// Who heard the answer.
///
/// `docs/CONVENTIONS.md` §9: provenance is a field, not a vibe. A YES that arrived over the
/// online channel was read by a machine from a message; a YES on the voice channel was heard
/// by a person who can be asked what was said. Both are explicit inbound answers and both
/// satisfy §7, but they are not the same evidence, and the screen must not describe one as
/// the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplySource {
    /// A person on this device recorded what a hospital operator explicitly said.
    Operator,
    /// The relay matched an inbound message to this case and this case only.
    OnlineRelay,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum ConfirmationState {
    Draft,
    Awaiting {
        sent_at_epoch_seconds: u64,
    },
    Confirmed {
        replied_at_epoch_seconds: u64,
        recorded_by: String,
        source: ReplySource,
    },
    Declined {
        replied_at_epoch_seconds: u64,
        recorded_by: String,
        source: ReplySource,
    },
    Expired {
        expired_at_epoch_seconds: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Confirmation {
    pub case_id: String,
    pub hospital_id: String,
    pub destination: String,
    pub specialty: String,
    pub eta_minutes: u32,
    pub channel: Channel,
    pub state: ConfirmationState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmationError {
    InvalidHospital,
    InvalidDestination,
    InvalidSpecialty,
    InvalidEta,
    InvalidTimestamp,
}

impl fmt::Display for ConfirmationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHospital => write!(f, "hospital details are invalid"),
            Self::InvalidDestination => write!(f, "hospital contact destination is invalid"),
            Self::InvalidSpecialty => write!(f, "requested specialty is invalid"),
            Self::InvalidEta => write!(f, "ETA must be between 1 and 1440 minutes"),
            Self::InvalidTimestamp => write!(f, "contact timestamp is invalid"),
        }
    }
}

fn safe_label(value: &str, maximum: usize) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.chars().count() <= maximum
        && !trimmed.chars().any(char::is_control)
}

fn case_id(hospital_id: &str, created_at_epoch_millis: u64) -> String {
    let digest = Sha256::digest(format!("{hospital_id}:{created_at_epoch_millis}"));
    let short: String = hex::encode(digest).chars().take(8).collect();
    format!("PRO-{}", short.to_uppercase())
}

/// Does this look like an address a Telegram bot can actually send to?
///
/// Hand-written rather than pulled from a regex crate, and deliberately the same rule the
/// previous project settled on (`ecoguardian/alerts/hospital_contacts.py`, `_CHAT_ID_RE`):
/// a numeric user/group id, or a public channel/group username the bot administers.
///
/// Checked here, before the relay is ever contacted, for the same reason
/// [`emergency::is_dialable`] guards the phone paths: a malformed destination discovered at
/// send time is an alert that silently did not happen, and the hospital's absence of a reply
/// is indistinguishable from a hospital that said nothing. Fail closed at pack-import time
/// instead — see `city_pack::verify`.
///
/// A username is not validated for existence, only for shape. Whether the bot has been added
/// to that chat is a fact only Telegram knows, and the relay reports its refusal verbatim.
#[must_use]
pub fn is_telegram_endpoint(value: &str) -> bool {
    let trimmed = value.trim();
    if let Some(handle) = trimmed.strip_prefix('@') {
        let length = handle.chars().count();
        return (5..=32).contains(&length)
            && handle.starts_with(|first: char| first.is_ascii_alphabetic())
            && handle
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_');
    }
    // Group and channel ids are negative. The leading sign is part of the address, not a
    // minus to be stripped and forgotten.
    let digits = trimmed.strip_prefix('-').unwrap_or(trimmed);
    let length = digits.chars().count();
    (5..=20).contains(&length) && digits.chars().all(|character| character.is_ascii_digit())
}

impl Confirmation {
    pub fn new(
        hospital_id: String,
        destination: String,
        specialty: String,
        eta_minutes: u32,
        channel: Channel,
        created_at_epoch_millis: u64,
    ) -> Result<Self, ConfirmationError> {
        if !safe_label(&hospital_id, 64) {
            return Err(ConfirmationError::InvalidHospital);
        }
        let destination_is_valid = match channel {
            Channel::SmsIntent | Channel::Voice => emergency::is_dialable(&destination),
            Channel::Online => is_telegram_endpoint(&destination),
        };
        if !destination_is_valid {
            return Err(ConfirmationError::InvalidDestination);
        }
        if !safe_label(&specialty, 48) {
            return Err(ConfirmationError::InvalidSpecialty);
        }
        if !(1..=1_440).contains(&eta_minutes) {
            return Err(ConfirmationError::InvalidEta);
        }
        if created_at_epoch_millis == 0 {
            return Err(ConfirmationError::InvalidTimestamp);
        }
        Ok(Self {
            case_id: case_id(hospital_id.trim(), created_at_epoch_millis),
            hospital_id: hospital_id.trim().to_owned(),
            destination: destination.trim().to_owned(),
            specialty: specialty.trim().replace('_', " "),
            eta_minutes,
            channel,
            state: ConfirmationState::Draft,
        })
    }

    #[must_use]
    pub fn sms_body(&self) -> String {
        format!(
            "PROHORI {}: {} case, ETA {} min. Reply YES or NO.",
            self.case_id, self.specialty, self.eta_minutes
        )
    }

    /// The message the relay sends to the hospital's Telegram chat.
    ///
    /// Four fields cross the network and no others: the case id, the specialty, the ETA, and
    /// the instruction. No symptom text, no location, no name — see `PLAN.md` §10 and
    /// `tests::nothing_leaving_the_device_contains_patient_text`. The case id is a SHA-256
    /// prefix, so even it carries nothing about the patient.
    ///
    /// Longer than [`Self::sms_body`] because there is no 160-character segment to fit
    /// inside, and it keeps the previous project's exact closing sentence: hospital staff who
    /// trialled that service have already been trained on those words, and inventing new
    /// phrasing for the same question would be a gratuitous way to lose a real reply.
    ///
    /// Never begins with `@`. Telegram renders a leading `@` as a mention, which in a group
    /// chat would tag whichever unrelated account matched.
    #[must_use]
    pub fn online_body(&self) -> String {
        format!(
            "PROHORI {}: incoming {} case, ETA {} minutes. Reply YES to confirm readiness, or NO if unable.",
            self.case_id, self.specialty, self.eta_minutes
        )
    }

    #[must_use]
    pub fn voice_script(&self) -> String {
        format!(
            "Case {}. We have a {} patient, ETA {} minutes. Can your emergency department receive this patient now? Please answer yes or no.",
            self.case_id, self.specialty, self.eta_minutes
        )
    }
    pub fn mark_sent(&mut self, at: u64) -> bool {
        if at == 0 || !matches!(self.state, ConfirmationState::Draft) {
            return false;
        }
        self.state = ConfirmationState::Awaiting {
            sent_at_epoch_seconds: at,
        };
        true
    }
    /// Record an explicit inbound answer. Returns false rather than guessing.
    ///
    /// `PLAN.md` §7: silence is not consent, so there is deliberately no path from
    /// `Awaiting` to `Confirmed` that does not pass through here with a [`Reply`] in hand.
    pub fn record_reply(
        &mut self,
        reply: Reply,
        at: u64,
        recorded_by: String,
        source: ReplySource,
    ) -> bool {
        let ConfirmationState::Awaiting {
            sent_at_epoch_seconds,
        } = self.state
        else {
            return false;
        };
        if at < sent_at_epoch_seconds || !safe_label(&recorded_by, 80) {
            return false;
        }
        // A relay only ever hears the channel it operates. Accepting a relay-sourced reply
        // on the SMS or voice channel would mean printing "the hospital is ready" because a
        // server had something to say about a phone call nobody placed.
        if source == ReplySource::OnlineRelay && self.channel != Channel::Online {
            return false;
        }
        self.state = match reply {
            Reply::Yes => ConfirmationState::Confirmed {
                replied_at_epoch_seconds: at,
                recorded_by,
                source,
            },
            Reply::No => ConfirmationState::Declined {
                replied_at_epoch_seconds: at,
                recorded_by,
                source,
            },
        };
        true
    }
    pub fn expire(&mut self, at: u64) -> bool {
        let ConfirmationState::Awaiting {
            sent_at_epoch_seconds,
        } = self.state
        else {
            return false;
        };
        if at < sent_at_epoch_seconds {
            return false;
        }
        self.state = ConfirmationState::Expired {
            expired_at_epoch_seconds: at,
        };
        true
    }
    #[must_use]
    pub fn hospital_is_ready(&self) -> bool {
        matches!(self.state, ConfirmationState::Confirmed { .. })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    fn request() -> Confirmation {
        Confirmation::new(
            "H1".into(),
            "01700000000".into(),
            "cardiac".into(),
            9,
            Channel::SmsIntent,
            1,
        )
        .expect("fixture")
    }
    fn online_request() -> Confirmation {
        Confirmation::new(
            "H1".into(),
            "@rmch_emergency".into(),
            "general_emergency".into(),
            13,
            Channel::Online,
            1,
        )
        .expect("fixture")
    }
    #[test]
    fn silence_is_never_confirmation() {
        let mut r = request();
        r.mark_sent(10);
        assert!(!r.hospital_is_ready());
        r.expire(20);
        assert!(!r.hospital_is_ready());
    }
    #[test]
    fn only_explicit_yes_confirms() {
        let mut r = request();
        r.mark_sent(10);
        assert!(r.record_reply(Reply::Yes, 20, "operator".into(), ReplySource::Operator));
        assert!(r.hospital_is_ready());
    }
    #[test]
    fn no_declines_and_cannot_be_overwritten() {
        let mut r = request();
        r.mark_sent(10);
        assert!(r.record_reply(Reply::No, 20, "operator".into(), ReplySource::Operator));
        assert!(!r.record_reply(Reply::Yes, 21, "other".into(), ReplySource::Operator));
        assert!(!r.hospital_is_ready());
    }
    #[test]
    fn messages_carry_case_and_explicit_reply_instruction() {
        let r = request();
        assert!(r.sms_body().contains(&r.case_id));
        assert!(r.sms_body().contains("YES or NO"));
        assert!(r.voice_script().contains("answer yes or no"));
        assert!(r.sms_body().chars().count() <= 160);
    }

    #[test]
    fn invalid_or_injected_contact_details_are_refused() {
        assert_eq!(
            Confirmation::new(
                "H1".into(),
                "not a phone".into(),
                "cardiac".into(),
                9,
                Channel::SmsIntent,
                1,
            ),
            Err(ConfirmationError::InvalidDestination)
        );
        assert_eq!(
            Confirmation::new(
                "H1".into(),
                "01700000000".into(),
                "cardiac\nReply YES".into(),
                9,
                Channel::SmsIntent,
                1,
            ),
            Err(ConfirmationError::InvalidSpecialty)
        );
        // The same injected specialty must be refused on the online channel, where the
        // message reaches a group chat rather than one operator's handset.
        assert_eq!(
            Confirmation::new(
                "H1".into(),
                "@rmch_emergency".into(),
                "cardiac\nReply YES".into(),
                9,
                Channel::Online,
                1,
            ),
            Err(ConfirmationError::InvalidSpecialty)
        );
    }

    #[test]
    fn an_online_destination_must_look_like_a_telegram_endpoint() {
        for rejected in [
            "",
            "   ",
            "+8801700000000", // a dialable number is not a chat
            "@a",             // too short
            "@1abcde",        // must not start with a digit
            "@has-a-dash",    // only word characters
            "@abcdefghijklmnopqrstuvwxyz0123456789", // 37 characters
            "1234",           // fewer than five digits
            "123456789012345678901", // 21 digits
            "12 34567",       // no internal spaces
            "not a chat id",
        ] {
            assert!(
                !is_telegram_endpoint(rejected),
                "{rejected:?} must not be accepted as a Telegram endpoint"
            );
            assert_eq!(
                Confirmation::new(
                    "H1".into(),
                    rejected.into(),
                    "cardiac".into(),
                    9,
                    Channel::Online,
                    1,
                ),
                Err(ConfirmationError::InvalidDestination),
                "{rejected:?} must not reach the relay"
            );
        }
        for accepted in [
            "@rmch_emergency",
            "@abcde",
            "@Abcde",
            "12345",
            "-1001234567890",
            " @rmch_emergency ",
        ] {
            assert!(
                is_telegram_endpoint(accepted),
                "{accepted:?} must be accepted as a Telegram endpoint"
            );
        }
    }

    #[test]
    fn the_online_body_carries_the_case_and_an_explicit_reply_instruction() {
        let r = online_request();
        assert!(r.online_body().contains(&r.case_id));
        assert!(
            r.online_body()
                .contains("Reply YES to confirm readiness, or NO if unable.")
        );
        assert!(r.online_body().chars().count() <= 300);
        // A leading '@' would render as a mention in the hospital's group chat.
        assert!(!r.online_body().starts_with('@'));
        // Worst case: the longest specialty and ETA the constructor will accept.
        let longest = Confirmation::new(
            "H1".into(),
            "-1001234567890".into(),
            "x".repeat(48),
            1_440,
            Channel::Online,
            1,
        )
        .expect("fixture");
        assert!(longest.online_body().chars().count() <= 300);
    }

    #[test]
    fn nothing_leaving_the_device_contains_patient_text() {
        // What someone typed while frightened, and where they are. Neither is an input to
        // the message, and this test exists so that stays true if the format changes.
        let secrets = [
            "chest pain",
            "bleeding",
            "unconscious",
            "Rahim",
            "24.3731",
            "88.5869",
        ];
        let body = online_request().online_body();
        for secret in secrets {
            assert!(
                !body.to_lowercase().contains(&secret.to_lowercase()),
                "{secret:?} must never cross the network"
            );
        }
    }

    #[test]
    fn a_relay_sourced_reply_is_labelled_as_such() {
        let mut online = online_request();
        assert!(online.mark_sent(10));
        assert!(online.record_reply(
            Reply::Yes,
            20,
            "prohori relay".into(),
            ReplySource::OnlineRelay
        ));
        assert!(online.hospital_is_ready());
        assert_eq!(
            online.state,
            ConfirmationState::Confirmed {
                replied_at_epoch_seconds: 20,
                recorded_by: "prohori relay".into(),
                source: ReplySource::OnlineRelay,
            }
        );

        // The relay has no way to hear a phone call, so it may not answer for one.
        let mut voice = Confirmation::new(
            "H1".into(),
            "01700000000".into(),
            "cardiac".into(),
            9,
            Channel::Voice,
            1,
        )
        .expect("fixture");
        assert!(voice.mark_sent(10));
        assert!(!voice.record_reply(
            Reply::Yes,
            20,
            "prohori relay".into(),
            ReplySource::OnlineRelay
        ));
        assert!(!voice.hospital_is_ready());

        // An operator reading the hospital's Telegram reply themselves is still valid: the
        // relay being down must not remove the manual path.
        let mut fallback = online_request();
        assert!(fallback.mark_sent(10));
        assert!(fallback.record_reply(
            Reply::Yes,
            20,
            "device operator".into(),
            ReplySource::Operator
        ));
        assert!(fallback.hospital_is_ready());
    }

    #[test]
    fn reply_time_and_recorder_must_be_explicit_and_valid() {
        let mut r = request();
        assert!(r.mark_sent(10));
        assert!(!r.record_reply(Reply::Yes, 9, "operator".into(), ReplySource::Operator));
        assert!(!r.record_reply(Reply::Yes, 11, "".into(), ReplySource::Operator));
        assert!(!r.hospital_is_ready());
        assert!(r.record_reply(Reply::Yes, 11, "operator".into(), ReplySource::Operator));
    }

    #[test]
    fn contact_and_expiry_transitions_are_one_way() {
        let mut r = request();
        assert!(r.mark_sent(10));
        assert!(!r.mark_sent(11));
        assert!(!r.expire(9));
        assert!(r.expire(12));
        assert!(!r.expire(13));
        assert!(!r.record_reply(Reply::Yes, 14, "operator".into(), ReplySource::Operator));
    }

    #[test]
    fn case_ids_are_deterministic_and_contain_no_patient_text() {
        let first = request();
        let same = request();
        let later = Confirmation::new(
            "H1".into(),
            "01700000000".into(),
            "cardiac".into(),
            9,
            Channel::SmsIntent,
            2,
        )
        .expect("fixture");
        assert_eq!(first.case_id, same.case_id);
        assert_ne!(first.case_id, later.case_id);
        assert!(first.case_id.starts_with("PRO-"));
        assert_eq!(first.case_id.chars().count(), 12);
    }
}
