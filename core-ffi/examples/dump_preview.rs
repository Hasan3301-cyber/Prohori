//! Throwaway: dump what the FFI actually returns, as JSON, so a preview can render the
//! real strings instead of hand-written approximations. Not part of the product.
//!
//! ```text
//! cargo run --locked -p prohori-ffi --example dump_preview > .preview/dump.json
//! ```
//!
//! Errors come back as a message and a non-zero exit, the way every other example in this
//! workspace does it. There is no `expect` here for the same reason there is none in
//! `core/`: a tool that panics tells you a line number, and a tool that returns a message
//! tells you what it could not read.

use ed25519_dalek::{Signer, SigningKey};
use prohori_core::city_pack::SignedManifest;
use prohori_ffi::{
    CityPackFile, HospitalConfirmationRequest, HospitalConfirmationView, HospitalContactChannel,
    HospitalReply, OfflineRouteRequest, OfflineRouteResult, Prohori,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::process::ExitCode;
use std::sync::Arc;

const ASSETS: &str = "../app/src/main/assets/city-pack/ruet-demo";
const PAYLOADS: [&str; 6] = [
    "conditions.snap",
    "emergency.json",
    "hospitals.json",
    "roads.graph",
    "shelters.json",
    "zones.geojson",
];
/// The chat id `core-ffi`'s own online-channel tests use. Not a real chat.
const DRILL_CHAT: &str = "-1001234567890";

fn read(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(format!("{ASSETS}/{path}")).map_err(|error| format!("{path}: {error}"))
}

fn card_json(card: &prohori_ffi::FirstAidCard) -> Value {
    json!({
        "protocol_id": card.protocol_id,
        "title": card.title,
        "applies_to": card.applies_to,
        "steps": card.steps.iter().map(|step| json!({
            "number": step.number,
            "kind": format!("{:?}", step.kind),
            "text": step.text,
        })).collect::<Vec<_>>(),
        "do_not": card.do_not,
        "escalate_if": card.escalate_if,
        "sources": card.sources,
        "clinically_reviewed": card.clinically_reviewed,
        "reviewed_by": card.reviewed_by,
        "provenance": card.provenance,
        "plain_text": card.plain_text,
    })
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(text: &str) -> Vec<u8> {
    text.trim()
        .as_bytes()
        .chunks(2)
        .filter_map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|byte| u8::from_str_radix(byte, 16).ok())
        })
        .collect()
}

/// Install the committed pack. With `telegram`, add a chat to the one hospital and re-sign
/// exactly the way `core-ffi`'s tests do — the endpoint is inside the signature, so a
/// hand-edited pack would be refused rather than routed.
fn core_with_pack(telegram: Option<&str>) -> Result<Arc<Prohori>, String> {
    let core = Prohori::new();
    let mut files: Vec<CityPackFile> = PAYLOADS
        .iter()
        .map(|path| {
            Ok(CityPackFile {
                path: (*path).to_owned(),
                bytes: read(path)?,
            })
        })
        .collect::<Result<_, String>>()?;
    let mut envelope = read("manifest.json")?;
    let mut key = decode_hex(
        &String::from_utf8(read("verification-key.hex")?)
            .map_err(|error| format!("verification-key.hex is not text: {error}"))?,
    );

    if let Some(chat) = telegram {
        let mut document: Value = serde_json::from_slice(&read("hospitals.json")?)
            .map_err(|error| format!("hospitals.json does not parse: {error}"))?;
        *document
            .get_mut("hospitals")
            .and_then(|hospitals| hospitals.get_mut(0))
            .and_then(|hospital| hospital.get_mut("telegram_chat_id"))
            .ok_or_else(|| "hospitals.json has no first hospital to bind a chat to".to_owned())? =
            Value::String(chat.to_owned());
        let rewritten = serde_json::to_vec(&document)
            .map_err(|error| format!("hospitals.json does not re-serialise: {error}"))?;
        let digest: [u8; 32] = Sha256::digest(&rewritten).into();
        for file in &mut files {
            if file.path == "hospitals.json" {
                file.bytes = rewritten.clone();
            }
        }
        // Typed, not `Value`: `city_pack::verify` recomputes the signed bytes with
        // `serde_json::to_vec(&manifest)` over the struct, and a `Value` round-trip would
        // reorder keys and produce a fixture that fails for the wrong reason.
        let signed: SignedManifest = serde_json::from_slice(&envelope)
            .map_err(|error| format!("manifest.json does not parse: {error}"))?;
        let mut manifest = signed.manifest;
        manifest
            .files
            .insert("hospitals.json".to_owned(), hex_encode(&digest));
        let canonical = serde_json::to_vec(&manifest)
            .map_err(|error| format!("manifest does not re-serialise: {error}"))?;
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        envelope = serde_json::to_vec(&SignedManifest {
            signature: hex_encode(&signing_key.sign(&canonical).to_bytes()),
            manifest,
        })
        .map_err(|error| format!("envelope does not re-serialise: {error}"))?;
        key = signing_key.verifying_key().to_bytes().to_vec();
    }

    let install = core.install_city_pack(envelope, files, key);
    if !install.accepted {
        return Err(format!("the pack was refused: {:?}", install.error));
    }
    Ok(core)
}

