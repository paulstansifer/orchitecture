use crate::autotile::{
    build_empty_anchor_index, char_matches_name, collapse_motif_atoms, compile,
    evaluate_autotile_rules, evaluate_empty_anchor_rules, parse, AutotileOriented, DefectAtom,
    Motif, MotifAtom, MotifAxis, MotifOccurrence, OrientedCase,
};
use crate::city::Cell;
use crate::eorf::{EorfId, EorfInfo};
use crate::sparse3d::{Facing, RelSlot, RelSlotCoord, SlotCoord, Sparse3D};
use bevy::math::IVec3;
use burn::prelude::*;
use burn::tensor::{Float, TensorData};
use std::error::Error;

#[cfg(feature = "training")]
use burn::backend::Autodiff;
#[cfg(feature = "training")]
use burn::data::dataset::InMemDataset;
#[cfg(feature = "training")]
use std::collections::HashMap;

pub const EMBEDDING_SIZE: usize = 5 + 1; // Keep this in sync with structure.rs (+ 1 for "visibility")

/// How to interpret a score target during training.
#[cfg(feature = "training")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScoreConstraint {
    /// Penalise any deviation from the target (standard MSE).
    Exact,
    /// Penalise only when prediction exceeds the target (pred should be ≤ target).
    AtMost,
    /// Penalise only when prediction falls below the target (pred should be ≥ target).
    AtLeast,
}

#[cfg(feature = "training")]
impl crate::city::ConstrainedScore {
    pub fn disassemble(self) -> (f32, ScoreConstraint) {
        match self {
            Self::Exact(v) => (v, ScoreConstraint::Exact),
            Self::AtMost { at_most: v } => (v, ScoreConstraint::AtMost),
            Self::AtLeast { at_least: v } => (v, ScoreConstraint::AtLeast),
        }
    }
}

#[cfg(feature = "training")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Metric {
    Interest,
    Order,
}

#[cfg(feature = "training")]
impl std::fmt::Display for Metric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Metric::Interest => write!(f, "interest"),
            Metric::Order => write!(f, "order"),
        }
    }
}

fn idx_to_range(idx: i32, expand: bool) -> std::ops::Range<usize> {
    let idx = idx as usize;
    if !expand {
        idx..(idx + 1)
    } else {
        (idx - 1)..(idx + 2)
    }
}

// Returns a 5D index into the voxels. Each grid cell is represented by a 2x2x2 cluster of voxels,
// with each slot occupying a particular position.
fn grid_coord_to_voxel_coord(
    pos: IVec3,
    min: IVec3,
    slot: RelSlot,
    channel: usize,
) -> [std::ops::Range<usize>; 5] {
    use RelSlot::{Floor, Room, XLoWall, ZLoWall};
    let adj_vec = (pos - min) * 2 + IVec3::new(1, 1, 1);
    let vox_vec = adj_vec
        + match slot {
            // Match on RelSlot
            Room => IVec3::new(0, 0, 0),
            ZLoWall => IVec3::new(0, 0, -1),
            Floor => IVec3::new(0, -1, 0),
            XLoWall => IVec3::new(-1, 0, 0),
            _ => panic!("We're only using lo slots"),
        };
    let x = idx_to_range(vox_vec.x, slot == Floor || slot == ZLoWall);
    let y = idx_to_range(vox_vec.y, false);
    let z = idx_to_range(vox_vec.z, slot == Floor || slot == XLoWall);

    [0..1, channel..channel + 1, x, y, z]
}

#[cfg(feature = "training")]
#[derive(Clone, Debug)]
pub struct GroundTruth<B: Backend> {
    pub voxels: Tensor<B, 5, Float>,
    /// See `motif_occurrences_to_tensors`; computed unconditionally (cheap, and reused once a
    /// metric other than `Interest` gets its own motif training data).
    pub motif_interest: Tensor<B, 2, Float>,
    pub motif_order: Tensor<B, 2, Float>,
    /// See `motif_order_stats`; also computed unconditionally, same reasoning as above.
    pub order_stats: Tensor<B, 2, Float>,
    pub scores: Tensor<B, 1, Float>,
    pub constraint: ScoreConstraint,
    pub filename: String,
}

