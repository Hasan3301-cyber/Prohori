//! Fail-closed offline road routing.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoadStatus {
    Open,
    Flooded,
    Blocked,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Condition {
    pub status: RoadStatus,
    pub source: String,
    pub observed_at_epoch_seconds: u64,
    pub stale_after_seconds: u64,
}

impl Condition {
    #[must_use]
    pub fn is_fresh(&self, now: u64) -> bool {
        now.saturating_sub(self.observed_at_epoch_seconds) <= self.stale_after_seconds
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: u32,
    pub latitude: f64,
    pub longitude: f64,
    pub zone: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub id: u32,
    pub from: u32,
    pub to: u32,
    pub seconds: u32,
    pub width_millimetres: u32,
    pub clearance_millimetres: u32,
    pub zone: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteProfile {
    pub now_epoch_seconds: u64,
    pub vehicle_width_millimetres: u32,
    pub vehicle_height_millimetres: u32,
    pub patient_zone: String,
    /// Set only when the caller has established there is no dry exit from the zone.
    pub permit_flooded_origin_zone: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub edge_ids: Vec<u32>,
    pub estimated_seconds: u64,
}

/// Why one road segment cannot be used.
///
/// [`edge_cost`] used to return `Option<u64>`, which collapsed every one of these into a bare
/// `None`. The router still chose correctly, but it could not say *why* — so a family was told
/// only that no route existed, never that the short way was under water. Naming the veto is
/// what lets a hospital be rejected out loud instead of silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeVeto {
    /// The vehicle is wider than the carriageway.
    TooNarrow,
    /// The vehicle is taller than the overhead clearance.
    TooLow,
    /// No one has filed a condition for this segment at all.
    NoReport,
    /// A condition exists but is older than its own `stale_after_seconds`.
    Stale,
    Blocked,
    Flooded,
    /// Reported, fresh, and explicitly not known to be passable.
    Unknown,
}

impl EdgeVeto {
    /// Report order. A blocked road is more useful to hear about than a missing report,
    /// because it is certain and it tells the reader which way not to set off.
    const fn severity(self) -> u8 {
        match self {
            Self::Blocked => 0,
            Self::Flooded => 1,
            Self::TooNarrow => 2,
            Self::TooLow => 3,
            Self::Stale => 4,
            Self::NoReport => 5,
            Self::Unknown => 6,
        }
    }

    /// One clause, lowercase and unpunctuated so [`CorridorVerdict::reason`] can join it.
    ///
    /// Deliberately short words: this is read by a frightened person on a phone, and it is
    /// held to the same plain-language standard as the first-aid corpus.
    #[must_use]
    pub fn phrase(self, zone: &str) -> String {
        match self {
            Self::TooNarrow => format!("the road through {zone} is too narrow for this vehicle"),
            Self::TooLow => format!("the way through {zone} is too low for this vehicle"),
            Self::NoReport => format!("no one has reported on the road through {zone}"),
            Self::Stale => format!("the road news for {zone} is too old to trust"),
            Self::Blocked => format!("the road through {zone} is blocked"),
            Self::Flooded => format!("the road through {zone} is under water"),
            Self::Unknown => format!("no one knows if the road through {zone} is open"),
        }
    }
}

/// The graded verdict on the corridor a traveller would naturally take.
///
/// Produced by [`Graph::explain`], which searches with every veto lifted. An empty `vetoes`
/// list therefore means the natural corridor is genuinely usable, and a non-empty one means
/// the fast way is shut — whether or not some longer way survives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorridorVerdict {
    /// Zones along the corridor, in travel order, without consecutive repeats.
    pub zones: Vec<String>,
    /// Distinct `(zone, veto)` pairs, most severe first.
    pub vetoes: Vec<(String, EdgeVeto)>,
    /// How long the corridor would take if every road on it were open.
    ///
    /// The floor a real route is measured against, so "how much longer is the detour" is a
    /// subtraction rather than a guess. Zero when no corridor exists at all.
    pub fastest_seconds: u64,
}