/// The same request `EmergencyScreen` builds for the demo button.
fn route_of(core: &Prohori) -> OfflineRouteResult {
    core.offline_route(OfflineRouteRequest {
        latitude: 24.3630,
        longitude: 88.6280,
        specialty: "general_emergency".to_owned(),
        now_epoch_seconds: 1_787_284_500,
        vehicle_width_millimetres: 2_400,
        vehicle_height_millimetres: 3_000,
        permit_flooded_origin_zone: false,
    })
}

fn route_json(route: &OfflineRouteResult) -> Value {
    json!({
        "accepted": route.accepted,
        "error": route.error,
        "pack_id": route.pack_id,
        "field_checked": route.field_checked,
        "hospital_id": route.hospital_id,
        "hospital_name": route.hospital_name,
        "hospital_hotline": route.hospital_hotline,
        "hospital_sms": route.hospital_sms,
        "hospital_telegram": route.hospital_telegram,
        "edge_ids": route.edge_ids,
        "estimated_seconds": route.estimated_seconds,
        "condition_age_seconds": route.condition_age_seconds,
        "condition_sources": route.condition_sources,
        "facility_age_seconds": route.facility_age_seconds,
        "attribution": route.attribution,
    })
}

fn confirmation_json(view: &HospitalConfirmationView) -> Value {
    json!({
        "pack_id": view.pack_id,
        "case_id": view.case_id,
        "hospital_id": view.hospital_id,
        "hospital_name": view.hospital_name,
        "destination": view.destination,
        "specialty": view.specialty,
        "eta_minutes": view.eta_minutes,
        "channel": format!("{:?}", view.channel),
        "status": format!("{:?}", view.status),
        "explicit_ready": view.explicit_ready,
        "sms_body": view.sms_body,
        "voice_script": view.voice_script,
        "online_body": view.online_body,
        "recorded_by": view.recorded_by,
        "reply_source": view.reply_source.map(|source| format!("{source:?}")),
    })
}

