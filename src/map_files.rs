//! Saving and loading whole city maps: the compile-time bundled map table,
//! user-map discovery, the native file dialogs, and [`load_map`] — the one
//! way a map becomes the current city.

use bevy::prelude::*;
use bevy_egui::egui;
use bevy_file_dialog::prelude::*;

use crate::city::{
    apply_changes, clear_proposal_entities, clear_proposed_cut_entities, AssembledCity, Cell,
    ConstructedCity, ProposedCity, ViewableWorld,
};
use crate::construction::load_from_offline;
use crate::eorf::{EorfInfo, EorfList};
use crate::serialization;
use crate::sparse3d::Sparse3D;

/// Expands a list of map names into the `(name, contents)` table, including
/// each `assets/static/training/{name}.txt` at compile time.
macro_rules! bundled_maps {
    ($($name:literal),* $(,)?) => {
        &[$((
            $name,
            include_str!(concat!("../assets/static/training/", $name, ".txt")),
        )),*]
    };
}

/// Maps bundled at compile time; always available on all platforms.
const BUNDLED_MAPS: &[(&str, &str)] = bundled_maps![
    "boring_room",
    "boring_room_blob",
    "boring_room_tall",
    "boring_room_tall_with_boxes",
    "boring_room_tiny",
    "boring_room_with_alcove",
    "cavern",
    "chaotic_apartment",
    "corner_in_corner",
    "corners",
    "double_balconies",
    "gallery",
    "hall_turn_fat_pillars",
    "hall_turn_stations",
    "long_apartment",
    "meta_pillars",
    "random_but_coherent",
    "rotational_apartment",
    "sanctuary",
    "simple_apartment",
    "simple_balcony",
    "two_level_apartment",
    "porches",
];

fn find_bundled(name: &str) -> Option<&'static str> {
    BUNDLED_MAPS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| *c)
}

/// Loads a map by dropdown name: a bundled map on any platform, or (on
/// native) a user-created file in `USER_DIR`. `None` if unavailable.
pub fn load_named_map(name: &str, eorfs: &[EorfInfo]) -> Option<Sparse3D<Cell>> {
    if let Some(content) = find_bundled(name) {
        return serialization::load_from_str(content, eorfs).ok();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = std::path::PathBuf::from(crate::paths::USER_DIR).join(name);
        serialization::load(&path, eorfs).ok()
    }
    #[cfg(target_arch = "wasm32")]
    None
}

pub struct SaveDialog;
pub struct LoadDialog;

/// Replaces the whole city with `new_contents`: clears proposal ghosts and
/// proposed-cut entities, swaps the grid contents, and syncs the ECS.
pub fn load_map(
    commands: &mut Commands,
    constructed: &mut ConstructedCity,
    pending: &mut ProposedCity,
    assembled: &mut AssembledCity,
    viewable: &mut ViewableWorld,
    structure_list: &EorfList,
    new_contents: Sparse3D<Cell>,
) {
    clear_proposal_entities(commands, assembled);
    clear_proposed_cut_entities(commands, viewable);
    let changes = load_from_offline(constructed, pending, new_contents);
    apply_changes(commands, assembled, structure_list, changes);
}

