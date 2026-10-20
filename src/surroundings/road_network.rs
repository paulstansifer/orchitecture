//! The road network over the surroundings map.
//!
//! Farms and travelers move resources into the city along a graph built from
//! the Voronoi cell borders: the graph's nodes are the polygon corners and its
//! edges are the borders between corners. A resource from a farm may teleport
//! freely to any corner of its own polygon (except the city corner), then
//! travels road edges to the city.
//!
//! Every border is a dirt road with a per-unit-distance cost of
//! [`UNPAVED_FACTOR`]. The player can also build *paved* roads
//! ([`RoadSegment`]s): straight lines between two Voronoi corners that may cut
//! across farms, costing only [`PAVED_FACTOR`] per unit distance. Where a paved
//! road crosses an existing edge, both are split at a new junction node, so
//! traffic can turn there. Routing minimizes `length * factor` summed along the
//! path.

use bevy::prelude::Vec2;
use serde::{Deserialize, Serialize};
use std::collections::BinaryHeap;

/// Two polygon corners within this map-unit distance are the same graph node.
/// Matches `farmstead::ADJACENCY_EPS`.
const NODE_EPS: f32 = 0.01;

/// Cost factor of an ordinary (unpaved) dirt road along a farm border.
pub const UNPAVED_FACTOR: f32 = 3.0;
/// Cost factor of a paved road.
pub const PAVED_FACTOR: f32 = 1.0;
/// Fieldstone needed to pave one map unit of road.
pub const FIELDSTONE_PER_LENGTH: f32 = 1.0;

/// A paved road the player has built (or proposed): a straight line between
/// two Voronoi corners, identified by position so it survives the graph being
/// rebuilt.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoadSegment {
    pub a: Vec2,
    pub b: Vec2,
}

impl RoadSegment {
    pub fn length(&self) -> f32 {
        self.a.distance(self.b)
    }

    /// Fieldstone it costs to pave this segment (at least 1).
    pub fn fieldstone_cost(&self) -> u32 {
        ((self.length() * FIELDSTONE_PER_LENGTH).ceil() as u32).max(1)
    }

    /// Distance from `p` to the nearest point on this segment.
    pub fn distance_to(&self, p: Vec2) -> f32 {
        segment_dist(p, self.a, self.b)
    }
}

/// Distance from `p` to the nearest point on the segment `a`-`b`.
pub fn segment_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len_sq = ab.dot(ab);
    if len_sq < 1e-6 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

/// Where segments `p0`-`p1` and `q0`-`q1` properly cross, as `(t, u)`
/// parameters along each (`p0 + t * (p1 - p0)`). Touching at (or within a hair
/// of) either segment's endpoints doesn't count, nor do parallel segments.
fn crossing(p0: Vec2, p1: Vec2, q0: Vec2, q1: Vec2) -> Option<(f32, f32)> {
    const PARAM_EPS: f32 = 1e-4;
    let r = p1 - p0;
    let s = q1 - q0;
    let denom = r.perp_dot(s);
    if denom.abs() < 1e-9 {
        return None;
    }
    let qp = q0 - p0;
    let t = qp.perp_dot(s) / denom;
    let u = qp.perp_dot(r) / denom;
    let inside = |x: f32| x > PARAM_EPS && x < 1.0 - PARAM_EPS;
    (inside(t) && inside(u)).then_some((t, u))
}

/// A road between two graph nodes.
pub struct RoadEdge {
    pub a: usize,
    pub b: usize,
    pub length: f32,
    pub paved: bool,
}

impl RoadEdge {
    pub fn factor(&self) -> f32 {
        if self.paved {
            PAVED_FACTOR
        } else {
            UNPAVED_FACTOR
        }
    }

    fn weight(&self) -> f32 {
        self.length * self.factor()
    }
}

/// The road graph over the farm polygons, plus a cached shortest-path tree
/// rooted at the city.
pub struct RoadNetwork {
    pub nodes: Vec<Vec2>,
    /// `nodes[..corner_count]` are Voronoi corners; any nodes after that are
    /// junctions where a paved road crosses another edge.
    pub corner_count: usize,
    pub edges: Vec<RoadEdge>,
    /// `adjacency[node]` = `[(edge_idx, other_node), ...]`.
    adjacency: Vec<Vec<(usize, usize)>>,
    pub city_node: usize,
    /// Minimum weighted distance from each node to the city.
    dist: Vec<f32>,
    /// For each node, the `(edge_idx, next_node)` step toward the city, or
    /// `None` for the city itself and any unreachable node.
    pred_edge: Vec<Option<(usize, usize)>>,
}

