use std::collections::HashMap;

use bevy::math::{IVec3, Vec3};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::city::{Cell, ConstructedCity};
use crate::eorf::EorfInfo;
use crate::flood_fill::{flood_fill, has_sky_above};
use crate::place::{count_named_near, place_location, FulfilledPorf, PlacedPlaceId, QualityAspect};
use crate::sparse3d::{RelSlot, RelSlotCoord, SlotCoord, Sparse3D};

/// Falloff per hop through open air (and through any structure that isn't a
/// window or a doorway).
const FALLOFF: f32 = 0.3;
/// Downward hops don't attenuate
const FALLOFF_DOWNWARD: f32 = 0.0;
/// Windows let light through but not weather/access: they block outdoorsness
/// entirely.
const FALLOFF_WINDOW: f32 = 1.0;
/// Doors are a partial barrier to outdoorsness.
const FALLOFF_DOORWAY: f32 = 0.7;

/// Returns the per-hop outdoorsness multiplier for the boundary between
/// adjacent cubes `from` and `to`: `0.0` blocks propagation entirely (walls,
/// floors, and windows all fully block outdoorsness), otherwise `1 -
/// falloff`.
fn boundary_multiplier(
    contents: &Sparse3D<Cell>,
    structures: &[EorfInfo],
    from: IVec3,
    to: IVec3,
) -> f32 {
    let falloff = match contents.get(SlotCoord::boundary(from, to)) {
        Some(cell) => match structures[cell.id.as_usize()].name.as_str() {
            "wall" | "floor" | "window" | "roof" => FALLOFF_WINDOW, // 1.0: fully blocks
            "doorway" => FALLOFF_DOORWAY,
            _ => default_falloff(from, to),
        },
        None => default_falloff(from, to),
    };
    1.0 - falloff
}

fn default_falloff(from: IVec3, to: IVec3) -> f32 {
    if to - from == IVec3::NEG_Y {
        FALLOFF_DOWNWARD
    } else {
        FALLOFF
    }
}

/// Flood-fills "outdoorsness" from sky-visible cube voxels: 1.0 = fully
/// outdoors, falling off towards 0.0 the further a cube is from open sky.
///
/// Seeds all cubes within the grid bounding box (expanded by 1) that have an
/// unobstructed vertical view of the sky (see [`has_sky_above`]), then
/// propagates outward. Each hop through open air multiplies the current
/// level by `1 - FALLOFF` (no falloff going straight down); doorways use a
/// steeper `1 - FALLOFF_DOORWAY`, and windows block outdoorsness entirely
/// (`1 - FALLOFF_WINDOW == 0`) even though they don't block light. Walls and
/// floors block propagation outright.
///
/// Returns a map from cube coordinate to outdoorsness in [0.0, 1.0].
pub fn compute_outdoorsness(
    contents: &Sparse3D<Cell>,
    structures: &[EorfInfo],
) -> HashMap<IVec3, f32> {
    if contents.size() == 0 {
        return HashMap::new();
    }

    let (min_cube, max_cube) = contents.bounding_box();
    let search_min = min_cube - IVec3::ONE;
    let search_max = max_cube + IVec3::ONE;
    let top_y = search_max.y;

    let mut seeds = Vec::new();
    for z in search_min.z..=search_max.z {
        for y in search_min.y..=search_max.y {
            for x in search_min.x..=search_max.x {
                let cube = IVec3::new(x, y, z);
                if has_sky_above(contents, cube, top_y) {
                    seeds.push(cube);
                }
            }
        }
    }

    flood_fill(
        seeds,
        search_min,
        search_max,
        /*max_sources=*/ 3,
        |from, to| boundary_multiplier(contents, structures, from, to),
    )
}

/// Structure names that fully block a sightline for `compute_spaciousness`.
///
/// TODO: this should become a proper flag on `Element` (like the falloffs
/// used by `boundary_multiplier`/`compute_outdoorsness`) instead of matching
/// on name.
const SIGHTLINE_BLOCKING_NAMES: [&str; 5] = ["wall", "window", "doorway", "floor", "roof"];

/// True if `cell` fully blocks a sightline. Furniture never blocks one.
fn blocks_sightline(cw: &ConstructedCity, cell: &Cell) -> bool {
    let info = &cw.eorfs[cell.id.as_usize()];
    !info.is_furniture() && SIGHTLINE_BLOCKING_NAMES.contains(&info.name.as_str())
}

