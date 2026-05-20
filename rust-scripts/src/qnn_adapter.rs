use crate::qnn::Cnn;
use burn::{backend::Autodiff, prelude::Backend, record::Recorder};
use std::path::PathBuf;

pub struct ModelHolder {
    pub interest: Cnn<burn::backend::Wgpu>,
    pub coherence: Cnn<burn::backend::Wgpu>,
}

impl ModelHolder {
    pub fn new() -> Self {
        type B = burn::backend::Wgpu;
        use burn::record::DefaultFileRecorder;
        use burn::record::HalfPrecisionSettings;

        let device: <Autodiff<B> as Backend>::Device = Default::default();

        use burn::module::Module;

        let model_dir: PathBuf = concat!(env!("CARGO_MANIFEST_DIR"), "/../models/").into();

        let args: crate::qnn::Args =
            serde_json::from_str(&std::fs::read_to_string(model_dir.join("model_args.json")).unwrap()).unwrap();


        let recorder = DefaultFileRecorder::<HalfPrecisionSettings>::new();
        let i_record: <Cnn<B> as burn::module::Module<B>>::Record = recorder
            .load(model_dir.join("interest_model.mpk"), &device)
            .unwrap();
        let i_model = Cnn::<B>::new(&device, &args).load_record(i_record);
        let c_record: <Cnn<B> as burn::module::Module<B>>::Record = recorder
            .load(model_dir.join("coherence_model.mpk"), &device)
            .unwrap();
        let c_model = Cnn::<B>::new(&device, &args).load_record(c_record);

        ModelHolder {
            interest: i_model,
            coherence: c_model,
        }
    }
}
