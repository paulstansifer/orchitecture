// This file contains the implementation of a 3D convolutional neural network
// for processing voxel data in the Godot City Template project.

use crate::sparse3d::{Slot, Sparse3D};
use godot::builtin::Vector3i;
use std::error::Error;
use tch::nn::Adam;
use tch::nn::Module;
use tch::nn::OptimizerConfig;
use tch::Reduction;
use tch::Scalar;
use tch::{nn, Kind, Tensor}; // Added import for Scalar

// Define the input dimensions
const INPUT_DEPTH: i64 = 14; // 7 * 2
const INPUT_HEIGHT: i64 = 30; // 15 * 2
const INPUT_WIDTH: i64 = 30; // 15 * 2
const INPUT_CHANNELS: i64 = 16; // 16 colors

// Define the neural network structure
pub struct VoxelCNN {
    vs: nn::VarStore,
    conv1: nn::Conv3D,
    conv2: nn::Conv3D,
    fc1: nn::Linear,
    fc2: nn::Linear,
}

impl VoxelCNN {
    /// Creates a new instance of the VoxelCNN.
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let vs = nn::VarStore::new(tch::Device::Cpu);
        let root = vs.root();

        // Define Conv3D layers using nn::conv3d function
        let conv1 = nn::conv3d(
            &root / "conv1",
            INPUT_CHANNELS,
            32,
            3,
            nn::ConvConfig {
                padding: 1,
                ..Default::default()
            },
        );

        let conv2 = nn::conv3d(
            &root / "conv2",
            32,
            64,
            3,
            nn::ConvConfig {
                padding: 1,
                ..Default::default()
            },
        );

        // Define linear layers
        // Calculate flat_size based on the output shape after conv and pooling
        // Create a dummy sequential model just to calculate the output shape
        let dummy_seq = nn::seq()
            .add(nn::conv3d(
                // Used nn::conv3d for dummy
                &root / "dummy_conv1",
                INPUT_CHANNELS,
                32,
                3,
                nn::ConvConfig {
                    padding: 1,
                    ..Default::default()
                },
            ))
            .add_fn(|xs| xs.relu())
            .add_fn(|xs| xs.max_pool3d(&[2, 2, 2], &[2, 2, 2], &[0, 0, 0], &[1, 1, 1], false))
            .add(nn::conv3d(
                // Used nn::conv3d for dummy
                &root / "dummy_conv2",
                32,
                64,
                3,
                nn::ConvConfig {
                    padding: 1,
                    ..Default::default()
                },
            ))
            .add_fn(|xs| xs.relu())
            .add_fn(|xs| xs.max_pool3d(&[2, 2, 2], &[2, 2, 2], &[0, 0, 0], &[1, 1, 1], false));

        let dummy_input = Tensor::zeros(
            &[1, INPUT_CHANNELS, INPUT_DEPTH, INPUT_HEIGHT, INPUT_WIDTH],
            (Kind::Float, tch::Device::Cpu),
        );
        let output_shape = dummy_seq.forward(&dummy_input).size();
        let flat_size: i64 = output_shape.iter().skip(1).product();

        let fc1 = nn::linear(&root / "fc1", flat_size, 128, Default::default());
        let fc2 = nn::linear(&root / "fc2", 128, 1, Default::default());

