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
use orchitecture_lib::qnn::{Args, MotifNn, OrderStatsNn};

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

/// Reads a metric's per-epoch loss curve back from `FileMetricLogger`'s CSVs (written to
/// `{log_dir}/{train,valid}/epoch-N/Loss.log`), prints its final loss/skill line, and renders
/// train+valid ascii charts onto `plots` (later shown side-by-side with any others).
fn report_loss_curve(
    label: impl std::fmt::Display,
    log_dir: impl std::fmt::Display,
    num_epochs: usize,
    baseline: (f32, f32),
    histogram: &str,
    plots: &mut Vec<String>,
) {
    let mut train_curve = vec![];
    let mut valid_curve = vec![];

    for epoch in 1..=num_epochs {
        for mode in ["train", "valid"] {
            let csv_path = PathBuf::from(format!("{log_dir}/{mode}/epoch-{epoch}/Loss.log"));
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
    let (train_baseline, valid_baseline) = baseline;
    let train_skill = 1.0 - final_train_loss / train_baseline;
    let valid_skill = 1.0 - final_valid_loss / valid_baseline;

    println!(
        "Final {label} loss (t/v) {:.3} {:.3}, skill (t/v) {:.3} {:.3}  score hist: [{histogram}]",
        final_train_loss, final_valid_loss, train_skill, valid_skill
    );

    let plot_file = "/tmp/plot_text";

    for curve in &[train_curve, valid_curve] {
        let _guard = stdio_override::StdoutOverride::from_file(plot_file).unwrap();

        textplots::Chart::new_with_y_range(100, 35, 0.0, num_epochs as f32, 0.0, 0.2)
            .linecolorplot(&textplots::Shape::Lines(curve), rgb::RGB::new(255, 0, 0))
            .nice();

        plots.push(std::fs::read_to_string(plot_file).unwrap());
    }
}

/// Space-separated `count(h1,h2,h3)` per example, in dataset order: `count` is the number of
/// `motif_order` rows (paired-up motif occurrences) and `h1,h2,h3` are that example's
/// `motif_order_stats` h-indices (see its doc comment). Handy for spotting "no data" -- or "no
/// variation" -- as an explanation for a metric that won't train.
fn motif_order_pair_counts<B: Backend, D: Dataset<GroundTruth<B>>>(data: &D) -> String {
    (0..data.len())
        .map(|i| {
            let datum = data.get(i).unwrap();
            let count = datum.motif_order.dims()[0];
            let stats = datum.order_stats.into_data().to_vec::<f32>().unwrap();
            format!("{count}({},{},{})", stats[0], stats[1], stats[2])
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn create_artifact_dir(artifact_dir: &str) {
    // Remove existing artifacts before to get an accurate learner summary
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
    let mut motif_baseline_losses: HashMap<Metric, (f32, f32)> = HashMap::new();
    let mut motif_histograms: HashMap<Metric, String> = HashMap::new();

    // MotifNn: scores a room purely from its visible motifs (see MotifNn's doc comment).
    for metric in [Metric::Interest, Metric::Order] {
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

        // Display the H-indices in the data:
        // if metric == Metric::Order {
        //     println!(
        //         "motif_order pair counts, train: {}",
        //         motif_order_pair_counts(&train_data)
        //     );
        //     println!(
        //         "motif_order pair counts, valid: {}",
        //         motif_order_pair_counts(&test_data)
        //     );
        // }

        // Each room contributes a different number of occurrence/pair rows (see
        // `GroundTruthBatcher::batch`'s doc comment), so batching more than one room together
        // would silently merge unrelated rooms' rows into a single set. Force batch_size=1 here,
        // regardless of `config.batch_size`.
        let dataloader_train: Arc<dyn DataLoader<Autodiff<B>, GroundTruth<Autodiff<B>>>> =
            burn::data::dataloader::DataLoaderBuilder::new(batcher.clone())
                .batch_size(1)
                .shuffle(config.seed)
                .num_workers(config.num_workers)
                .build(train_data);

        let dataloader_test: Arc<dyn DataLoader<B, GroundTruth<B>>> =
            burn::data::dataloader::DataLoaderBuilder::new(batcher)
                .batch_size(1)
                .shuffle(config.seed)
                .num_workers(config.num_workers)
                .build(test_data);

        let motif_artifact_dir = format!("{artifact_dir}/motif");
        create_artifact_dir(&motif_artifact_dir);

        let mut errors: Vec<(f32, String, bool, f32, f32, ScoreConstraint)> = Vec::new();
        let mut predictions: Vec<f32> = Vec::new();
        // (is_val, filename, "<=0.70"-style goal string, goal (for sorting), pred, h1, h2, h3)
        let mut order_debug_rows: Vec<(bool, String, String, f32, f32, usize, usize, usize)> =
            Vec::new();

        let model = MotifNn::<Autodiff<B>>::new(&device, &args);
        println!(
            "MotifNn params: {}. Training items: {}. Test items: {}",
            model.num_params(),
            data_size.0,
            data_size.1
        );

        let model_trained = burn::train::SupervisedTraining::new(
            &motif_artifact_dir,
            dataloader_train,
            dataloader_test,
        )
        .metric_train_numeric(burn::train::metric::LossMetric::new())
        .metric_valid_numeric(burn::train::metric::LossMetric::new())
        .num_epochs(config.num_epochs)
        .with_metric_logger(FileMetricLogger::new(format!("/tmp/logs/motif-{metric}/")))
        .launch(burn::train::Learner::new(
            model,
            config.optimizer.init(),
            config.learning_rate,
        ));

        {
            let (train_data, test_data) = load_data();
            for idx in 0..train_data.len() {
                let datum = train_data.get(idx).unwrap();
                let order_stats = datum.order_stats.to_data().to_vec::<f32>().unwrap();
                let pred: f32 = model_trained
                    .model
                    .forward(
                        datum.motif_interest.inner(),
                        datum.motif_order.inner(),
                        datum.order_stats.inner(),
                    )
                    .into_scalar()
                    .elem();
                let goal: f32 = datum.scores.into_scalar().elem();
                predictions.push(pred);
                if metric == Metric::Order && args.show_scores {
                    order_debug_rows.push((
                        false,
                        datum.filename.clone(),
                        format!("{}{:.2}", constraint_symbol(datum.constraint), goal),
                        goal,
                        pred,
                        order_stats[0] as usize,
                        order_stats[1] as usize,
                        order_stats[2] as usize,
                    ));
                }
                errors.push((
                    constrained_error(pred, goal, datum.constraint),
                    datum.filename.clone(),
                    false,
                    pred,
                    goal,
                    datum.constraint,
                ));
            }
            for idx in 0..test_data.len() {
                let datum = test_data.get(idx).unwrap();
                let order_stats = datum.order_stats.to_data().to_vec::<f32>().unwrap();
                let pred: f32 = model_trained
                    .model
                    .forward(datum.motif_interest, datum.motif_order, datum.order_stats)
                    .into_scalar()
                    .elem();
                let goal: f32 = datum.scores.into_scalar().elem();
                predictions.push(pred);
                if metric == Metric::Order && args.show_scores {
                    order_debug_rows.push((
                        true,
                        datum.filename.clone(),
                        format!("{}{:.2}", constraint_symbol(datum.constraint), goal),
                        goal,
                        pred,
                        order_stats[0] as usize,
                        order_stats[1] as usize,
                        order_stats[2] as usize,
                    ));
                }
                errors.push((
                    constrained_error(pred, goal, datum.constraint),
                    datum.filename.clone(),
                    true,
                    pred,
                    goal,
                    datum.constraint,
                ));
            }
        }

        model_trained
            .model
            .save_file::<DefaultFileRecorder<HalfPrecisionSettings>, String>(
                format!("{artifact_dir}/{metric}_model"),
                &burn::record::CompactRecorder::new(),
            )
            .expect("Trained MotifNn model should be saved successfully");
        std::fs::write(
            format!("{artifact_dir}/model_args.ron"),
            ron::to_string(&args).unwrap(),
        )
        .unwrap();

        {
            let train_goals: Vec<f32> = errors.iter().filter(|e| !e.2).map(|e| e.4).collect();
            let valid_goals: Vec<f32> = errors.iter().filter(|e| e.2).map(|e| e.4).collect();
            motif_baseline_losses.insert(
                metric,
                (baseline_mse(&train_goals), baseline_mse(&valid_goals)),
            );

            errors.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

            if args.show_worst_errors {
                score_output += &format!("\nWorst motif-{metric} errors:\n");
                for (err, filename, is_val, pred, goal, constraint) in errors.iter().take(10) {
                    let marker = if *is_val { "*" } else { " " };
                    let symbol = constraint_symbol(*constraint);
                    score_output += &format!(
                        "  {marker} {filename}: {symbol}{goal:.1}=>{pred:.2} (err {err:.2})\n"
                    );
                }
            }

            if !order_debug_rows.is_empty() {
                order_debug_rows
                    .sort_by(|a, b| a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal));

                let name_width = order_debug_rows.iter().map(|r| r.1.len()).max().unwrap();
                let goal_width = order_debug_rows.iter().map(|r| r.2.len()).max().unwrap();

                score_output += "\nOrder data points (sorted by ground-truth score):\n";
                for (is_val, filename, goal_str, _goal, pred, h1, h2, h3) in &order_debug_rows {
                    let marker = if *is_val { "*" } else { " " };
                    score_output += &format!(
                        "  {marker} {filename:<name_width$} {goal_str:>goal_width$} => {pred:>5.2}  h=({h1:>2}, {h2:>2}, {h3:>2})\n"
                    );
                }
            }

            motif_histograms.insert(metric, quintile_histogram(&predictions));
        }
    }
    // Gotta let the trainer go out of scope to get access to the terminal back?

    println!("Parameters: {:?}", Args::parse());

    let mut plots: Vec<String> = Vec::new();

    // MotifNn's per-epoch loss curves, read back from the logger's CSVs.
    for metric in [Metric::Interest, Metric::Order] {
        report_loss_curve(
            format!("motif-{metric}"),
            format!("/tmp/logs/motif-{metric}"),
            config.num_epochs,
            motif_baseline_losses[&metric],
            &motif_histograms[&metric],
            &mut plots,
        );
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
