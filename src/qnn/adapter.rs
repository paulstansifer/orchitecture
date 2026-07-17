#![allow(unused)]
use super::model::MotifNn;
use bevy::asset::{Asset, AssetLoader, Assets, Handle, LoadContext};
use bevy::prelude::*;
use burn::record::Recorder;
use std::sync::Mutex;

const MODEL_ARGS: &str = include_str!("../../assets/static/models/model_args.ron");

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
    pub interest: MotifNn<AppBackend>,
    pub order: MotifNn<AppBackend>,
}

impl ModelHolder {
    fn from_bytes(interest_bytes: Vec<u8>, order_bytes: Vec<u8>) -> Self {
        use burn::module::Module;
        use burn::record::HalfPrecisionSettings;
        use burn::record::NamedMpkBytesRecorder;

        let device = Default::default();
        let args: super::model::Args = ron::from_str(MODEL_ARGS).unwrap();

        let recorder = NamedMpkBytesRecorder::<HalfPrecisionSettings>::new();
        let i_record: <MotifNn<AppBackend> as Module<AppBackend>>::Record =
            recorder.load(interest_bytes, &device).unwrap();
        let i_model = MotifNn::<AppBackend>::new(&device, &args).load_record(i_record);
        let o_record: <MotifNn<AppBackend> as Module<AppBackend>>::Record =
            recorder.load(order_bytes, &device).unwrap();
        let o_model = MotifNn::<AppBackend>::new(&device, &args).load_record(o_record);

        ModelHolder {
            interest: i_model,
            order: o_model,
        }
    }
}

pub fn compute_metrics(
    holder: &ModelHolder,
    contents: &crate::sparse3d::Sparse3D<crate::city::Cell>,
    structures: &[crate::eorf::EorfInfo],
    location: Vec3,
) -> Vec<f32> {
    use crate::sparse3d::{RelSlot, RelSlotCoord};

    let pos = location.round().as_ivec3();
    let vantage = RelSlotCoord::new(pos.x, pos.y, pos.z, RelSlot::Room);
    let (occurrences, _defects) =
        super::translate::visible_motifs_and_defects(contents, vantage, structures);
    let (motif_interest, motif_order) =
        super::translate::motif_occurrences_to_tensors::<AppBackend>(&occurrences);
    let order_stats = super::translate::motif_order_stats::<AppBackend>(&occurrences);

    vec![
        holder
            .order
            .forward(
                motif_interest.clone(),
                motif_order.clone(),
                order_stats.clone(),
            )
            .into_scalar(),
        holder
            .interest
            .forward(motif_interest, motif_order, order_stats)
            .into_scalar(),
    ]
}

// ---------------------------------------------------------------------------
// Bevy plugin and systems
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub struct ModelState {
    interest_handle: Handle<ModelBytes>,
    order_handle: Handle<ModelBytes>,
    pub holder: Option<Mutex<ModelHolder>>,
}

fn setup_model_loading(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(ModelState {
        interest_handle: asset_server.load("assets/static/models/interest_model.mpk"),
        order_handle: asset_server.load("assets/static/models/order_model.mpk"),
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
        || model_bytes.get(&model_state.order_handle).is_none()
    {
        return;
    }
    let interest = model_bytes
        .remove(model_state.interest_handle.id())
        .unwrap();
    let order = model_bytes.remove(model_state.order_handle.id()).unwrap();
    model_state.holder = Some(Mutex::new(ModelHolder::from_bytes(interest.0, order.0)));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Rebuilds `ModelHolder` from the checked-in weights/args, on `NdArray` rather than
    /// `AppBackend` so it doesn't need a GPU -- catches a `MotifNn` architecture change (in
    /// `model.rs`/`Args`) that no longer matches what's actually saved at
    /// `assets/static/models/{interest,order}_model.mpk`.
    #[test]
    fn model_holder_loads_from_committed_assets() {
        use burn::backend::NdArray;
        use burn::module::Module;
        use burn::record::{HalfPrecisionSettings, NamedMpkBytesRecorder, Recorder};

        let interest_bytes =
            include_bytes!("../../assets/static/models/interest_model.mpk").to_vec();
        let order_bytes = include_bytes!("../../assets/static/models/order_model.mpk").to_vec();

        let device = Default::default();
        let args: super::super::model::Args = ron::from_str(MODEL_ARGS).unwrap();
        let recorder = NamedMpkBytesRecorder::<HalfPrecisionSettings>::new();

        for bytes in [interest_bytes, order_bytes] {
            let record: <MotifNn<NdArray<f32>> as Module<NdArray<f32>>>::Record =
                recorder.load(bytes, &device).unwrap();
            MotifNn::<NdArray<f32>>::new(&device, &args).load_record(record);
        }
    }
}
