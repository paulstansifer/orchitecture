#[cfg(not(target_arch = "wasm32"))]
use super::model::Cnn;
#[cfg(not(target_arch = "wasm32"))]
use burn::{backend::Autodiff, prelude::Backend, record::Recorder};
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static MODELS: ModelHolder = ModelHolder::new();
}

pub fn metrics_at(
    contents: &crate::sparse3d::Sparse3D<crate::wall_grid::Cell>,
    structures: &[crate::structure::StructureInfo],
    location: bevy::math::Vec3,
) -> Vec<f32> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let pos = location.round().as_ivec3();
        let tensor: burn::tensor::Tensor<burn::backend::Wgpu, 5> =
            super::translate::sparse3d_to_tensor(contents, pos, |cell: &crate::wall_grid::Cell| {
                let semb = &structures[cell.id as usize].embedding;
                vec![semb.tall, semb.decorative, semb.passable, semb.striated]
            })
            .unwrap();

        return MODELS.with(|models| {
            vec![
                models.coherence.forward(tensor.clone()).sum().into_scalar(),
                models.interest.forward(tensor).sum().into_scalar(),
            ]
        });
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (contents, structures, location);
    vec![]
}

#[cfg(not(target_arch = "wasm32"))]
pub struct ModelHolder {
    pub interest: Cnn<burn::backend::Wgpu>,
    pub coherence: Cnn<burn::backend::Wgpu>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ModelHolder {
    pub fn new() -> Self {
        type B = burn::backend::Wgpu;
        use burn::record::DefaultFileRecorder;
        use burn::record::HalfPrecisionSettings;

        let device: <Autodiff<B> as Backend>::Device = Default::default();

        use burn::module::Module;

        let model_dir: PathBuf = crate::paths::MODELS_DIR.into();

        let args: super::model::Args = serde_json::from_str(
            &std::fs::read_to_string(model_dir.join("model_args.json")).unwrap(),
        )
        .unwrap();

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
