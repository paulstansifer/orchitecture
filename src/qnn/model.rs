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
use super::translate::{GroundTruth, ScoreConstraint};
#[cfg(feature = "training")]
use burn::{
    tensor::backend::AutodiffBackend,
    train::{RegressionOutput, TrainOutput},
};

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "training", derive(clap::Parser))]
pub struct Args {
    #[cfg_attr(feature = "training", arg(short, long, default_value = "5/12,3/24"))]
    pub conv: String,

    #[cfg_attr(feature = "training", arg(short, long, default_value = "128,64,32"))]
    pub fc: String,

    /// Sizes of `MotifNn`'s per-row shared layers: one MLP stack (this size spec) applied
    /// independently to every row of the `interest` occurrence set, and a separate one (same
    /// sizes, its own weights, since the row width differs) applied to every row of the `order`
    /// pair set -- before each set is sum-pooled into a single fixed-size vector for `fc`.
    #[cfg_attr(feature = "training", arg(short, long, default_value = "64,32"))]
    pub motif_shared: String,

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
    pub fn new(device: &B::Device, args: &Args) -> Self {
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
        constraint: ScoreConstraint,
    ) -> RegressionOutput<B> {
        let output = self.forward(rooms);
        let batch_size = output.dims()[0];

        let targets_reshaped = targets.clone().reshape([batch_size, 1]);
        let output_reshaped = output.reshape([batch_size, 1]);

        let loss = match constraint {
            ScoreConstraint::Exact => nn::loss::MseLoss::new().forward(
                output_reshaped.clone(),
                targets_reshaped.clone(),
                nn::loss::Reduction::Mean,
            ),
            ScoreConstraint::AtMost => {
                // Zero loss when pred ≤ target; penalise (pred - target)² otherwise.
                let excess = (output_reshaped.clone() - targets_reshaped.clone()).clamp_min(0.0);
                (excess.clone() * excess).mean()
            }
            ScoreConstraint::AtLeast => {
                // Zero loss when pred ≥ target; penalise (target - pred)² otherwise.
                let deficit = (targets_reshaped.clone() - output_reshaped.clone()).clamp_min(0.0);
                (deficit.clone() * deficit).mean()
            }
        };

        RegressionOutput::new(loss, output_reshaped, targets_reshaped)
    }
}

#[cfg(feature = "training")]
impl<B: AutodiffBackend> burn::train::TrainStep for Cnn<B> {
    type Input = GroundTruth<B>;
    type Output = RegressionOutput<B>;

    fn step(&self, batch: GroundTruth<B>) -> TrainOutput<RegressionOutput<B>> {
        let item = self.forward_classification(batch.voxels, batch.scores, batch.constraint);
        TrainOutput::new::<B, Cnn<B>>(self, item.loss.backward(), item)
    }
}

#[cfg(feature = "training")]
impl<B: Backend> burn::train::InferenceStep for Cnn<B> {
    type Input = GroundTruth<B>;
    type Output = RegressionOutput<B>;

    fn step(&self, batch: GroundTruth<B>) -> RegressionOutput<B> {
        self.forward_classification(batch.voxels, batch.scores, batch.constraint)
    }
}

/// Scores a room from its visible motifs alone (`GroundTruth::motif_interest`/`motif_order`, see
/// `motif_occurrences_to_tensors`), with no convolutional layers. Each set is run through its own
/// per-row MLP -- the same weights applied independently to every row, so the network works on
/// however many occurrences/pairs a room happens to have -- then summed over rows (a
/// permutation-invariant "sum-pool") into one fixed-size vector per set. The two pooled vectors
/// are concatenated and fed to `fc`, exactly like `Cnn`'s flattened conv output.
///
/// Only meaningful with batch_size=1: sum-pooling collapses the row dimension per room, but
/// nothing marks where one room's rows end and the next begin if `GroundTruthBatcher` is ever fed
/// more than one room at once (see its `batch` method).
#[derive(Module, Debug)]
pub struct MotifNn<B: Backend> {
    relu: nn::LeakyRelu,
    sigmoid: Sigmoid,
    dropout: Dropout,
    interest_shared: Vec<Linear<B>>,
    order_shared: Vec<Linear<B>>,
    pub fc: Vec<Linear<B>>,
}

impl<B: Backend> MotifNn<B> {
    fn shared_layers(
        device: &B::Device,
        args: &Args,
        input_width: usize,
    ) -> (Vec<Linear<B>>, usize) {
        let mut features = input_width;
        let mut layers = vec![];
        for spec in args.motif_shared.split(",") {
            let next_features: usize = spec.parse().unwrap();
            layers.push(LinearConfig::new(features, next_features).init(device));
            features = next_features;
        }
        (layers, features)
    }

