use ed25519_dalek::{Signer, SigningKey};
use prohori_core::city_pack::{BoundingBox, Manifest, SignedManifest};
use prohori_core::routing::{Condition, Edge, Graph, Node, RoadStatus};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let output = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| usage("missing output directory"))?;
    let key_path = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| usage("missing signing-key path"))?;
    let epoch: u64 = args
        .next()
        .ok_or_else(|| usage("missing snapshot epoch"))?
        .parse()
        .map_err(|_| usage("snapshot epoch must be an integer"))?;
    let scenario = args.next().unwrap_or_else(|| "open".to_owned());
    if !matches!(scenario.as_str(), "open" | "blocked" | "flooded" | "stale") {
        return Err(usage("scenario must be open, blocked, flooded, or stale"));
    }
    let key_hex = fs::read_to_string(&key_path)
        .map_err(|error| format!("could not read {}: {error}", key_path.display()))?;
    let key_bytes = hex::decode(key_hex.trim()).map_err(|_| "signing key is not hex")?;
    let key_array: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| "signing key must contain exactly 32 bytes")?;
    let signing_key = SigningKey::from_bytes(&key_array);

    fs::create_dir_all(&output)
        .map_err(|error| format!("could not create {}: {error}", output.display()))?;
    let payloads = payloads(epoch, &scenario)?;
    for (path, bytes) in &payloads {
        fs::write(output.join(path), bytes)
            .map_err(|error| format!("could not write {path}: {error}"))?;
    }
    let manifest = Manifest {
        schema_version: 1,
        pack_id: format!("bd-rajshahi-ruet-demo-{scenario}"),
        city: "Rajshahi".to_owned(),
        country: "BD".to_owned(),
        version: 1,
        built_at_epoch_seconds: epoch,
        bbox: BoundingBox {
            south: 24.3500,
            west: 88.5800,
            north: 24.3900,
            east: 88.6400,
        },
        attribution: "DEMO TOPOLOGY — NOT FIELD CHECKED. Facility/contact: official RMCH pages; RUET referral context: official RUET page; coordinates: DGHS/OpenStreetMap. © OpenStreetMap contributors, ODbL."
            .to_owned(),
        field_checked: false,
        files: payloads
            .iter()
            .map(|(path, bytes)| (path.clone(), hex::encode(Sha256::digest(bytes))))
            .collect(),
    };
    let canonical = serde_json::to_vec(&manifest).map_err(|error| error.to_string())?;
    let envelope = SignedManifest {
        signature: hex::encode(signing_key.sign(&canonical).to_bytes()),
        manifest,
    };
    write_json(output.join("manifest.json"), &envelope)?;
    fs::write(
        output.join("verification-key.hex"),
        hex::encode(signing_key.verifying_key().to_bytes()),
    )
    .map_err(|error| format!("could not write verification key: {error}"))?;
    println!("built {}", output.display());
    println!(
        "public_key={}",
        hex::encode(signing_key.verifying_key().to_bytes())
    );
    Ok(())
}