#[cfg(feature = "training")]
fn convert_ground_truth_to_autodiff<B: Backend>(gt: GroundTruth<B>) -> GroundTruth<Autodiff<B>> {
    GroundTruth {
        voxels: Tensor::from_inner(gt.voxels),
        motif_interest: Tensor::from_inner(gt.motif_interest),
        motif_order: Tensor::from_inner(gt.motif_order),
        order_stats: Tensor::from_inner(gt.order_stats),
        scores: Tensor::from_inner(gt.scores),
        constraint: gt.constraint,
        filename: gt.filename,
    }
}

#[cfg(feature = "training")]
#[derive(Clone, Debug)]
pub struct GroundTruthBatcher {}

/// Rotational augmentation only. Used to also mess rooms up (`build_helpers::add_noise`) and
/// dock the messed copy's `order` label by a fixed amount for `Metric::Order`, but that fixed
/// docking has no reliable relationship to `motif_order_stats`'s h-indices -- noise that doesn't
/// happen to break enough motif pairs to move an h-index still got the same label penalty,
/// planting contradictory training examples. Dropped.
#[cfg(feature = "training")]
fn augment_datum(s: (Sparse3D<Cell>, String)) -> Vec<(Sparse3D<Cell>, String)> {
    use crate::sparse3d::Rotateable;
    let mut res = vec![];

    res.push((
        s.0.clone().rotate(crate::sparse3d::Rotation::Clockwise),
        format!("{}-cw", s.1),
    ));
    // res.push(
    //     s.clone()
    //         .rotate(crate::sparse3d::Rotation::CounterClockwise),
    // );
    // res.push(s.clone().rotate(crate::sparse3d::Rotation::OneEighty));
    res.push(s);
    res
}

#[cfg(feature = "training")]
impl<B: Backend> burn::data::dataloader::batcher::Batcher<B, GroundTruth<B>, GroundTruth<B>>
    for GroundTruthBatcher
{
    fn batch(&self, ds: Vec<GroundTruth<B>>, _device: &B::Device) -> GroundTruth<B> {
        let mut voxels: Vec<Tensor<B, 5, Float>> = Vec::new();
        // Concatenating these along the row dimension is only meaningful when every item in the
        // batch is the same room (batch_size=1): each room contributes a different number of
        // rows, and unlike `voxels`, nothing here marks where one room's rows end and the next
        // begin. MotifNn's dataloader enforces batch_size=1 for exactly this reason.
        let mut motif_interest: Vec<Tensor<B, 2, Float>> = Vec::new();
        let mut motif_order: Vec<Tensor<B, 2, Float>> = Vec::new();
        let mut order_stats: Vec<Tensor<B, 2, Float>> = Vec::new();
        let mut scores: Vec<Tensor<B, 1, Float>> = Vec::new();
        let mut files: Vec<String> = Vec::new();
        // batch_size=1; all items in a batch should have the same constraint
        let constraint = ds[0].constraint;

        for gt in ds {
            voxels.push(gt.voxels);
            motif_interest.push(gt.motif_interest);
            motif_order.push(gt.motif_order);
            order_stats.push(gt.order_stats);
            scores.push(gt.scores);
            files.push(gt.filename);
        }

        let voxels = Tensor::cat(voxels, 0);
        let motif_interest = Tensor::cat(motif_interest, 0);
        let motif_order = Tensor::cat(motif_order, 0);
        let order_stats = Tensor::cat(order_stats, 0);
        let scores = Tensor::cat(scores, 0);
        GroundTruth {
            voxels,
            motif_interest,
            motif_order,
            order_stats,
            scores,
            constraint,
            filename: files.join("/"),
        }
    }
}

#[cfg(feature = "training")]
use std::{fs, path::Path};