        Ok(VoxelCNN {
            vs,
            conv1,
            conv2,
            fc1,
            fc2,
        })
    }

    /// Performs a forward pass through the network.
    ///
    /// Accepts input voxel data and returns a score.
    pub fn forward(&self, input: &Tensor) -> Result<Tensor, Box<dyn Error>> {
        let mut output = input.apply(&self.conv1).relu();
        output = output.max_pool3d(&[2, 2, 2], &[2, 2, 2], &[0, 0, 0], &[1, 1, 1], false);

        output = output.apply(&self.conv2).relu();
        output = output.max_pool3d(&[2, 2, 2], &[2, 2, 2], &[0, 0, 0], &[1, 1, 1], false);

        // Flatten the output and apply linear layers for scoring
        let flat_size = output.size().iter().skip(1).product(); // Recalculate flattened size
        let flat_output = output.view([-1, flat_size]);

        let score = flat_output.apply(&self.fc1).relu().apply(&self.fc2);

        Ok(score)
    }

    /// Trains the neural network.
    ///
    /// Accepts input data and corresponding target scores for training.
    pub fn train(
        &mut self,
        train_data: &[(Tensor, Tensor)],
        val_data: &[(Tensor, Tensor)],
        epochs: usize,
        learning_rate: f64,
    ) -> Result<(), Box<dyn Error>> {
        let mut opt = Adam::default().build(&self.vs, learning_rate)?;

        for epoch in 1..=epochs {
            let mut train_loss = 0.0;
            for (input, target) in train_data.iter() {
                let prediction = self.forward(input)?;
                let loss = prediction.mse_loss(target, Reduction::Mean);
                opt.backward_step(&loss);

                train_loss += loss.double_value(&[]); // Use the converted loss value
            }
            train_loss /= train_data.len() as f64;

            let mut val_loss = 0.0;
            for (input, target) in val_data.iter() {
                let prediction = self.forward(input)?;
                let loss = prediction.mse_loss(target, Reduction::Mean);

                val_loss += loss.double_value(&[]); // Use the converted loss value
            }
            val_loss /= val_data.len() as f64;

            println!(
                "Epoch {} - Train Loss: {:.4} - Val Loss: {:.4}",
                epoch, train_loss, val_loss
            );
        }

        Ok(())
    }

    /// Evaluates the neural network and returns a score for given input data.
    ///
    /// This is a simplified scoring function, you might want a more complex one
    /// depending on your application.
    pub fn score(&self, input: &Tensor) -> Result<f64, Box<dyn Error>> {
        let score_tensor = self.forward(input)?;
        // Assuming the output is a single f64
        let score = score_tensor.double_value(&[]);
        Ok(score)
    }
}

