use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::city::ConstructedCity;
use crate::resource::{ToolKind, UniformResource};
use crate::surroundings::road_network::{RoadNetwork, RoadSegment};

/// Flat bonus on top of a farm's acreage-based production. Stands in for the
/// old market-attendance boost, now that every eligible farm comes every month.
pub const PRODUCTION_BONUS: u32 = 4;

/// Potatoes (from city storage) it costs to set a farm up with a tool.
pub const SPECIALIZE_COST: u32 = 10;

/// Identifies a farm by its position in `FarmsResource::farms`. Farms are
/// never added or removed after generation, so a plain index is stable —
/// this newtype exists purely so a farm index can't be silently mixed up
/// with some other `usize` (a place id, a vertex index, ...) at a function
/// boundary. Raw indices still cross I/O boundaries (headless commands,
/// `query farms` output) as plain integers; `FarmId` is the internal
/// game-logic vocabulary.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FarmId(usize);

impl FarmId {
    pub fn new(index: usize) -> Self {
        FarmId(index)
    }

    pub fn index(self) -> usize {
        self.0
    }
}

impl std::fmt::Display for FarmId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Serialize, Deserialize)]
pub struct FarmData {
    pub seed: Vec2,
    pub polygon: Vec<Vec2>,
    pub area: f32,
    pub fertility: f32,
    /// The inedible resource this farm grows. Fixed at generation.
    pub resource: UniformResource,
    /// A tool the player has given this farm, if any. While set, the farm
    /// processes its neighbors' `resource` into the tool's output instead of
    /// growing its own (see `compute_production`).
    pub tool: Option<ToolKind>,
    /// The least guest-room quality this farm will put up with when it needs
    /// a room (see `attendance::plan_attendance`). Rolled at generation.
    pub required_quality: f32,
}

impl FarmData {
    pub fn centroid(&self) -> Vec2 {
        self.seed
    }

    pub fn production_capacity(&self) -> u32 {
        self.area.round() as u32 + PRODUCTION_BONUS
    }

    /// What this farm brings to market besides potatoes: its own resource,
    /// or its tool's output while it has one.
    pub fn produced_resource(&self) -> UniformResource {
        match self.tool {
            Some(tool) => tool.specialization().output,
            None => self.resource,
        }
    }
}

/// Permanent game-state resource: generated once, persists across mode changes.
#[derive(Resource, Serialize, Deserialize)]
pub struct FarmsResource {
    pub farms: Vec<FarmData>,
    pub circle_pos: Vec2,
    /// Paths revealed by traveler arrivals.
    #[serde(default)]
    pub traveler_reveals: Vec<Vec<Vec2>>,
    /// Voronoi adjacency: `neighbors[i]` lists the farms sharing an edge with farm `i`.
    /// Rebuilt from polygons when empty (covers fresh generation and loaded saves).
    #[serde(skip)]
    pub neighbors: Vec<Vec<FarmId>>,
    /// Paved roads the player has finished building. Proposed ones live in
    /// `ProposedCity::proposed_roads` until the city project completes.
    #[serde(default)]
    pub paved_roads: Vec<RoadSegment>,
    /// The road network over the farm polygons and `paved_roads`. Rebuilt
    /// when absent (fresh generation, loaded saves, or after paving).
    #[serde(skip)]
    pub roads: Option<RoadNetwork>,
}

impl std::ops::Index<FarmId> for FarmsResource {
    type Output = FarmData;
    fn index(&self, id: FarmId) -> &FarmData {
        &self.farms[id.index()]
    }
}

impl std::ops::IndexMut<FarmId> for FarmsResource {
    fn index_mut(&mut self, id: FarmId) -> &mut FarmData {
        &mut self.farms[id.index()]
    }
}

/// Two polygon vertices are "the same" corner if within this map-unit distance.
const ADJACENCY_EPS: f32 = 0.01;

/// Build Voronoi adjacency: two cells are neighbours when their polygons share an
/// edge, i.e. they have two vertices in common.
pub fn build_adjacency(farms: &[FarmData]) -> Vec<Vec<FarmId>> {
    let n = farms.len();
    let mut neighbors = vec![Vec::new(); n];
    for i in 0..n {
        for j in (i + 1)..n {
            let shared = farms[i]
                .polygon
                .iter()
                .filter(|&&vi| {
                    farms[j]
                        .polygon
                        .iter()
                        .any(|&vj| vi.distance(vj) <= ADJACENCY_EPS)
                })
                .count();
            if shared >= 2 {
                neighbors[i].push(FarmId::new(j));
                neighbors[j].push(FarmId::new(i));
            }
        }
    }
    neighbors
}

