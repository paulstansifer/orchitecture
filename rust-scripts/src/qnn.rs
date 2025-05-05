use burn::{
    nn::{
        conv::{Conv3d, Conv3dConfig},
        Dropout, DropoutConfig, Linear, LinearConfig, PaddingConfig3d, Relu,
    },
    prelude::*,
};

type Gpu = burn::backend::Wgpu<f32, i32>;

#[derive(Module, Debug, Clone)]
pub struct Cnn {
    activation: Relu,
    dropout: Dropout,
    conv1: Conv3d<Gpu>,
    conv2: Conv3d<Gpu>,
    fc1: Linear<Gpu>,
    fc2: Linear<Gpu>,
}

impl Cnn {
    pub fn new(device: &Device<Gpu>) -> Self {
        let conv1 = Conv3dConfig::new([16, 32], [3, 3, 3])
            .with_padding(PaddingConfig3d::Same)
            .init(device);

        let conv2 = Conv3dConfig::new([32, 64], [3, 3, 3])
            .with_padding(PaddingConfig3d::Same)
            .init(device);

        // Calculate the output size after pooling (example: 10/2 = 5, 22/2 = 11)
        // If the input was [16, 10, 22, 22], after two pooling layers it becomes [64, 2, 5, 5] (approx.)
        let flattened_size = 64 * 2 * 5 * 5; // Example, adjust based on your layers

        let fc1 = LinearConfig::new(flattened_size, 128).init(device);
        let fc2 = LinearConfig::new(128, 1).init(device); // Output a single score

        let dropout = DropoutConfig::new(0.3).init();
        let activation = Relu::new();

        Self {
            activation: activation.clone(),
            dropout,
            conv1,
            conv2,
            fc1,
            fc2,
        }
    }

    pub fn forward(&self, x: Tensor<Gpu, 5>) -> Tensor<Gpu, 2> {
        let x = self.conv1.forward(x);
        let x = self.activation.forward(x);
        let x = self.dropout.forward(x);

        let x = self.conv2.forward(x);
        let x = self.activation.forward(x);
        let x = self.dropout.forward(x);
        let dims_left = x.dims().len() - 1;

        let x = x.flatten(1, dims_left); // Flatten from the channel dimension onwards
        let x = self.fc1.forward(x);
        let x = self.activation.forward(x);
        let x = self.dropout.forward(x);

        self.fc2.forward(x) // raw score
    }
}
