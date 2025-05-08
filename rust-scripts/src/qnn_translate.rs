use crate::sparse3d::{Slot, Sparse3D};
use crate::wall_grid::OfflineCell;
use burn::data::dataset::InMemDataset;
use burn::prelude::*;
use burn::tensor::{Float, TensorData};
use godot::builtin::Vector3i;
use std::collections::HashMap;
use std::error::Error;

use crate::structure::{self};

const INPUT_CHANNELS: usize = 16; // 16 colors

type Gpu = burn::backend::Autodiff<burn::backend::NdArray<f32, i32>>;

// Returns a 5D index into the voxels. Each grid cell is represented by a 2x2x2 cluster of voxels,
// with each slot occupying a particular position.
fn grid_coord_to_voxel_coord(
    pos: Vector3i,
    min: Vector3i,
    slot: Slot,
    channel: usize,
) -> [std::ops::Range<usize>; 5] {
    let adj_vec = (pos - min) * 2;
    let vox_vec = adj_vec
        + match slot {
            Slot::Room => Vector3i::new(0, 0, 0),
            Slot::ZWall => Vector3i::new(0, 0, 1),
            Slot::YFloor => Vector3i::new(0, 1, 0),
            Slot::XWall => Vector3i::new(1, 0, 0),
        };
    let x = vox_vec.x as usize;
    let y = vox_vec.y as usize;
    let z = vox_vec.z as usize;

    [0..1, channel..channel + 1, x..x + 1, y..y + 1, z..z + 1]
}

#[derive(Clone, Debug)]
pub struct GroundTruth {
    pub voxels: Tensor<Gpu, 5, Float>,
    pub scores: Tensor<Gpu, 1, Float>,
}

#[derive(Clone, Debug)]
pub struct GroundTruthBatcher {}

impl burn::data::dataloader::batcher::Batcher<GroundTruth, GroundTruth> for GroundTruthBatcher {
    fn batch(&self, ds: Vec<GroundTruth>) -> GroundTruth {
        let mut voxels: Vec<Tensor<Gpu, 5, Float>> = Vec::new();
        let mut scores: Vec<Tensor<Gpu, 1, Float>> = Vec::new();

        for gt in ds {
            voxels.push(gt.voxels);
            scores.push(gt.scores);
        }

        let voxels = Tensor::cat(voxels, 0);
        let scores = Tensor::cat(scores, 0);
        GroundTruth { voxels, scores }
    }
}

use std::{fs, path::Path};

pub fn load_training_data(
    directory: &str,
) -> (InMemDataset<GroundTruth>, InMemDataset<GroundTruth>) {
    let path = Path::new(directory);
    let mut training_data = Vec::new();

    let structures = structure::load_structures();
    let mut structures_by_char = HashMap::new();
    for (id, structure) in structures.iter().enumerate() {
        if let Some(x_char) = structure.info.x_char {
            structures_by_char.insert(x_char, id as i32);
        }
        if let Some(z_char) = structure.info.z_char {
            structures_by_char.insert(z_char, id as i32);
        }
    }

    for entry in fs::read_dir(path).expect("Failed to read directory") {
        let entry = entry.expect("Failed to read entry");
        let path = entry.path();

        if path.extension().map_or(false, |ext| ext == "txt") {
            let content = fs::read_to_string(&path).expect("Failed to read file");
            let sparse_data = crate::serialization::deserialize_sparse3d::<OfflineCell, _, ()>(
                &content,
                |c, _slot, structures_by_char| {
                    let id = crate::serialization::deserialize(c, structures_by_char);
                    Ok(OfflineCell {
                        id,
                        evaluation: None,
                    })
                },
                &structures_by_char,
            )
            .expect("Failed to deserialize");

            training_data.push(sparse3d_at_vantage(&sparse_data));
        }
    }
    let mut rng = rand::rng();
    use rand::seq::SliceRandom;
    training_data.shuffle(&mut rng);

    let split_index = (training_data.len() as f32 * 0.66).ceil() as usize;
    let (train_data, test_data) = training_data.split_at(split_index);

    (
        InMemDataset::new(train_data.to_vec()),
        InMemDataset::new(test_data.to_vec()),
    )
}