impl FarmsResource {
    /// Ensure the adjacency cache is populated (no-op once built).
    pub fn ensure_adjacency(&mut self) {
        if self.neighbors.len() != self.farms.len() {
            self.neighbors = build_adjacency(&self.farms);
        }
    }

    /// Ensure the road network is built (no-op once built; paving clears it).
    pub fn ensure_roads(&mut self) {
        if self.roads.is_none() {
            self.roads = Some(self.road_network_with(&[]));
        }
    }

    /// A fresh road network over the farms, `paved_roads`, and `extra` paved
    /// roads (e.g. ones only proposed so far).
    pub fn road_network_with(&self, extra: &[RoadSegment]) -> RoadNetwork {
        let polygons: Vec<Vec<Vec2>> = self.farms.iter().map(|f| f.polygon.clone()).collect();
        let paved: Vec<RoadSegment> = self.paved_roads.iter().chain(extra).copied().collect();
        RoadNetwork::build(&polygons, self.circle_pos, &paved)
    }

    /// Finishes paving `segments`, rebuilding the road network.
    pub fn add_paved_roads(&mut self, segments: impl IntoIterator<Item = RoadSegment>) {
        let before = self.paved_roads.len();
        self.paved_roads.extend(segments);
        if self.paved_roads.len() != before {
            self.roads = None;
            self.ensure_roads();
        }
    }

    /// Whether the player can see farm `id` (it's out of the fog).
    pub fn is_visible(&self, id: FarmId) -> bool {
        is_point_visible(self[id].centroid(), &self.traveler_reveals)
    }

    /// The road-cost of farm `id`'s cheapest delivery route into the city, or
    /// `None` if it has no route (or the road network isn't built yet).
    pub fn travel_cost(&self, id: FarmId) -> Option<f32> {
        self.roads
            .as_ref()?
            .farm_delivery(&self[id].polygon)
            .map(|(w, _)| w)
    }

    pub fn neighbors_of(&self, id: FarmId) -> &[FarmId] {
        self.neighbors.get(id.index()).map_or(&[], |v| v.as_slice())
    }
}

/// Why a paved road can't join corners `a` and `b` of `network` (the road
/// network including roads already proposed, see
/// `FarmsResource::road_network_with`), or `None` if it can. The road must
/// start on the paved network (or at the city), and the player must be able
/// to see both ends.
pub fn road_blocker(
    fr: &FarmsResource,
    network: &RoadNetwork,
    a: usize,
    b: usize,
) -> Option<&'static str> {
    if a >= network.corner_count || b >= network.corner_count {
        Some("Roads must start and end at corners")
    } else if a == b {
        Some("Zero-length road")
    } else if !network.on_paved_network(a) {
        Some("Roads must connect to the city")
    } else if !is_point_visible(network.nodes[a], &fr.traveler_reveals)
        || !is_point_visible(network.nodes[b], &fr.traveler_reveals)
    {
        Some("Roads must be on the known map.")
    } else {
        None
    }
}

/// Paves a road between corners `a` and `b` of `network` (see
/// [`road_blocker`]): instantly and for free in sandbox mode, otherwise by
/// adding it to the city project.
pub fn propose_road(
    fr: &mut FarmsResource,
    pending: &mut crate::city::ProposedCity,
    network: &RoadNetwork,
    a: usize,
    b: usize,
    sandbox_enabled: bool,
) -> Result<RoadSegment, &'static str> {
    if let Some(blocker) = road_blocker(fr, network, a, b) {
        return Err(blocker);
    }
    let seg = RoadSegment {
        a: network.nodes[a],
        b: network.nodes[b],
    };
    if sandbox_enabled {
        fr.add_paved_roads([seg]);
    } else {
        pending.proposed_roads.push(seg);
    }
    Ok(seg)
}

/// Whether a map point is out of the fog.
pub fn is_point_visible(p: Vec2, traveler_reveals: &[Vec<Vec2>]) -> bool {
    use super::map::{fog_alpha_at, REVEAL_THRESHOLD};
    fog_alpha_at(p, traveler_reveals) < REVEAL_THRESHOLD
}

/// Transient resource: exists only while in Surroundings mode.
#[derive(Resource, Default)]
pub struct SurroundingsState {
    pub viewport_offset: Vec2,
    /// Whether the map is in road-building mode.
    pub road_mode: bool,
    /// The corner a paved road being drawn starts from, after the first click.
    pub road_start: Option<Vec2>,
    /// Why the last attempt to place a road failed, if it did.
    pub road_error: Option<&'static str>,
}

