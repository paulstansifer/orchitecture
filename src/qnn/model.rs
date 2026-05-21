use burn::nn::Sigmoid;
use burn::{
    nn::{
        conv::{Conv3d, Conv3dConfig},
        Dropout, DropoutConfig, Linear, LinearConfig, PaddingConfig3d,
    },
    prelude::*,
};
use serde::{Deserialize, Serialize};

#[cfg(feature = "training")]
use burn::{
    backend::Autodiff,
    tensor::backend::AutodiffBackend,
    train::{RegressionOutput, TrainOutput},
};
#[cfg(feature = "training")]
use super::translate::GroundTruth;

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "training", derive(clap::Parser))]
pub struct Args {
    #[cfg_attr(feature = "training", arg(short, long, default_value = "5/12,3/24"))]
    pub conv: String,

    #[cfg_attr(feature = "training", arg(short, long, default_value = "128,64,32"))]
    pub fc: String,

    #[cfg_attr(feature = "training", arg(short, long, default_value = "1.0e-5"))]
    pub lr: f64,

    #[cfg_attr(feature = "training", arg(short, long, default_value = "10"))]
    pub epochs: usize,

    #[cfg_attr(feature = "training", arg(short, long, default_value = "42"))]
    pub seed: u64,

    #[cfg_attr(feature = "training", arg(long, action = clap::ArgAction::SetTrue))]
    pub show_scores: bool,

    #[cfg_attr(feature = "training", arg(long, action = clap::ArgAction::SetTrue))]
    pub fake_data: bool,
}

#[derive(Module, Debug)]
pub struct Cnn<B: Backend> {
    relu: nn::LeakyRelu,
    sigmoid: Sigmoid,
    dropout: Dropout,
    conv: Vec<Conv3d<B>>,
    pub fc: Vec<Linear<B>>,
}

impl<B: Backend> Cnn<B> {
    pub fn new(device: &<B as Backend>::Device, args: &Args) -> Self {
        let mut features = super::translate::EMBEDDING_SIZE;
        let mut conv = vec![];
        if args.conv.contains("/") {
            for conv_spec in args.conv.split(",") {
                let (size, next_features) = conv_spec.split_once("/").unwrap();
                let size: usize = size.parse().unwrap();
                let next_features: usize = next_features.parse().unwrap();
                conv.push(
                    Conv3dConfig::new([features, next_features], [size, size, size])
                        .with_padding(PaddingConfig3d::Same)
                        .init(device),
                );
                features = next_features;
            }
        }

        // Calculate the output size after pooling
        let mut nodes = features * 12 * 23 * 23;
        let mut fc = vec![];
        for fc_spec in args.fc.split(",") {
            let next_nodes: usize = fc_spec.parse().unwrap();
            fc.push(LinearConfig::new(nodes, next_nodes).init(device));
            nodes = next_nodes;
        }
        fc.push(LinearConfig::new(nodes, 1).init(device)); // Output a single score

        let dropout = DropoutConfig::new(0.3).init();

        Self {
            relu: nn::LeakyReluConfig::new().init(),
            sigmoid: Sigmoid::new(),
            dropout,
            conv,
            fc,
        }
    }

    pub fn forward(&self, mut x: Tensor<B, 5>) -> Tensor<B, 2> {
        // let x = self.conv[0].forward(x);
        // let x = self.relu.forward(x);
        // let x = self.dropout.forward(x);
        // let x = self.conv[1].forward(x);
        // let x = self.relu.forward(x);
        // let x = self.dropout.forward(x);
        // let dims_left = x.dims().len() - 1;

        // let x: Tensor<B, 2> = x.flatten(1, dims_left); // Flatten from the channel dimension onwards
        // let x = self.fc[0].forward(x);
        // let x = self.relu.forward(x);
        // let x = self.dropout.forward(x);

        // let x = self.fc[1].forward(x);
        // let x = self.relu.forward(x);
        // let x = self.dropout.forward(x);

        // let x = self.fc[2].forward(x);
        // self.sigmoid.forward(x)

        for conv in &self.conv {
            x = conv.forward(x);
            x = self.relu.forward(x);
            x = self.dropout.forward(x)
        }

        let dims_left = x.dims().len() - 1;

        // Flatten from the channel dimension onwards
        let mut x: Tensor<B, 2> = x.flatten(1, dims_left);

        for fc in self.fc.iter().take(self.fc.len() - 1) {
            x = fc.forward(x);
            x = self.relu.forward(x);
            x = self.dropout.forward(x);
        }
        x = self.fc.last().unwrap().forward(x);

        self.sigmoid.forward(x)
    }

    #[cfg(feature = "training")]
    pub fn forward_classification(
        &self,
        rooms: Tensor<B, 5>,
        targets: Tensor<B, 1, Float>,
    ) -> RegressionOutput<B> {
        let output = self.forward(rooms);
        let batch_size = output.dims()[0];

        // We have only one output metric:
        let targets_reshaped = targets.clone().reshape([batch_size, 1]);

        let loss = nn::loss::MseLoss::new().forward(
            output.clone(),
            targets_reshaped.clone(),
            nn::loss::Reduction::Mean,
        );

        let output_reshaped = output.clone().reshape([batch_size, 1]);

        RegressionOutput::new(loss, output_reshaped, targets_reshaped)
    }
}

#[cfg(feature = "training")]
impl<B: AutodiffBackend> burn::train::TrainStep<GroundTruth<B>, RegressionOutput<B>> for Cnn<B> {
    fn step(&self, batch: GroundTruth<B>) -> TrainOutput<RegressionOutput<B>> {
        let item = self.forward_classification(batch.voxels, batch.scores);
        TrainOutput::new::<B, Cnn<B>>(self, item.loss.backward(), item)
    }
}

#[cfg(feature = "training")]
impl<B: Backend> burn::train::ValidStep<GroundTruth<B>, RegressionOutput<B>> for Cnn<B> {
    fn step(&self, batch: GroundTruth<B>) -> RegressionOutput<B> {
        self.forward_classification(batch.voxels, batch.scores)
    }
}