impl RoadNetwork {
    /// Builds the graph from the farm polygons plus the given paved roads,
    /// locates the city node nearest to `circle_pos`, and computes the
    /// city-rooted distance tree.
    pub fn build(polygons: &[Vec<Vec2>], circle_pos: Vec2, paved: &[RoadSegment]) -> Self {
        let mut nodes: Vec<Vec2> = Vec::new();

        let node_of = |p: Vec2, nodes: &mut Vec<Vec2>| -> usize {
            if let Some(i) = nodes.iter().position(|&n| n.distance(p) <= NODE_EPS) {
                i
            } else {
                nodes.push(p);
                nodes.len() - 1
            }
        };

        // Dedup edges by their unordered node pair.
        let mut seen_edges: std::collections::HashSet<(usize, usize)> =
            std::collections::HashSet::new();
        let mut edges: Vec<RoadEdge> = Vec::new();

        for polygon in polygons {
            let n = polygon.len();
            for i in 0..n {
                let a = node_of(polygon[i], &mut nodes);
                let b = node_of(polygon[(i + 1) % n], &mut nodes);
                if a == b {
                    continue;
                }
                let key = if a < b { (a, b) } else { (b, a) };
                if seen_edges.insert(key) {
                    edges.push(RoadEdge {
                        a,
                        b,
                        length: nodes[a].distance(nodes[b]),
                        paved: false,
                    });
                }
            }
        }

        let city_node = nodes
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                a.distance_squared(circle_pos)
                    .total_cmp(&b.distance_squared(circle_pos))
            })
            .map(|(i, _)| i)
            .unwrap_or(0);

        let corner_count = nodes.len();
        let mut roads = RoadNetwork {
            nodes,
            corner_count,
            edges,
            adjacency: Vec::new(),
            city_node,
            dist: Vec::new(),
            pred_edge: Vec::new(),
        };
        for seg in paved {
            roads.add_paved(*seg);
        }
        roads.rebuild_adjacency();
        roads.recompute_dist();
        roads
    }

    /// Node index at `p`, adding a new node if there isn't one already.
    fn node_or_insert(&mut self, p: Vec2) -> usize {
        self.node_at(p).unwrap_or_else(|| {
            self.nodes.push(p);
            self.nodes.len() - 1
        })
    }

    /// Adds a paved road along `seg`, splitting every edge it crosses at a
    /// junction node. Leaves adjacency and distances stale; `build` refreshes
    /// both once every segment is in.
    fn add_paved(&mut self, seg: RoadSegment) {
        let a = self.node_or_insert(seg.a);
        let b = self.node_or_insert(seg.b);
        if a == b {
            return;
        }

        // A straight edge can cross a straight segment at most once, so
        // checking only the edges that existed beforehand (not the second
        // halves split off below) finds every crossing.
        let mut junctions: Vec<(f32, usize)> = Vec::new();
        for i in 0..self.edges.len() {
            let (ea, eb) = (self.edges[i].a, self.edges[i].b);
            let Some((t, u)) = crossing(seg.a, seg.b, self.nodes[ea], self.nodes[eb]) else {
                continue;
            };
            let point = self.nodes[ea].lerp(self.nodes[eb], u);
            let junction = self.node_or_insert(point);
            let paved = self.edges[i].paved;
            self.edges[i] = RoadEdge {
                a: ea,
                b: junction,
                length: self.nodes[ea].distance(point),
                paved,
            };
            self.edges.push(RoadEdge {
                a: junction,
                b: eb,
                length: point.distance(self.nodes[eb]),
                paved,
            });
            junctions.push((t, junction));
        }
        junctions.sort_by(|x, y| x.0.total_cmp(&y.0));

        let chain: Vec<usize> = std::iter::once(a)
            .chain(junctions.into_iter().map(|(_, n)| n))
            .chain(std::iter::once(b))
            .collect();
        for pair in chain.windows(2) {
            if pair[0] != pair[1] {
                self.edges.push(RoadEdge {
                    a: pair[0],
                    b: pair[1],
                    length: self.nodes[pair[0]].distance(self.nodes[pair[1]]),
                    paved: true,
                });
            }
        }
    }

    fn rebuild_adjacency(&mut self) {
        self.adjacency = vec![Vec::new(); self.nodes.len()];
        for (idx, edge) in self.edges.iter().enumerate() {
            self.adjacency[edge.a].push((idx, edge.b));
            self.adjacency[edge.b].push((idx, edge.a));
        }
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// The cached weighted distance from `node` to the city.
    pub fn dist_to_city(&self, node: usize) -> f32 {
        self.dist[node]
    }

    /// Whether `node` touches a paved road (or is the city itself), i.e. a
    /// paved road can be extended from it.
    pub fn on_paved_network(&self, node: usize) -> bool {
        node == self.city_node
            || self.adjacency[node]
                .iter()
                .any(|&(edge, _)| self.edges[edge].paved)
    }

    /// Recomputes the shortest-path tree from the city over `length * factor`
    /// weights (Dijkstra).
    fn recompute_dist(&mut self) {
        let n = self.nodes.len();
        self.dist = vec![f32::INFINITY; n];
        self.pred_edge = vec![None; n];
        if self.city_node >= n {
            return;
        }
        self.dist[self.city_node] = 0.0;

        // Min-heap on (distance, node) via Reverse-ordered f32 keys.
        let mut heap: BinaryHeap<(std::cmp::Reverse<OrdF32>, usize)> = BinaryHeap::new();
        heap.push((std::cmp::Reverse(OrdF32(0.0)), self.city_node));

        while let Some((std::cmp::Reverse(OrdF32(d)), node)) = heap.pop() {
            if d > self.dist[node] {
                continue;
            }
            for &(edge_idx, next) in &self.adjacency[node] {
                let nd = d + self.edges[edge_idx].weight();
                if nd < self.dist[next] {
                    self.dist[next] = nd;
                    // `pred_edge[next]` steps from `next` toward the city (`node`).
                    self.pred_edge[next] = Some((edge_idx, node));
                    heap.push((std::cmp::Reverse(OrdF32(nd)), next));
                }
            }
        }
    }

    /// Snaps an arbitrary map position to the nearest graph node.
    pub fn nearest_node(&self, pos: Vec2) -> usize {
        self.nodes
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.distance_squared(pos).total_cmp(&b.distance_squared(pos)))
            .map(|(i, _)| i)
            .unwrap_or(self.city_node)
    }

    /// The Voronoi corner nearest to `pos`, if one lies within `max_dist`.
    pub fn nearest_corner_within(&self, pos: Vec2, max_dist: f32) -> Option<usize> {
        (0..self.corner_count)
            .map(|i| (i, self.nodes[i].distance(pos)))
            .filter(|&(_, d)| d <= max_dist)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// The graph node index for a given corner position, if it exists.
    fn node_at(&self, pos: Vec2) -> Option<usize> {
        self.nodes.iter().position(|&n| n.distance(pos) <= NODE_EPS)
    }

    /// Cheapest delivery from a farm: the resource teleports free to whichever
    /// of the farm's corners (excluding the city corner) is cheapest to reach
    /// the city from, then follows roads in. Returns `(weight, corner_node)`.
    pub fn farm_delivery(&self, polygon: &[Vec2]) -> Option<(f32, usize)> {
        polygon
            .iter()
            .filter_map(|&corner| self.node_at(corner))
            .filter(|&node| node != self.city_node)
            .filter(|&node| self.dist[node].is_finite())
            .map(|node| (self.dist[node], node))
            .min_by(|(a, _), (b, _)| a.total_cmp(b))
    }

    /// Edge indices along the cheapest route from `from_node` to the city.
    pub fn path_edges(&self, from_node: usize) -> Vec<usize> {
        let mut edges = Vec::new();
        let mut node = from_node;
        while let Some((edge_idx, next)) = self.pred_edge[node] {
            edges.push(edge_idx);
            node = next;
            if node == self.city_node {
                break;
            }
        }
        edges
    }

    /// Node positions along the cheapest route from `from_node` to the city,
    /// starting at `from_node` and ending at the city.
    pub fn path_points(&self, from_node: usize) -> Vec<Vec2> {
        let mut points = vec![self.nodes[from_node]];
        let mut node = from_node;
        while let Some((_, next)) = self.pred_edge[node] {
            points.push(self.nodes[next]);
            node = next;
            if node == self.city_node {
                break;
            }
        }
        points
    }
}

