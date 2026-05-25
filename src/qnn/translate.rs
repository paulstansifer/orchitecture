use crate::sparse3d::{RelSlot, SlotLocation, Sparse3D};
use bevy::math::IVec3;
use burn::prelude::*;
use burn::tensor::Float;
use std::error::Error;

#[cfg(feature = "training")]
use crate::structure::{self, StructureInfo};
#[cfg(feature = "training")]
use crate::wall_grid::Cell;
#[cfg(feature = "training")]
use burn::backend::Autodiff;
#[cfg(feature = "training")]
use burn::data::dataset::InMemDataset;
#[cfg(feature = "training")]
use burn::tensor::TensorData;
#[cfg(feature = "training")]
use rand::rngs::StdRng;
#[cfg(feature = "training")]
use std::collections::HashMap;

pub const EMBEDDING_SIZE: usize = 4 + 1; // Keep this in sync with structure.rs (+ 1 for "indoors")

#[cfg(feature = "training")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Metric {
    Interest,
    Coherence,
}

#[cfg(feature = "training")]
impl std::fmt::Display for Metric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Metric::Interest => write!(f, "interest"),
            Metric::Coherence => write!(f, "coherence"),
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
    pub scores: Tensor<B, 1, Float>,
    pub filename: String,
}

#[cfg(feature = "training")]
fn convert_ground_truth_to_autodiff<B: Backend>(gt: GroundTruth<B>) -> GroundTruth<Autodiff<B>> {
    GroundTruth {
        voxels: Tensor::from_inner(gt.voxels),
        scores: Tensor::from_inner(gt.scores),
        filename: gt.filename,
    }
}

#[cfg(feature = "training")]
#[derive(Clone, Debug)]
pub struct GroundTruthBatcher {}