/// In-game calendar.
#[derive(Resource, Default, Serialize, Deserialize)]
pub struct GameClock {
    pub months: u32,
}

impl GameClock {
    pub fn month(&self) -> u32 {
        self.months
    }

    pub fn advance_month(&mut self) {
        self.months += 1;
    }
}

/// Sums `production_capacity()` per produced resource across farms the
/// player can see. Used as a discard-priority tie-break in
/// `resource::distribute_incoming_resources`.
pub fn known_farm_plentifulness(fr: &FarmsResource) -> HashMap<UniformResource, u32> {
    let mut totals = HashMap::new();
    for (i, farm) in fr.farms.iter().enumerate() {
        if fr.is_visible(FarmId::new(i)) {
            *totals.entry(farm.produced_resource()).or_insert(0) += farm.production_capacity();
        }
    }
    totals
}

/// A month's production, computed by `compute_production`.
#[derive(Clone, Debug)]
pub struct ProductionPlan {
    /// Per farm: potatoes grown this month.
    pub potatoes: Vec<u32>,
    /// Per farm: units of `produced_resource()` this month. For a farm with a
    /// tool this is its tool's output, which (until a tool has a non-1:1
    /// conversion ratio) equals how much input it took from its neighbours.
    pub inedible: Vec<u32>,
    /// `(supplier, tool farm, quantity)`: input each farm with a tool took
    /// from a neighbour this month.
    pub supplies: Vec<(FarmId, FarmId, u32)>,
}

/// Compute this month's production for all farms. A farm with a tool converts
/// its tool's input, taken from adjacent tool-less farms growing it this month,
/// into the tool's output, drawing from neighbours in ascending id order.
pub fn compute_production(fr: &FarmsResource) -> ProductionPlan {
    let n = fr.farms.len();
    let cap: Vec<u32> = fr.farms.iter().map(|f| f.production_capacity()).collect();
    let potatoes = cap.clone();
    let mut inedible = cap.clone();
    let mut supplies = Vec::new();

    for s in (0..n).map(FarmId::new) {
        let Some(tool) = fr[s].tool else {
            continue;
        };
        let spec = tool.specialization();
        let mut suppliers: Vec<FarmId> = fr
            .neighbors_of(s)
            .iter()
            .copied()
            .filter(|&j| fr[j].tool.is_none() && fr[j].resource == spec.input)
            .collect();
        suppliers.sort();

        let mut demand = cap[s.index()];
        let mut consumed = 0;
        for j in suppliers {
            if demand == 0 {
                break;
            }
            let take = demand.min(inedible[j.index()]);
            if take == 0 {
                continue;
            }
            inedible[j.index()] -= take;
            demand -= take;
            consumed += take;
            supplies.push((j, s, take));
        }
        // A farm with a tool brings its tool's output instead of its own resource.
        inedible[s.index()] = consumed;
    }

    ProductionPlan {
        potatoes,
        inedible,
        supplies,
    }
}

/// Why farm `id` can't be given `tool` right now, or `None` if it can. The
/// farm must be coming to market (`coming`), and the city must have the tool
/// and the potatoes on hand.
pub fn specialize_blocker(
    fr: &FarmsResource,
    cw: &ConstructedCity,
    id: FarmId,
    tool: ToolKind,
    coming: bool,
) -> Option<&'static str> {
    if fr[id].tool.is_some() {
        Some("This farm is already using a tool.")
    } else if !coming {
        Some("Farms need to be coming to the market to get a tool.")
    } else if crate::place::total_tools_of(cw, tool) == 0 {
        Some("No tools available.")
    } else if crate::place::total_uniform(cw, UniformResource::Potato) < SPECIALIZE_COST {
        Some("Not enough potatoes available.")
    } else {
        None
    }
}

/// Gives farm `id` a tool from storage, paying `SPECIALIZE_COST` potatoes from
/// storage. Returns whether it happened (see `specialize_blocker`).
pub fn specialize(
    fr: &mut FarmsResource,
    cw: &mut ConstructedCity,
    id: FarmId,
    tool: ToolKind,
    coming: bool,
) -> bool {
    if specialize_blocker(fr, cw, id, tool, coming).is_some() {
        return false;
    }
    if !crate::place::consume_tool(cw, tool) {
        return false;
    }
    if !crate::place::consume_uniform(cw, UniformResource::Potato, SPECIALIZE_COST) {
        crate::place::deposit_tool(cw, tool);
        return false;
    }
    fr[id].tool = Some(tool);
    true
}