/// Number of rays cast per `compute_spaciousness` call.
const SPACIOUSNESS_RAYS: u32 = 12;

/// Deterministically seeds an RNG from a cube coordinate, so evaluating the
/// same place twice gives the same result.
fn seed_from_coord(c: IVec3) -> u64 {
    ((c.x as u32 as u64) << 42) | ((c.y as u32 as u64) << 21) | (c.z as u32 as u64)
}

/// A uniformly-random unit direction that is horizontal or points upward
/// (never downward).
fn random_horizontal_or_up_dir(rng: &mut impl Rng) -> Vec3 {
    let theta = rng.random_range(0.0..std::f32::consts::TAU);
    let y: f32 = rng.random_range(0.0..=1.0);
    let r = (1.0 - y * y).sqrt();
    Vec3::new(r * theta.cos(), y, r * theta.sin())
}

/// Average distance (in cells) from `origin` to the first sightline-blocking
/// structure, along `SPACIOUSNESS_RAYS` random directions that are
/// horizontal or higher (never downward). A ray that reaches `sightline_max`
/// unobstructed counts as `sightline_max` cells clear.
///
/// Casts each ray with `Sparse3D::ray_trace_with_t`, ignoring the hit-group
/// at `origin` itself (the place's own core furniture).
pub fn compute_spaciousness(cw: &ConstructedCity, origin: IVec3, sightline_max: u8) -> f32 {
    let mut rng = StdRng::seed_from_u64(seed_from_coord(origin));
    let total: f32 = (0..SPACIOUSNESS_RAYS)
        .map(|_| {
            let dir = random_horizontal_or_up_dir(&mut rng);
            let dest = origin + (dir * sightline_max as f32).round().as_ivec3();
            let ray_origin = RelSlotCoord::new(origin.x, origin.y, origin.z, RelSlot::Room);
            let ray_dest = RelSlotCoord::new(dest.x, dest.y, dest.z, RelSlot::Room);
            let hits = cw.contents.ray_trace_with_t(ray_origin, ray_dest);
            // TODO: I think `.all` might be more correct, but we should pull the post-RNG part
            // of this into a function to test it.
            let first_block_t = hits
                .iter()
                .filter(|(t, _)| *t > 1e-6)
                .find(|(_, items)| items.iter().any(|c| blocks_sightline(cw, c)))
                .map(|(t, _)| *t);
            match first_block_t {
                Some(t) => t as f32 * sightline_max as f32,
                None => sightline_max as f32,
            }
        })
        .sum();
    total / SPACIOUSNESS_RAYS as f32
}

/// The raw (pre-normalization) score for one `QualityAspect` of the place at
/// `placed_places[idx]`, located at `origin`. `None` means the aspect isn't
/// applicable here (e.g. `Subplaces` with no nested places) and should
/// contribute no multiplier to the overall quality.
fn raw_score(
    cw: &ConstructedCity,
    id: PlacedPlaceId,
    origin: IVec3,
    aspect: &QualityAspect,
    outdoorsness: &HashMap<IVec3, f32>,
) -> Option<f32> {
    match aspect {
        // TODO: derive from the place's actual floor area.
        QualityAspect::FloorArea => Some(1.0),
        // TODO: derive from nearby noise sources.
        QualityAspect::Quiet => Some(1.0),
        QualityAspect::Spaciousness { sightline_max } => {
            Some(compute_spaciousness(cw, origin, *sightline_max))
        }
        QualityAspect::Indoors => Some(1.0 - outdoorsness.get(&origin).copied().unwrap_or(0.0)),
        QualityAspect::Subplaces => {
            let scores: Vec<f32> = cw.placed_places[id]
                .fulfillments
                .iter()
                .filter_map(|f| match f {
                    FulfilledPorf::Place(inner) => Some(evaluate_place(cw, *inner, outdoorsness)),
                    FulfilledPorf::Furniture(_) => None,
                })
                .collect();
            (!scores.is_empty()).then(|| scores.iter().sum::<f32>() / scores.len() as f32)
        }
        QualityAspect::NumberOf { porf_name } => {
            Some(count_named_near(cw, origin, porf_name) as f32)
        }
    }
}

