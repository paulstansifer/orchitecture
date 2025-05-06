use std::path::PathBuf;

use burn::{
    backend::wgpu::WgpuDevice,
    data::dataloader::batcher::Batcher,
    nn::{
        conv::{Conv3d, Conv3dConfig},
        Dropout, DropoutConfig, Linear, LinearConfig, PaddingConfig3d, Relu,
    },
    optim::AdamConfig,
    prelude::*,
    record::{DefaultFileRecorder, FileRecorder, HalfPrecisionSettings},
    train::RegressionOutput,
};

type Gpu = burn::backend::Autodiff<burn::backend::Wgpu<f32, i32>>;

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
        let flattened_size = 64 * 12 * 22 * 22; // Example, adjust based on your layers

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

        let x: Tensor<Gpu, 2> = x.flatten(1, dims_left); // Flatten from the channel dimension onwards
        let x = self.fc1.forward(x);
        let x = self.activation.forward(x);
        let x = self.dropout.forward(x);

        self.fc2.forward(x) // raw score
    }

    pub fn forward_classification(
        &self,
        rooms: Tensor<Gpu, 5>,
        targets: Tensor<Gpu, 1, Float>,
    ) -> RegressionOutput<Gpu> {
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

use burn::train::TrainOutput;
use serde::de;

use crate::qnn_translate::{load_training_data, GroundTruth, GroundTruthBatcher};

impl burn::train::TrainStep<GroundTruth, RegressionOutput<Gpu>> for Cnn {
    fn step(&self, batch: GroundTruth) -> TrainOutput<RegressionOutput<Gpu>> {
        let item = self.forward_classification(batch.voxels, batch.scores);

        TrainOutput::new::<Gpu, Cnn>(self, item.loss.backward(), item)
    }
}

impl burn::train::ValidStep<GroundTruth, RegressionOutput<Gpu>> for Cnn {
    fn step(&self, batch: GroundTruth) -> RegressionOutput<Gpu> {
        self.forward_classification(batch.voxels, batch.scores)
    }
}

#[derive(Config)]
pub struct TrainingConfig {
    pub optimizer: burn::optim::AdamConfig,
    #[config(default = 5)]
    pub num_epochs: usize,
    #[config(default = 1)]
    pub batch_size: usize,
    #[config(default = 1)]
    pub num_workers: usize,
    #[config(default = 42)]
    pub seed: u64,
    #[config(default = 1.0e-4)]
    pub learning_rate: f64,
}

fn create_artifact_dir(artifact_dir: &str) {
    // Remove existing artifacts before to get an accurate learner summary
    //std::fs::remove_dir_all(artifact_dir).unwrap();
    std::fs::create_dir_all(artifact_dir).unwrap();
    println!("{artifact_dir} created successfully");
}

pub fn train() {
    let device: WgpuDevice = Default::default();
    let config = TrainingConfig::new(AdamConfig::new());
    let artifact_dir = "/tmp/artifacts/";
    create_artifact_dir(artifact_dir);
    config
        .save(format!("{artifact_dir}/config.json"))
        .expect("Config should be saved successfully");

    Gpu::seed(config.seed);

    let batcher = GroundTruthBatcher {};

    let (train_data, test_data) = load_training_data("training");

    let dataloader_train = burn::data::dataloader::DataLoaderBuilder::new(batcher.clone())
        .batch_size(config.batch_size)
        .shuffle(config.seed)
        .num_workers(config.num_workers)
        .build(train_data);

    let dataloader_test = burn::data::dataloader::DataLoaderBuilder::new(batcher)
        .batch_size(config.batch_size)
        .shuffle(config.seed)
        .num_workers(config.num_workers)
        .build(test_data);

    let learner = burn::train::LearnerBuilder::new(artifact_dir)
        // .metric_train_numeric(burn::train::metric::AccuracyMetric::new())
        // .metric_valid_numeric(burn::train::metric::AccuracyMetric::new())
        .metric_train_numeric(burn::train::metric::LossMetric::new())
        .metric_valid_numeric(burn::train::metric::LossMetric::new())
        // .with_file_checkpointer(burn::record::CompactRecorder::new())
        .devices(vec![device.clone()])
        .num_epochs(config.num_epochs)
        .summary()
        .build(
            Cnn::new(&device),
            config.optimizer.init::<Gpu, Cnn>(),
            config.learning_rate,
        );

    let model_trained: Cnn = learner.fit(dataloader_train, dataloader_test);

    <Cnn as Module<Gpu>>::save_file::<DefaultFileRecorder<HalfPrecisionSettings>, String>(
        model_trained,
        format!("{artifact_dir}/model"),
        &burn::record::CompactRecorder::new(),
    )
    .expect("Trained model should be saved successfully");
}
