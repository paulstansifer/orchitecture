//! The road network over the surroundings map.
//!
//! Farms and travelers move resources into the city along a graph built from
//! the Voronoi cell borders: the graph's nodes are the polygon corners and its
//! edges are the borders between corners. A resource from a farm may teleport
//! freely to any corner of its own polygon (except the city corner), then
//! travels road edges to the city.
//!
//! Each edge has a per-unit-distance cost `factor` that starts at a plain
//! field's `5.0` and drops toward a well-worn dirt road's `3.0` as the edge is
//! used: after [`FULL_DEVELOPMENT_TRIPS`] trips it reaches `3.0`. An edge can
//! also be *paved* (spending fieldstone, see `farmstead::pave_fieldstone_routes`),
//! which pulls its factor further down toward [`PAVED_FACTOR`] in proportion to
//! how much of its length is paved. Routing minimizes `length * factor` summed
//! along the path.

use bevy::prelude::Vec2;
use std::collections::BinaryHeap;

/// Two polygon corners within this map-unit distance are the same graph node.
/// Matches `farmstead::ADJACENCY_EPS`.
const NODE_EPS: f32 = 0.01;

/// Cost factor of a plain field (an undeveloped edge).
pub const FIELD_FACTOR: f32 = 5.0;
/// Cost factor of a fully-developed dirt road.
pub const ROAD_FACTOR: f32 = 3.0;
/// Trips needed for a field to develop all the way into a `ROAD_FACTOR` road.
pub const FULL_DEVELOPMENT_TRIPS: u32 = 10;
/// Inter-farm edges start here: a half-developed dirt road (factor `4.0`).
pub const INITIAL_TRIPS: u32 = 5;

/// Cost factor of a fully-paved road — cheaper than even a well-worn dirt road.
pub const PAVED_FACTOR: f32 = 1.0;
/// How much length one unit of fieldstone fully paves.
pub const PAVED_LENGTH_PER_FIELDSTONE: f32 = 0.1;

/// Cost factor for an edge worn by `trips` crossings: `5.0` at 0 trips, falling
/// linearly to `3.0` at `FULL_DEVELOPMENT_TRIPS`. This is the *unpaved* portion's
/// factor; see [`RoadEdge::factor`] for how paving blends in.
pub fn dirt_factor(trips: u32) -> f32 {
    let t = (trips as f32 / FULL_DEVELOPMENT_TRIPS as f32).min(1.0);
    FIELD_FACTOR - (FIELD_FACTOR - ROAD_FACTOR) * t
}

/// How "developed" an edge's dirt (unpaved) portion is, in `0.0` (field) ..=
/// `1.0` (full dirt road). Drives the brown road line's opacity.
pub fn development(trips: u32) -> f32 {
    ((FIELD_FACTOR - dirt_factor(trips)) / (FIELD_FACTOR - ROAD_FACTOR)).clamp(0.0, 1.0)
}

/// A border between two graph nodes (Voronoi corners).
pub struct RoadEdge {
    pub a: usize,
    pub b: usize,
    pub length: f32,
    pub trips: u32,
    /// Fraction of this edge's length that's been paved, `0.0..=1.0`, growing
    /// from the city-side end (see `farmstead::pave_fieldstone_routes`).
    pub paved: f32,
}

impl RoadEdge {
    /// The edge's effective cost factor: its dirt factor for the unpaved
    /// remainder, pulled toward `PAVED_FACTOR` in proportion to `paved`.
    pub fn factor(&self) -> f32 {
        let dirt = dirt_factor(self.trips);
        dirt + (PAVED_FACTOR - dirt) * self.paved.clamp(0.0, 1.0)
    }

    pub fn development(&self) -> f32 {
        development(self.trips)
    }

    fn weight(&self) -> f32 {
        self.length * self.factor()
    }
}

