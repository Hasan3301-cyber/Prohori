//! Signed city-pack boundary.
//!
//! The Android layer reads bytes, but this module decides whether those bytes are a pack.
//! No geographic datum is usable until both the Ed25519 signature and every listed SHA-256
//! digest pass. This keeps a corrupted download from becoming an ambulance destination.

use crate::confirmation;
use crate::emergency::{self, PackEmergency};
use crate::routing::{Condition, Graph, Route, RouteProfile};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

pub const REQUIRED_PAYLOADS: [&str; 6] = [
    "conditions.snap",
    "emergency.json",
    "hospitals.json",
    "roads.graph",
    "shelters.json",
    "zones.geojson",
];

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundingBox {
    pub south: f64,
    pub west: f64,
    pub north: f64,
    pub east: f64,
}

impl BoundingBox {
    #[must_use]
    pub fn is_valid(self) -> bool {
        self.south.is_finite()
            && self.west.is_finite()
            && self.north.is_finite()
            && self.east.is_finite()
            && (-90.0..=90.0).contains(&self.south)
            && (-90.0..=90.0).contains(&self.north)
            && (-180.0..=180.0).contains(&self.west)
            && (-180.0..=180.0).contains(&self.east)
            && self.south < self.north
            && self.west < self.east
    }

    #[must_use]
    pub fn contains(self, latitude: f64, longitude: f64) -> bool {
        latitude.is_finite()
            && longitude.is_finite()
            && (self.south..=self.north).contains(&latitude)
            && (self.west..=self.east).contains(&longitude)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub pack_id: String,
    pub city: String,
    pub country: String,
    pub version: u32,
    pub built_at_epoch_seconds: u64,
    pub bbox: BoundingBox,
    /// Required attribution for derived map data, shown in the app.
    pub attribution: String,
    /// True only after the route graph and facilities were checked on the ground.
    pub field_checked: bool,
    /// Relative path -> lowercase SHA-256.
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedManifest {
    pub manifest: Manifest,
    /// Ed25519 signature over canonical JSON bytes of `manifest`, lowercase hex.
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackError {
    InvalidEnvelope,
    UnsupportedSchema(u32),
    InvalidPublicKey,
    InvalidSignature,
    UnsafePath(String),
    MissingFile(String),
    InvalidDigest(String),
    DigestMismatch(String),
    InvalidMetadata(String),
    RequiredFileNotListed(String),
    InvalidPayload(String),
    InvalidGraph(String),
}

impl fmt::Display for PackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEnvelope => write!(f, "city-pack manifest is not valid JSON"),
            Self::UnsupportedSchema(version) => {
                write!(f, "unsupported city-pack schema {version}")
            }
            Self::InvalidPublicKey => write!(f, "city-pack verification key is invalid"),
            Self::InvalidSignature => write!(f, "city-pack signature does not match"),
            Self::UnsafePath(path) => write!(f, "unsafe city-pack path {path:?}"),
            Self::MissingFile(path) => write!(f, "city-pack file {path:?} is missing"),
            Self::InvalidDigest(path) => write!(f, "invalid SHA-256 for {path:?}"),
            Self::DigestMismatch(path) => write!(f, "SHA-256 mismatch for {path:?}"),
            Self::InvalidMetadata(reason) => write!(f, "invalid city-pack metadata: {reason}"),
            Self::RequiredFileNotListed(path) => {
                write!(f, "required city-pack file {path:?} is not signed")
            }
            Self::InvalidPayload(path) => write!(f, "invalid city-pack payload {path:?}"),
            Self::InvalidGraph(reason) => write!(f, "invalid road graph: {reason}"),
        }
    }
}

pub fn verify(
    envelope_json: &[u8],
    files: &HashMap<String, Vec<u8>>,
    public_key: &[u8],
) -> Result<Manifest, PackError> {
    let signed: SignedManifest =
        serde_json::from_slice(envelope_json).map_err(|_| PackError::InvalidEnvelope)?;
    if signed.manifest.schema_version != 1 {
        return Err(PackError::UnsupportedSchema(signed.manifest.schema_version));
    }
    validate_manifest(&signed.manifest)?;
    for path in signed.manifest.files.keys() {
        if path.starts_with('/')
            || path.starts_with('\\')
            || path
                .split(['/', '\\'])
                .any(|part| part.is_empty() || part == "..")
        {
            return Err(PackError::UnsafePath(path.clone()));
        }
    }

    let key_bytes: [u8; 32] = public_key
        .try_into()
        .map_err(|_| PackError::InvalidPublicKey)?;
    let key = VerifyingKey::from_bytes(&key_bytes).map_err(|_| PackError::InvalidPublicKey)?;
    let signature_bytes =
        hex::decode(&signed.signature).map_err(|_| PackError::InvalidSignature)?;
    let signature =
        Signature::from_slice(&signature_bytes).map_err(|_| PackError::InvalidSignature)?;
    let canonical = serde_json::to_vec(&signed.manifest).map_err(|_| PackError::InvalidEnvelope)?;
    key.verify(&canonical, &signature)
        .map_err(|_| PackError::InvalidSignature)?;

    for (path, expected) in &signed.manifest.files {
        if expected.len() != 64
            || !expected
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(PackError::InvalidDigest(path.clone()));
        }
        let bytes = files
            .get(path)
            .ok_or_else(|| PackError::MissingFile(path.clone()))?;
        let actual = hex::encode(Sha256::digest(bytes));
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(PackError::DigestMismatch(path.clone()));
        }
    }
    Ok(signed.manifest)
}