#[cfg(feature = "training")]
pub fn load_training_data<B: Backend>(
    directory: &str,
    seed: u64,
    metric: Metric,
) -> (
    InMemDataset<GroundTruth<Autodiff<B>>>,
    InMemDataset<GroundTruth<B>>,
) {
    let path = Path::new(directory);
    let mut all_sparse_data = Vec::new();

    let structures = crate::eorf::load_structure_info();
    let mut structures_by_char = HashMap::new();
    for (id, structure) in structures.iter().enumerate() {
        if let Some(x_char) = structure.x_char {
            structures_by_char.insert(x_char, crate::eorf::EorfId(id as u32));
        }
        if let Some(z_char) = structure.z_char {
            structures_by_char.insert(z_char, crate::eorf::EorfId(id as u32));
        }
    }

    for entry in fs::read_dir(path).expect(&format!("Failed to read {:?}", path)) {
        let entry = entry.expect("Failed to read entry");
        let path = entry.path();

        if path.extension().map_or(false, |ext| ext == "txt") {
            let content = fs::read_to_string(&path).expect("Failed to read file");
            let sparse_data = crate::serialization::deserialize_sparse3d::<Cell, _, anyhow::Error>(
                &content,
                |c, _slot, structures_by_char| {
                    let id = crate::serialization::deserialize(c, structures_by_char);
                    Ok(Cell {
                        id: id.unwrap(),
                        // The voxel representation doesn't care about orientation:
                        facing: crate::sparse3d::Facing::NegX, // TODO!!!
                        evaluation: None,
                        build_material: crate::materials::BuildMaterialId::default(),
                    })
                },
                &structures_by_char,
            )
            .expect("Failed to deserialize");

            // println!("== {:?} ==", path);
            // let gt: GroundTruth<B> = ground_truth_at_vantage(&sparse_data);
            // print_voxels(&gt.voxels);

            all_sparse_data.push((
                sparse_data,
                path.to_str()
                    .unwrap()
                    .split("/")
                    .last()
                    .unwrap()
                    .strip_suffix(".txt")
                    .unwrap()
                    .to_owned(),
            ));
        }
    }

    use rand::SeedableRng;
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);

    if metric == Metric::Interest {
        for _ in 0..10 {
            all_sparse_data.push(crate::build_helpers::make_boring_room(
                &structures,
                &mut rng,
            ))
        }
    }

    // if metric == Metric::Order {
    //     for exemplar in all_sparse_data.iter().take(6) {
    //         println!("{}", exemplar.1);
    //         print_voxels(
    //             &ground_truth_at_vantage::<B>(exemplar, Metric::Order, &structures).voxels,
    //         );
    //         println!("{} messed up", exemplar.1);

    //         print_voxels(
    //             &ground_truth_at_vantage::<B>(
    //                 &(
    //                     crate::build_helpers::add_noise(exemplar.0.clone(), &structures, &mut rng)
    //                         [0]
    //                     .clone(),
    //                     format!("---"),
    //                 ),
    //                 Metric::Order,
    //                 &structures,
    //             )
    //             .voxels,
    //         );
    //     }
    //     panic!()
    // }

    use rand::seq::SliceRandom;
    all_sparse_data.shuffle(&mut rng);

    let split_index = (all_sparse_data.len() as f32 * 0.66).ceil() as usize;
    let (t_rooms, v_rooms) = all_sparse_data.split_at(split_index);

    // Currently unclear if augmentation is helping at all; investigate more!
    let t_rooms: Vec<_> = t_rooms.into_iter().cloned().collect();
    let v_rooms: Vec<_> = v_rooms.into_iter().cloned().collect();

    let mut t_rooms_augmented = Vec::new();
    for data in t_rooms {
        t_rooms_augmented.extend(augment_datum(data));
    }

    let mut v_rooms_augmented = Vec::new();
    for data in v_rooms {
        v_rooms_augmented.extend(augment_datum(data));
    }

    let train_data: Vec<_> = t_rooms_augmented
        .into_iter()
        .filter_map(|sparse_data| ground_truth_at_vantage(&sparse_data, metric, &structures))
        .map(convert_ground_truth_to_autodiff)
        .collect();

    let test_data: Vec<_> = v_rooms_augmented
        .into_iter()
        .filter_map(|sparse_data| ground_truth_at_vantage(&sparse_data, metric, &structures))
        .collect();

    (InMemDataset::new(train_data), InMemDataset::new(test_data))
}

// Just handles a single datum, but the tensors could hold a batch
#[cfg(feature = "training")]
pub fn ground_truth_at_vantage<B: Backend>(
    data: &(Sparse3D<Cell>, String),
    metric: Metric,
    structures: &Vec<EorfInfo>,
) -> Option<GroundTruth<B>> {
    for (loc, cell) in data.0.iter() {
        if let Some(eval) = &cell.evaluation {
            let constrained = match metric {
                Metric::Interest => eval.interest?,
                Metric::Order => eval.order?,
            };
            let (val, constraint) = constrained.disassemble();

            let tensor = sparse3d_to_tensor(&data.0, /*center_coord=*/ loc.cube, |cell| {
                structures[cell.id.as_usize()].embedding.to_vec()
            })
            .unwrap();

            // print_voxels(&tensor);

            let vantage = RelSlotCoord::new(loc.cube.x, loc.cube.y, loc.cube.z, RelSlot::Room);
            let (occurrences, _defects) = visible_motifs_and_defects(&data.0, vantage, structures);
            let (motif_interest, motif_order) = motif_occurrences_to_tensors(&occurrences);
            let order_stats = motif_order_stats(&occurrences);

            return Some(GroundTruth {
                voxels: tensor,
                motif_interest,
                motif_order,
                order_stats,
                scores: Tensor::from_data(TensorData::from([val]), &Default::default()),
                constraint,
                filename: data.1.clone(),
            });
        }
    }
    None
}