/// Converts a region of Sparse3D data centered around a coordinate to a Tensor,
/// expanding each Sparse3D cell into a 2x2x2 voxel block.
pub fn sparse3d_to_tensor(
    sparse_data: &Sparse3D<i32>,
    center_coord: Vector3i,
) -> Result<Tensor, Box<dyn Error>> {
    // Initialize a zero tensor with the desired shape (batch_size=1, channels=16, depth=14, height=30, width=30)
    let tensor = Tensor::zeros(
        // Removed mut
        &[1, INPUT_CHANNELS, INPUT_DEPTH, INPUT_HEIGHT, INPUT_WIDTH],
        tch::kind::FLOAT_CPU,
    );

    // Define the bounding box for the 15x15x7 region in Sparse3D coordinates
    let min_sparse_x = center_coord.x - 7;
    let max_sparse_x = center_coord.x + 7;
    let min_sparse_y = center_coord.y - 7;
    let max_sparse_y = center_coord.y + 7;
    let min_sparse_z = center_coord.z - 3;
    let max_sparse_z = center_coord.z + 3;

    // Iterate through the Sparse3D coordinates within the bounding box
    for sparse_x in min_sparse_x..=max_sparse_x {
        for sparse_y in min_sparse_y..=max_sparse_y {
            for sparse_z in min_sparse_z..=max_sparse_z {
                let current_sparse_coord = Vector3i::new(sparse_x, sparse_y, sparse_z);

                // Calculate the starting indices for the 2x2x2 block in the output tensor
                let base_tensor_x = ((sparse_x - min_sparse_x) * 2) as i64; // Cast to i64
                let base_tensor_y = ((sparse_y - min_sparse_y) * 2) as i64; // Cast to i64
                let base_tensor_z = ((sparse_z - min_sparse_z) * 2) as i64; // Cast to i64

                // Define the mapping from Slot to position within the 2x2x2 block
                let slot_positions: [(Slot, (i64, i64, i64)); 4] = [
                    (Slot::Room, (0, 0, 0)),
                    (Slot::XWall, (1, 0, 0)),
                    (Slot::YFloor, (0, 1, 0)),
                    (Slot::ZWall, (0, 0, 1)),
                ];

                // Process each slot
                for (slot, (dx, dy, dz)) in &slot_positions {
                    let tensor_x = base_tensor_x + dx;
                    let tensor_y = base_tensor_y + dy;
                    let tensor_z = base_tensor_z + dz;

                    if let Some(id) = sparse_data.get(current_sparse_coord, *slot) {
                        // Set the channel corresponding to the slot (0-3)
                        let slot_channel = match slot {
                            Slot::Room => 0,
                            Slot::XWall => 1,
                            Slot::YFloor => 2,
                            Slot::ZWall => 3,
                        };
                        let _ = tensor
                            .get(0)
                            .get(slot_channel as i64)
                            .get(tensor_z)
                            .get(tensor_y)
                            .get(tensor_x)
                            .fill_(1.0);

                        // Map id (0-11) to channels 4-15
                        if *id >= 0 && *id < 12 {
                            let _ = tensor
                                .get(0)
                                .get(4 + *id as i64)
                                .get(tensor_z)
                                .get(tensor_y)
                                .get(tensor_x)
                                .fill_(1.0);
                        }
                    } else {
                        // If no data for this slot, set the slot channel to 1.0 and other channels to 0.0
                        let slot_channel = match slot {
                            Slot::Room => 0,
                            Slot::XWall => 1,
                            Slot::YFloor => 2,
                            Slot::ZWall => 3,
                        };
                        let _ = tensor
                            .get(0)
                            .get(slot_channel as i64)
                            .get(tensor_z)
                            .get(tensor_y)
                            .get(tensor_x)
                            .fill_(1.0);
                    }
                }
            }
        }
    }

    Ok(tensor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sparse3d::Sparse3D;
    use tch::Device; // Import Sparse3D for tests

    #[test]
    fn test_voxel_cnn() -> Result<(), Box<dyn Error>> {
        // Initialize the CNN
        let cnn = VoxelCNN::new()?;

        // Create dummy input data (replace with actual data loading)
        // Batch size of 1, 16 channels, depth 14, height 30, width 30
        let input_data = Tensor::rand(
            &[1, INPUT_CHANNELS, INPUT_DEPTH, INPUT_HEIGHT, INPUT_WIDTH],
            tch::kind::FLOAT_CPU,
        );

        // Perform a forward pass and get the score
        let score = cnn.score(&input_data)?;
        println!("Predicted score: {}", score);

        Ok(())
    }

    #[test]
    fn test_sparse3d_to_tensor() -> Result<(), Box<dyn Error>> {
        let mut sparse_data: Sparse3D<i32> = Sparse3D::new();
        // Add some dummy data to the sparse grid
        sparse_data.set(Vector3i::new(0, 0, 0), Slot::Room, 5);
        sparse_data.set(Vector3i::new(1, 0, 0), Slot::XWall, 10);
        sparse_data.set(Vector3i::new(0, 1, 0), Slot::YFloor, 2);
        sparse_data.set(Vector3i::new(0, 0, 1), Slot::ZWall, 11);

        // Convert a region around (0, 0, 0) to a tensor
        let center_coord = Vector3i::new(0, 0, 0);
        let tensor = sparse3d_to_tensor(&sparse_data, center_coord)?;

        // Assert the shape of the output tensor
        assert_eq!(
            tensor.size(),
            &[1, INPUT_CHANNELS, INPUT_DEPTH, INPUT_HEIGHT, INPUT_WIDTH]
        );

        let score = VoxelCNN::new().unwrap().score(&tensor)?;

        assert!(score.is_finite(), "Score should be a finite number");

        Ok(())
    }
}
