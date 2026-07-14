#![recursion_limit = "256"]

use burn::{
    backend::Autodiff,
    data::{dataloader::DataLoader, dataset::Dataset},
    optim::AdamConfig,
    prelude::*,
    record::{DefaultFileRecorder, HalfPrecisionSettings},
    train::logger::FileMetricLogger,
};
use clap::Parser;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use orchitecture_lib::qnn::translate::{
    load_training_data, GroundTruth, GroundTruthBatcher, Metric, ScoreConstraint,
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

/// Distance from `pred` to satisfying the constraint: 0 when `pred` is already
/// on the constraint's satisfied side of `goal`, matching the loss used in training.
fn constrained_error(pred: f32, goal: f32, constraint: ScoreConstraint) -> f32 {
    match constraint {
        ScoreConstraint::Exact => (pred - goal).abs(),
        ScoreConstraint::AtMost => (pred - goal).max(0.0),
        ScoreConstraint::AtLeast => (goal - pred).max(0.0),
    }
}

/// How to display a target value, so debug output distinguishes exact targets from bounds.
fn constraint_symbol(constraint: ScoreConstraint) -> &'static str {
    match constraint {
        ScoreConstraint::Exact => "=",
        ScoreConstraint::AtMost => "<=",
        ScoreConstraint::AtLeast => ">=",
    }
}

/// MSE of always predicting the mean of `goals` — the "predict nothing useful" baseline
/// a skill score is measured against. (Treats every target as `Exact`; a constraint-aware
/// baseline can wait until we actually need to compare across constraint types.)
fn baseline_mse(goals: &[f32]) -> f32 {
    let mean = goals.iter().sum::<f32>() / goals.len() as f32;
    goals.iter().map(|g| (g - mean).powi(2)).sum::<f32>() / goals.len() as f32
}

fn create_artifact_dir(artifact_dir: &str) {
    // Remove existing artifacts before to get an accurate learner summary
    //std::fs::remove_dir_all(artifact_dir).unwrap();
    std::fs::create_dir_all(artifact_dir).unwrap();
}

fn quintile_histogram(predictions: &[f32]) -> String {
    if predictions.is_empty() {
        return "empty".to_string();
    }

    let min = predictions.iter().copied().fold(f32::INFINITY, f32::min);
    let max = predictions
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    let range = if max > min { max - min } else { 1.0 };

    let mut buckets = [0; 5];
    for &pred in predictions {
        let normalized = (pred - min) / range;
        let bucket = (normalized * 5.0).min(4.0) as usize;
        buckets[bucket] += 1;
    }

    let max_count = *buckets.iter().max().unwrap();
    buckets
        .iter()
        .map(|count| (count * 9 / max_count).to_string())
        .collect()
}

fn train<B: Backend>() {
    let args = Args::parse();
    let device = B::Device::default();
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
    let mut baseline_losses: HashMap<Metric, (f32, f32)> = HashMap::new();

    for metric in [Metric::Interest, Metric::Coherence] {
        let batcher = GroundTruthBatcher {};

        let load_data = || {
            load_training_data::<B>(
                if args.fake_data {
                    "fake_training"
                } else {
                    "assets/static/training"
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

        let model_trained =
            burn::train::SupervisedTraining::new(artifact_dir, dataloader_train, dataloader_test)
                .metric_train_numeric(burn::train::metric::LossMetric::new())
                .metric_valid_numeric(burn::train::metric::LossMetric::new())
                .num_epochs(config.num_epochs)
                .with_metric_logger(FileMetricLogger::new(format!("/tmp/logs/{metric}/")))
                .launch(burn::train::Learner::new(
                    model,
                    config.optimizer.init(),
                    config.learning_rate,
                ));
        println!(
            "last layer: {:?}",
            model_trained.model.fc.last().unwrap().weight
        );

        {
            let (train_data, test_data) = load_data();
            let mut errors: Vec<(f32, String, bool, f32, f32, ScoreConstraint)> = Vec::new();
            let mut predictions: Vec<f32> = Vec::new();

            for idx in 0..train_data.len() {
                let datum = train_data.get(idx).unwrap();
                let pred: f32 = model_trained
                    .model
                    .forward(datum.voxels.inner())
                    .into_scalar()
                    .elem();
                let goal: f32 = datum.scores.into_scalar().elem();
                if args.show_scores {
                    score_output += &format!(
                        "{}: {:.2}=>{}{:.1} ",
                        datum.filename,
                        pred,
                        constraint_symbol(datum.constraint),
                        goal
                    );
                }
                predictions.push(pred);
                errors.push((
                    constrained_error(pred, goal, datum.constraint),
                    datum.filename.clone(),
                    false,
                    pred,
                    goal,
                    datum.constraint,
                ));
            }
            if args.show_scores {
                score_output += "/// ";
            }
            for idx in 0..test_data.len() {
                let datum = test_data.get(idx).unwrap();
                let pred: f32 = model_trained
                    .model
                    .forward(datum.voxels)
                    .into_scalar()
                    .elem();
                let goal: f32 = datum.scores.into_scalar().elem();
                if args.show_scores {
                    score_output += &format!(
                        "{}: {:.2}=>{}{:.1} ",
                        datum.filename,
                        pred,
                        constraint_symbol(datum.constraint),
                        goal
                    );
                }
                predictions.push(pred);
                errors.push((
                    constrained_error(pred, goal, datum.constraint),
                    datum.filename.clone(),
                    true,
                    pred,
                    goal,
                    datum.constraint,
                ));
            }
            if args.show_scores {
                score_output += "\n";
            }

            let train_goals: Vec<f32> = errors.iter().filter(|e| !e.2).map(|e| e.4).collect();
            let valid_goals: Vec<f32> = errors.iter().filter(|e| e.2).map(|e| e.4).collect();
            baseline_losses.insert(
                metric,
                (baseline_mse(&train_goals), baseline_mse(&valid_goals)),
            );

            errors.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            score_output += &format!("\nWorst {metric} errors:\n");
            for (err, filename, is_val, pred, goal, constraint) in errors.iter().take(10) {
                let marker = if *is_val { "*" } else { " " };
                let symbol = constraint_symbol(*constraint);
                score_output += &format!(
                    "  {marker} {filename}: {symbol}{goal:.1}=>{pred:.2} (err {err:.2})\n"
                );
            }

            let histogram = quintile_histogram(&predictions);
            score_output += &format!("{metric} score distribution (quintiles): {histogram}\n");
        }

        model_trained
            .model
            .save_file::<DefaultFileRecorder<HalfPrecisionSettings>, String>(
                format!("{artifact_dir}/{metric}_model"),
                &burn::record::CompactRecorder::new(),
            )
            .expect("Trained model should be saved successfully");

        let args_ron = ron::to_string(&args).unwrap();
        std::fs::write(format!("{artifact_dir}/model_args.ron"), args_ron).unwrap();
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

        let final_train_loss = train_curve.last().unwrap().1;
        let final_valid_loss = valid_curve.last().unwrap().1;
        let (train_baseline, valid_baseline) = baseline_losses[&metric];
        let train_skill = 1.0 - final_train_loss / train_baseline;
        let valid_skill = 1.0 - final_valid_loss / valid_baseline;

        println!(
            "Final {metric} loss (t/v) {:.3} {:.3}, skill (t/v) {:.3} {:.3}",
            final_train_loss, final_valid_loss, train_skill, valid_skill
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

    print!("{}", score_output);
}

fn main() {
    train::<burn::backend::Wgpu>();
}
