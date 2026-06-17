use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiGlobalSettings};
use bevy_file_dialog::prelude::*;

use crate::cutaway::CutawayMode;
use crate::input::BuildState;
use crate::serialization;
use crate::sparse3d::{Slot, SlotCoord};
use crate::structure::StructureList;
use crate::wall_grid::{
    apply_changes, apply_proposal_changes, Material, ProposalOverlayAssets, WallGrid,
};

/// Maps bundled at compile time; always available on all platforms.
const BUNDLED_MAPS: &[(&str, &str)] = &[
    ("boring_room", include_str!("../training/boring_room.txt")),
    (
        "boring_room_blob",
        include_str!("../training/boring_room_blob.txt"),
    ),
    (
        "boring_room_tall",
        include_str!("../training/boring_room_tall.txt"),
    ),
    (
        "boring_room_tall_with_boxes",
        include_str!("../training/boring_room_tall_with_boxes.txt"),
    ),
    (
        "boring_room_tiny",
        include_str!("../training/boring_room_tiny.txt"),
    ),
    (
        "boring_room_with_alcove",
        include_str!("../training/boring_room_with_alcove.txt"),
    ),
    ("cavern", include_str!("../training/cavern.txt")),
    (
        "chaotic_apartment",
        include_str!("../training/chaotic_apartment.txt"),
    ),
    (
        "corner_in_corner",
        include_str!("../training/corner_in_corner.txt"),
    ),
    ("corners", include_str!("../training/corners.txt")),
    (
        "double_balconies",
        include_str!("../training/double_balconies.txt"),
    ),
    ("gallery", include_str!("../training/gallery.txt")),
    (
        "hall_turn_fat_pillars",
        include_str!("../training/hall_turn_fat_pillars.txt"),
    ),
    (
        "hall_turn_stations",
        include_str!("../training/hall_turn_stations.txt"),
    ),
    (
        "long_apartment",
        include_str!("../training/long_apartment.txt"),
    ),
    ("meta_pillars", include_str!("../training/meta_pillars.txt")),
    (
        "random_but_coherent",
        include_str!("../training/random_but_coherent.txt"),
    ),
    (
        "rotational_apartment",
        include_str!("../training/rotational_apartment.txt"),
    ),
    ("sanctuary", include_str!("../training/sanctuary.txt")),
    (
        "simple_apartment",
        include_str!("../training/simple_apartment.txt"),
    ),
    (
        "simple_balcony",
        include_str!("../training/simple_balcony.txt"),
    ),
    (
        "two_level_apartment",
        include_str!("../training/two_level_apartment.txt"),
    ),
    ("porches", include_str!("../training/porches.txt")),
];

fn find_bundled(name: &str) -> Option<&'static str> {
    BUNDLED_MAPS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| *c)
}

pub struct SaveDialog;
pub struct LoadDialog;

/// Which content the left-hand panel shows.
#[derive(Default, Clone)]
pub enum LeftPanel {
    /// Normal palette / material / construct controls.
    #[default]
    Build,
    /// Station view for the furniture cube that was right-clicked.
    Station { cube: IVec3 },
}

#[derive(Resource, Default)]
pub struct UiState {
    pub load_filename: String,
    pub example_idx: String,
    pub available_files: Vec<String>,
    pub left_panel: LeftPanel,
}

/// A right-click pick on a real furniture cell, produced by `building_input_system`
/// and consumed by `ui_system` to open the station panel.
#[derive(Resource, Default)]
pub struct FurnitureRightClick(pub Option<IVec3>);

/// When enabled, construction edits commit immediately instead of becoming
/// proposals, and (eventually) edits are free. Loading structures is only
/// available in sandbox mode. Enabled on startup.
#[derive(Resource)]
pub struct SandboxMode {
    pub enabled: bool,
}

impl Default for SandboxMode {
    fn default() -> Self {
        SandboxMode { enabled: true }
    }
}

pub fn enable_ui_input_absorption(mut egui_settings: ResMut<EguiGlobalSettings>) {
    egui_settings.enable_absorb_bevy_input_system = true;
}