/// A totally-ordered wrapper for the finite, non-negative `f32` distances used
/// as Dijkstra keys.
#[derive(Clone, Copy, PartialEq)]
struct OrdF32(f32);
impl Eq for OrdF32 {}
impl PartialOrd for OrdF32 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for OrdF32 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two unit squares side by side. The city sits at a shared corner; a farm
    /// occupies the right square. Its cheapest delivery must skip the city
    /// corner (free teleport is disallowed there) and take a length-1 road edge.
    fn two_square_farm() -> (Vec<Vec<Vec2>>, Vec<Vec2>) {
        let city = Vec2::new(1.0, 0.0);
        let left = vec![
            Vec2::new(0.0, 0.0),
            city,
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ];
        let farm = vec![
            city,
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 1.0),
            Vec2::new(1.0, 1.0),
        ];
        (vec![left, farm.clone()], farm)
    }

    #[test]
    fn delivery_skips_city_corner_and_uses_a_road() {
        let (polygons, farm) = two_square_farm();
        let roads = RoadNetwork::build(&polygons, Vec2::new(1.0, 0.0), &[]);

        // The farm shares the city corner but must not deliver from it; the
        // cheapest legal corner is (1,1) or (2,0), one unpaved unit-length edge away.
        let (weight, corner) = roads.farm_delivery(&farm).expect("a delivery route");
        assert_ne!(corner, roads.city_node);
        assert!(
            (weight - UNPAVED_FACTOR).abs() < 1e-4,
            "weight was {weight}"
        );
        assert_eq!(roads.path_edges(corner).len(), 1);
    }

    #[test]
    fn a_paved_diagonal_forms_a_junction_and_shortens_routes() {
        // A 2x2 block of unit squares; the city is the bottom-left corner.
        let sq = |x: f32, y: f32| {
            vec![
                Vec2::new(x, y),
                Vec2::new(x + 1.0, y),
                Vec2::new(x + 1.0, y + 1.0),
                Vec2::new(x, y + 1.0),
            ]
        };
        let polygons = vec![sq(0.0, 0.0), sq(1.0, 0.0), sq(0.0, 1.0), sq(1.0, 1.0)];
        let city = Vec2::new(0.0, 0.0);
        let far = Vec2::new(2.0, 1.0);

        let unpaved = RoadNetwork::build(&polygons, city, &[]);
        let far_node = unpaved.nearest_node(far);
        assert!((unpaved.dist_to_city(far_node) - 3.0 * UNPAVED_FACTOR).abs() < 1e-4);
        assert!(!unpaved.on_paved_network(far_node));

        // Paving (0,0)-(2,1) cuts across the bottom-left square's interior
        // and crosses the border x=1 at (1, 0.5), splitting it there.
        let seg = RoadSegment { a: city, b: far };
        let paved = RoadNetwork::build(&polygons, city, &[seg]);
        assert_eq!(paved.nodes.len(), unpaved.nodes.len() + 1, "one junction");
        let junction = paved.nearest_node(Vec2::new(1.0, 0.5));
        assert!(paved.nodes[junction].distance(Vec2::new(1.0, 0.5)) < 1e-4);
        assert!(paved.on_paved_network(junction));

        let far_node = paved.nearest_node(far);
        assert!(paved.on_paved_network(far_node));
        assert!(
            (paved.dist_to_city(far_node) - seg.length() * PAVED_FACTOR).abs() < 1e-4,
            "the paved diagonal is now the cheapest route"
        );
        // The split border still connects (1,0) to (1,1) via the junction.
        let top = paved.nearest_node(Vec2::new(1.0, 1.0));
        assert!(paved.dist_to_city(top).is_finite());
    }

    #[test]
    fn fieldstone_cost_rounds_up() {
        let seg = RoadSegment {
            a: Vec2::ZERO,
            b: Vec2::new(2.5, 0.0),
        };
        assert_eq!(
            seg.fieldstone_cost(),
            (2.5 * FIELDSTONE_PER_LENGTH).ceil() as u32
        );
    }
}