/// Converts a region of Sparse3D data centered around a coordinate to a Tensor,
/// expanding each Sparse3D cell into a 2x2x2 voxel block.
pub fn sparse3d_to_tensor<B: Backend, T, F>(
    sparse_data: &Sparse3D<T>,
    center_coord: IVec3,
    embedding: F,
) -> Result<Tensor<B, 5, Float>, Box<dyn Error>>
where
    F: Fn(&T) -> Vec<f32>,
{
    let device = Default::default();

    let min_coord = center_coord - IVec3::new(5, 2, 5);
    let max_coord = center_coord + IVec3::new(5, 3, 5);
    let size = max_coord - min_coord + IVec3::new(1, 1, 1);

    let shape = Shape::new([
        1_usize,
        EMBEDDING_SIZE,
        (size.x * 2) as usize + 1,
        (size.y * 2) as usize,
        (size.z * 2) as usize + 1,
    ]);

    let mut voxels = Tensor::<B, 5>::zeros(shape, &device);

    let vantage = RelSlotCoord::new(
        center_coord.x,
        center_coord.y,
        center_coord.z,
        RelSlot::Room,
    );

    // The last channel, `.visibility`, distinguishes indoor open air (0.0) that the
    // vantage can see, from regular structure (0.5), from anything not reachable from
    // the vantage without crossing a wall or window (1.0) -- which lumps together
    // outdoor open air and fully-occluded space, since the window-opacity hack below
    // already treats "seen through a window" the same as "blocked".
    let visibility_channel = EMBEDDING_SIZE - 1;

    for grid_y in min_coord.y..=max_coord.y {
        for grid_x in min_coord.x..=max_coord.x {
            for grid_z in min_coord.z..=max_coord.z {
                for slot in [
                    RelSlot::Room,
                    RelSlot::XLoWall,
                    RelSlot::Floor,
                    RelSlot::ZLoWall,
                ] {
                    let grid_pos = IVec3::new(grid_x, grid_y, grid_z);
                    let slot_location = RelSlotCoord::new(grid_x, grid_y, grid_z, slot);

                    let obstacles = sparse_data.ray_trace_with_t(vantage, slot_location);

                    let mut view_blocked = false;
                    for (t, obstacle_collection) in obstacles {
                        // `ray_trace_with_t` reports the contents of both endpoints too
                        // (see its test in sparse3d.rs), but a voxel's own contents --
                        // or the vantage's -- shouldn't count as blocking the view of
                        // itself; only genuine obstacles strictly between the two count.
                        if t == 0.0 || t == 1.0 {
                            continue;
                        }

                        let mut any_transparent = false;
                        for obstacle in obstacle_collection {
                            if let [tall, decorative, passable, striated, ..] =
                                &embedding(obstacle)[..]
                            {
                                // HACK! Identify walls and floors:
                                let opaque = tall + decorative + passable + striated == 1.0
                                    && (tall == &1.0 || passable == &1.0);
                                // Also, uh, treat windows as opaque: (later I think we should just
                                // measure 'indoors' by whether a window is passed through)
                                let opaque = opaque || decorative == &0.75;
                                any_transparent = any_transparent || !opaque;
                            } else {
                                panic!()
                            }
                        }

                        if !any_transparent {
                            view_blocked = true;
                            break;
                        }
                    }

                    if view_blocked {
                        let voxel_slice = grid_coord_to_voxel_coord(
                            grid_pos,
                            min_coord,
                            slot,
                            visibility_channel,
                        );
                        voxels = voxels.slice_fill(voxel_slice, 1.0);
                        continue;
                    }

                    if let Some(cell) = sparse_data.get(slot_location) {
                        let mut emb = embedding(cell);
                        emb.push(0.5); // .visibility: regular structure.

                        for channel in 0..EMBEDDING_SIZE {
                            let voxel_slice =
                                grid_coord_to_voxel_coord(grid_pos, min_coord, slot, channel);
                            voxels = voxels.slice_fill(voxel_slice, emb[channel]);
                        }
                    }
                    // Else: visible open air with nothing placed there, reachable from the
                    // vantage without crossing a wall or window -- indoor open air.
                    // `.visibility` stays at its zero default.
                }
            }
        }
    }

    Ok(voxels)
}