impl CorridorVerdict {
    /// Swap zone ids for the names residents use, where the pack supplies one.
    ///
    /// An [`Edge`] carries the zone id (`ruet`) because that is what routing needs. A sentence
    /// read by a frightened person needs "RUET corridor". Unmapped ids are left as they are
    /// rather than dropped: a clumsy zone name is recoverable, a reason with no place in it
    /// is not.
    #[must_use]
    pub fn with_zone_names(mut self, names: &BTreeMap<String, String>) -> Self {
        let resolve = |zone: &String| names.get(zone).unwrap_or(zone).clone();
        self.zones = self.zones.iter().map(resolve).collect();
        self.vetoes = self
            .vetoes
            .iter()
            .map(|(zone, veto)| (resolve(zone), *veto))
            .collect();
        self
    }

    /// A whole sentence naming why this destination was rejected.
    ///
    /// Authored in Rust rather than assembled in Kotlin for the same reason
    /// `FirstAidCard::provenance` is: the UI displays this text and never composes it, so
    /// there is one place where the wording can be reviewed.
    #[must_use]
    pub fn reason(&self) -> String {
        if self.zones.is_empty() {
            return "There is no mapped road from here to this hospital.".to_owned();
        }
        let mut phrases = self
            .vetoes
            .iter()
            .take(2)
            .map(|(zone, veto)| veto.phrase(zone));
        let Some(first) = phrases.next() else {
            return "The roads on the way to this hospital could not be graded.".to_owned();
        };
        let joined = match phrases.next() {
            Some(second) => format!("{first}, and {second}"),
            None => first,
        };
        // Rebuilt through the iterator rather than sliced: `indexing_slicing` is denied, and a
        // byte slice would split a multi-byte first character.
        let mut characters = joined.chars();
        let mut sentence = match characters.next() {
            Some(initial) => initial.to_uppercase().collect::<String>() + characters.as_str(),
            None => joined,
        };
        sentence.push('.');
        if self.vetoes.len() > 2 {
            sentence.push_str(" Other roads on the way are shut too.");
        }
        sentence
    }

    /// What to say about a hospital the router *did* reach, when it had to go around something.
    ///
    /// The chosen way being open is true but incomplete: a caller told only that reads it as
    /// "the roads are fine" and cannot tell that the fast way is gone. Returning `None` when
    /// nothing was avoided keeps the ordinary case from carrying a warning it has not earned.
    #[must_use]
    pub fn detour_reason(&self, detour_seconds: u64) -> Option<String> {
        let (zone, veto) = self.vetoes.first()?;
        let mut sentence = format!(
            "The short way is closed — {}. This way goes around it",
            veto.phrase(zone)
        );
        // Minutes, not seconds: a number here is read aloud to someone who is counting.
        let minutes = detour_seconds / 60;
        if minutes > 0 {
            sentence.push_str(&format!(" and takes about {minutes} minutes longer"));
        }
        sentence.push('.');
        Some(sentence)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QueueEntry {
    cost: u64,
    node: u32,
}
impl Ord for QueueEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .cmp(&self.cost)
            .then_with(|| self.node.cmp(&other.node))
    }
}
impl PartialOrd for QueueEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Graph {
    #[must_use]
    pub fn route(
        &self,
        start: u32,
        goal: u32,
        conditions: &HashMap<u32, Condition>,
        profile: &RouteProfile,
    ) -> Option<Route> {
        self.shortest(start, goal, |edge| {
            edge_cost(edge, conditions.get(&edge.id), profile).ok()
        })
    }