pub fn discover_user_files(mut ui_state: ResMut<UiState>) {
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

/// Despawns all proposal preview entities and drains `proposal_entities`.
fn clear_proposal_entities(commands: &mut Commands, wall_grid: &mut WallGrid) {
    for (_, entities) in wall_grid.proposal_entities.drain() {
        for entity in entities {
            commands.entity(entity).despawn();
        }
    }
}

/// Despawns all persistent proposed-cut entities and drains `proposed_cut_entities`.
fn clear_proposed_cut_entities(commands: &mut Commands, wall_grid: &mut WallGrid) {
    for (_, entities) in wall_grid.proposed_cut_entities.drain() {
        for entity in entities {
            commands.entity(entity).despawn();
        }
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
    structure_list: Res<StructureList>,
    mut wall_grid: ResMut<WallGrid>,
) {
    for ev in ev_loaded.read() {
        if let Ok(content) = std::str::from_utf8(&ev.contents) {
            let new_contents = serialization::load_from_str(content, &wall_grid.structures);
            clear_proposal_entities(&mut commands, &mut wall_grid);
            clear_proposed_cut_entities(&mut commands, &mut wall_grid);
            let changes = wall_grid.load_from_offline(new_contents);
            apply_changes(&mut commands, &mut wall_grid, &structure_list, changes);
        }
    }
}

pub fn ui_system(
    mut commands: Commands,
    mut contexts: EguiContexts,
    structure_list: Res<StructureList>,
    mut wall_grid: ResMut<WallGrid>,
    mut build_state: ResMut<BuildState>,
    mut ui_state: ResMut<UiState>,
    overlay_assets: Res<ProposalOverlayAssets>,
    mut cutaway_mode: ResMut<CutawayMode>,
    mut sandbox: ResMut<SandboxMode>,
    mut furniture_right_click: ResMut<FurnitureRightClick>,
    mut station_highlight: ResMut<crate::wall_grid::StationHighlight>,
    mut isometric: ResMut<crate::camera::IsometricCamera>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // A right-click pick (from building_input_system) opens the station panel.
    if let Some(cube) = furniture_right_click.0.take() {
        ui_state.left_panel = LeftPanel::Station { cube };
    }

    // Captures a map name selected via the dropdown; handled after the egui block.
    let mut dropdown_load: Option<String> = None;

    // Bottom panel must be added before side panels.
    egui::TopBottomPanel::bottom("controls_bottom").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                "Up/Dn=layer  R=rotate  Z=undo  Y=redo  Drag=place  Ctrl+drag=erase  V=evaluate",
            );

            ui.separator();
            ui.checkbox(&mut isometric.enabled, "Isometric");

            ui.separator();
            let was_sandbox = sandbox.enabled;
            ui.checkbox(&mut sandbox.enabled, "Sandbox");
            if sandbox.enabled && !was_sandbox {
                // Switching into sandbox commits any pending proposals immediately.
                let real_changes = wall_grid.construct();
                clear_proposal_entities(&mut commands, &mut wall_grid);
                clear_proposed_cut_entities(&mut commands, &mut wall_grid);
                apply_changes(&mut commands, &mut wall_grid, &structure_list, real_changes);
            }

            ui.separator();
            if ui.button("Save").clicked() {
                let bytes = serialization::serialize(&wall_grid.contents, &wall_grid.structures);
                commands
                    .dialog()
                    .add_filter("Orchitecture Map", &["txt"])
                    .save_file::<SaveDialog>(bytes);
            }
            // Loading structures is only available in sandbox mode.
            if sandbox.enabled && ui.button("Load").clicked() {
                commands
                    .dialog()
                    .add_filter("Orchitecture Map", &["txt"])
                    .load_file::<LoadDialog>();
            }

            ui.separator();
            ui.label("Cutaway:");
            egui::ComboBox::from_id_salt("cutaway_mode")
                .selected_text(match *cutaway_mode {
                    CutawayMode::FloorEdge => "FloorEdge",
                    CutawayMode::SimpleOctant => "SimpleOctant",
                    CutawayMode::FloorEdgePlusOctant => "FloorEdge+Octant",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut *cutaway_mode, CutawayMode::FloorEdge, "FloorEdge");
                    ui.selectable_value(
                        &mut *cutaway_mode,
                        CutawayMode::SimpleOctant,
                        "SimpleOctant",
                    );
                    ui.selectable_value(
                        &mut *cutaway_mode,
                        CutawayMode::FloorEdgePlusOctant,
                        "FloorEdge+Octant",
                    );
                });

            if sandbox.enabled && !ui_state.available_files.is_empty() {
                ui.separator();
                egui::ComboBox::from_id_salt("file_select")
                    .selected_text(if ui_state.load_filename.is_empty() {
                        "Select a map..."
                    } else {
                        ui_state.load_filename.as_str()
                    })
                    .show_ui(ui, |ui| {
                        for name in ui_state.available_files.clone() {
                            let resp = ui.selectable_value(
                                &mut ui_state.load_filename,
                                name.clone(),
                                &name,
                            );
                            if resp.clicked() {
                                dropdown_load = Some(name);
                            }
                        }
                    });
            }

            #[cfg(not(target_arch = "wasm32"))]
            if sandbox.enabled {
                ui.separator();
                ui.add(egui::TextEdit::singleline(&mut ui_state.example_idx).desired_width(40.0));
                if ui.button("Load example").clicked() && !ui_state.example_idx.is_empty() {
                    if let Ok(idx) = ui_state.example_idx.parse::<usize>() {
                        let examples = crate::example_structures::make_structures();
                        if let Some(map) = examples.into_iter().nth(idx) {
                            clear_proposal_entities(&mut commands, &mut wall_grid);
                            clear_proposed_cut_entities(&mut commands, &mut wall_grid);
                            let changes = wall_grid.load_from_offline(map);
                            apply_changes(&mut commands, &mut wall_grid, &structure_list, changes);
                        }
                    }
                }
            }
        });
    });

    // A dropdown click loads the selected map immediately on all platforms.
    if let Some(name) = dropdown_load {
        let new_contents_opt = if let Some(content) = find_bundled(&name) {
            Some(serialization::load_from_str(content, &wall_grid.structures))
        } else {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let path = std::path::PathBuf::from(crate::paths::USER_DIR).join(&name);
                Some(serialization::load(&path, &wall_grid.structures))
            }
            #[cfg(target_arch = "wasm32")]
            None
        };
        if let Some(new_contents) = new_contents_opt {
            clear_proposal_entities(&mut commands, &mut wall_grid);
            clear_proposed_cut_entities(&mut commands, &mut wall_grid);
            let changes = wall_grid.load_from_offline(new_contents);
            apply_changes(&mut commands, &mut wall_grid, &structure_list, changes);
        }
    }

    // Deferred mutations so the panel closure only borrows `wall_grid` immutably
    // in the station arms (the build arm still mutates it directly).
    let mut next_panel: Option<LeftPanel> = None;
    let mut assign: Option<(IVec3, usize)> = None;
    let mut unassign: Option<usize> = None;
    let mut highlight: Vec<IVec3> = Vec::new();
    let panel = ui_state.left_panel.clone();

    egui::SidePanel::left("controls")
        .min_width(100.0)
        .max_width(140.0)
        .show(ctx, |ui| {
            match panel {
                LeftPanel::Station { cube } => {
                    ui.heading("Station");
                    ui.separator();
                    if ui.button("← Back").clicked() {
                        next_panel = Some(LeftPanel::Build);
                    }
                    ui.separator();
                    match crate::station::station_index_at(&wall_grid, cube) {
                        Some(idx) => {
                            let ps = &wall_grid.placed_stations[idx];
                            let def = &wall_grid.stations[ps.station];
                            ui.label(format!("Name: {}", def.name));

                            let mut counts: std::collections::BTreeMap<String, usize> =
                                std::collections::BTreeMap::new();
                            for loc in &ps.structure_locations {
                                if let Some(cell) = wall_grid.contents.get(SlotCoord {
                                    cube: *loc,
                                    slot: Slot::Room,
                                }) {
                                    *counts
                                        .entry(
                                            wall_grid.structures[cell.id.as_usize()].name.clone(),
                                        )
                                        .or_default() += 1;
                                }
                            }
                            ui.separator();
                            ui.label("Structures:");
                            for (name, c) in &counts {
                                ui.label(format!("  {}: {}", name, c));
                            }

                            ui.separator();
                            ui.label("Contents:");
                            let totals = ps.contents.uniform_totals();
                            if totals.is_empty() {
                                ui.label("  (empty)");
                            }
                            for (res, qty) in totals {
                                ui.label(format!("  {}: {}", res.label(), qty));
                            }

                            // Highlight this station's furniture in 3D.
                            highlight = ps.structure_locations.clone();

                            ui.separator();
                            if ui.button("Unassign station").clicked() {
                                unassign = Some(idx);
                                next_panel = Some(LeftPanel::Build);
                            }
                        }
                        None => {
                            // A `Some` plan is exactly the validity test, so one pass over
                            // the stations both filters and yields the "Pulls N" count.
                            let plans: Vec<(usize, usize)> = (0..wall_grid.stations.len())
                                .filter_map(|s_idx| {
                                    crate::station::plan_assignment(&wall_grid, cube, s_idx)
                                        .map(|plan| (s_idx, plan.pulled))
                                })
                                .collect();
                            if plans.is_empty() {
                                ui.label("No valid stations here.");
                            } else {
                                ui.label("Create station:");
                            }
                            for (s_idx, pulled) in plans {
                                if ui.button(&wall_grid.stations[s_idx].name).clicked() {
                                    assign = Some((cube, s_idx));
                                }
                                if pulled > 0 {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "Pulls {} already-assigned structures",
                                            pulled
                                        ))
                                        .italics(),
                                    );
                                }
                            }
                        }
                    }
                }
                LeftPanel::Build => {
                    ui.heading("Orchitecture");
                    ui.separator();

                    ui.label("Structure:");
                    let names = wall_grid.get_structure_names();
                    for (i, name) in names.iter().enumerate() {
                        let selected = build_state.selected_structure == i;
                        let label = if i < 9 {
                            format!("{}. {}", i + 1, name)
                        } else {
                            name.clone()
                        };
                        if ui.selectable_label(selected, &label).clicked() {
                            build_state.selected_structure = i;
                        }
                    }

                    ui.separator();
                    ui.label("Material:");
                    for material in Material::ALL {
                        ui.radio_value(
                            &mut build_state.selected_material,
                            material,
                            material.label(),
                        );
                    }

                    ui.separator();
                    ui.label(format!("Layer (Y): {}", build_state.cur_y));
                    ui.label(format!("Direction: {}", build_state.cur_dir));

                    if let Some((coherence, interest)) = build_state.evaluation {
                        ui.separator();
                        ui.label(format!("Coherence: {:.3}", coherence));
                        ui.label(format!("Interest:  {:.3}", interest));
                    }

                    let n = wall_grid.num_proposed_changes();
                    if n > 0 {
                        ui.separator();
                        let months = wall_grid.months_for_construction();
                        let months_label = if months == 1 {
                            "month".to_string()
                        } else {
                            "months".to_string()
                        };
                        if ui
                            .button(format!("Construct! ({months} {months_label})"))
                            .clicked()
                        {
                            // commit() writes to contents (triggers ceiling-light recompute)
                            let real_changes = wall_grid.construct();
                            clear_proposal_entities(&mut commands, &mut wall_grid);
                            clear_proposed_cut_entities(&mut commands, &mut wall_grid);
                            apply_changes(
                                &mut commands,
                                &mut wall_grid,
                                &structure_list,
                                real_changes,
                            );
                        }
                        if ui.button("Reset").clicked() {
                            // Reset clears proposals without committing; bypass detection so
                            // ceiling lights don't recompute.
                            let deltas = {
                                let wg = wall_grid.bypass_change_detection();
                                let locs: Vec<_> =
                                    wg.proposed_changes.iter().map(|(l, _)| l).collect();
                                wg.reset_proposals();
                                locs.into_iter()
                                    .map(|loc| (loc, crate::wall_grid::ProposalView::None))
                                    .collect::<Vec<_>>()
                            };
                            // Despawn all proposal entities (bypassing is fine since we already have mut wg above)
                            apply_proposal_changes(
                                &mut commands,
                                &mut wall_grid,
                                &structure_list,
                                &overlay_assets,
                                deltas,
                            );
                            clear_proposed_cut_entities(&mut commands, &mut wall_grid);
                        }
                    }
                }
            }
        });

    // Apply deferred station mutations now that the panel closure has ended.
    if let Some((cube, s_idx)) = assign {
        crate::station::commit_assignment(&mut wall_grid, cube, s_idx);
    }
    if let Some(idx) = unassign {
        crate::station::unassign_station(&mut wall_grid, idx);
    }
    if let Some(p) = next_panel {
        ui_state.left_panel = p;
    }
    // Write only on change (set_if_neq), so the highlight system doesn't respawn
    // the overlay meshes every frame.
    station_highlight.set_if_neq(crate::wall_grid::StationHighlight(highlight));

    resource_sidebar(ctx, &wall_grid);
}