pub fn discover_user_files(mut ui_state: ResMut<crate::build_ui::UiState>) {
    // Bundled maps are always available.
    ui_state.available_files = BUNDLED_MAPS
        .iter()
        .map(|(name, _)| name.to_string())
        .collect();

    // On native, also add any user-created files not already in the bundled list.
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(dir) = std::fs::read_dir(crate::paths::USER_DIR) {
        let bundled: std::collections::HashSet<&str> =
            BUNDLED_MAPS.iter().map(|(n, _)| *n).collect();
        let mut extra: Vec<String> = dir
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if !name.starts_with('.') && !bundled.contains(name.as_str()) {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();
        extra.sort();
        ui_state.available_files.extend(extra);
    }
}

pub fn handle_file_save(mut ev_saved: MessageReader<DialogFileSaved<SaveDialog>>) {
    for ev in ev_saved.read() {
        if let Err(e) = &ev.result {
            eprintln!("Failed to save file: {e}");
        }
    }
}

pub fn handle_file_load(
    mut ev_loaded: MessageReader<DialogFileLoaded<LoadDialog>>,
    mut commands: Commands,
    structure_list: Res<EorfList>,
    mut constructed: ResMut<ConstructedCity>,
    mut pending: ResMut<ProposedCity>,
    mut assembled: ResMut<AssembledCity>,
    mut viewable: ResMut<ViewableWorld>,
) {
    for ev in ev_loaded.read() {
        if let Ok(content) = std::str::from_utf8(&ev.contents) {
            if let Ok(new_contents) = serialization::load_from_str(content, &constructed.eorfs) {
                load_map(
                    &mut commands,
                    &mut constructed,
                    &mut pending,
                    &mut assembled,
                    &mut viewable,
                    &structure_list,
                    new_contents,
                );
            }
        }
    }
}

/// A map the user asked to load from the bottom control strip, resolved to
/// actual contents (after the egui closure ends) via [`Self::resolve`].
pub enum MapLoadRequest {
    /// A bundled or user map picked from the dropdown, by name.
    Named(String),
    /// A `crate::example_structures` structure, by index.
    Example(usize),
}

impl MapLoadRequest {
    pub fn resolve(self, eorfs: &[EorfInfo]) -> Option<Sparse3D<Cell>> {
        match self {
            MapLoadRequest::Named(name) => load_named_map(&name, eorfs),
            MapLoadRequest::Example(idx) => crate::example_structures::make_structures()
                .into_iter()
                .nth(idx),
        }
    }
}

/// The save/load controls in the bottom strip: the Save/Load file-dialog
/// buttons, the bundled/user-map dropdown, and (on native) the numbered
/// example loader. Loading is only offered in sandbox mode. Returns any load
/// the user requested this frame (the caller performs it after the egui
/// closure ends, since loading needs mutable world access).
pub fn map_file_controls_ui(
    ui: &mut egui::Ui,
    commands: &mut Commands,
    constructed: &ConstructedCity,
    ui_state: &mut crate::build_ui::UiState,
    sandbox_enabled: bool,
) -> Option<MapLoadRequest> {
    let mut load_request = None;

    ui.separator();
    if ui.button("Save").clicked() {
        if let Ok(bytes) = serialization::serialize(&constructed.contents, &constructed.eorfs) {
            commands
                .dialog()
                .add_filter("Orchitecture Map", &["txt"])
                .save_file::<SaveDialog>(bytes);
        }
    }
    // Loading structures is only available in sandbox mode.
    if sandbox_enabled && ui.button("Load").clicked() {
        commands
            .dialog()
            .add_filter("Orchitecture Map", &["txt"])
            .load_file::<LoadDialog>();
    }

    if sandbox_enabled && !ui_state.available_files.is_empty() {
        ui.separator();
        egui::ComboBox::from_id_salt("file_select")
            .selected_text(if ui_state.load_filename.is_empty() {
                "Select a map..."
            } else {
                ui_state.load_filename.as_str()
            })
            .show_ui(ui, |ui| {
                for name in ui_state.available_files.clone() {
                    let resp =
                        ui.selectable_value(&mut ui_state.load_filename, name.clone(), &name);
                    if resp.clicked() {
                        load_request = Some(MapLoadRequest::Named(name));
                    }
                }
            });
    }

    #[cfg(not(target_arch = "wasm32"))]
    if sandbox_enabled {
        ui.separator();
        ui.add(egui::TextEdit::singleline(&mut ui_state.example_idx).desired_width(40.0));
        if ui.button("Load example").clicked() && !ui_state.example_idx.is_empty() {
            if let Ok(idx) = ui_state.example_idx.parse::<usize>() {
                load_request = Some(MapLoadRequest::Example(idx));
            }
        }
    }

    load_request
}