fn validate_manifest(manifest: &Manifest) -> Result<(), PackError> {
    if manifest.pack_id.trim().is_empty()
        || manifest.city.trim().is_empty()
        || manifest.country.len() != 2
        || !manifest
            .country
            .bytes()
            .all(|byte| byte.is_ascii_uppercase())
        || manifest.version == 0
        || manifest.built_at_epoch_seconds == 0
        || manifest.attribution.trim().is_empty()
        || !manifest.bbox.is_valid()
    {
        return Err(PackError::InvalidMetadata(
            "pack id, city, uppercase country, version, time, bbox, and attribution are required"
                .to_owned(),
        ));
    }
    for required in REQUIRED_PAYLOADS {
        if !manifest.files.contains_key(required) {
            return Err(PackError::RequiredFileNotListed(required.to_owned()));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hospital {
    pub id: String,
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
    pub node_id: u32,
    pub specialties: Vec<String>,
    pub emergency_capable: bool,
    pub hotline: String,
    #[serde(default)]
    pub sms_number: Option<String>,
    /// The hospital's Telegram chat, if one has been registered.
    ///
    /// A numeric chat id or an `@username`, validated by
    /// [`confirmation::is_telegram_endpoint`] at import time. Optional and defaulted, so a
    /// pack signed before this field existed still deserialises and still verifies — its
    /// bytes are unchanged, which is the only reason its signature survives.
    ///
    /// This is a *capability*, not the authoritative address. The relay resolves the chat
    /// from its own registry and refuses to send if the two disagree; a signed pack alone
    /// cannot redirect hospital traffic somewhere new.
    #[serde(default)]
    pub telegram_chat_id: Option<String>,
    pub verified_at_epoch_seconds: u64,
    pub verified_by: String,
    #[serde(default)]
    pub source_urls: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HospitalsFile {
    schema_version: u32,
    hospitals: Vec<Hospital>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConditionsFile {
    schema_version: u32,
    conditions: Vec<EdgeCondition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EdgeCondition {
    edge_id: u32,
    #[serde(flatten)]
    condition: Condition,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SheltersFile {
    schema_version: u32,
    shelters: Vec<Shelter>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Shelter {
    id: String,
    name: String,
    latitude: f64,
    longitude: f64,
    node_id: u32,
    verified_at_epoch_seconds: u64,
    verified_by: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ZoneCollection {
    #[serde(rename = "type")]
    kind: String,
    features: Vec<ZoneFeature>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ZoneFeature {
    #[serde(rename = "type")]
    kind: String,
    properties: ZoneProperties,
    geometry: ZoneGeometry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ZoneProperties {
    id: String,
    name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ZoneGeometry {
    #[serde(rename = "type")]
    kind: String,
    coordinates: Vec<Vec<[f64; 2]>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedCityPack {
    pub manifest: Manifest,
    pub emergency: PackEmergency,
    pub hospitals: Vec<Hospital>,
    pub graph: Graph,
    pub conditions: HashMap<u32, Condition>,
    /// Zone id to the name a resident would use, from `zones.geojson`.
    ///
    /// An `Edge` carries the zone *id* (`ruet`), which is fine for routing and wrong in a
    /// sentence a frightened person reads. Kept so a rejection can say "RUET corridor".
    pub zone_names: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteFailure {
    OutsidePack,
    NoRoadNode,
    NoCapableHospital,
    NoPassableRoute,
}

impl fmt::Display for RouteFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutsidePack => write!(f, "location is outside the installed city pack"),
            Self::NoRoadNode => write!(f, "city pack has no road node for this location"),
            Self::NoCapableHospital => write!(f, "city pack has no matching emergency hospital"),
            Self::NoPassableRoute => write!(
                f,
                "no route has fresh, known, open conditions for this vehicle"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HospitalRoute {
    pub hospital: Hospital,
    pub start_node_id: u32,
    pub route: Route,
    pub condition_age_seconds: u64,
    pub condition_sources: Vec<String>,
    pub facility_age_seconds: u64,
}

/// One candidate hospital, graded, carrying the reason it was kept or rejected.
///
/// The nearest hospital is not the fastest, and the fastest way to it is not always passable.
/// [`LoadedCityPack::route_to_hospital`] answers only with the survivor, which meant a family
/// was never told that the hospital a kilometre away had been considered and ruled out. Keeping
/// the rejects — each with the words for why — is what makes that decision inspectable.
#[derive(Debug, Clone, PartialEq)]
pub struct HospitalOption {
    pub hospital: Hospital,
    /// True when a route survived every freshness, hazard, and vehicle check.
    pub usable: bool,
    /// A whole sentence, plain enough for a frightened reader.
    ///
    /// Authored here rather than in Kotlin for the same reason `FirstAidCard::provenance` is:
    /// the UI displays this text and never composes it.
    pub reason: String,
    /// Present only when `usable`.
    pub route: Option<Route>,
    pub start_node_id: u32,
    pub condition_age_seconds: u64,
    pub condition_sources: Vec<String>,
    pub facility_age_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HospitalRouteRequest<'a> {
    pub latitude: f64,
    pub longitude: f64,
    pub specialty: &'a str,
    pub now_epoch_seconds: u64,
    pub vehicle_width_millimetres: u32,
    pub vehicle_height_millimetres: u32,
    pub permit_flooded_origin_zone: bool,
}

impl LoadedCityPack {
    pub fn load(
        envelope_json: &[u8],
        files: &HashMap<String, Vec<u8>>,
        public_key: &[u8],
    ) -> Result<Self, PackError> {
        let manifest = verify(envelope_json, files, public_key)?;
        let emergency: PackEmergency = parse_payload(files, "emergency.json")?;
        let hospitals: HospitalsFile = parse_payload(files, "hospitals.json")?;
        let graph: Graph = parse_payload(files, "roads.graph")?;
        let conditions: ConditionsFile = parse_payload(files, "conditions.snap")?;
        let shelters: SheltersFile = parse_payload(files, "shelters.json")?;
        let zones: ZoneCollection = parse_payload(files, "zones.geojson")?;

        if hospitals.schema_version != 1
            || conditions.schema_version != 1
            || shelters.schema_version != 1
        {
            return Err(PackError::InvalidPayload(
                "payload schema version".to_owned(),
            ));
        }
        validate_payloads(
            &manifest,
            &emergency,
            &hospitals.hospitals,
            &graph,
            &conditions.conditions,
            &shelters.shelters,
            &zones,
        )?;
        let condition_map = conditions
            .conditions
            .into_iter()
            .map(|entry| (entry.edge_id, entry.condition))
            .collect();
        Ok(Self {
            manifest,
            emergency,
            hospitals: hospitals.hospitals,
            graph,
            conditions: condition_map,
            zone_names: zones
                .features
                .iter()
                .filter(|feature| !feature.properties.name.trim().is_empty())
                .map(|feature| {
                    (
                        feature.properties.id.clone(),
                        feature.properties.name.clone(),
                    )
                })
                .collect(),
        })
    }

    pub fn route_to_hospital(
        &self,
        request: HospitalRouteRequest<'_>,
    ) -> Result<HospitalRoute, RouteFailure> {
        let chosen = self
            .rank_hospitals(request)?
            .into_iter()
            .find(|option| option.usable)
            .ok_or(RouteFailure::NoPassableRoute)?;
        // `usable` is only ever set alongside a route, so this is the same refusal rather than
        // a second, different one.
        let route = chosen.route.ok_or(RouteFailure::NoPassableRoute)?;
        Ok(HospitalRoute {
            hospital: chosen.hospital,
            start_node_id: chosen.start_node_id,
            route,
            condition_age_seconds: chosen.condition_age_seconds,
            condition_sources: chosen.condition_sources,
            facility_age_seconds: chosen.facility_age_seconds,
        })
    }

    /// Grade every capable hospital and keep the ones that were ruled out.
    ///
    /// Ordered survivors first by ETA, then rejects nearest-first — because the point of
    /// showing rejects at all is that the closest hospital is often the unreachable one, and
    /// that is the fact a family would otherwise discover at the blockage.
    ///
    /// Ties break on hospital id so the same pack and the same clock always produce the same
    /// list; `core` stays clock-free and RNG-free, and this ordering must not be the exception.
    pub fn rank_hospitals(
        &self,
        request: HospitalRouteRequest<'_>,
    ) -> Result<Vec<HospitalOption>, RouteFailure> {
        if !self
            .manifest
            .bbox
            .contains(request.latitude, request.longitude)
        {
            return Err(RouteFailure::OutsidePack);
        }
        let start = self
            .graph
            .nodes
            .iter()
            .min_by(|left, right| {
                squared_distance(
                    request.latitude,
                    request.longitude,
                    left.latitude,
                    left.longitude,
                )
                .total_cmp(&squared_distance(
                    request.latitude,
                    request.longitude,
                    right.latitude,
                    right.longitude,
                ))
                .then_with(|| left.id.cmp(&right.id))
            })
            .ok_or(RouteFailure::NoRoadNode)?;
        let requested = request.specialty.trim();
        let candidates: Vec<&Hospital> = self
            .hospitals
            .iter()
            .filter(|hospital| {
                hospital.emergency_capable
                    && (requested.is_empty()
                        || requested == "unknown"
                        || requested == "general_emergency"
                        || hospital
                            .specialties
                            .iter()
                            .any(|value| value == requested || value == "general_emergency"))
            })
            .collect();
        if candidates.is_empty() {
            return Err(RouteFailure::NoCapableHospital);
        }
        let profile = RouteProfile {
            now_epoch_seconds: request.now_epoch_seconds,
            vehicle_width_millimetres: request.vehicle_width_millimetres,
            vehicle_height_millimetres: request.vehicle_height_millimetres,
            patient_zone: start.zone.clone(),
            permit_flooded_origin_zone: request.permit_flooded_origin_zone,
        };
        let facility_age_seconds = |hospital: &Hospital| {
            request
                .now_epoch_seconds
                .saturating_sub(hospital.verified_at_epoch_seconds)
        };

        let mut graded: Vec<(HospitalOption, f64)> = Vec::with_capacity(candidates.len());
        for hospital in candidates {
            let proximity = squared_distance(
                request.latitude,
                request.longitude,
                hospital.latitude,
                hospital.longitude,
            );
            let option =
                match self
                    .graph
                    .route(start.id, hospital.node_id, &self.conditions, &profile)
                {
                    Some(route) => {
                        let (condition_age_seconds, condition_sources) =
                            self.route_condition_provenance(&route, request.now_epoch_seconds);
                        // The chosen way being open does not mean nothing was avoided. Grade the
                        // corridor a traveller would have taken with no vetoes at all, and if it
                        // was shut, say so — otherwise a longer drive looks like the normal one.
                        let unvetoed = self.graph.explain(
                            start.id,
                            hospital.node_id,
                            &self.conditions,
                            &profile,
                        );
                        let detour = route
                            .estimated_seconds
                            .saturating_sub(unvetoed.fastest_seconds);
                        let reason = unvetoed
                            .with_zone_names(&self.zone_names)
                            .detour_reason(detour)
                            .unwrap_or_else(|| {
                                "Every road on this way is open, and the reports are fresh."
                                    .to_owned()
                            });
                        HospitalOption {
                            hospital: hospital.clone(),
                            usable: true,
                            reason,
                            route: Some(route),
                            start_node_id: start.id,
                            condition_age_seconds,
                            condition_sources,
                            facility_age_seconds: facility_age_seconds(hospital),
                        }
                    }
                    None => HospitalOption {
                        hospital: hospital.clone(),
                        usable: false,
                        reason: self
                            .graph
                            .explain(start.id, hospital.node_id, &self.conditions, &profile)
                            .with_zone_names(&self.zone_names)
                            .reason(),
                        route: None,
                        start_node_id: start.id,
                        condition_age_seconds: 0,
                        condition_sources: Vec::new(),
                        facility_age_seconds: facility_age_seconds(hospital),
                    },
                };
            graded.push((option, proximity));
        }

        graded.sort_by(|(left, left_proximity), (right, right_proximity)| {
            right
                .usable
                .cmp(&left.usable)
                .then_with(|| match (left.route.as_ref(), right.route.as_ref()) {
                    (Some(l), Some(r)) => l.estimated_seconds.cmp(&r.estimated_seconds),
                    _ => left_proximity.total_cmp(right_proximity),
                })
                .then_with(|| left.hospital.id.cmp(&right.hospital.id))
        });
        Ok(graded.into_iter().map(|(option, _)| option).collect())
    }

    /// How old the worst condition on this route is, and which sources it came from.
    fn route_condition_provenance(
        &self,
        route: &Route,
        now_epoch_seconds: u64,
    ) -> (u64, Vec<String>) {
        let route_edge_ids: HashSet<u32> = route.edge_ids.iter().copied().collect();
        let route_conditions: Vec<&Condition> = self
            .conditions
            .iter()
            .filter_map(|(edge_id, condition)| {
                route_edge_ids.contains(edge_id).then_some(condition)
            })
            .collect();
        let condition_age_seconds = route_conditions
            .iter()
            .map(|condition| now_epoch_seconds.saturating_sub(condition.observed_at_epoch_seconds))
            .max()
            .unwrap_or(0);
        let condition_sources: BTreeSet<String> = route_conditions
            .iter()
            .map(|condition| condition.source.clone())
            .collect();
        (
            condition_age_seconds,
            condition_sources.into_iter().collect(),
        )
    }
}

fn parse_payload<T: for<'de> Deserialize<'de>>(
    files: &HashMap<String, Vec<u8>>,
    path: &str,
) -> Result<T, PackError> {
    let bytes = files
        .get(path)
        .ok_or_else(|| PackError::MissingFile(path.to_owned()))?;
    serde_json::from_slice(bytes).map_err(|_| PackError::InvalidPayload(path.to_owned()))
}

#[allow(clippy::too_many_arguments)]
fn validate_payloads(
    manifest: &Manifest,
    emergency: &PackEmergency,
    hospitals: &[Hospital],
    graph: &Graph,
    conditions: &[EdgeCondition],
    shelters: &[Shelter],
    zones: &ZoneCollection,
) -> Result<(), PackError> {
    if emergency.country != manifest.country
        || !emergency::is_dialable(&emergency.ambulance)
        || emergency
            .police
            .as_deref()
            .is_some_and(|number| !emergency::is_dialable(number))
        || emergency
            .fire
            .as_deref()
            .is_some_and(|number| !emergency::is_dialable(number))
    {
        return Err(PackError::InvalidPayload("emergency.json".to_owned()));
    }
    if graph.nodes.is_empty() || graph.edges.is_empty() {
        return Err(PackError::InvalidGraph(
            "at least one node and edge are required".to_owned(),
        ));
    }
    let mut node_ids = HashSet::new();
    for node in &graph.nodes {
        if !node_ids.insert(node.id)
            || node.zone.trim().is_empty()
            || !manifest.bbox.contains(node.latitude, node.longitude)
        {
            return Err(PackError::InvalidGraph(format!(
                "invalid or duplicate node {}",
                node.id
            )));
        }
    }
    let mut edge_ids = HashSet::new();
    for edge in &graph.edges {
        if !edge_ids.insert(edge.id)
            || !node_ids.contains(&edge.from)
            || !node_ids.contains(&edge.to)
            || edge.from == edge.to
            || edge.seconds == 0
            || edge.width_millimetres == 0
            || edge.clearance_millimetres == 0
            || edge.zone.trim().is_empty()
        {
            return Err(PackError::InvalidGraph(format!(
                "invalid or duplicate edge {}",
                edge.id
            )));
        }
    }
    let mut condition_ids = HashSet::new();
    for entry in conditions {
        if !condition_ids.insert(entry.edge_id)
            || !edge_ids.contains(&entry.edge_id)
            || entry.condition.source.trim().is_empty()
            || entry.condition.stale_after_seconds == 0
        {
            return Err(PackError::InvalidPayload("conditions.snap".to_owned()));
        }
    }
    if condition_ids != edge_ids {
        return Err(PackError::InvalidPayload(
            "conditions.snap does not cover every edge".to_owned(),
        ));
    }
    if hospitals.is_empty() {
        return Err(PackError::InvalidPayload("hospitals.json".to_owned()));
    }
    let mut hospital_ids = HashSet::new();
    for hospital in hospitals {
        if !hospital_ids.insert(hospital.id.as_str())
            || hospital.name.trim().is_empty()
            || !manifest
                .bbox
                .contains(hospital.latitude, hospital.longitude)
            || !node_ids.contains(&hospital.node_id)
            || hospital.specialties.is_empty()
            || !emergency::is_dialable(&hospital.hotline)
            || hospital
                .sms_number
                .as_deref()
                .is_some_and(|number| !emergency::is_dialable(number))
            || hospital
                .telegram_chat_id
                .as_deref()
                .is_some_and(|chat| !confirmation::is_telegram_endpoint(chat))
            || hospital.verified_at_epoch_seconds == 0
            || hospital.verified_by.trim().is_empty()
            || hospital.source_urls.is_empty()
        {
            return Err(PackError::InvalidPayload("hospitals.json".to_owned()));
        }
    }
    for shelter in shelters {
        if shelter.id.trim().is_empty()
            || shelter.name.trim().is_empty()
            || !manifest.bbox.contains(shelter.latitude, shelter.longitude)
            || !node_ids.contains(&shelter.node_id)
            || shelter.verified_at_epoch_seconds == 0
            || shelter.verified_by.trim().is_empty()
        {
            return Err(PackError::InvalidPayload("shelters.json".to_owned()));
        }
    }
    if zones.kind != "FeatureCollection" || zones.features.is_empty() {
        return Err(PackError::InvalidPayload("zones.geojson".to_owned()));
    }
    let mut zone_ids = HashSet::new();
    for feature in &zones.features {
        if feature.kind != "Feature"
            || feature.geometry.kind != "Polygon"
            || feature.properties.id.trim().is_empty()
            || feature.properties.name.trim().is_empty()
            || !zone_ids.insert(feature.properties.id.as_str())
            || feature.geometry.coordinates.is_empty()
            || feature
                .geometry
                .coordinates
                .iter()
                .flatten()
                .any(|point| !manifest.bbox.contains(point[1], point[0]))
        {
            return Err(PackError::InvalidPayload("zones.geojson".to_owned()));
        }
    }
    if graph
        .nodes
        .iter()
        .any(|node| !zone_ids.contains(node.zone.as_str()))
        || graph
            .edges
            .iter()
            .any(|edge| !zone_ids.contains(edge.zone.as_str()))
    {
        return Err(PackError::InvalidGraph(
            "node or edge refers to an unknown zone".to_owned(),
        ));
    }
    Ok(())
}

fn squared_distance(lat_a: f64, lon_a: f64, lat_b: f64, lon_b: f64) -> f64 {
    let lat = lat_a - lat_b;
    let lon = lon_a - lon_b;
    lat.mul_add(lat, lon * lon)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use crate::routing::{Edge, Node, RoadStatus};
    use ed25519_dalek::{Signer, SigningKey};

    fn fixture() -> (Vec<u8>, HashMap<String, Vec<u8>>, [u8; 32]) {
        let key = SigningKey::from_bytes(&[7; 32]);
        let files: HashMap<String, Vec<u8>> = REQUIRED_PAYLOADS
            .into_iter()
            .map(|path| (path.to_owned(), b"{}".to_vec()))
            .collect();
        let manifest = Manifest {
            schema_version: 1,
            pack_id: "bd-dhaka".to_owned(),
            city: "Dhaka".to_owned(),
            country: "BD".to_owned(),
            version: 3,
            built_at_epoch_seconds: 1_700_000_000,
            bbox: BoundingBox {
                south: 24.0,
                west: 88.0,
                north: 25.0,
                east: 89.0,
            },
            attribution: "Demo map data".to_owned(),
            field_checked: false,
            files: files
                .iter()
                .map(|(path, bytes)| (path.clone(), hex::encode(Sha256::digest(bytes))))
                .collect(),
        };
        let canonical = serde_json::to_vec(&manifest).unwrap_or_default();
        let signed = SignedManifest {
            signature: hex::encode(key.sign(&canonical).to_bytes()),
            manifest,
        };
        (
            serde_json::to_vec(&signed).unwrap_or_default(),
            files,
            key.verifying_key().to_bytes(),
        )
    }

    #[test]
    fn accepts_a_signed_intact_pack() {
        let (json, files, public) = fixture();
        assert_eq!(
            verify(&json, &files, &public).map(|m| m.city),
            Ok("Dhaka".into())
        );
    }

    #[test]
    fn refuses_tampered_content() {
        let (json, mut files, public) = fixture();
        files.insert("hospitals.json".into(), b"[]".to_vec());
        assert_eq!(
            verify(&json, &files, &public),
            Err(PackError::DigestMismatch("hospitals.json".into()))
        );
    }

    #[test]
    fn refuses_path_traversal_even_when_signed() {
        let key = SigningKey::from_bytes(&[8; 32]);
        let manifest = Manifest {
            schema_version: 1,
            pack_id: "bad".into(),
            city: "x".into(),
            country: "BD".into(),
            version: 1,
            built_at_epoch_seconds: 1,
            bbox: BoundingBox {
                south: 24.0,
                west: 88.0,
                north: 25.0,
                east: 89.0,
            },
            attribution: "test".into(),
            field_checked: false,
            files: REQUIRED_PAYLOADS
                .into_iter()
                .map(|path| (path.to_owned(), hex::encode(Sha256::digest(b"{}"))))
                .chain([("../escape".into(), hex::encode(Sha256::digest(b"x")))])
                .collect(),
        };
        let signature = key.sign(&serde_json::to_vec(&manifest).unwrap_or_default());
        let json = serde_json::to_vec(&SignedManifest {
            manifest,
            signature: hex::encode(signature.to_bytes()),
        })
        .unwrap_or_default();
        assert_eq!(
            verify(&json, &HashMap::new(), &key.verifying_key().to_bytes()),
            Err(PackError::UnsafePath("../escape".into()))
        );
    }

    fn loaded_fixture() -> (Vec<u8>, HashMap<String, Vec<u8>>, [u8; 32]) {
        loaded_fixture_with_telegram(None)
    }

    fn loaded_fixture_with_telegram(
        telegram_chat_id: Option<&str>,
    ) -> (Vec<u8>, HashMap<String, Vec<u8>>, [u8; 32]) {
        loaded_fixture_tuned(telegram_chat_id, Vec::new(), &[])
    }

    /// The demo pack, optionally with extra hospitals and with named edges reported blocked.
    fn loaded_fixture_tuned(
        telegram_chat_id: Option<&str>,
        extra_hospitals: Vec<Hospital>,
        blocked_edges: &[u32],
    ) -> (Vec<u8>, HashMap<String, Vec<u8>>, [u8; 32]) {
        let key = SigningKey::from_bytes(&[11; 32]);
        let graph = Graph {
            nodes: vec![
                Node {
                    id: 1,
                    latitude: 24.3630,
                    longitude: 88.6280,
                    zone: "ruet".into(),
                },
                Node {
                    id: 2,
                    latitude: 24.3680,
                    longitude: 88.6100,
                    zone: "ruet".into(),
                },
                Node {
                    id: 3,
                    latitude: 24.3730,
                    longitude: 88.5870,
                    zone: "ruet".into(),
                },
            ],
            edges: vec![
                Edge {
                    id: 1,
                    from: 1,
                    to: 2,
                    seconds: 300,
                    width_millimetres: 3200,
                    clearance_millimetres: 3500,
                    zone: "ruet".into(),
                },
                Edge {
                    id: 2,
                    from: 2,
                    to: 3,
                    seconds: 300,
                    width_millimetres: 3200,
                    clearance_millimetres: 3500,
                    zone: "ruet".into(),
                },
                Edge {
                    id: 3,
                    from: 1,
                    to: 3,
                    seconds: 900,
                    width_millimetres: 3200,
                    clearance_millimetres: 3500,
                    zone: "ruet".into(),
                },
            ],
        };
        let condition = |edge_id| EdgeCondition {
            edge_id,
            condition: Condition {
                status: if blocked_edges.contains(&edge_id) {
                    RoadStatus::Blocked
                } else {
                    RoadStatus::Open
                },
                source: "demo_snapshot".into(),
                observed_at_epoch_seconds: 900,
                stale_after_seconds: 200,
            },
        };
        let files = HashMap::from([
            (
                "emergency.json".to_owned(),
                serde_json::to_vec(&PackEmergency {
                    country: "BD".into(),
                    ambulance: "999".into(),
                    police: Some("999".into()),
                    fire: Some("999".into()),
                })
                .unwrap_or_default(),
            ),
            (
                "hospitals.json".to_owned(),
                serde_json::to_vec(&HospitalsFile {
                    schema_version: 1,
                    hospitals: vec![Hospital {
                        id: "rmch".into(),
                        name: "Rajshahi Medical College Hospital".into(),
                        latitude: 24.3730,
                        longitude: 88.5870,
                        node_id: 3,
                        specialties: vec!["general_emergency".into(), "cardiac".into()],
                        emergency_capable: true,
                        hotline: "+880721776001".into(),
                        sms_number: None,
                        telegram_chat_id: telegram_chat_id.map(str::to_owned),
                        verified_at_epoch_seconds: 800,
                        verified_by: "official contact page".into(),
                        source_urls: vec!["https://contacts.rmch.gov.bd/contacts/".into()],
                    }]
                    .into_iter()
                    .chain(extra_hospitals)
                    .collect(),
                })
                .unwrap_or_default(),
            ),
            (
                "roads.graph".to_owned(),
                serde_json::to_vec(&graph).unwrap_or_default(),
            ),
            (
                "conditions.snap".to_owned(),
                serde_json::to_vec(&ConditionsFile {
                    schema_version: 1,
                    conditions: vec![condition(1), condition(2), condition(3)],
                })
                .unwrap_or_default(),
            ),
            (
                "shelters.json".to_owned(),
                serde_json::to_vec(&SheltersFile {
                    schema_version: 1,
                    shelters: Vec::new(),
                })
                .unwrap_or_default(),
            ),
            (
                "zones.geojson".to_owned(),
                serde_json::to_vec(&ZoneCollection {
                    kind: "FeatureCollection".into(),
                    features: vec![ZoneFeature {
                        kind: "Feature".into(),
                        properties: ZoneProperties {
                            id: "ruet".into(),
                            name: "RUET corridor".into(),
                        },
                        geometry: ZoneGeometry {
                            kind: "Polygon".into(),
                            coordinates: vec![vec![
                                [88.58, 24.35],
                                [88.64, 24.35],
                                [88.64, 24.39],
                                [88.58, 24.39],
                                [88.58, 24.35],
                            ]],
                        },
                    }],
                })
                .unwrap_or_default(),
            ),
        ]);
        let manifest = Manifest {
            schema_version: 1,
            pack_id: "bd-rajshahi-ruet-demo".into(),
            city: "Rajshahi".into(),
            country: "BD".into(),
            version: 1,
            built_at_epoch_seconds: 800,
            bbox: BoundingBox {
                south: 24.35,
                west: 88.58,
                north: 24.39,
                east: 88.64,
            },
            attribution: "Demo topology; facility source RMCH".into(),
            field_checked: false,
            files: files
                .iter()
                .map(|(path, bytes)| (path.clone(), hex::encode(Sha256::digest(bytes))))
                .collect(),
        };
        let canonical = serde_json::to_vec(&manifest).unwrap_or_default();
        let envelope = SignedManifest {
            signature: hex::encode(key.sign(&canonical).to_bytes()),
            manifest,
        };
        (
            serde_json::to_vec(&envelope).unwrap_or_default(),
            files,
            key.verifying_key().to_bytes(),
        )
    }

    /// Failure 1: the closest hospital is often the unreachable one, and a family that is not
    /// told so drives at the blockage, turns around, and loses the time it did not have.
    #[test]
    fn a_nearer_hospital_that_cannot_be_reached_is_reported_and_not_silently_dropped() {
        // The clinic sits on node 2, closer to the caller than RMCH on node 3. Edge 1 is the
        // only way to node 2, and it is reported blocked — so the near option dies and the far
        // one survives on the long edge 3.
        let clinic = Hospital {
            id: "motihar-clinic".into(),
            name: "Motihar Clinic".into(),
            latitude: 24.3680,
            longitude: 88.6100,
            node_id: 2,
            specialties: vec!["general_emergency".into(), "cardiac".into()],
            emergency_capable: true,
            hotline: "+880721776002".into(),
            sms_number: None,
            telegram_chat_id: None,
            verified_at_epoch_seconds: 800,
            verified_by: "official contact page".into(),
            source_urls: vec!["https://contacts.motihar.example/".into()],
        };
        let (envelope, files, public_key) = loaded_fixture_tuned(None, vec![clinic], &[1]);
        let pack = LoadedCityPack::load(&envelope, &files, &public_key).expect("valid pack");
        let request = HospitalRouteRequest {
            latitude: 24.3630,
            longitude: 88.6280,
            specialty: "cardiac",
            now_epoch_seconds: 1_000,
            vehicle_width_millimetres: 2_400,
            vehicle_height_millimetres: 3_000,
            permit_flooded_origin_zone: false,
        };

        let ranked = pack.rank_hospitals(request).expect("ranking");
        assert_eq!(ranked.len(), 2, "both hospitals must be accounted for");

        let survivor = ranked.first().expect("a survivor");
        assert_eq!(survivor.hospital.id, "rmch");
        assert!(survivor.usable);
        assert_eq!(
            survivor.route.as_ref().map(|route| route.edge_ids.clone()),
            Some(vec![3]),
            "the long way is the only open way"
        );

        let rejected = ranked.get(1).expect("a reject");
        assert_eq!(rejected.hospital.id, "motihar-clinic");
        assert!(!rejected.usable);
        assert!(rejected.route.is_none());
        // The zone id is `ruet`; the sentence must use the name from zones.geojson.
        assert_eq!(
            rejected.reason, "The road through RUET corridor is blocked.",
            "a rejected hospital has to say why in words a resident would use"
        );

        // The chooser still agrees with the ranking, so adding the explanation changed no
        // decision — only what can be seen about it.
        let route = pack.route_to_hospital(request).expect("route");
        assert_eq!(route.hospital.id, "rmch");
        assert_eq!(route.route.edge_ids, vec![3]);
    }

    #[test]
    fn a_reachable_hospital_says_the_way_is_open_rather_than_saying_nothing() {
        let (envelope, files, public_key) = loaded_fixture();
        let pack = LoadedCityPack::load(&envelope, &files, &public_key).expect("valid pack");
        let ranked = pack
            .rank_hospitals(HospitalRouteRequest {
                latitude: 24.3630,
                longitude: 88.6280,
                specialty: "cardiac",
                now_epoch_seconds: 1_000,
                vehicle_width_millimetres: 2_400,
                vehicle_height_millimetres: 3_000,
                permit_flooded_origin_zone: false,
            })
            .expect("ranking");
        let chosen = ranked.first().expect("a survivor");
        assert!(chosen.usable);
        assert_eq!(
            chosen.reason,
            "Every road on this way is open, and the reports are fresh."
        );
    }

    #[test]
    fn loads_a_complete_pack_and_routes_to_a_capable_hospital() {
        let (envelope, files, public_key) = loaded_fixture();
        let pack = LoadedCityPack::load(&envelope, &files, &public_key).expect("valid pack");
        let route = pack
            .route_to_hospital(HospitalRouteRequest {
                latitude: 24.3630,
                longitude: 88.6280,
                specialty: "cardiac",
                now_epoch_seconds: 1_000,
                vehicle_width_millimetres: 2_400,
                vehicle_height_millimetres: 3_000,
                permit_flooded_origin_zone: false,
            })
            .expect("route");
        assert_eq!(route.hospital.id, "rmch");
        assert_eq!(route.route.edge_ids, vec![1, 2]);
        assert_eq!(route.route.estimated_seconds, 600);
        assert_eq!(route.condition_age_seconds, 100);
    }

    #[test]
    fn a_blocked_segment_forces_the_longer_known_open_route() {
        let (envelope, files, public_key) = loaded_fixture();
        let mut pack = LoadedCityPack::load(&envelope, &files, &public_key).expect("valid pack");
        if let Some(condition) = pack.conditions.get_mut(&2) {
            condition.status = RoadStatus::Blocked;
        }
        let route = pack
            .route_to_hospital(HospitalRouteRequest {
                latitude: 24.3630,
                longitude: 88.6280,
                specialty: "general_emergency",
                now_epoch_seconds: 1_000,
                vehicle_width_millimetres: 2_400,
                vehicle_height_millimetres: 3_000,
                permit_flooded_origin_zone: false,
            })
            .expect("alternative route");
        assert_eq!(route.route.edge_ids, vec![3]);
        assert_eq!(route.route.estimated_seconds, 900);
    }

    #[test]
    fn stale_conditions_fail_closed_end_to_end() {
        let (envelope, files, public_key) = loaded_fixture();
        let pack = LoadedCityPack::load(&envelope, &files, &public_key).expect("valid pack");
        assert_eq!(
            pack.route_to_hospital(HospitalRouteRequest {
                latitude: 24.3630,
                longitude: 88.6280,
                specialty: "general_emergency",
                now_epoch_seconds: 2_000,
                vehicle_width_millimetres: 2_400,
                vehicle_height_millimetres: 3_000,
                permit_flooded_origin_zone: false,
            }),
            Err(RouteFailure::NoPassableRoute)
        );
    }

    #[test]
    fn a_pack_signed_before_the_telegram_field_existed_still_loads() {
        // `deny_unknown_fields` is on this struct, so the reverse direction is already
        // covered. What has to hold here is that omission means absent rather than invalid:
        // a pack whose bytes were signed before `telegram_chat_id` was added must keep
        // working, and its signature only survives because those bytes are not rewritten.
        let without = r#"{
            "schema_version": 1,
            "hospitals": [{
                "id": "rmch",
                "name": "Rajshahi Medical College Hospital",
                "latitude": 24.3730,
                "longitude": 88.5870,
                "node_id": 3,
                "specialties": ["general_emergency"],
                "emergency_capable": true,
                "hotline": "+880721776001",
                "verified_at_epoch_seconds": 800,
                "verified_by": "official contact page",
                "source_urls": ["https://contacts.rmch.gov.bd/contacts/"]
            }]
        }"#;
        let parsed: HospitalsFile = serde_json::from_str(without).expect("older pack payload");
        let hospital = parsed.hospitals.first().expect("one hospital");
        assert_eq!(hospital.telegram_chat_id, None);
        assert_eq!(hospital.sms_number, None);
    }

    #[test]
    fn a_registered_telegram_endpoint_survives_verification() {
        let (envelope, files, public_key) = loaded_fixture_with_telegram(Some("@rmch_emergency"));
        let pack = LoadedCityPack::load(&envelope, &files, &public_key).expect("valid pack");
        assert_eq!(
            pack.hospitals
                .first()
                .and_then(|hospital| hospital.telegram_chat_id.as_deref()),
            Some("@rmch_emergency")
        );
    }

    #[test]
    fn a_malformed_telegram_endpoint_fails_the_pack_closed() {
        // Same treatment as an undialable `sms_number`. An endpoint the relay would reject
        // must not survive to become a button that silently sends nothing, because a
        // hospital that was never asked looks exactly like a hospital that did not answer.
        for rejected in ["@a", "not a chat", "+8801700000000", "12 34567"] {
            let (envelope, files, public_key) = loaded_fixture_with_telegram(Some(rejected));
            assert_eq!(
                LoadedCityPack::load(&envelope, &files, &public_key),
                Err(PackError::InvalidPayload("hospitals.json".to_owned())),
                "{rejected:?} must fail the pack closed"
            );
        }
    }
}
