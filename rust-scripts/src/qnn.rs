use std::path::PathBuf;
use std::sync::Arc;

use burn::nn::Sigmoid;
use burn::train::logger::FileMetricLogger;
use burn::{
    backend::Autodiff,
    data::dataloader::DataLoader,
    nn::{
        conv::{Conv3d, Conv3dConfig},
        Dropout, DropoutConfig, Linear, LinearConfig, PaddingConfig3d, Relu,
    },
    optim::AdamConfig,
    prelude::*,
    record::{DefaultFileRecorder, HalfPrecisionSettings},
    tensor::backend::AutodiffBackend,
    train::RegressionOutput,
};
use clap::Parser;

#[derive(Parser, Debug)]
struct Args {
    #[arg(short, long, default_value = "5/12,24/3")]
    conv: String,

    #[arg(short, long, default_value = "128,64,32")]
    fc: String,
    // #[arg(short, long, default_value = "1.0e-5")]
    // lr: f32,
}

#[derive(Module, Debug)]
pub struct Cnn<B: Backend> {
    relu: Relu,
    sigmoid: Sigmoid,
    dropout: Dropout,
    conv: Vec<Conv3d<B>>,
    // conv1: Conv3d<B>,
    // conv2: Conv3d<B>,
    fc: Vec<Linear<B>>,
    // fc1: Linear<B>,
    // fc2: Linear<B>,
    // fc3: Linear<B>,
    // fc4: Linear<B>,
}