fn payloads(epoch: u64, scenario: &str) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let graph = Graph {
        nodes: vec![
            node(1, 24.3630, 88.6280),
            node(2, 24.3655, 88.6180),
            node(3, 24.3685, 88.6070),
            node(4, 24.3710, 88.5960),
            node(5, 24.3731, 88.5869),
            node(6, 24.3605, 88.6060),
        ],
        edges: vec![
            edge(1001, 1, 2, 180),
            edge(1002, 2, 3, 210),
            edge(1003, 3, 4, 210),
            edge(1004, 4, 5, 180),
            edge(1010, 1, 6, 480),
            edge(1011, 6, 5, 480),
        ],
    };
    let observed = if scenario == "stale" {
        epoch.saturating_sub(172_800)
    } else {
        epoch
    };
    let conditions: Vec<Value> = graph
        .edges
        .iter()
        .map(|edge| {
            let status = match (scenario, edge.id) {
                ("blocked", 1002) => RoadStatus::Blocked,
                ("flooded", 1010 | 1011) => RoadStatus::Flooded,
                _ => RoadStatus::Open,
            };
            let condition = Condition {
                status,
                source: format!("hackathon_{scenario}_snapshot"),
                observed_at_epoch_seconds: observed,
                stale_after_seconds: 86_400,
            };
            json!({
                "edge_id": edge.id,
                "status": condition.status,
                "source": condition.source,
                "observed_at_epoch_seconds": condition.observed_at_epoch_seconds,
                "stale_after_seconds": condition.stale_after_seconds
            })
        })
        .collect();
    let mut files = BTreeMap::new();
    files.insert(
        "emergency.json".to_owned(),
        json_bytes(&json!({
            "country": "BD",
            "ambulance": "999",
            "police": "999",
            "fire": "999"
        }))?,
    );
    files.insert(
        "hospitals.json".to_owned(),
        json_bytes(&json!({
            "schema_version": 1,
            "hospitals": [{
                "id": "rmch",
                "name": "Rajshahi Medical College Hospital",
                "latitude": 24.3731,
                "longitude": 88.5869,
                "node_id": 5,
                "specialties": ["general_emergency", "cardiac", "neurology", "trauma", "respiratory", "burns"],
                "emergency_capable": true,
                "hotline": "+880721760254",
                // Both null for the same reason: RMCH has published an emergency hotline and
                // has not registered an SMS number or a Telegram chat with this project.
                // Writing a plausible-looking one here would produce a pack that verifies, a
                // button that appears to work, and an alert nobody receives. Present-as-null
                // documents the shape without asserting a contact that does not exist.
                "sms_number": null,
                "telegram_chat_id": null,
                "verified_at_epoch_seconds": epoch,
                "verified_by": "Official RMCH contact/emergency pages; capability still requires field confirmation",
                "source_urls": [
                    "https://contacts.rmch.gov.bd/contacts/",
                    "https://contacts.rmch.gov.bd/contacts/emergency/",
                    "https://hris.mohfw.gov.bd/public/facility-registry/facilities/1554/profile?tab=detailed-information"
                ]
            }]
        }))?,
    );
    files.insert(
        "roads.graph".to_owned(),
        serde_json::to_vec(&graph).map_err(|error| error.to_string())?,
    );
    files.insert(
        "conditions.snap".to_owned(),
        json_bytes(&json!({"schema_version": 1, "conditions": conditions}))?,
    );
    files.insert(
        "shelters.json".to_owned(),
        json_bytes(&json!({"schema_version": 1, "shelters": []}))?,
    );
    files.insert(
        "zones.geojson".to_owned(),
        json_bytes(&json!({
            "type": "FeatureCollection",
            "features": [{
                "type": "Feature",
                "properties": {"id": "ruet_corridor", "name": "RUET–RMCH demo corridor"},
                "geometry": {
                    "type": "Polygon",
                    "coordinates": [[[88.5800,24.3500],[88.6400,24.3500],[88.6400,24.3900],[88.5800,24.3900],[88.5800,24.3500]]]
                }
            }]
        }))?,
    );
    Ok(files)
}

fn node(id: u32, latitude: f64, longitude: f64) -> Node {
    Node {
        id,
        latitude,
        longitude,
        zone: "ruet_corridor".to_owned(),
    }
}

fn edge(id: u32, from: u32, to: u32, seconds: u32) -> Edge {
    Edge {
        id,
        from,
        to,
        seconds,
        width_millimetres: 3_200,
        clearance_millimetres: 3_500,
        zone: "ruet_corridor".to_owned(),
    }
}

fn json_bytes(value: &Value) -> Result<Vec<u8>, String> {
    serde_json::to_vec(value).map_err(|error| error.to_string())
}

fn write_json(path: PathBuf, value: &impl serde::Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    fs::write(&path, bytes).map_err(|error| format!("could not write {}: {error}", path.display()))
}

fn usage(reason: &str) -> String {
    format!(
        "{reason}\nusage: build_p3_demo <output-dir> <32-byte-key-hex-file> <snapshot-epoch> [open|blocked|flooded|stale]"
    )
}