// ─── Motif visibility translation ─────────────────────────────────────────────

/// Compiled `Motif` rules parsed from `buildables/motifs.autotile`.
fn motif_rules() -> Vec<AutotileOriented<Motif>> {
    let src = include_str!("../../buildables/motifs.autotile");
    let file = parse::<Motif>(src).expect("motifs.autotile parse failed");
    compile(&file)
}

/// `'='` matches the anchor's own structure name; other characters use the usual structure-type
/// predicates. Mirrors `autotile::display`'s `char_matches`.
fn motif_char_matches(
    ch: char,
    id: EorfId,
    _facing: Facing,
    anchor_name: &str,
    names: &[String],
) -> bool {
    let name = &names[id.as_usize()];
    match ch {
        '=' => name == anchor_name,
        other => char_matches_name(other, name),
    }
}

/// Whether `point` is visible from `vantage`: no opaque obstacle lies strictly between them.
/// Mirrors the visibility check baked into `sparse3d_to_tensor`'s `.visibility` channel.
fn point_visible_from(
    sparse_data: &Sparse3D<Cell>,
    vantage: RelSlotCoord,
    point: RelSlotCoord,
    structures: &[EorfInfo],
) -> bool {
    for (t, obstacles) in sparse_data.ray_trace_with_t(vantage, point) {
        // The endpoints' own contents don't block the view of themselves.
        if t == 0.0 || t == 1.0 {
            continue;
        }
        let any_transparent = obstacles.iter().any(|obstacle| {
            let emb = structures[obstacle.id.as_usize()].embedding.to_vec();
            let [tall, decorative, passable, striated, ..] = emb[..] else {
                panic!("embedding shorter than expected")
            };
            // HACK! Identify walls and floors (see `sparse3d_to_tensor`):
            let opaque =
                tall + decorative + passable + striated == 1.0 && (tall == 1.0 || passable == 1.0);
            let opaque = opaque || decorative == 0.75; // treat windows as opaque too
            !opaque
        });
        if !any_transparent {
            return false;
        }
    }
    true
}

/// Every `SlotCoord` a `MotifOccurrence` spans, from `base` to `base + (length - 1)` steps along
/// its axis (a single point when the axis is `None`).
fn occurrence_points(occ: &MotifOccurrence) -> impl Iterator<Item = SlotCoord> + '_ {
    let step = match occ.axis {
        MotifAxis::X => IVec3::new(1, 0, 0),
        MotifAxis::Y => IVec3::new(0, 1, 0),
        MotifAxis::Z => IVec3::new(0, 0, 1),
        MotifAxis::None => IVec3::ZERO,
    };
    let base = occ.base;
    (0..occ.length).map(move |i| SlotCoord {
        cube: base.cube + step * i as i32,
        slot: base.slot,
    })
}

