use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiGlobalSettings};

use crate::input::BuildState;
use crate::structure::StructureList;
use crate::serialization;
use crate::wall_grid::apply_changes;
use crate::wall_grid::WallGrid;

/// Maps bundled at compile time; always available on all platforms.
const BUNDLED_MAPS: &[(&str, &str)] = &[
    ("boring_room.txt",               include_str!("../training/boring_room.txt")),
    ("boring_room_blob.txt",          include_str!("../training/boring_room_blob.txt")),
    ("boring_room_tall.txt",          include_str!("../training/boring_room_tall.txt")),
    ("boring_room_tall_with_boxes.txt", include_str!("../training/boring_room_tall_with_boxes.txt")),
    ("boring_room_tiny.txt",          include_str!("../training/boring_room_tiny.txt")),
    ("boring_room_with_alcove.txt",   include_str!("../training/boring_room_with_alcove.txt")),
    ("cavern.txt",                    include_str!("../training/cavern.txt")),
    ("chaotic_apartment.txt",         include_str!("../training/chaotic_apartment.txt")),
    ("corner_in_corner.txt",          include_str!("../training/corner_in_corner.txt")),
    ("corners.txt",                   include_str!("../training/corners.txt")),
    ("double_balconies.txt",          include_str!("../training/double_balconies.txt")),
    ("gallery.txt",                   include_str!("../training/gallery.txt")),
    ("hall_turn_fat_pillars.txt",     include_str!("../training/hall_turn_fat_pillars.txt")),
    ("hall_turn_stations.txt",        include_str!("../training/hall_turn_stations.txt")),
    ("long_apartment.txt",            include_str!("../training/long_apartment.txt")),
    ("meta_pillars.txt",              include_str!("../training/meta_pillars.txt")),
    ("random_but_coherent.txt",       include_str!("../training/random_but_coherent.txt")),
    ("rotational_apartment.txt",      include_str!("../training/rotational_apartment.txt")),
    ("sanctuary.txt",                 include_str!("../training/sanctuary.txt")),
    ("simple_apartment.txt",          include_str!("../training/simple_apartment.txt")),
    ("simple_balcony.txt",            include_str!("../training/simple_balcony.txt")),
    ("two_level_apartment.txt",       include_str!("../training/two_level_apartment.txt")),
];

fn find_bundled(name: &str) -> Option<&'static str> {
    BUNDLED_MAPS.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

#[derive(Resource, Default)]
pub struct UiState {
    pub save_filename: String,
    pub load_filename: String,
    pub available_files: Vec<String>,
}

pub fn enable_ui_input_absorption(mut egui_settings: ResMut<EguiGlobalSettings>) {
    egui_settings.enable_absorb_bevy_input_system = true;
}

pub fn discover_user_files(mut ui_state: ResMut<UiState>) {
    // Bundled maps are always available.
    ui_state.available_files = BUNDLED_MAPS.iter().map(|(name, _)| name.to_string()).collect();

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

pub fn ui_system(
    mut commands: Commands,
    mut contexts: EguiContexts,
    structure_list: Res<StructureList>,
    mut wall_grid: ResMut<WallGrid>,
    mut build_state: ResMut<BuildState>,
    mut ui_state: ResMut<UiState>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // Captures a map name selected via the dropdown; handled after the egui block.
    #[cfg(target_arch = "wasm32")]
    let mut wasm_load: Option<String> = None;

    // Bottom panel must be added before side panels.
    egui::TopBottomPanel::bottom("controls_bottom").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label("Up/Dn=layer  R=rotate  Z=undo  Drag=place  Ctrl+drag=erase  V=evaluate");

            #[cfg(not(target_arch = "wasm32"))]
            {
                let user_dir: std::path::PathBuf = crate::paths::USER_DIR.into();

                ui.separator();
                ui.label("Save:");
                ui.add(egui::TextEdit::singleline(&mut ui_state.save_filename).desired_width(110.0));
                if ui.button("Save").clicked() && !ui_state.save_filename.is_empty() {
                    let path = user_dir.join(&ui_state.save_filename);
                    serialization::save(&wall_grid.contents, &wall_grid.structures, &path);
                }

                ui.separator();
                ui.label("Load:");
                ui.add(egui::TextEdit::singleline(&mut ui_state.load_filename).desired_width(110.0));
                if ui.button("Load").clicked() && !ui_state.load_filename.is_empty() {
                    let name = ui_state.load_filename.clone();
                    let new_contents = if let Some(content) = find_bundled(&name) {
                        serialization::load_from_str(content, &wall_grid.structures)
                    } else {
                        serialization::load(&user_dir.join(&name), &wall_grid.structures)
                    };
                    let changes = wall_grid.load_from_offline(new_contents);
                    apply_changes(&mut commands, &mut wall_grid, &structure_list, changes);
                }
            }

            // Dropdown is visible on all platforms.
            if !ui_state.available_files.is_empty() {
                ui.separator();
                egui::ComboBox::from_id_salt("file_select")
                    .selected_text(if ui_state.load_filename.is_empty() {
                        "Select a map..."
                    } else {
                        ui_state.load_filename.as_str()
                    })
                    .show_ui(ui, |ui| {
                        for name in ui_state.available_files.clone() {
                            let _resp = ui.selectable_value(
                                &mut ui_state.load_filename,
                                name.clone(),
                                &name,
                            );
                            #[cfg(target_arch = "wasm32")]
                            if _resp.clicked() {
                                wasm_load = Some(name);
                            }
                        }
                    });
            }

            #[cfg(not(target_arch = "wasm32"))]
            {
                ui.separator();
                if ui.button("Load example").clicked() && !ui_state.load_filename.is_empty() {
                    if let Ok(idx) = ui_state.load_filename.parse::<usize>() {
                        let examples = crate::example_structures::make_structures();
                        if let Some(map) = examples.into_iter().nth(idx) {
                            let changes = wall_grid.load_from_offline(map);
                            apply_changes(&mut commands, &mut wall_grid, &structure_list, changes);
                        }
                    }
                }
            }
        });
    });

    // On wasm, a dropdown click directly loads the selected bundled map.
    #[cfg(target_arch = "wasm32")]
    if let Some(name) = wasm_load {
        if let Some(content) = find_bundled(&name) {
            let new_contents = serialization::load_from_str(content, &wall_grid.structures);
            let changes = wall_grid.load_from_offline(new_contents);
            apply_changes(&mut commands, &mut wall_grid, &structure_list, changes);
        }
    }

    egui::SidePanel::left("controls")
        .min_width(140.0)
        .show(ctx, |ui| {
            ui.heading("Orchitecture");
            ui.separator();

            ui.label("Structure:");
            let names = wall_grid.get_structure_names();
            for (i, name) in names.iter().enumerate() {
                let selected = build_state.selected_structure == i;
                if ui.selectable_label(selected, name).clicked() {
                    build_state.selected_structure = i;
                }
            }

            ui.separator();
            ui.label(format!("Layer (Y): {}", build_state.cur_y));
            ui.label(format!("Direction: {}", build_state.cur_dir));

            if let Some((coherence, interest)) = build_state.evaluation {
                ui.separator();
                ui.label(format!("Coherence: {:.3}", coherence));
                ui.label(format!("Interest:  {:.3}", interest));
            }
        });
}