    /// Grade the corridor a traveller would take if nothing were checked.
    ///
    /// Call this only after [`Self::route`] has returned `None`. It re-runs the search with
    /// every veto lifted — raw segment times only — which recovers the short obvious way, then
    /// grades each segment of it so the refusal can be explained in the reader's own terms.
    /// Finding the corridor first, rather than reporting every impassable edge in the city, is
    /// what keeps the reason about the journey the reader was going to make.
    #[must_use]
    pub fn explain(
        &self,
        start: u32,
        goal: u32,
        conditions: &HashMap<u32, Condition>,
        profile: &RouteProfile,
    ) -> CorridorVerdict {
        let Some(corridor) = self.shortest(start, goal, |edge| Some(u64::from(edge.seconds)))
        else {
            return CorridorVerdict {
                zones: Vec::new(),
                vetoes: Vec::new(),
                fastest_seconds: 0,
            };
        };
        let by_id: HashMap<u32, &Edge> = self.edges.iter().map(|edge| (edge.id, edge)).collect();
        let mut zones: Vec<String> = Vec::new();
        let mut vetoes: Vec<(String, EdgeVeto)> = Vec::new();
        for edge_id in &corridor.edge_ids {
            let Some(edge) = by_id.get(edge_id) else {
                continue;
            };
            if zones.last() != Some(&edge.zone) {
                zones.push(edge.zone.clone());
            }
            if let Err(veto) = edge_cost(edge, conditions.get(&edge.id), profile) {
                let already = vetoes
                    .iter()
                    .any(|(zone, seen)| zone == &edge.zone && *seen == veto);
                if !already {
                    vetoes.push((edge.zone.clone(), veto));
                }
            }
        }
        vetoes.sort_by(|(left_zone, left), (right_zone, right)| {
            left.severity()
                .cmp(&right.severity())
                .then_with(|| left_zone.cmp(right_zone))
        });
        CorridorVerdict {
            zones,
            vetoes,
            fastest_seconds: corridor.estimated_seconds,
        }
    }

    /// Dijkstra over `cost`, which returns `None` for a segment that may not be used.
    ///
    /// Shared by [`Self::route`] and [`Self::explain`] so the two can never disagree about
    /// the shape of the graph — only about which segments are allowed.
    fn shortest<F>(&self, start: u32, goal: u32, cost_of: F) -> Option<Route>
    where
        F: Fn(&Edge) -> Option<u64>,
    {
        if start == goal {
            return Some(Route {
                edge_ids: Vec::new(),
                estimated_seconds: 0,
            });
        }
        let mut outgoing: HashMap<u32, Vec<&Edge>> = HashMap::new();
        for edge in &self.edges {
            outgoing.entry(edge.from).or_default().push(edge);
        }
        for edges in outgoing.values_mut() {
            edges.sort_by_key(|edge| edge.id);
        }
        let mut distances = HashMap::from([(start, 0_u64)]);
        let mut previous: HashMap<u32, (u32, u32)> = HashMap::new();
        let mut queue = BinaryHeap::from([QueueEntry {
            cost: 0,
            node: start,
        }]);

        while let Some(QueueEntry { cost, node }) = queue.pop() {
            if node == goal {
                break;
            }
            if distances.get(&node).is_some_and(|known| cost > *known) {
                continue;
            }
            for edge in outgoing.get(&node).into_iter().flatten() {
                let Some(edge_cost) = cost_of(edge) else {
                    continue;
                };
                let next = cost.saturating_add(edge_cost);
                if next < distances.get(&edge.to).copied().unwrap_or(u64::MAX) {
                    distances.insert(edge.to, next);
                    previous.insert(edge.to, (node, edge.id));
                    queue.push(QueueEntry {
                        cost: next,
                        node: edge.to,
                    });
                }
            }
        }
        let total = distances.get(&goal).copied()?;
        let mut cursor = goal;
        let mut edges = Vec::new();
        while cursor != start {
            let (parent, edge) = previous.get(&cursor).copied()?;
            edges.push(edge);
            cursor = parent;
        }
        edges.reverse();
        Some(Route {
            edge_ids: edges,
            estimated_seconds: total,
        })
    }
}