/// Runs the Motif autotiler over `sparse_data`, then keeps only the `MotifOccurrence`s and
/// `DefectAtom`s with at least one point visible from `vantage` (per the raytracer).
///
/// TODO: translate the surviving occurrences/defects into whatever representation the QNN
/// actually consumes.
pub fn visible_motifs_and_defects(
    sparse_data: &Sparse3D<Cell>,
    vantage: RelSlotCoord,
    structures: &[EorfInfo],
) -> (Vec<MotifOccurrence>, Vec<DefectAtom>) {
    let rules = motif_rules();
    let names: Vec<String> = structures.iter().map(|s| s.name.clone()).collect();
    // `'='` (only meaningful relative to a named anchor's own structure) never matches here;
    // empty-anchored rules have no such anchor, so their dispatch character can't sensibly use it.
    let empty_index = build_empty_anchor_index(&rules, &names, |ch, id, _facing| {
        char_matches_name(ch, &names[id.as_usize()])
    });

    let mut atoms = Vec::new();
    for (loc, cell) in sparse_data.iter() {
        let anchor_name = &names[cell.id.as_usize()];
        let rel_loc: RelSlotCoord = loc.into();
        if let Some(results) = evaluate_autotile_rules(
            rel_loc,
            anchor_name,
            &rules,
            |l| sparse_data.get(l).map(|c| (c.id, c.facing)),
            |ch, id, facing| motif_char_matches(ch, id, facing, anchor_name, &names),
            |name, id| names[id.as_usize()] == name,
        ) {
            for motif in results {
                if matches!(motif, Motif::Discard) {
                    continue;
                }
                atoms.push(MotifAtom { motif, loc });
            }
        }

        // `@`-is-empty rules: `loc`/`anchor_name` here serve as the dispatch anchor (a real
        // structure), not `@` itself — the motif is recorded at the offset `@` position instead.
        for (out_loc, motif) in evaluate_empty_anchor_rules(
            rel_loc,
            anchor_name,
            &empty_index,
            |l| sparse_data.get(l).map(|c| (c.id, c.facing)),
            |ch, id, facing| motif_char_matches(ch, id, facing, anchor_name, &names),
            |name, id| names[id.as_usize()] == name,
        ) {
            if matches!(motif, Motif::Discard) {
                continue;
            }
            atoms.push(MotifAtom {
                motif,
                loc: out_loc.into(),
            });
        }
    }

    let (occurrences, defects) = collapse_motif_atoms(&atoms);

    let occurrences: Vec<MotifOccurrence> = occurrences
        .into_iter()
        .filter(|occ| {
            occurrence_points(occ)
                .any(|p| point_visible_from(sparse_data, vantage, p.into(), structures))
        })
        .collect();

    let defects: Vec<DefectAtom> = defects
        .into_iter()
        .filter(|d| point_visible_from(sparse_data, vantage, d.loc.into(), structures))
        .collect();

    (occurrences, defects)
}

/// Row width of `motif_occurrences_to_tensors`'s `interest` tensor: one-hot id, nonmundanity,
/// one-hot axis.
pub fn motif_interest_width() -> usize {
    motif_id_slots() + 1 + 4
}

/// Row width of `motif_occurrences_to_tensors`'s `order` tensor: `motif_interest_width` plus the
/// 3 distance fields.
pub fn motif_order_width() -> usize {
    motif_interest_width() + 3
}

/// Number of one-hot slots for a `MotifId`: sized to the largest id among `motif_rules()`'s
/// `Nonmundane` results. Those ids are assigned compactly (0, 1, 2, ... in order of each name's
/// first appearance -- see `Motif::finalize`), so this is exactly the number of distinct named
/// motifs, with no over-allocation. `Defect` ids (still per-line, and not fed into
/// `motif_occurrences_to_tensors`) are deliberately excluded from this count.
fn motif_id_slots() -> usize {
    fn max_nonmundane_id(cases: &[OrientedCase<Motif>]) -> Option<usize> {
        cases
            .iter()
            .filter_map(|c| match &c.result {
                Motif::Nonmundane { id, .. } => Some(id.0),
                Motif::Discard | Motif::Defect { .. } => None,
            })
            .max()
    }
    motif_rules()
        .iter()
        .filter_map(|rule| {
            max_nonmundane_id(&rule.cases).max(max_nonmundane_id(&rule.cases_plus_90))
        })
        .max()
        .map_or(0, |max_id| max_id + 1)
}

fn motif_axis_index(axis: MotifAxis) -> usize {
    match axis {
        MotifAxis::None => 0,
        MotifAxis::X => 1,
        MotifAxis::Y => 2,
        MotifAxis::Z => 3,
    }
}

/// The features shared by every occurrence of a given `MotifId`: a one-hot `id` (see
/// `motif_id_slots`), the `nonmundanity` score, then a one-hot `axis` (`None`/`X`/`Y`/`Z`).
fn motif_occurrence_features(occ: &MotifOccurrence, id_slots: usize) -> Vec<f32> {
    let mut row = vec![0.0; id_slots + 1 + 4];
    row[occ.id.0] = 1.0;
    row[id_slots] = occ.nonmundanity as f32;
    row[id_slots + 1 + motif_axis_index(occ.axis)] = 1.0;
    row
}