fn start(core: &Prohori, channel: HospitalContactChannel, eta_minutes: u32) -> Value {
    let started = core.start_hospital_confirmation(HospitalConfirmationRequest {
        hospital_id: "rmch".to_owned(),
        specialty: "general_emergency".to_owned(),
        eta_minutes,
        channel,
        created_at_epoch_millis: 1_787_284_500_000,
    });
    match started.confirmation.as_ref() {
        Some(view) => confirmation_json(view),
        None => json!({"refused": started.error}),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<Value, String> {
    let core = Prohori::new();
    let messages = [
        "he is not breathing",
        "she is not waking up but breathing",
        "my child swallowed something and is choking",
        "burnt my hand on the stove",
        "my head feels funny",
        "my neighbour is trapped under a concrete slab",
        "the tv remote is broken",
        "",
    ];

    let mut states = Map::new();
    for message in messages {
        let triage = core.triage(message.to_owned());
        let search = core.search(message.to_owned());
        states.insert(
            message.to_owned(),
            json!({
                "severity": triage.severity.map(|s| format!("{s:?}")),
                "bypasses_model": triage.bypasses_model,
                "card": triage.card.as_ref().map(card_json),
                "hits": triage.hits.iter().map(|hit| json!({
                    "rule_id": hit.rule_id,
                    "matched": hit.matched,
                    "severity": format!("{:?}", hit.severity),
                    "guidance_pending": hit.guidance_pending,
                })).collect::<Vec<_>>(),
                "recognised_without_guidance": triage.recognised_without_guidance
                    .iter()
                    .map(|hit| json!({"rule_id": hit.rule_id, "matched": hit.matched}))
                    .collect::<Vec<_>>(),
                "search": search.iter().map(|result| json!({
                    "matched": result.matched,
                    "card": card_json(&result.card),
                })).collect::<Vec<_>>(),
                // The unmatched path: whether the model may write, and why not when it may
                // not. `fallback_permitted` is what the screen branches on.
                "fallback_permitted": core.fallback_permitted(message.to_owned()),
                "fallback_suppression": core.fallback_suppression(message.to_owned()),
            }),
        );
    }

    // As shipped: the demo pack binds a hotline, no SMS number and no Telegram chat.
    let shipped = core_with_pack(None)?;
    let shipped_route = route_of(&shipped);
    let shipped_info = shipped
        .city_pack_info()
        .ok_or_else(|| "the pack installed but reports no info".to_owned())?;
    let voice_draft = start(&shipped, HospitalContactChannel::Voice, 13);
    let sms_refusal = start(&shipped, HospitalContactChannel::SmsIntent, 13);

    // Re-signed with the tests' drill chat, so the online channel has somewhere to go.
    let online = core_with_pack(Some(DRILL_CHAT))?;
    let online_route = route_of(&online);
    let eta = online_route.estimated_seconds.map_or(1, |seconds| {
        u32::try_from(seconds.div_ceil(60)).unwrap_or(1).max(1)
    });
    let online_draft = start(&online, HospitalContactChannel::Online, eta);
    let awaiting = online
        .mark_hospital_contacted(1_787_284_520)
        .confirmation
        .as_ref()
        .map(confirmation_json);
    let confirmed = online
        .ingest_online_reply(
            HospitalReply::Yes,
            1_787_284_560,
            online_draft
                .get("case_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        )
        .confirmation
        .as_ref()
        .map(confirmation_json);

    let numbers = core.emergency_numbers(Some("BD".to_owned()), None, None);
    Ok(json!({
        "states": Value::Object(states),
        "cards": core.cards().iter().map(|card| json!({
            "protocol_id": card.protocol_id,
            "title": card.title,
            "clinically_reviewed": card.clinically_reviewed,
        })).collect::<Vec<_>>(),
        "safety_net": core.safety_net_card().as_ref().map(card_json),
        "emergency": {
            "ambulance": numbers.ambulance,
            "ambulance_dial": numbers.ambulance_dial,
            "country": numbers.country,
            "country_name": numbers.country_name,
            "provenance": format!("{:?}", numbers.provenance),
            "confirmed_local": numbers.confirmed_local,
            "gsm_112_also_works": numbers.gsm_112_also_works,
        },
        "pack": {
            "pack_id": shipped_info.pack_id,
            "city": shipped_info.city,
            "version": shipped_info.version,
            "field_checked": shipped_info.field_checked,
            "attribution": shipped_info.attribution,
            "hospital_count": shipped_info.hospital_count,
            "road_edge_count": shipped_info.road_edge_count,
        },
        "route_shipped": route_json(&shipped_route),
        "route_online": route_json(&online_route),
        "confirmation": {
            "voice_draft": voice_draft,
            "sms_refusal": sms_refusal,
            "online_draft": online_draft,
            "online_awaiting": awaiting,
            "online_confirmed": confirmed,
        },
        "core_version": prohori_ffi::core_version(),
        "inference_prompt": core.inference_contract("he is not breathing".to_owned()).prompt,
        "fallback_prompt": core.fallback_contract(
            "my neighbour is trapped under a concrete slab".to_owned()
        ).prompt,
        "fallback_grammar": core.fallback_contract("x".to_owned()).grammar,
    }))
}
