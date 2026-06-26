use super::model::Cnn;
use bevy::asset::{Asset, AssetLoader, Assets, Handle, LoadContext};
use bevy::prelude::*;
use burn::record::Recorder;
use std::sync::Mutex;

const MODEL_ARGS: &str = include_str!("../../models/model_args.ron");

#[cfg(not(target_arch = "wasm32"))]
type AppBackend = burn::backend::Wgpu;

// Wasm should support Wgpu... seems there is a problem while loading.
// Maybe this will ne fast enough?
#[cfg(target_arch = "wasm32")]
type AppBackend = burn::backend::NdArray<f32>;

// ---------------------------------------------------------------------------
// Raw-bytes asset (used to ferry .mpk data through the Bevy asset pipeline)
// ---------------------------------------------------------------------------

#[derive(Asset, TypePath)]
pub struct ModelBytes(pub Vec<u8>);

#[derive(Default, TypePath)]
pub struct ModelBytesLoader;

impl AssetLoader for ModelBytesLoader {
    type Asset = ModelBytes;
    type Settings = ();
    type Error = std::io::Error;

    async fn load(
        &self,
        reader: &mut dyn bevy::asset::io::Reader,
        _settings: &(),
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(ModelBytes(bytes))
    }

    fn extensions(&self) -> &[&str] {
        &["mpk"]
    }
}

// ---------------------------------------------------------------------------
// Model holder and evaluation
// ---------------------------------------------------------------------------

pub struct ModelHolder {
    pub interest: Cnn<AppBackend>,
    pub coherence: Cnn<AppBackend>,
}

impl ModelHolder {
    fn from_bytes(interest_bytes: Vec<u8>, coherence_bytes: Vec<u8>) -> Self {
        use burn::module::Module;
        use burn::record::HalfPrecisionSettings;
        use burn::record::NamedMpkBytesRecorder;

        let device = Default::default();
        let args: super::model::Args = ron::from_str(MODEL_ARGS).unwrap();

        let recorder = NamedMpkBytesRecorder::<HalfPrecisionSettings>::new();
        let i_record: <Cnn<AppBackend> as Module<AppBackend>>::Record =
            recorder.load(interest_bytes, &device).unwrap();
        let i_model = Cnn::<AppBackend>::new(&device, &args).load_record(i_record);
        let c_record: <Cnn<AppBackend> as Module<AppBackend>>::Record =
            recorder.load(coherence_bytes, &device).unwrap();
        let c_model = Cnn::<AppBackend>::new(&device, &args).load_record(c_record);

        ModelHolder {
            interest: i_model,
            coherence: c_model,
        }
    }
}

pub fn compute_metrics(
    holder: &ModelHolder,
    contents: &crate::sparse3d::Sparse3D<crate::world::Cell>,
    structures: &[crate::structure::StructureInfo],
    location: Vec3,
) -> Vec<f32> {
    let pos = location.round().as_ivec3();
    let tensor: burn::tensor::Tensor<AppBackend, 5> =
        super::translate::sparse3d_to_tensor(contents, pos, |cell: &crate::world::Cell| {
            let semb = &structures[cell.id.as_usize()].embedding;
            vec![semb.tall, semb.decorative, semb.passable, semb.striated]
        })
        .unwrap();

    vec![
        holder.coherence.forward(tensor.clone()).sum().into_scalar(),
        holder.interest.forward(tensor).sum().into_scalar(),
    ]
}

// ---------------------------------------------------------------------------
// Bevy plugin and systems
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub struct ModelState {
    interest_handle: Handle<ModelBytes>,
    coherence_handle: Handle<ModelBytes>,
    pub holder: Option<Mutex<ModelHolder>>,
}

fn setup_model_loading(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(ModelState {
        interest_handle: asset_server.load("models/interest_model.mpk"),
        coherence_handle: asset_server.load("models/coherence_model.mpk"),
        holder: None,
    });
}

fn build_model_when_ready(
    mut model_state: ResMut<ModelState>,
    mut model_bytes: ResMut<Assets<ModelBytes>>,
) {
    if model_state.holder.is_some() {
        return;
    }
    // Check both are present before removing either.
    if model_bytes.get(&model_state.interest_handle).is_none()
        || model_bytes.get(&model_state.coherence_handle).is_none()
    {
        return;
    }
    let interest = model_bytes
        .remove(model_state.interest_handle.id())
        .unwrap();
    let coherence = model_bytes
        .remove(model_state.coherence_handle.id())
        .unwrap();
    model_state.holder = Some(Mutex::new(ModelHolder::from_bytes(interest.0, coherence.0)));
}

pub struct ModelPlugin;

impl Plugin for ModelPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<ModelBytes>()
            .register_asset_loader(ModelBytesLoader)
            .add_systems(Startup, setup_model_loading)
            .add_systems(Update, build_model_when_ready);
    }
}