/// One `QualityFactor`'s contribution to a place's overall quality, for
/// display in the UI's breakdown. `None` fields mean the aspect didn't apply
/// (see `raw_score`) and contributed no multiplier.
pub struct QualityFactorResult {
    pub aspect: QualityAspect,
    pub raw: Option<f32>,
    pub normalized: Option<f32>,
    pub strength: f32,
    /// `normalized.powf(strength)`, or `1.0` (no effect) if `raw` is `None`.
    pub contribution: f32,
}

/// Overall quality of the place at `placed_places[idx]`, together with each
/// `QualityFactor`'s individual contribution. `outdoorsness` should be
/// computed once (via `compute_outdoorsness`) and shared across all place
/// evaluations rather than recomputed per place.
pub fn evaluate_place_breakdown(
    cw: &ConstructedCity,
    id: PlacedPlaceId,
    outdoorsness: &HashMap<IVec3, f32>,
) -> (f32, Vec<QualityFactorResult>) {
    let origin = place_location(cw, id);
    let def = &cw.places[cw.placed_places[id].place];
    let mut overall = 1.0;
    let results = def
        .quality_factors
        .iter()
        .map(|factor| {
            let raw = raw_score(cw, id, origin, &factor.aspect, outdoorsness);
            let (normalized, contribution) = match raw {
                Some(raw) => {
                    let normalized = ((raw - factor.range.start)
                        / (factor.range.end - factor.range.start))
                        .clamp(0.0, 1.0);
                    (Some(normalized), normalized.powf(factor.strength))
                }
                None => (None, 1.0),
            };
            overall *= contribution;
            QualityFactorResult {
                aspect: factor.aspect.clone(),
                raw,
                normalized,
                strength: factor.strength,
                contribution,
            }
        })
        .collect();
    (overall, results)
}

/// Overall quality of the place at `placed_places[idx]`: the product of every
/// `QualityFactor`'s normalized score raised to its `strength`. `outdoorsness`
/// should be computed once (via `compute_outdoorsness`) and shared across all
/// place evaluations rather than recomputed per place.
pub fn evaluate_place(
    cw: &ConstructedCity,
    id: PlacedPlaceId,
    outdoorsness: &HashMap<IVec3, f32>,
) -> f32 {
    evaluate_place_breakdown(cw, id, outdoorsness).0
}

#[cfg(test)]
mod tests {
    use assert2::check;
    use bevy::math::IVec3;

    use crate::build_helpers::Builder;
    use crate::city::ConstructedCity;
    use crate::eorf::load_structure_info;
    use crate::materials::BuildMaterialId;
    use crate::place::{
        FulfilledPorf, ParticularPlace, Place, PlaceReq, Porf, QualityAspect, QualityFactor,
    };
    use crate::resource::{Inventory, StorageKind};
    use crate::sparse3d::{Facing, RelSlot, Slot, SlotCoord};

    use super::{compute_outdoorsness, compute_spaciousness, evaluate_place};

    fn place_def(quality_factors: Vec<QualityFactor>) -> Place {
        Place {
            name: "test place".to_string(),
            requirements: vec![PlaceReq {
                requirement: Porf::Furniture("bin".to_string()),
                min: 1,
                max: None,
                worker_visit_weight: 1.0,
                worker_visit_duration: 1.0,
            }],
            public_storage: false,
            accounting: None,
            quality_factors,
            assignable_for: None,
            work: None,
            gate: None,
        }
    }

    /// Places a "bin" core furniture at `core` and registers a `ParticularPlace`
    /// for `def` around it. Returns the new place's id.
    fn place_at(cw: &mut ConstructedCity, def: Place, core: IVec3) -> crate::place::PlacedPlaceId {
        let bin_id = cw.find_structure_by_name("bin").unwrap();
        cw.contents.set(
            SlotCoord {
                cube: core,
                slot: Slot::Room,
            },
            crate::city::Cell {
                id: bin_id,
                facing: Facing::default(),
                evaluation: None,
                build_material: BuildMaterialId::default(),
            },
        );
        cw.places.push(def);
        let place = cw.places.len() - 1;
        cw.placed_places.insert(ParticularPlace {
            place,
            fulfillments: vec![FulfilledPorf::Furniture(SlotCoord {
                cube: core,
                slot: Slot::Room,
            })],
            contents: Inventory::new([(StorageKind::Bulk, 20.0)]),
            restriction: crate::place::ParentRestriction::Unrestricted,
        })
    }

