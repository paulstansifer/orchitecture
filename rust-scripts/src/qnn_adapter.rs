use burn::{backend::Autodiff, prelude::Backend, record::Recorder};

use crate::qnn::Cnn;

pub struct ModelHolder {
    pub interest: Cnn<burn::backend::Wgpu>,
    pub symmetry: Cnn<burn::backend::Wgpu>,
}

impl ModelHolder {
    pub fn new() -> Self {
        type B = burn::backend::Wgpu;
        use burn::record::DefaultFileRecorder;
        use burn::record::HalfPrecisionSettings;

        let device: <Autodiff<B> as Backend>::Device = Default::default();

        use burn::module::Module;

        let recorder = DefaultFileRecorder::<HalfPrecisionSettings>::new();
        let i_record: <Cnn<B> as burn::module::Module<B>>::Record = recorder
            .load("models/interest_model.mpk".into(), &device)
            .unwrap();
        let i_model = Cnn::<B>::new(&device, panic!()).load_record(i_record);
        let s_record: <Cnn<B> as burn::module::Module<B>>::Record = recorder
            .load("models/symmetry_model.mpk".into(), &device)
            .unwrap();
        let s_model = Cnn::<B>::new(&device, panic!()).load_record(s_record);

        ModelHolder {
            interest: i_model,
            symmetry: s_model,
        }
    }
}
