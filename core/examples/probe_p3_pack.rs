use prohori_core::city_pack::{HospitalRouteRequest, LoadedCityPack, RouteFailure, SignedManifest};
use serde_json::json;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(value) => match serde_json::to_string_pretty(&value) {
            Ok(text) => {
                println!("{text}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("could not render evidence: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<serde_json::Value, String> {
    let mut args = env::args().skip(1);
    let directory = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| "missing pack directory".to_owned())?;
    let now: u64 = args
        .next()
        .ok_or_else(|| "missing route epoch".to_owned())?
        .parse()
        .map_err(|_| "route epoch must be an integer")?;
    let scenario = args.next().unwrap_or_else(|| "open".to_owned());
    let envelope = fs::read(directory.join("manifest.json"))
        .map_err(|error| format!("could not read manifest: {error}"))?;
    let signed: SignedManifest = serde_json::from_slice(&envelope)
        .map_err(|error| format!("could not inspect manifest: {error}"))?;
    let mut files = HashMap::new();
    for path in signed.manifest.files.keys() {
        let bytes = fs::read(directory.join(path))
            .map_err(|error| format!("could not read {path}: {error}"))?;
        files.insert(path.clone(), bytes);
    }
    let key_hex = fs::read_to_string(directory.join("verification-key.hex"))
        .map_err(|error| format!("could not read verification key: {error}"))?;
    let public_key = hex::decode(key_hex.trim()).map_err(|_| "verification key is not hex")?;
    let pack = LoadedCityPack::load(&envelope, &files, &public_key)
        .map_err(|error| format!("pack refused: {error}"))?;
    let request = HospitalRouteRequest {
        latitude: 24.3630,
        longitude: 88.6280,
        specialty: "general_emergency",
        now_epoch_seconds: now,
        vehicle_width_millimetres: 2_400,
        vehicle_height_millimetres: 3_000,
        permit_flooded_origin_zone: false,
    };
    // Every candidate the router graded, with the sentence it would show. Emitted for all four
    // scenarios so the claim "states, in plain words, why each unusable one was rejected" is
    // reproducible host evidence rather than a screenshot.
    let considered = pack
        .rank_hospitals(request)
        .map_err(|error| format!("scenario {scenario} could not rank hospitals at all: {error}"))?;
    let ranking: Vec<serde_json::Value> = considered
        .iter()
        .map(|option| {
            json!({
                "hospital_id": option.hospital.id,
                "usable": option.usable,
                "reason": option.reason,
                "estimated_seconds": option.route.as_ref().map(|route| route.estimated_seconds)
            })
        })
        .collect();
    for option in &considered {
        if option.reason.trim().is_empty() {
            return Err(format!(
                "scenario {scenario} left hospital {} unexplained",
                option.hospital.id
            ));
        }
    }
    let result = pack.route_to_hospital(request);
    if scenario == "stale" {
        if result != Err(RouteFailure::NoPassableRoute) {
            return Err(format!("stale pack did not fail closed: {result:?}"));
        }
        // Failing closed is not enough on its own. A refusal that cannot distinguish "the news
        // is old" from "the road is shut" sends a family to the wrong decision, so the wording
        // is asserted, not merely printed.
        let stale_named = considered
            .iter()
            .all(|option| !option.usable && option.reason.contains("too old to trust"));
        if !stale_named {
            return Err(format!(
                "stale pack refused without saying the reports are stale: {ranking:?}"
            ));
        }
        return Ok(json!({
            "schema_version": 1,
            "scenario": scenario,
            "pack_id": pack.manifest.pack_id,
            "signature_and_digests_passed": true,
            "route_refused": true,
            "reason": "all road observations are stale",
            "considered": ranking,
            "field_checked": pack.manifest.field_checked
        }));
    }
    let route = result.map_err(|error| format!("route refused: {error}"))?;
    let expected_edges: &[u32] = if scenario == "blocked" {
        &[1010, 1011]
    } else {
        &[1001, 1002, 1003, 1004]
    };
    if route.route.edge_ids != expected_edges {
        return Err(format!(
            "scenario {scenario} chose {:?}, expected {expected_edges:?}",
            route.route.edge_ids
        ));
    }
    Ok(json!({
        "schema_version": 1,
        "scenario": scenario,
        "pack_id": pack.manifest.pack_id,
        "signature_and_digests_passed": true,
        "route_refused": false,
        "hospital_id": route.hospital.id,
        "edge_ids": route.route.edge_ids,
        "estimated_seconds": route.route.estimated_seconds,
        "condition_age_seconds": route.condition_age_seconds,
        "condition_sources": route.condition_sources,
        "considered": ranking,
        "field_checked": pack.manifest.field_checked,
        "attribution": pack.manifest.attribution
    }))
}