/// Why farm `id`'s tool can't be taken back right now, or `None` if it can.
pub fn unspecialize_blocker(
    fr: &FarmsResource,
    cw: &ConstructedCity,
    id: FarmId,
) -> Option<&'static str> {
    use crate::resource::RackContents;
    if fr[id].tool.is_none() {
        Some("HUMANIZE: Has no tool")
    } else if crate::place::rack_free_capacity(cw, RackContents::Tools) < 1.0 {
        Some("HUMANIZE: No rack room to store the tool")
    } else {
        None
    }
}

/// Takes farm `id`'s tool back into storage (free). Returns whether it happened.
pub fn unspecialize(fr: &mut FarmsResource, cw: &mut ConstructedCity, id: FarmId) -> bool {
    if unspecialize_blocker(fr, cw, id).is_some() {
        return false;
    }
    let Some(tool) = fr[id].tool else {
        return false;
    };
    if !crate::place::deposit_tool(cw, tool) {
        return false;
    }
    fr[id].tool = None;
    true
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn mk_farm(resource: UniformResource, area: f32) -> FarmData {
        FarmData {
            seed: Vec2::ZERO,
            polygon: Vec::new(),
            area,
            fertility: 1.0,
            resource,
            tool: None,
            required_quality: 0.5,
        }
    }

    pub(crate) fn farms_with(farms: Vec<FarmData>, neighbors: Vec<Vec<usize>>) -> FarmsResource {
        FarmsResource {
            farms,
            circle_pos: Vec2::ZERO,
            traveler_reveals: Vec::new(),
            neighbors: neighbors
                .into_iter()
                .map(|v| v.into_iter().map(FarmId::new).collect())
                .collect(),
            paved_roads: Vec::new(),
            roads: None,
        }
    }

    #[test]
    fn adjacency_detects_shared_edge() {
        use UniformResource::Straw;
        let square = |x: f32| FarmData {
            polygon: vec![
                Vec2::new(x, 0.0),
                Vec2::new(x + 1.0, 0.0),
                Vec2::new(x + 1.0, 1.0),
                Vec2::new(x, 1.0),
            ],
            ..mk_farm(Straw, 5.0)
        };
        // a and b share the edge x=1; c is far away and shares nothing.
        let a = square(0.0);
        let b = square(1.0);
        let c = FarmData {
            polygon: vec![
                Vec2::new(50.0, 50.0),
                Vec2::new(51.0, 50.0),
                Vec2::new(51.0, 51.0),
            ],
            ..mk_farm(Straw, 5.0)
        };
        let adj = build_adjacency(&[a, b, c]);
        assert_eq!(adj[0], vec![FarmId::new(1)]);
        assert_eq!(adj[1], vec![FarmId::new(0)]);
        assert!(adj[2].is_empty());
    }

    fn tool_farm(area: f32) -> FarmData {
        FarmData {
            tool: Some(ToolKind::CarpentersTools),
            ..mk_farm(UniformResource::Straw, area)
        }
    }

    #[test]
    fn specialized_farm_is_supply_limited() {
        use UniformResource::Timber;
        // Capacity 16 exceeds the 7 + 8 = 15 timber available, so planks ==
        // 15 and the neighbours are drained.
        let s = tool_farm(12.0);
        let t = mk_farm(Timber, 3.0);
        let u = mk_farm(Timber, 4.0);
        let fr = farms_with(vec![s, t, u], vec![vec![1, 2], vec![0], vec![0]]);
        let plan = compute_production(&fr);
        assert_eq!(plan.inedible, vec![15, 0, 0]);
        assert_eq!(
            plan.supplies,
            vec![
                (FarmId::new(1), FarmId::new(0), 7),
                (FarmId::new(2), FarmId::new(0), 8)
            ]
        );
    }

    #[test]
    fn specialized_farm_is_capacity_limited_and_drains_lowest_ids_first() {
        use UniformResource::Timber;
        // Capacity 10 + 4 = 14; 12 + 13 timber available. Farm 1 is drained
        // first, then farm 2 gives the remaining 2.
        let s = tool_farm(10.0);
        let t = mk_farm(Timber, 8.0);
        let u = mk_farm(Timber, 9.0);
        let fr = farms_with(vec![s, t, u], vec![vec![1, 2], vec![0], vec![0]]);
        let plan = compute_production(&fr);
        assert_eq!(plan.inedible, vec![14, 0, 11]);
        // Potatoes are unaffected by the tool.
        assert_eq!(plan.potatoes, vec![14, 12, 13]);
    }
}
