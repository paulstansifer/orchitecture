use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiGlobalSettings};

use crate::input::BuildState;
use crate::structure::StructureList;
use crate::serialization;
use crate::wall_grid::apply_changes;
use crate::wall_grid::WallGrid;

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
    if let Ok(dir) = std::fs::read_dir(crate::paths::USER_DIR) {
        ui_state.available_files = dir
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if !name.starts_with('.') {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();
        ui_state.available_files.sort();
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

    let user_dir: std::path::PathBuf = crate::paths::USER_DIR.into();

    // Bottom panel must be added before side panels.
    egui::TopBottomPanel::bottom("controls_bottom").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label("Up/Dn=layer  R=rotate  Z=undo  Drag=place  Ctrl+drag=erase  V=evaluate");

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
                let path = user_dir.join(&ui_state.load_filename);
                let new_contents = serialization::load(&path, &wall_grid.structures);
                let changes = wall_grid.load_from_offline(new_contents);
                apply_changes(&mut commands, &mut wall_grid, &structure_list, changes);
            }
            if !ui_state.available_files.is_empty() {
                egui::ComboBox::from_id_salt("file_select")
                    .selected_text(ui_state.load_filename.as_str())
                    .show_ui(ui, |ui| {
                        for name in ui_state.available_files.clone() {
                            ui.selectable_value(&mut ui_state.load_filename, name.clone(), &name);
                        }
                    });
            }

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
        });
    });

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