#[cfg(feature = "training")]
fn augment_datum(
    s: (Sparse3D<Cell>, String),
    metric: Metric,
    structure_info: &[StructureInfo],
    rng: &mut StdRng,
) -> Vec<(Sparse3D<Cell>, String)> {
    use crate::sparse3d::Rotateable;
    let mut res = vec![];

    if metric == Metric::Coherence {
        let messed_up = crate::build_helpers::add_noise(s.0.clone(), structure_info, rng);
        for messed in messed_up {
            res.push((messed, format!("{}-messed", s.1)));
        }
    }

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
        let mut scores: Vec<Tensor<B, 1, Float>> = Vec::new();
        let mut files: Vec<String> = Vec::new();

        for gt in ds {
            voxels.push(gt.voxels);
            scores.push(gt.scores);
            files.push(gt.filename);
        }

        let voxels = Tensor::cat(voxels, 0);
        let scores = Tensor::cat(scores, 0);
        GroundTruth {
            voxels,
            scores,
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

    let structures = structure::load_structure_info();
    let mut structures_by_char = HashMap::new();
    for (id, structure) in structures.iter().enumerate() {
        if let Some(x_char) = structure.x_char {
            structures_by_char.insert(x_char, id as i32);
        }
        if let Some(z_char) = structure.z_char {
            structures_by_char.insert(z_char, id as i32);
        }
    }

    for entry in fs::read_dir(path).expect("Failed to read directory") {
        let entry = entry.expect("Failed to read entry");
        let path = entry.path();

        if path.extension().map_or(false, |ext| ext == "txt") {
            let content = fs::read_to_string(&path).expect("Failed to read file");
            let sparse_data = crate::serialization::deserialize_sparse3d::<Cell, _, ()>(
                &content,
                |c, _slot, structures_by_char| {
                    let id = crate::serialization::deserialize(c, structures_by_char);
                    Ok(Cell {
                        id,
                        facing: crate::sparse3d::Facing::NegX, // TODO!!!
                        evaluation: None,
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
        for _ in 0..25 {
            all_sparse_data.push(crate::build_helpers::make_boring_room(
                &structures,
                &mut rng,
            ))
        }
    }

    // if metric == Metric::Coherence {
    //     for exemplar in all_sparse_data.iter().take(6) {
    //         println!("{}", exemplar.1);
    //         print_voxels(
    //             &ground_truth_at_vantage::<B>(exemplar, Metric::Coherence, &structures).voxels,
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
    //                 Metric::Coherence,
    //                 &structures,
    //             )
    //             .voxels,
    //         );
    //     }
    //     panic!()
    // }

    if metric == Metric::Interest {
        for _ in 0..25 {
            all_sparse_data.push(crate::build_helpers::make_boring_room(
                &structures,
                &mut rng,
            ))
        }
    }

    use rand::seq::SliceRandom;
    all_sparse_data.shuffle(&mut rng);

    let split_index = (all_sparse_data.len() as f32 * 0.66).ceil() as usize;
    let (t_rooms, v_rooms) = all_sparse_data.split_at(split_index);

    // Currently unclear if augmentation is helping at all; investigate more!
    let t_rooms: Vec<_> = t_rooms.into_iter().cloned().collect();
    let v_rooms: Vec<_> = v_rooms.into_iter().cloned().collect();

    let mut t_rooms_augmented = Vec::new();
    for data in t_rooms {
        t_rooms_augmented.extend(augment_datum(data, metric, &structures, &mut rng));
    }

    let mut v_rooms_augmented = Vec::new();
    for data in v_rooms {
        v_rooms_augmented.extend(augment_datum(data, metric, &structures, &mut rng));
    }

    let train_data: Vec<_> = t_rooms_augmented
        .into_iter()
        .map(|sparse_data| ground_truth_at_vantage(&sparse_data, metric, &structures))
        .map(convert_ground_truth_to_autodiff)
        .collect();

    let test_data: Vec<_> = v_rooms_augmented
        .into_iter()
        .map(|sparse_data| ground_truth_at_vantage(&sparse_data, metric, &structures))
        .collect();

    (InMemDataset::new(train_data), InMemDataset::new(test_data))
}

// Just handles a single datum, but the tensors could hold a batch
// Just handles a single datum, but the tensors could hold a batch
#[cfg(feature = "training")]
pub fn ground_truth_at_vantage<B: Backend>(
    data: &(Sparse3D<Cell>, String),
    metric: Metric,
    structures: &Vec<StructureInfo>,
) -> GroundTruth<B> {
    for (loc, cell) in data.0.iter() {
        if let Some(eval) = &cell.evaluation {
            let tensor = sparse3d_to_tensor(&data.0, /*center_coord=*/ loc.cube, |cell| {
                let semb = &structures[cell.id as usize].embedding;

                vec![semb.tall, semb.decorative, semb.passable, semb.striated]
            })
            .unwrap();
            let val = match metric {
                Metric::Interest => eval.interest,
                Metric::Coherence => eval.coherence,
            };

            // print_voxels(&tensor);

            return GroundTruth {
                voxels: tensor,
                scores: Tensor::from_data(TensorData::from([val]), &Default::default()),
                filename: data.1.clone(),
            };
        }
    }
    panic!("No vantage found! {}", data.1)
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
        EMBEDDING_SIZE as usize,
        (size.x * 2) as usize + 1,
        (size.y * 2) as usize,
        (size.z * 2) as usize + 1,
    ]);

    let mut voxels = Tensor::<B, 5>::zeros(shape, &device);

    let vantage = SlotLocation::new(
        center_coord.x,
        center_coord.y,
        center_coord.z,
        RelSlot::Room,
    );

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
                    let slot_location = SlotLocation::new(grid_x, grid_y, grid_z, slot);

                    let obstacles = sparse_data.ray_trace(vantage, slot_location);

                    let mut view_blocked = false;
                    for obstacle_collection in obstacles {
                        let mut any_transparent = false;
                        for obstacle in obstacle_collection {
                            if let [tall, decorative, passable, striated] =
                                &embedding(&obstacle)[..]
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
                        continue;
                    }

                    if let Some(cell) = sparse_data.get(slot_location) {
                        let mut emb = embedding(cell);
                        emb.push(1.0); // Window are opaque: if we can see it at all, it's indoors.

                        for channel in 0..EMBEDDING_SIZE {
                            let voxel_slice =
                                grid_coord_to_voxel_coord(grid_pos, min_coord, slot, channel);
                            voxels = voxels.slice_fill(voxel_slice, emb[channel]);
                        }
                    }
                }
            }
        }
    }

    Ok(voxels)
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
        let si = structure::load_structure_info();

        type B = backend::Autodiff<backend::NdArray<f32, i32>>;

        let mut sparse_data: Sparse3D<usize> = Sparse3D::new();
        // Add some dummy data to the sparse grid
        sparse_data.set(SlotLocation::new(0, 0, 0, RelSlot::Room), 0);
        sparse_data.set(SlotLocation::new(1, 0, 0, RelSlot::XLoWall), 5);
        sparse_data.set(SlotLocation::new(0, 1, 0, RelSlot::Floor), 3);
        sparse_data.set(SlotLocation::new(0, 0, 1, RelSlot::ZLoWall), 5);
        sparse_data.set(SlotLocation::new(3, 0, 0, RelSlot::XLoWall), 6);

        let embedding = |id: &usize| vec![*id as f32, 0.0, 0.0, 0.0];

        // Convert a region around (0, 0, 0) to a tensor
        let center_coord = IVec3::new(0, 0, 0);
        // TODO: there's a bunch of stuff that needs to stay in sync here!
        let tensor = sparse3d_to_tensor::<B, _, _>(&sparse_data, center_coord, |id| {
            let semb = &si[*id].embedding;

            vec![semb.tall, semb.decorative, semb.passable, semb.striated]
        })?;

        // Check the shape of the resulting tensor
        let expected_shape = Shape::new([1, EMBEDDING_SIZE, 23, 12, 23]);
        assert_eq!(
            tensor.dims(),
            expected_shape.dims(),
            "Tensor shape mismatch"
        );

        // Check that the tensor is not all zeros (some data should be present)

        print_voxels(&tensor);

        assert_ne!(
            tensor.clone().sum().into_scalar(),
            0.0,
            "Tensor should have some entries"
        );

        let tensor_way_far_away =
            sparse3d_to_tensor::<B, _, _>(&sparse_data, IVec3::new(50, 0, 0), embedding)?;

        assert_eq!(
            tensor_way_far_away.clone().sum().into_scalar(),
            0.0,
            "Tensor should be all zeros"
        );

        Ok(())
    }
}