// Just handles a single datum, but the tensors could hold a batch
pub fn sparse3d_at_vantage(sparse_data: &Sparse3D<OfflineCell>) -> GroundTruth {
    for (pos, _slot, cell) in sparse_data.iter() {
        if let Some(eval) = &cell.evaluation {
            let tensor = sparse3d_to_tensor(
                sparse_data,
                /*center_coord=*/ pos,
                |cell| cell.id as usize,
            )
            .unwrap();

            return GroundTruth {
                voxels: tensor,
                scores: Tensor::from_data(TensorData::from([eval.symmetry]), &Default::default()),
            };
        }
    }
    panic!("No vantage found!")
}

/// Converts a region of Sparse3D data centered around a coordinate to a Tensor,
/// expanding each Sparse3D cell into a 2x2x2 voxel block.
pub fn sparse3d_to_tensor<T>(
    sparse_data: &Sparse3D<T>,
    center_coord: Vector3i,
    embedding: fn(&T) -> usize,
) -> Result<Tensor<Gpu, 5, Float>, Box<dyn Error>> {
    let device = Default::default();

    let min_coord = center_coord - Vector3i::new(5, 2, 5);
    let max_coord = center_coord + Vector3i::new(5, 3, 5);
    let size = max_coord - min_coord + Vector3i::new(1, 1, 1);

    let shape = Shape::new([
        1_usize,
        INPUT_CHANNELS as usize,
        (size.x * 2) as usize,
        (size.y * 2) as usize,
        (size.z * 2) as usize,
    ]);

    let mut voxels = Tensor::<Gpu, 5>::zeros(shape, &device);

    // Iterate through the Sparse3D coordinates within the bounding box
    for grid_x in min_coord.x..=max_coord.x {
        for grid_y in min_coord.y..=max_coord.y {
            for grid_z in min_coord.z..=max_coord.z {
                for slot in [Slot::Room, Slot::XWall, Slot::YFloor, Slot::ZWall] {
                    let grid_pos = Vector3i::new(grid_x, grid_y, grid_z);

                    if let Some(ref cell) = sparse_data.get(grid_pos, slot) {
                        let channel = embedding(cell);
                        assert!(channel < INPUT_CHANNELS);
                        // Right now, we're using a one-hot representation of grid elements
                        voxels = voxels.slice_assign(
                            grid_coord_to_voxel_coord(grid_pos, min_coord, slot, channel),
                            // A single 1.0, in five dimensions:
                            Tensor::<Gpu, 5, Float>::ones([1, 1, 1, 1, 1], &device),
                        );
                    }
                }
            }
        }
    }

    Ok(voxels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sparse3d::Sparse3D;

    // Commented out as it depends on a CNN model not provided
    // #[test]
    // fn test_voxel_cnn() -> Result<(), Box<dyn Error>> {
    //     // Create dummy input data (replace with actual data loading)
    //     // Batch size of 1, 16 channels, depth 14, height 30, width 30
    //     let device = <Gpu as Backend>::Device::new(); // Modified backend initialization
    //     let input_data = Tensor::<Gpu, 5>::random(
    //         [1, INPUT_CHANNELS as usize, INPUT_DEPTH as usize, INPUT_HEIGHT as usize, INPUT_WIDTH as usize],
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
        let mut sparse_data: Sparse3D<usize> = Sparse3D::new();
        // Add some dummy data to the sparse grid
        sparse_data.set(Vector3i::new(0, 0, 0), Slot::Room, 5);
        sparse_data.set(Vector3i::new(1, 0, 0), Slot::XWall, 10);
        sparse_data.set(Vector3i::new(0, 1, 0), Slot::YFloor, 2);
        sparse_data.set(Vector3i::new(0, 0, 1), Slot::ZWall, 11);

        // Convert a region around (0, 0, 0) to a tensor
        let center_coord = Vector3i::new(0, 0, 0);
        let tensor = sparse3d_to_tensor(&sparse_data, center_coord, |id| *id)?;

        // Check the shape of the resulting tensor
        let expected_shape = Shape::new([1, INPUT_CHANNELS, 22, 12, 22]);
        assert_eq!(
            tensor.dims(),
            expected_shape.dims(),
            "Tensor shape mismatch"
        );

        // Check that the tensor is not all zeros (some data should be present)

        assert_eq!(
            tensor.clone().sum().into_scalar(),
            4.0,
            "Tensor should have four entries"
        );

        let tensor_way_far_away =
            sparse3d_to_tensor(&sparse_data, Vector3i::new(50, 0, 0), |id| *id)?;

        assert_eq!(
            tensor_way_far_away.clone().sum().into_scalar(),
            0.0,
            "Tensor should be all zeros"
        );

        Ok(())
    }
}
