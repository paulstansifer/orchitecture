#![recursion_limit = "256"]

use burn::{
    backend::Autodiff,
    data::{dataloader::DataLoader, dataset::Dataset},
    optim::AdamConfig,
    prelude::*,
    record::{DefaultFileRecorder, HalfPrecisionSettings},
    tensor::backend::AutodiffBackend,
    train::logger::FileMetricLogger,
};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;

use orchitecture_lib::qnn::translate::{
    load_training_data, GroundTruth, GroundTruthBatcher, Metric,
};
use orchitecture_lib::qnn::{Args, Cnn};

#[derive(Config, Debug)]
struct TrainingConfig {
    pub optimizer: AdamConfig,
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
}

fn train<B: Backend>() {
    let args = Args::parse();
    let device: <Autodiff<B> as Backend>::Device = Default::default();
    let config = TrainingConfig::new(AdamConfig::new())
        .with_learning_rate(args.lr)
        .with_num_epochs(args.epochs)
        .with_seed(args.seed);
    let artifact_dir = "/tmp/artifacts/";
    create_artifact_dir(artifact_dir);
    config
        .save(format!("{artifact_dir}/config.json"))
        .expect("Config should be saved successfully");

    B::seed(&device, config.seed);

    let mut score_output = String::new();

    for metric in [Metric::Interest, Metric::Coherence] {
        let batcher = GroundTruthBatcher {};

        let load_data = || {
            load_training_data::<B>(
                if args.fake_data {
                    "../fake_training"
                } else {
                    "../training"
                },
                config.seed,
                metric,
            )
        };

        let (train_data, test_data) = load_data();
        let data_size = (train_data.len(), test_data.len());
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

        let model = Cnn::<Autodiff<B>>::new(&device, &args);

        if metric == Metric::Interest {
            // Only need to show this once:
            println!(
                "Model params: {}. Training items: {}. Test items: {}",
                model.num_params(),
                data_size.0,
                data_size.1
            );
        }
        // println!("### Model: {model}");
        // println!("### last layer: {:?}", model.fc.last().unwrap().weight);

        let learner = burn::train::LearnerBuilder::new(artifact_dir)
            .metric_train_numeric(burn::train::metric::LossMetric::new())
            .metric_valid_numeric(burn::train::metric::LossMetric::new())
            .num_epochs(config.num_epochs)
            .with_metric_logger(FileMetricLogger::new(format!("/tmp/logs/{metric}/")))
            .build(
                model,
                config.optimizer.init(),
                config.learning_rate,
                burn::train::LearningStrategy::SingleDevice(device.clone()),
            );

        let model_trained = { learner.fit(dataloader_train, dataloader_test) };
        println!(
            "last layer: {:?}",
            model_trained.model.fc.last().unwrap().weight
        );

        if args.show_scores {
            let (train_data, test_data) = load_data();

            for idx in 0..train_data.len() {
                let datum = train_data.get(idx).unwrap();
                let evaluation = model_trained
                    .model
                    .forward(datum.voxels.inner())
                    .into_scalar();
                let goal = datum.scores.into_scalar();
                score_output += &format!("{}: {:.2}=>{:.1} ", datum.filename, evaluation, goal);
            }
            score_output += "/// ";
            for idx in 0..test_data.len() {
                let datum = test_data.get(idx).unwrap();
                let evaluation = model_trained.model.forward(datum.voxels).into_scalar();
                let goal = datum.scores.into_scalar();
                score_output += &format!("{}: {:.2}=>{:.1} ", datum.filename, evaluation, goal);
            }
            score_output += "\n";
        }

        model_trained
            .model
            .save_file::<DefaultFileRecorder<HalfPrecisionSettings>, String>(
                format!("{artifact_dir}/{metric}_model"),
                &burn::record::CompactRecorder::new(),
            )
            .expect("Trained model should be saved successfully");

        let args_json = serde_json::to_string_pretty(&args).unwrap();
        std::fs::write(format!("{artifact_dir}/model_args.json"), args_json).unwrap();
    }

    println!("Parameters: {:?}", Args::parse());
    let mut plots = vec![];

    // Gotta let the trainer go out of scope to get access to the terminal back?

    for metric in [Metric::Interest, Metric::Coherence] {
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

        let plot_file = "/tmp/plot_text";

        for curve in &[train_curve, valid_curve] {
            let _guard = stdio_override::StdoutOverride::from_file(plot_file).unwrap();

            // println!("{metric} {name}: {:.3}", curve.last().unwrap().1);
            textplots::Chart::new_with_y_range(100, 35, 0.0, config.num_epochs as f32, 0.0, 0.2)
                .linecolorplot(&textplots::Shape::Lines(&curve), rgb::RGB::new(255, 0, 0))
                .nice();

            plots.push(std::fs::read_to_string(plot_file).unwrap());
        }
    }

    // Display the plots side-by-side:
    let max_lines = plots.iter().map(|p| p.lines().count()).max().unwrap();
    let max_width = plots
        .iter()
        .map(|p| p.lines().map(|l| l.len()).max().unwrap())
        .max()
        .unwrap();
    let plot_lines: Vec<Vec<&str>> = plots.iter().map(|p| p.lines().collect()).collect();
    for line_idx in 0..max_lines {
        for (plot_idx, lines) in plot_lines.iter().enumerate() {
            if plot_idx > 0 {
                print!("  ");
            }
            let mut this_line = lines.get(line_idx).unwrap_or(&"").to_string();
            while this_line.len() < max_width {
                this_line += " ";
            }
            print!("{this_line}");
        }
        println!();
    }

    if args.show_scores {
        print!("{}", score_output);
    }
}

fn main() {
    train::<burn::backend::Wgpu>();
}