impl<B: Backend> Cnn<B> {
    pub fn new(device: &<B as Backend>::Device) -> Self {
        let args = Args::parse();

        let mut features = qnn_translate::EMBEDDING_SIZE;
        let mut conv = vec![];
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

        // let conv1 = Conv3dConfig::new([qnn_translate::EMBEDDING_SIZE, 12], [3, 3, 3])
        //     .with_padding(PaddingConfig3d::Same)
        //     .init(device);

        // let conv2 = Conv3dConfig::new([12, 24], [3, 3, 3])
        //     .with_padding(PaddingConfig3d::Same)
        //     .init(device);

        // Calculate the output size after pooling (example: 10/2 = 5, 22/2 = 11)
        // If the input was [16, 10, 22, 22], after two pooling layers it becomes [64, 2, 5, 5] (approx.)
        let mut nodes = features * 12 * 23 * 23;
        let mut fc = vec![];
        for fc_spec in args.fc.split(",") {
            let next_nodes: usize = fc_spec.parse().unwrap();
            fc.push(LinearConfig::new(nodes, next_nodes).init(device));
            nodes = next_nodes;
        }
        fc.push(LinearConfig::new(nodes, 1).init(device));

        // let fc1 = LinearConfig::new(flattened_size, 128).init(device);
        // let fc2 = LinearConfig::new(128, 64).init(device);
        // let fc3 = LinearConfig::new(64, 32).init(device);
        // let fc4 = LinearConfig::new(32, 1).init(device); // Output a single score

        let dropout = DropoutConfig::new(0.3).init();

        Self {
            relu: Relu::new(),
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

        // let x = self.conv1.forward(x);
        // let x = self.relu.forward(x);
        // let x = self.dropout.forward(x);

        // let x = self.conv2.forward(x);
        // let x = self.relu.forward(x);
        // let x = self.dropout.forward(x);
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

use burn::train::TrainOutput;

use crate::qnn_translate::{self, load_training_data, GroundTruth, GroundTruthBatcher};

impl<B: AutodiffBackend> burn::train::TrainStep<GroundTruth<B>, RegressionOutput<B>> for Cnn<B> {
    fn step(&self, batch: GroundTruth<B>) -> TrainOutput<RegressionOutput<B>> {
        let item = self.forward_classification(batch.voxels, batch.scores);

        TrainOutput::new::<B, Cnn<B>>(self, item.loss.backward(), item)
    }
}

impl<B: Backend> burn::train::ValidStep<GroundTruth<B>, RegressionOutput<B>> for Cnn<B> {
    fn step(&self, batch: GroundTruth<B>) -> RegressionOutput<B> {
        self.forward_classification(batch.voxels, batch.scores)
    }
}

#[derive(Config, Debug)]
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
    #[config(default = 1.0e-5)]
    pub learning_rate: f64,
}

fn create_artifact_dir(artifact_dir: &str) {
    // Remove existing artifacts before to get an accurate learner summary
    //std::fs::remove_dir_all(artifact_dir).unwrap();
    std::fs::create_dir_all(artifact_dir).unwrap();
    println!("{artifact_dir} created successfully");
}

pub fn train<B: Backend>() {
    let device: <Autodiff<B> as Backend>::Device = Default::default();
    let config = TrainingConfig::new(AdamConfig::new());
    let artifact_dir = "/tmp/artifacts/";
    create_artifact_dir(artifact_dir);
    config
        .save(format!("{artifact_dir}/config.json"))
        .expect("Config should be saved successfully");

    B::seed(&device, config.seed);

    for metric in [
        crate::qnn_translate::Metric::Interest,
        crate::qnn_translate::Metric::Symmetry,
    ] {
        let batcher = GroundTruthBatcher {};

        let (train_data, test_data) = load_training_data::<B>("../training", config.seed, metric);

        let dataloader_train: Arc<dyn DataLoader<Autodiff<B>, GroundTruth<Autodiff<B>>>> =
            burn::data::dataloader::DataLoaderBuilder::new(batcher.clone())
                .batch_size(config.batch_size)
                .shuffle(config.seed)
                .num_workers(config.num_workers)
                .build(train_data);

        let dataloader_test: Arc<dyn DataLoader<B, GroundTruth<B>>> =
            burn::data::dataloader::DataLoaderBuilder::new(batcher)
                .batch_size(config.batch_size)
                .shuffle(config.seed)
                .num_workers(config.num_workers)
                .build(test_data);

        let learner = burn::train::LearnerBuilder::new(artifact_dir)
            .metric_train_numeric(burn::train::metric::LossMetric::new())
            .metric_valid_numeric(burn::train::metric::LossMetric::new())
            .num_epochs(config.num_epochs)
            .with_metric_logger(FileMetricLogger::new(format!("/tmp/logs/{metric}/")))
            .build(
                Cnn::<Autodiff<B>>::new(&device),
                config.optimizer.init(),
                config.learning_rate,
                burn::train::LearningStrategy::SingleDevice(device.clone()),
            );

        let model_trained = { learner.fit(dataloader_train, dataloader_test) };

        model_trained
            .model
            .save_file::<DefaultFileRecorder<HalfPrecisionSettings>, String>(
                format!("{artifact_dir}/{metric}_model"),
                &burn::record::CompactRecorder::new(),
            )
            .expect("Trained model should be saved successfully");
    }

    println!("Parameters: {:?}", Args::parse());

    // Gotta let the trainer go out of scope to get access to the terminal back?

    for metric in [
        crate::qnn_translate::Metric::Interest,
        crate::qnn_translate::Metric::Symmetry,
    ] {
        let mut train_curve = vec![];
        let mut valid_curve = vec![];

        for epoch in 1..=config.num_epochs {
            for mode in ["train", "valid"] {
                let csv_path =
                    PathBuf::from(format!("/tmp/logs/{metric}/{mode}/epoch-{epoch}/Loss.log"));
                let mut rdr = csv::Reader::from_path(csv_path).unwrap();
                let mut total_loss = 0.0;
                let mut count = 0;
                for result in rdr.records() {
                    let record = result.unwrap();
                    let loss: f32 = record.get(0).unwrap().parse().unwrap();
                    total_loss += loss;
                    count += 1;
                }
                let average_loss = total_loss / count as f32;

                if mode == "train" {
                    train_curve.push((epoch as f32, average_loss));
                } else {
                    valid_curve.push((epoch as f32, average_loss));
                }
            }
        }

        use textplots::ColorPlot;

        println!(
            "Final {metric} loss (t/v) {:.3} {:.3}",
            train_curve.last().unwrap().1,
            valid_curve.last().unwrap().1
        );

        textplots::Chart::new_with_y_range(100, 30, 0.0, config.num_epochs as f32, 0.0, 0.15)
            .linecolorplot(
                &textplots::Shape::Lines(&train_curve),
                rgb::RGB::new(255, 0, 0),
            )
            .linecolorplot(
                &textplots::Shape::Lines(&valid_curve),
                rgb::RGB::new(0, 255, 0),
            )
            .nice();
    }
}