    #[test]
    fn number_of_matches_count_named_near() {
        let structure_infos = load_structure_info();
        let mut cw = ConstructedCity::new(structure_infos);
        cw.road_forbidden_zone = false;
        let def = place_def(vec![QualityFactor {
            aspect: QualityAspect::NumberOf {
                porf_name: "bin".to_string(),
            },
            strength: 1.0,
            range: 0.0..2.0,
        }]);
        let idx = place_at(&mut cw, def, IVec3::new(0, 0, 0));

        let score = evaluate_place(&cw, idx, &Default::default());
        // One bin (the core itself) out of a 0.0..2.0 range -> normalized 0.5.
        check!(score == 0.5);
    }

    #[test]
    fn subplaces_with_no_nested_places_is_skipped() {
        let structure_infos = load_structure_info();
        let mut cw = ConstructedCity::new(structure_infos);
        cw.road_forbidden_zone = false;
        let def = place_def(vec![QualityFactor {
            aspect: QualityAspect::Subplaces,
            strength: 1.0,
            range: 0.0..1.2,
        }]);
        let idx = place_at(&mut cw, def, IVec3::new(0, 0, 0));

        // No nested places -> the factor contributes no multiplier, so the
        // (otherwise-empty) product stays 1.0 rather than being dragged to 0.
        let score = evaluate_place(&cw, idx, &Default::default());
        check!(score == 1.0);
    }

    #[test]
    fn quality_factor_normalizes_raw_score_through_its_range() {
        let structure_infos = load_structure_info();
        let mut cw = ConstructedCity::new(structure_infos);
        cw.road_forbidden_zone = false;
        // NumberOf raw score is exactly 1 (the core bin itself); a range of
        // 1.0..1.0-plus-epsilon-free 1.0..3.0 puts it at the bottom (0.0).
        let low = place_def(vec![QualityFactor {
            aspect: QualityAspect::NumberOf {
                porf_name: "bin".to_string(),
            },
            strength: 1.0,
            range: 1.0..3.0,
        }]);
        let idx_low = place_at(&mut cw, low, IVec3::new(10, 0, 10));
        check!(evaluate_place(&cw, idx_low, &Default::default()) == 0.0);

        let high = place_def(vec![QualityFactor {
            aspect: QualityAspect::NumberOf {
                porf_name: "bin".to_string(),
            },
            strength: 1.0,
            range: 0.0..1.0,
        }]);
        let idx_high = place_at(&mut cw, high, IVec3::new(20, 0, 20));
        check!(evaluate_place(&cw, idx_high, &Default::default()) == 1.0);
    }

    #[test]
    fn spaciousness_is_lower_inside_a_sealed_box_than_in_the_open() {
        let structure_infos = load_structure_info();

        let mut open_builder = Builder::new(&structure_infos);
        open_builder.build_plane(
            IVec3::new(0, 0, 0),
            IVec3::new(0, 0, 0),
            RelSlot::Floor,
            None,
        );
        let mut open_cw = ConstructedCity::new(structure_infos.clone());
        open_cw.contents = open_builder.get();
        let open_score = compute_spaciousness(&open_cw, IVec3::new(0, 0, 0), 5);

        let mut boxed_builder = Builder::new(&structure_infos);
        boxed_builder.build_box(IVec3::new(0, 0, 0), IVec3::new(2, 2, 2));
        let mut boxed_cw = ConstructedCity::new(structure_infos);
        boxed_cw.contents = boxed_builder.get();
        let boxed_score = compute_spaciousness(&boxed_cw, IVec3::new(1, 1, 1), 5);

        check!(boxed_score < open_score);
        check!(boxed_score > 0.0);
        check!(open_score == 5.0);
    }

    /// A sealed box has opaque walls and a roof on all sides. No outdoorsness
    /// should reach any interior cube.
    #[test]
    fn test_sealed_box_has_no_interior_outdoorsness() {
        let structure_infos = load_structure_info();
        let mut builder = Builder::new(&structure_infos);
        builder.build_box(IVec3::new(0, 0, 0), IVec3::new(2, 2, 2));
        let contents = builder.get();

        let outdoorsness = compute_outdoorsness(&contents, &structure_infos);

        let interior = IVec3::new(1, 1, 1);
        check!(outdoorsness.get(&interior).copied().unwrap_or(0.0) == 0.0);
    }