/// Right-hand sidebar summarizing total resources across every storage station.
///
/// Each station's count is run through its own accounting approximation and then
/// the rounded results are summed (round-each-then-sum). A `>` prefix is shown for
/// a resource when any contributing station rounded its count down.
fn resource_sidebar(ctx: &egui::Context, wall_grid: &WallGrid) {
    use crate::resource::{round, UniformResource};

    // Preserve a stable display order of resource kinds as first encountered.
    let mut order: Vec<UniformResource> = Vec::new();
    let mut totals: std::collections::HashMap<UniformResource, (u32, bool)> =
        std::collections::HashMap::new();

    for station in &wall_grid.placed_stations {
        let Some(info) = wall_grid.stations.get(station.station) else {
            continue;
        };
        let Some(spec) = &info.storage else {
            continue;
        };
        for (res, qty) in station.contents.uniform_totals() {
            let (rounded, dropped) = round(qty, spec.accounting);
            let entry = totals.entry(res).or_insert_with(|| {
                order.push(res);
                (0, false)
            });
            entry.0 += rounded as u32;
            entry.1 |= dropped;
        }
    }

    egui::SidePanel::right("resources")
        .min_width(120.0)
        .show(ctx, |ui| {
            ui.heading("Resources");
            ui.separator();
            if order.is_empty() {
                ui.label("(none)");
            }
            for res in order {
                let (total, rounded_down) = totals[&res];
                let prefix = if rounded_down { "> " } else { "" };
                ui.label(format!("{}{}: {}", prefix, res.label(), total));
            }
        });
}
