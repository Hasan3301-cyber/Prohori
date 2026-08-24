//! Fail-closed offline road routing.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

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
                let Some(edge_cost) = edge_cost(edge, conditions.get(&edge.id), profile) else {
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

fn edge_cost(edge: &Edge, condition: Option<&Condition>, profile: &RouteProfile) -> Option<u64> {
    if edge.width_millimetres < profile.vehicle_width_millimetres
        || edge.clearance_millimetres < profile.vehicle_height_millimetres
    {
        return None;
    }
    let condition = condition?;
    if !condition.is_fresh(profile.now_epoch_seconds) {
        return None;
    }
    match condition.status {
        RoadStatus::Open => Some(u64::from(edge.seconds)),
        RoadStatus::Blocked | RoadStatus::Unknown => None,
        RoadStatus::Flooded
            if profile.permit_flooded_origin_zone && edge.zone == profile.patient_zone =>
        {
            Some(u64::from(edge.seconds).saturating_mul(12))
        }
        RoadStatus::Flooded => None,
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
}