    pub fn new(device: &B::Device, args: &Args) -> Self {
        let (interest_shared, interest_pooled_size) =
            Self::shared_layers(device, args, super::translate::motif_interest_width());
        let (order_shared, order_pooled_size) =
            Self::shared_layers(device, args, super::translate::motif_order_width());

        let mut nodes = interest_pooled_size + order_pooled_size;
        let mut fc = vec![];
        for fc_spec in args.fc.split(",") {
            let next_nodes: usize = fc_spec.parse().unwrap();
            fc.push(LinearConfig::new(nodes, next_nodes).init(device));
            nodes = next_nodes;
        }
        fc.push(LinearConfig::new(nodes, 1).init(device)); // Output a single score

        Self {
            relu: nn::LeakyReluConfig::new().init(),
            sigmoid: Sigmoid::new(),
            dropout: DropoutConfig::new(0.3).init(),
            interest_shared,
            order_shared,
            fc,
        }
    }

    /// Runs `layers` independently over every row of `x` (`[rows, features]`), then sum-pools
    /// over the row dimension into a single `[1, features]` vector.
    fn pool_branch(&self, layers: &[Linear<B>], mut x: Tensor<B, 2>) -> Tensor<B, 2> {
        // A room with no visible occurrences/pairs is a legitimate `[0, features]` input, and
        // sum-pooling an empty set should just be zero -- but some backends' matmul kernels
        // choke on a zero-sized dimension, so skip the layers entirely rather than feed them
        // through.
        if x.dims()[0] == 0 {
            let device = x.device();
            let width = layers
                .last()
                .map(|l| l.weight.dims()[1])
                .unwrap_or(x.dims()[1]);
            return Tensor::zeros([1, width], &device);
        }
        for layer in layers {
            x = layer.forward(x);
            x = self.relu.forward(x);
            x = self.dropout.forward(x);
        }
        x.sum_dim(0)
    }

    pub fn forward(&self, interest: Tensor<B, 2>, order: Tensor<B, 2>) -> Tensor<B, 2> {
        let interest_pooled = self.pool_branch(&self.interest_shared, interest);
        let order_pooled = self.pool_branch(&self.order_shared, order);

        let mut x = Tensor::cat(vec![interest_pooled, order_pooled], 1);

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
        interest: Tensor<B, 2>,
        order: Tensor<B, 2>,
        targets: Tensor<B, 1, Float>,
        constraint: ScoreConstraint,
    ) -> RegressionOutput<B> {
        let output = self.forward(interest, order);
        let batch_size = output.dims()[0];

        let targets_reshaped = targets.clone().reshape([batch_size, 1]);
        let output_reshaped = output.reshape([batch_size, 1]);

        let loss = match constraint {
            ScoreConstraint::Exact => nn::loss::MseLoss::new().forward(
                output_reshaped.clone(),
                targets_reshaped.clone(),
                nn::loss::Reduction::Mean,
            ),
            ScoreConstraint::AtMost => {
                let excess = (output_reshaped.clone() - targets_reshaped.clone()).clamp_min(0.0);
                (excess.clone() * excess).mean()
            }
            ScoreConstraint::AtLeast => {
                let deficit = (targets_reshaped.clone() - output_reshaped.clone()).clamp_min(0.0);
                (deficit.clone() * deficit).mean()
            }
        };

        RegressionOutput::new(loss, output_reshaped, targets_reshaped)
    }
}

#[cfg(feature = "training")]
impl<B: AutodiffBackend> burn::train::TrainStep for MotifNn<B> {
    type Input = GroundTruth<B>;
    type Output = RegressionOutput<B>;

    fn step(&self, batch: GroundTruth<B>) -> TrainOutput<RegressionOutput<B>> {
        let item = self.forward_classification(
            batch.motif_interest,
            batch.motif_order,
            batch.scores,
            batch.constraint,
        );
        TrainOutput::new::<B, MotifNn<B>>(self, item.loss.backward(), item)
    }
}

#[cfg(feature = "training")]
impl<B: Backend> burn::train::InferenceStep for MotifNn<B> {
    type Input = GroundTruth<B>;
    type Output = RegressionOutput<B>;

    fn step(&self, batch: GroundTruth<B>) -> RegressionOutput<B> {
        self.forward_classification(
            batch.motif_interest,
            batch.motif_order,
            batch.scores,
            batch.constraint,
        )
    }
}