/// The grid-cell distance between two same-`MotifId` occurrences' `base` cubes, with the pair
/// ordered so the first nonzero of `(dx, dy, dz)` (in x, y, z priority) is positive.
fn canonical_occurrence_distance(a: &MotifOccurrence, b: &MotifOccurrence) -> IVec3 {
    let delta = b.base.cube - a.base.cube;
    let flip = match (delta.x, delta.y, delta.z) {
        (x, _, _) if x != 0 => x < 0,
        (_, y, _) if y != 0 => y < 0,
        (_, _, z) => z < 0,
    };
    if flip {
        -delta
    } else {
        delta
    }
}

/// Translates `MotifOccurrence`s (e.g. from `visible_motifs_and_defects`) into the two tensors
/// the QNN's motif branch consumes:
///
/// - `interest`: one row per occurrence: `[one-hot id, nonmundanity, one-hot axis]` (see
///   `motif_occurrence_features`).
/// - `order`: one row per unordered pair of occurrences sharing a `MotifId` -- and therefore
///   identical id/nonmundanity/axis, since those are assigned per-`MotifId` -- so a pair can only
///   differ in location. Each row holds that one copy of the shared features, followed by the
///   `(dx, dy, dz)` distance between the pair (see `canonical_occurrence_distance`).
pub fn motif_occurrences_to_tensors<B: Backend>(
    occurrences: &[MotifOccurrence],
) -> (Tensor<B, 2, Float>, Tensor<B, 2, Float>) {
    let device = Default::default();
    let id_slots = motif_id_slots();
    let interest_width = id_slots + 1 + 4;
    let order_width = interest_width + 3;

    let mut interest_data = Vec::with_capacity(occurrences.len() * interest_width);
    for occ in occurrences {
        interest_data.extend(motif_occurrence_features(occ, id_slots));
    }
    let interest = Tensor::from_data(
        TensorData::new(interest_data, [occurrences.len(), interest_width]),
        &device,
    );

    let mut order_data = Vec::new();
    let mut num_pairs = 0_usize;
    for (i, a) in occurrences.iter().enumerate() {
        for b in &occurrences[i + 1..] {
            if a.id != b.id {
                continue;
            }
            let distance = canonical_occurrence_distance(a, b);
            order_data.extend(motif_occurrence_features(a, id_slots));
            order_data.extend([distance.x as f32, distance.y as f32, distance.z as f32]);
            num_pairs += 1;
        }
    }
    let order = Tensor::from_data(
        TensorData::new(order_data, [num_pairs, order_width]),
        &device,
    );

    (interest, order)
}

/// The largest `h` such that at least `h` of `values` are themselves `>= h` (as in an "h-index").
/// A high h-index means many items are simultaneously large -- one enormous outlier can't inflate
/// it the way a sum or max could, which is the point of reaching for it here.
fn h_index(mut values: Vec<usize>) -> usize {
    values.sort_unstable_by(|a, b| b.cmp(a));
    values
        .iter()
        .enumerate()
        .take_while(|&(i, &v)| v >= i + 1)
        .count()
}

/// Row width of `motif_order_stats`'s output.
pub fn motif_order_stats_width() -> usize {
    3
}

/// Three hand-picked "h-index" statistics meant to proxy different flavors of repetition/alignment
/// among visible motifs -- an experiment to see whether handing the `order` model explicit
/// structure like this helps where raw pooled occurrence/pair features haven't:
///
/// 1. h-index of "occurrences per `MotifId`" -- high when many distinct motif classes are each
///    repeated many times (repetition richness).
/// 2. h-index of occurrence `length`s (one value per occurrence, not per class) -- high when many
///    occurrences are themselves long runs.
/// 3. h-index of "distinct `MotifId`s sharing a given length" (one value per distinct length seen)
///    -- high when many different lengths are each shared by several different motif types, i.e.
///    unrelated motifs lining up on the same scale.
pub fn motif_order_stats<B: Backend>(occurrences: &[MotifOccurrence]) -> Tensor<B, 2, Float> {
    let device = Default::default();

    let mut occurrences_per_id: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::new();
    let mut ids_per_length: std::collections::HashMap<u32, std::collections::HashSet<usize>> =
        std::collections::HashMap::new();
    let mut lengths = Vec::with_capacity(occurrences.len());
    for occ in occurrences {
        *occurrences_per_id.entry(occ.id.0).or_insert(0) += 1;
        ids_per_length
            .entry(occ.length)
            .or_default()
            .insert(occ.id.0);
        lengths.push(occ.length as usize);
    }

    let repetition_richness = h_index(occurrences_per_id.into_values().collect());
    let run_length_richness = h_index(lengths);
    let cross_motif_alignment =
        h_index(ids_per_length.into_values().map(|ids| ids.len()).collect());

    Tensor::from_data(
        TensorData::new(
            vec![
                repetition_richness as f32,
                run_length_richness as f32,
                cross_motif_alignment as f32,
            ],
            [1, motif_order_stats_width()],
        ),
        &device,
    )
}