    /// An open cube (nothing above it) seeds itself as fully outdoors.
    #[test]
    fn test_open_cube_is_fully_outdoors() {
        let structure_infos = load_structure_info();
        let mut builder = Builder::new(&structure_infos);
        builder.build_plane(
            IVec3::new(0, 0, 0),
            IVec3::new(0, 0, 0),
            RelSlot::Floor,
            None,
        );
        let contents = builder.get();
        let outdoorsness = compute_outdoorsness(&contents, &structure_infos);
        check!(
            outdoorsness
                .get(&IVec3::new(0, 0, 0))
                .copied()
                .unwrap_or(0.0)
                == 1.0
        );
    }

    /// A window in the wall admits light but should not admit any
    /// outdoorsness at all.
    #[test]
    fn test_window_blocks_outdoorsness() {
        let structure_infos = load_structure_info();
        let mut builder = Builder::new(&structure_infos);
        builder.build_box(IVec3::new(0, 0, 0), IVec3::new(2, 2, 2));
        builder.build_plane(
            IVec3::new(0, 1, 1),
            IVec3::new(0, 1, 1),
            RelSlot::XLoWall,
            Some("window"),
        );
        let contents = builder.get();

        let outdoorsness = compute_outdoorsness(&contents, &structure_infos);
        let at_window = IVec3::new(0, 1, 1);
        check!(outdoorsness.get(&at_window).copied().unwrap_or(0.0) == 0.0);
    }

    /// A doorway admits partial outdoorsness (less than full, but nonzero).
    #[test]
    fn test_doorway_admits_partial_outdoorsness() {
        let structure_infos = load_structure_info();
        let mut builder = Builder::new(&structure_infos);
        builder.build_box(IVec3::new(0, 0, 0), IVec3::new(2, 2, 2));
        builder.build_plane(
            IVec3::new(0, 1, 1),
            IVec3::new(0, 1, 1),
            RelSlot::XLoWall,
            Some("doorway"),
        );
        let contents = builder.get();

        let outdoorsness = compute_outdoorsness(&contents, &structure_infos);
        let at_doorway = IVec3::new(0, 1, 1);
        let level = outdoorsness.get(&at_doorway).copied().unwrap_or(0.0);
        check!(level > 0.0);
        check!(level < 1.0);
    }

    /// A cube directly beneath a sky-visible cube suffers no falloff, even
    /// after multiple hops straight down. Walled on all sides so the only
    /// route in is the open top, isolating the vertical hop from horizontal
    /// falloff.
    #[test]
    fn test_downward_propagation_has_no_falloff() {
        let structure_infos = load_structure_info();
        let mut builder = Builder::new(&structure_infos);
        builder.build_plane(
            IVec3::new(0, 0, 0),
            IVec3::new(0, 2, 0),
            RelSlot::XLoWall,
            None,
        );
        builder.build_plane(
            IVec3::new(0, 0, 0),
            IVec3::new(0, 2, 0),
            RelSlot::XHiWall,
            None,
        );
        builder.build_plane(
            IVec3::new(0, 0, 0),
            IVec3::new(0, 2, 0),
            RelSlot::ZLoWall,
            None,
        );
        builder.build_plane(
            IVec3::new(0, 0, 0),
            IVec3::new(0, 2, 0),
            RelSlot::ZHiWall,
            None,
        );
        builder.build_plane(
            IVec3::new(0, 0, 0),
            IVec3::new(0, 0, 0),
            RelSlot::Floor,
            None,
        );
        let contents = builder.get();
        let outdoorsness = compute_outdoorsness(&contents, &structure_infos);

        check!(
            outdoorsness
                .get(&IVec3::new(0, 2, 0))
                .copied()
                .unwrap_or(0.0)
                == 1.0
        );
        check!(
            outdoorsness
                .get(&IVec3::new(0, 1, 0))
                .copied()
                .unwrap_or(0.0)
                == 1.0
        );
        check!(
            outdoorsness
                .get(&IVec3::new(0, 0, 0))
                .copied()
                .unwrap_or(0.0)
                == 1.0
        );
    }
}