/// The cost of one segment, or the named reason it may not be used.
fn edge_cost(
    edge: &Edge,
    condition: Option<&Condition>,
    profile: &RouteProfile,
) -> Result<u64, EdgeVeto> {
    if edge.width_millimetres < profile.vehicle_width_millimetres {
        return Err(EdgeVeto::TooNarrow);
    }
    if edge.clearance_millimetres < profile.vehicle_height_millimetres {
        return Err(EdgeVeto::TooLow);
    }
    let condition = condition.ok_or(EdgeVeto::NoReport)?;
    if !condition.is_fresh(profile.now_epoch_seconds) {
        return Err(EdgeVeto::Stale);
    }
    match condition.status {
        RoadStatus::Open => Ok(u64::from(edge.seconds)),
        RoadStatus::Blocked => Err(EdgeVeto::Blocked),
        RoadStatus::Unknown => Err(EdgeVeto::Unknown),
        RoadStatus::Flooded
            if profile.permit_flooded_origin_zone && edge.zone == profile.patient_zone =>
        {
            Ok(u64::from(edge.seconds).saturating_mul(12))
        }
        RoadStatus::Flooded => Err(EdgeVeto::Flooded),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn graph() -> Graph {
        Graph {
            nodes: vec![],
            edges: vec![
                Edge {
                    id: 1,
                    from: 1,
                    to: 2,
                    seconds: 10,
                    width_millimetres: 3000,
                    clearance_millimetres: 3000,
                    zone: "A".into(),
                },
                Edge {
                    id: 2,
                    from: 2,
                    to: 3,
                    seconds: 10,
                    width_millimetres: 3000,
                    clearance_millimetres: 3000,
                    zone: "B".into(),
                },
                Edge {
                    id: 3,
                    from: 1,
                    to: 3,
                    seconds: 40,
                    width_millimetres: 3000,
                    clearance_millimetres: 3000,
                    zone: "A".into(),
                },
            ],
        }
    }
    fn profile() -> RouteProfile {
        RouteProfile {
            now_epoch_seconds: 100,
            vehicle_width_millimetres: 2000,
            vehicle_height_millimetres: 2500,
            patient_zone: "A".into(),
            permit_flooded_origin_zone: false,
        }
    }
    fn open() -> Condition {
        Condition {
            status: RoadStatus::Open,
            source: "snapshot".into(),
            observed_at_epoch_seconds: 90,
            stale_after_seconds: 20,
        }
    }

    #[test]
    fn chooses_short_open_route() {
        let c = HashMap::from([(1, open()), (2, open()), (3, open())]);
        assert_eq!(
            graph().route(1, 3, &c, &profile()).map(|r| r.edge_ids),
            Some(vec![1, 2])
        );
    }
    #[test]
    fn blocked_always_vetoes() {
        let mut c = HashMap::from([(1, open()), (2, open()), (3, open())]);
        if let Some(condition) = c.get_mut(&2) {
            condition.status = RoadStatus::Blocked;
        }
        assert_eq!(
            graph().route(1, 3, &c, &profile()).map(|r| r.edge_ids),
            Some(vec![3])
        );
    }
    #[test]
    fn missing_or_stale_is_not_passable() {
        let mut c = HashMap::from([(1, open()), (2, open())]);
        if let Some(condition) = c.get_mut(&2) {
            condition.observed_at_epoch_seconds = 0;
        }
        assert_eq!(graph().route(1, 3, &c, &profile()), None);
    }
    #[test]
    fn flood_only_allowed_in_origin_zone_when_explicit() {
        let mut c = HashMap::from([(1, open()), (2, open()), (3, open())]);
        if let Some(condition) = c.get_mut(&3) {
            condition.status = RoadStatus::Flooded;
        }
        let mut p = profile();
        assert_eq!(
            graph().route(1, 3, &c, &p).map(|r| r.edge_ids),
            Some(vec![1, 2])
        );
        p.permit_flooded_origin_zone = true;
        if let Some(condition) = c.get_mut(&1) {
            condition.status = RoadStatus::Blocked;
        }
        assert_eq!(
            graph().route(1, 3, &c, &p).map(|r| r.edge_ids),
            Some(vec![3])
        );
    }

    // ---------------------------------------------------------------------
    // Failure 1: the router has to say why, not only decide correctly.
    // ---------------------------------------------------------------------

    #[test]
    fn a_blocked_corridor_names_the_blocked_zone() {
        // Both ways out are shut, so `route` fails and `explain` has to account for it.
        let mut c = HashMap::from([(1, open()), (2, open()), (3, open())]);
        for edge_id in [1_u32, 3] {
            if let Some(condition) = c.get_mut(&edge_id) {
                condition.status = RoadStatus::Blocked;
            }
        }
        let g = graph();
        assert_eq!(g.route(1, 3, &c, &profile()), None, "route must fail first");
        let verdict = g.explain(1, 3, &c, &profile());
        // The short obvious way is 1 -> 2 (20s), not the 40s single hop.
        assert_eq!(verdict.zones, vec!["A".to_owned(), "B".to_owned()]);
        assert_eq!(verdict.vetoes, vec![("A".to_owned(), EdgeVeto::Blocked)]);
        assert_eq!(verdict.reason(), "The road through A is blocked.");
    }

    #[test]
    fn a_hospital_reached_the_long_way_round_still_says_the_short_way_is_shut() {
        // Only the fast hop is blocked. `route` succeeds on the 40s single hop, so nothing is
        // rejected — but saying "the roads are open" here would hide the blockage entirely.
        let mut c = HashMap::from([(1, open()), (2, open()), (3, open())]);
        if let Some(condition) = c.get_mut(&1) {
            condition.status = RoadStatus::Blocked;
        }
        let g = graph();
        // Asserted as a whole value rather than unwrapped: `expect` is denied in this crate,
        // and the route's own numbers are what the detour is measured from.
        assert_eq!(
            g.route(1, 3, &c, &profile()),
            Some(Route {
                edge_ids: vec![3],
                estimated_seconds: 40,
            }),
            "the long way must survive for this test to be about a detour"
        );

        let verdict = g.explain(1, 3, &c, &profile());
        assert_eq!(
            verdict.fastest_seconds, 20,
            "the veto-free corridor is the 1 -> 2 -> 3 pair"
        );
        assert_eq!(
            verdict.detour_reason(40 - verdict.fastest_seconds),
            Some(
                "The short way is closed — the road through A is blocked. This way goes around it."
                    .to_owned()
            ),
            "a detour under a minute must not claim a delay it cannot measure"
        );
    }

    #[test]
    fn a_long_detour_says_how_much_longer_in_minutes() {
        let mut c = HashMap::from([(1, open()), (2, open()), (3, open())]);
        if let Some(condition) = c.get_mut(&1) {
            condition.status = RoadStatus::Flooded;
        }
        let verdict = graph().explain(1, 3, &c, &profile());
        assert_eq!(
            verdict.detour_reason(600),
            Some(
                "The short way is closed — the road through A is under water. \
                 This way goes around it and takes about 10 minutes longer."
                    .to_owned()
            ),
            "flooding must not be reported as a blockage, and a measurable delay is worth saying"
        );
    }

    #[test]
    fn an_untroubled_route_is_given_no_detour_warning_to_carry() {
        let c = HashMap::from([(1, open()), (2, open()), (3, open())]);
        let verdict = graph().explain(1, 3, &c, &profile());
        assert!(verdict.vetoes.is_empty());
        assert_eq!(
            verdict.detour_reason(0),
            None,
            "an ordinary drive must not be dressed up as a survived obstacle"
        );
    }

    #[test]
    fn a_stale_report_is_named_as_stale_and_not_as_blocked() {
        // The distinction matters: "blocked" is knowledge, "too old to trust" is the absence
        // of it, and telling a family the first when you mean the second is a lie.
        let mut c = HashMap::from([(1, open()), (2, open()), (3, open())]);
        for edge_id in [1_u32, 3] {
            if let Some(condition) = c.get_mut(&edge_id) {
                condition.observed_at_epoch_seconds = 0;
            }
        }
        let g = graph();
        assert_eq!(g.route(1, 3, &c, &profile()), None);
        assert_eq!(
            g.explain(1, 3, &c, &profile()).reason(),
            "The road news for A is too old to trust."
        );
    }

    #[test]
    fn an_unreported_road_is_not_described_as_a_road_that_is_open() {
        let c = HashMap::from([(2, open())]);
        let g = graph();
        assert_eq!(g.route(1, 3, &c, &profile()), None);
        assert_eq!(
            g.explain(1, 3, &c, &profile()).reason(),
            "No one has reported on the road through A."
        );
    }

    #[test]
    fn a_vehicle_too_big_for_the_lane_is_told_that_and_not_that_the_road_is_shut() {
        let c = HashMap::from([(1, open()), (2, open()), (3, open())]);
        let wide = RouteProfile {
            vehicle_width_millimetres: 4000,
            ..profile()
        };
        let g = graph();
        assert_eq!(g.route(1, 3, &c, &wide), None);
        assert_eq!(
            g.explain(1, 3, &c, &wide).reason(),
            "The road through A is too narrow for this vehicle, \
             and the road through B is too narrow for this vehicle."
        );
    }

    #[test]
    fn an_unreachable_hospital_says_there_is_no_road_rather_than_inventing_one() {
        let c = HashMap::from([(1, open()), (2, open()), (3, open())]);
        let verdict = graph().explain(1, 99, &c, &profile());
        assert!(verdict.zones.is_empty());
        assert_eq!(
            verdict.reason(),
            "There is no mapped road from here to this hospital."
        );
    }

    #[test]
    fn the_worst_reason_is_reported_first_and_the_rest_are_summarised() {
        // Built directly: three vetoes need a longer corridor than the fixture has, and the
        // ordering contract belongs to CorridorVerdict rather than to any one graph.
        let verdict = CorridorVerdict {
            zones: vec!["A".to_owned(), "B".to_owned(), "C".to_owned()],
            vetoes: vec![
                ("A".to_owned(), EdgeVeto::Blocked),
                ("B".to_owned(), EdgeVeto::Flooded),
                ("C".to_owned(), EdgeVeto::NoReport),
            ],
            fastest_seconds: 60,
        };
        assert_eq!(
            verdict.reason(),
            "The road through A is blocked, and the road through B is under water. \
             Other roads on the way are shut too."
        );
    }

    #[test]
    fn every_veto_has_its_own_words() {
        let all = [
            EdgeVeto::TooNarrow,
            EdgeVeto::TooLow,
            EdgeVeto::NoReport,
            EdgeVeto::Stale,
            EdgeVeto::Blocked,
            EdgeVeto::Flooded,
            EdgeVeto::Unknown,
        ];
        let phrases: Vec<String> = all.iter().map(|veto| veto.phrase("Motihar")).collect();
        let distinct: std::collections::BTreeSet<&String> = phrases.iter().collect();
        assert_eq!(distinct.len(), all.len(), "two vetoes read the same");
        for phrase in &phrases {
            assert!(
                phrase.contains("Motihar"),
                "{phrase} does not name the zone"
            );
            // No digits: a road reason is read beside medical guidance that forbids them, and
            // a number here would invite one there.
            assert!(
                !phrase.chars().any(char::is_numeric),
                "{phrase} contains a digit"
            );
        }
    }
}