#[allow(dead_code)]
pub fn print_voxels<B: Backend>(voxels: &Tensor<B, 5, Float>) {
    let [rooms, _channels, x_size, y_size, z_size] = voxels.dims();
    use std::fmt::Write;
    assert_eq!(rooms, 1);

    for y in 0..y_size {
        let mut has_anything_y = false;
        let mut slice = String::new();
        for x in 0..x_size {
            for z in 0..z_size {
                let voxel = voxels
                    .clone()
                    .slice(s![0, .., x, y, z])
                    .into_data()
                    .to_vec::<f32>()
                    .unwrap();

                for channel in &voxel {
                    if *channel > 0.0 {
                        has_anything_y = true;
                        write!(slice, "{}", (channel * 9.0) as u8).unwrap();
                    } else {
                        write!(slice, " ").unwrap();
                    }
                }
            }
            writeln!(slice).unwrap()
        }
        if has_anything_y {
            println!("{}", slice);
            println!("----")
        }
    }
}

#[cfg(test)]
#[cfg(feature = "training")]
mod tests {
    use assert2::check;

    use burn::backend;

    use super::*;
    use crate::sparse3d::Sparse3D;

    // Commented out as it depends on a CNN model not provided
    // #[test]
    // fn test_voxel_cnn() -> Result<(), Box<dyn Error>> {
    //     // Create dummy input data (replace with actual data loading)
    //     // Batch size of 1, 16 channels, depth 14, height 30, width 30
    //     let device = <Gpu as Backend>::Device::new(); // Modified backend initialization
    //     let input_data = Tensor::<Gpu, 5>::random(
    //         [1, EMBEDDING_SIZE as usize, INPUT_DEPTH as usize, INPUT_HEIGHT as usize, INPUT_WIDTH as usize],
    //         burn::tensor::Distribution::Standard,
    //         &device,
    //     );

    //     // Perform a forward pass and get the score
    //     // let score = cnn.score(&input_data)?; // cnn is not defined
    //     // println!("Predicted score: {}", score);

    //     Ok(())
    // }

    #[test]
    fn test_sparse3d_to_tensor() -> Result<(), Box<dyn Error>> {
        let si = crate::eorf::load_structure_info();

        type B = backend::Autodiff<backend::NdArray<f32, i32>>;

        let mut sparse_data: Sparse3D<usize> = Sparse3D::new();
        // Add some dummy data to the sparse grid
        sparse_data.set(RelSlotCoord::new(0, 0, 0, RelSlot::Room), 0);
        sparse_data.set(RelSlotCoord::new(1, 0, 0, RelSlot::XLoWall), 5);
        sparse_data.set(RelSlotCoord::new(0, 1, 0, RelSlot::Floor), 3);
        sparse_data.set(RelSlotCoord::new(0, 0, 1, RelSlot::ZLoWall), 5);
        sparse_data.set(RelSlotCoord::new(3, 0, 0, RelSlot::XLoWall), 6);

        let embedding = |id: &usize| vec![*id as f32, 0.0, 0.0, 0.0, 0.0];

        // Convert a region around (0, 0, 0) to a tensor
        let center_coord = IVec3::new(0, 0, 0);
        // TODO: there's a bunch of stuff that needs to stay in sync here!
        let tensor = sparse3d_to_tensor::<B, _, _>(&sparse_data, center_coord, |id| {
            si[*id].embedding.to_vec()
        })?;

        let expected_shape = Shape::new([1, EMBEDDING_SIZE, 23, 12, 23]);
        check!(tensor.dims() == expected_shape.dims());

        print_voxels(&tensor);

        check!(tensor.clone().sum().into_scalar() != 0.0);

        let tensor_way_far_away =
            sparse3d_to_tensor::<B, _, _>(&sparse_data, IVec3::new(50, 0, 0), embedding)?;

        check!(tensor_way_far_away.clone().sum().into_scalar() == 0.0);

        Ok(())
    }
}