/// The road graph over the farm polygons, plus a cached shortest-path tree
/// rooted at the city.
pub struct RoadNetwork {
    pub nodes: Vec<Vec2>,
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
    /// Builds the graph topology from the farm polygons and locates the city
    /// node nearest to `circle_pos`. Edge trips are left at 0; the caller seeds
    /// them via [`RoadNetwork::set_trips`]. Does not compute the distance tree —
    /// call [`RoadNetwork::recompute_dist`] once trips are set.
    pub fn build(polygons: &[Vec<Vec2>], circle_pos: Vec2) -> Self {
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
                        trips: 0,
                        paved: 0.0,
                    });
                }
            }
        }

        let mut adjacency = vec![Vec::new(); nodes.len()];
        for (idx, edge) in edges.iter().enumerate() {
            adjacency[edge.a].push((idx, edge.b));
            adjacency[edge.b].push((idx, edge.a));
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

        let node_count = nodes.len();
        RoadNetwork {
            nodes,
            edges,
            adjacency,
            city_node,
            dist: vec![f32::INFINITY; node_count],
            pred_edge: vec![None; node_count],
        }
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Overwrites every edge's trip count (indexed to match [`RoadNetwork::edges`]).
    pub fn set_trips(&mut self, trips: &[u32]) {
        for (edge, &t) in self.edges.iter_mut().zip(trips) {
            edge.trips = t;
        }
    }

    /// The current per-edge trip counts, for persistence.
    pub fn trips(&self) -> Vec<u32> {
        self.edges.iter().map(|e| e.trips).collect()
    }

    pub fn bump_trips(&mut self, edge_idx: usize) {
        self.edges[edge_idx].trips += 1;
    }

    /// Overwrites every edge's paved fraction (indexed to match [`RoadNetwork::edges`]).
    pub fn set_paved(&mut self, paved: &[f32]) {
        for (edge, &p) in self.edges.iter_mut().zip(paved) {
            edge.paved = p;
        }
    }

    /// The current per-edge paved fractions, for persistence.
    pub fn paved(&self) -> Vec<f32> {
        self.edges.iter().map(|e| e.paved).collect()
    }

    /// The cached weighted distance from `node` to the city (after
    /// `recompute_dist`), used to tell which end of an edge is nearer the city.
    pub fn dist_to_city(&self, node: usize) -> f32 {
        self.dist[node]
    }

    /// Spends `fieldstone_budget` paving `edges` toward full pavedness,
    /// closest-to-city edge first, skipping any that are already fully paved.
    /// Returns unused budget (e.g. every candidate was already fully paved).
    pub fn pave_edges(
        &mut self,
        edges: impl IntoIterator<Item = usize>,
        mut fieldstone_budget: f32,
    ) -> f32 {
        let mut candidates: Vec<usize> = edges.into_iter().collect();
        candidates.sort_unstable();
        candidates.dedup();
        candidates.sort_by(|&x, &y| {
            let prox = |idx: usize| {
                let e = &self.edges[idx];
                self.dist[e.a].min(self.dist[e.b])
            };
            prox(x).total_cmp(&prox(y))
        });

        for edge_idx in candidates {
            if fieldstone_budget <= 0.0 {
                break;
            }
            let edge = &mut self.edges[edge_idx];
            if edge.paved >= 1.0 {
                continue;
            }
            let remaining_length = edge.length * (1.0 - edge.paved);
            let cost = remaining_length / PAVED_LENGTH_PER_FIELDSTONE;
            if fieldstone_budget >= cost {
                edge.paved = 1.0;
                fieldstone_budget -= cost;
            } else {
                edge.paved += fieldstone_budget * PAVED_LENGTH_PER_FIELDSTONE / edge.length;
                fieldstone_budget = 0.0;
            }
        }
        fieldstone_budget
    }

    /// Recomputes the shortest-path tree from the city over `length * factor`
    /// weights (Dijkstra). Must run after any trip change before routing reads.
    pub fn recompute_dist(&mut self) {
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

    #[test]
    fn factor_curve() {
        assert_eq!(dirt_factor(0), FIELD_FACTOR); // a plain field
        assert_eq!(dirt_factor(INITIAL_TRIPS), 4.0); // the dirt-road start
        assert_eq!(dirt_factor(FULL_DEVELOPMENT_TRIPS), ROAD_FACTOR); // fully worn in
        assert_eq!(dirt_factor(FULL_DEVELOPMENT_TRIPS + 5), ROAD_FACTOR); // stays at the floor
        assert!((development(INITIAL_TRIPS) - 0.5).abs() < 1e-6);
    }

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
        let mut roads = RoadNetwork::build(&polygons, Vec2::new(1.0, 0.0));
        roads.set_trips(&vec![INITIAL_TRIPS; roads.edge_count()]);
        roads.recompute_dist();

        // The farm shares the city corner but must not deliver from it; the
        // cheapest legal corner is (1,1), one unit-length factor-4.0 edge away.
        let (weight, corner) = roads.farm_delivery(&farm).expect("a delivery route");
        assert_ne!(corner, roads.city_node);
        assert!((weight - 4.0).abs() < 1e-4, "weight was {weight}");
        assert_eq!(roads.path_edges(corner).len(), 1);
    }

    #[test]
    fn trips_lower_the_delivery_cost_to_the_floor() {
        let (polygons, farm) = two_square_farm();
        let mut roads = RoadNetwork::build(&polygons, Vec2::new(1.0, 0.0));
        roads.set_trips(&vec![INITIAL_TRIPS; roads.edge_count()]);
        roads.recompute_dist();

        let before = roads.farm_delivery(&farm).unwrap().0;
        for _ in 0..FULL_DEVELOPMENT_TRIPS {
            let corner = roads.farm_delivery(&farm).unwrap().1;
            for e in roads.path_edges(corner) {
                roads.bump_trips(e);
            }
            roads.recompute_dist();
        }
        let after = roads.farm_delivery(&farm).unwrap().0;
        assert!(after < before);
        assert!((after - ROAD_FACTOR).abs() < 1e-4, "after was {after}"); // length-1 edge at the floor
    }

    #[test]
    fn pave_edges_prioritizes_the_city_end_and_partially_paves() {
        // A 3-square chain: city at (0,0); the far square's cheapest delivery
        // route is two length-1 edges back to the city.
        let city = Vec2::new(0.0, 0.0);
        let sq = |x: f32| {
            vec![
                Vec2::new(x, 0.0),
                Vec2::new(x + 1.0, 0.0),
                Vec2::new(x + 1.0, 1.0),
                Vec2::new(x, 1.0),
            ]
        };
        let polygons = vec![sq(0.0), sq(1.0), sq(2.0)];
        let mut roads = RoadNetwork::build(&polygons, city);
        roads.recompute_dist();

        let (_, corner) = roads.farm_delivery(&polygons[2]).expect("a delivery route");
        let edges = roads.path_edges(corner);
        assert_eq!(
            edges.len(),
            2,
            "two length-1 edges from the far farm to the city"
        );

        let city_adjacent = *edges
            .iter()
            .min_by(|&&x, &&y| {
                let prox = |i: usize| {
                    let e = &roads.edges[i];
                    roads.dist_to_city(e.a).min(roads.dist_to_city(e.b))
                };
                prox(x).total_cmp(&prox(y))
            })
            .unwrap();
        let farm_side = *edges.iter().find(|&&e| e != city_adjacent).unwrap();

        // Enough fieldstone to fully pave the city-adjacent edge (cost 10.0
        // at 0.1 length-per-fieldstone) plus half of the next one (5.0 more).
        let leftover = roads.pave_edges(edges.clone(), 15.0);
        assert_eq!(leftover, 0.0);
        assert_eq!(
            roads.edges[city_adjacent].paved, 1.0,
            "closest-to-city edge paves first"
        );
        assert!(
            (roads.edges[farm_side].paved - 0.5).abs() < 1e-4,
            "remaining budget partially paves the next edge: {}",
            roads.edges[farm_side].paved
        );

        // Already-fully-paved edges are skipped, so more fieldstone flows on
        // to whatever's still unpaved.
        let leftover2 = roads.pave_edges(edges, 5.0);
        assert_eq!(leftover2, 0.0);
        assert_eq!(roads.edges[farm_side].paved, 1.0);
    }
}
