use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiGlobalSettings};
use bevy_file_dialog::prelude::*;

use crate::construction::{construct, load_from_offline};
use crate::cutaway::CutawayMode;
use crate::input::BuildState;
use crate::materials::MaterialList;
use crate::population::Population;
use crate::resource_icons::{ResourceIcons, LARGE_SIZE};
use crate::serialization;
use crate::sparse3d::{Slot, SlotCoord};
use crate::structure::sorted_structure_indices;
use crate::structure::StructureList;
use crate::world::{
    apply_changes, apply_proposal_changes, clear_proposal_entities, clear_proposed_cut_entities,
    AssembledWorld, BuildWorldParams, ConstructedWorld, ProposalOverlayAssets, ProposedWorld,
    ViewableWorld,
};

/// Maps bundled at compile time; always available on all platforms.
const BUNDLED_MAPS: &[(&str, &str)] = &[
    (
        "boring_room",
        include_str!("../assets/static/training/boring_room.txt"),
    ),
    (
        "boring_room_blob",
        include_str!("../assets/static/training/boring_room_blob.txt"),
    ),
    (
        "boring_room_tall",
        include_str!("../assets/static/training/boring_room_tall.txt"),
    ),
    (
        "boring_room_tall_with_boxes",
        include_str!("../assets/static/training/boring_room_tall_with_boxes.txt"),
    ),
    (
        "boring_room_tiny",
        include_str!("../assets/static/training/boring_room_tiny.txt"),
    ),
    (
        "boring_room_with_alcove",
        include_str!("../assets/static/training/boring_room_with_alcove.txt"),
    ),
    (
        "cavern",
        include_str!("../assets/static/training/cavern.txt"),
    ),
    (
        "chaotic_apartment",
        include_str!("../assets/static/training/chaotic_apartment.txt"),
    ),
    (
        "corner_in_corner",
        include_str!("../assets/static/training/corner_in_corner.txt"),
    ),
    (
        "corners",
        include_str!("../assets/static/training/corners.txt"),
    ),
    (
        "double_balconies",
        include_str!("../assets/static/training/double_balconies.txt"),
    ),
    (
        "gallery",
        include_str!("../assets/static/training/gallery.txt"),
    ),
    (
        "hall_turn_fat_pillars",
        include_str!("../assets/static/training/hall_turn_fat_pillars.txt"),
    ),
    (
        "hall_turn_stations",
        include_str!("../assets/static/training/hall_turn_stations.txt"),
    ),
    (
        "long_apartment",
        include_str!("../assets/static/training/long_apartment.txt"),
    ),
    (
        "meta_pillars",
        include_str!("../assets/static/training/meta_pillars.txt"),
    ),
    (
        "random_but_coherent",
        include_str!("../assets/static/training/random_but_coherent.txt"),
    ),
    (
        "rotational_apartment",
        include_str!("../assets/static/training/rotational_apartment.txt"),
    ),
    (
        "sanctuary",
        include_str!("../assets/static/training/sanctuary.txt"),
    ),
    (
        "simple_apartment",
        include_str!("../assets/static/training/simple_apartment.txt"),
    ),
    (
        "simple_balcony",
        include_str!("../assets/static/training/simple_balcony.txt"),
    ),
    (
        "two_level_apartment",
        include_str!("../assets/static/training/two_level_apartment.txt"),
    ),
    (
        "porches",
        include_str!("../assets/static/training/porches.txt"),
    ),
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
    pub show_population: bool,
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
    mut constructed: ResMut<ConstructedWorld>,
    mut pending: ResMut<ProposedWorld>,
    mut assembled: ResMut<AssembledWorld>,
    mut viewable: ResMut<ViewableWorld>,
) {
    for ev in ev_loaded.read() {
        if let Ok(content) = std::str::from_utf8(&ev.contents) {
            let new_contents = serialization::load_from_str(content, &constructed.structures);
            clear_proposal_entities(&mut commands, &mut assembled);
            clear_proposed_cut_entities(&mut commands, &mut viewable);
            let changes = load_from_offline(&mut constructed, &mut pending, new_contents);
            apply_changes(&mut commands, &mut assembled, &structure_list, changes);
        }
    }
}

pub fn build_ui_system(
    mut commands: Commands,
    mut contexts: EguiContexts,
    structure_list: Res<StructureList>,
    mut world: BuildWorldParams,
    mut viewable: ResMut<ViewableWorld>,
    mut build_state: ResMut<BuildState>,
    mut ui_state: ResMut<UiState>,
    overlay_assets: Res<ProposalOverlayAssets>,
    mut cutaway_mode: ResMut<CutawayMode>,
    mut sandbox: ResMut<SandboxMode>,
    mut furniture_right_click: ResMut<FurnitureRightClick>,
    mut station_highlight: ResMut<crate::world::StationHighlight>,
    resource_icons: Res<ResourceIcons>,
    material_list: Res<MaterialList>,
    population: Res<Population>,
) {
    let icon_textures = resource_icons.texture_ids_large(&mut contexts);
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
            let was_sandbox = sandbox.enabled;
            ui.checkbox(&mut sandbox.enabled, "Sandbox");
            if sandbox.enabled && !was_sandbox {
                // Switching into sandbox commits any pending proposals immediately.
                let real_changes = construct(&mut *world.constructed, &mut *world.pending);
                clear_proposal_entities(&mut commands, &mut *world.assembled);
                clear_proposed_cut_entities(&mut commands, &mut viewable);
                apply_changes(&mut commands, &mut *world.assembled, &structure_list, real_changes);
            }

            ui.separator();
            if ui.button("Save").clicked() {
                let bytes =
                    serialization::serialize(&world.constructed.contents, &world.constructed.structures);
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
                            clear_proposal_entities(&mut commands, &mut *world.assembled);
                            clear_proposed_cut_entities(&mut commands, &mut viewable);
                            let changes = load_from_offline(&mut *world.constructed, &mut *world.pending, map);
                            apply_changes(&mut commands, &mut *world.assembled, &structure_list, changes);
                        }
                    }
                }
            }
        });
    });

    // A dropdown click loads the selected map immediately on all platforms.
    if let Some(name) = dropdown_load {
        let new_contents_opt = if let Some(content) = find_bundled(&name) {
            Some(serialization::load_from_str(
                content,
                &world.constructed.structures,
            ))
        } else {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let path = std::path::PathBuf::from(crate::paths::USER_DIR).join(&name);
                Some(serialization::load(&path, &world.constructed.structures))
            }
            #[cfg(target_arch = "wasm32")]
            None
        };
        if let Some(new_contents) = new_contents_opt {
            clear_proposal_entities(&mut commands, &mut *world.assembled);
            clear_proposed_cut_entities(&mut commands, &mut viewable);
            let changes = load_from_offline(&mut *world.constructed, &mut *world.pending, new_contents);
            apply_changes(&mut commands, &mut *world.assembled, &structure_list, changes);
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
                    match crate::station::station_index_at(&world.constructed, cube) {
                        Some(idx) => {
                            let ps = &world.constructed.placed_stations[idx];
                            let def = &world.constructed.stations[ps.station];
                            ui.label(format!("Name: {}", def.name));

                            let mut counts: std::collections::BTreeMap<String, usize> =
                                std::collections::BTreeMap::new();
                            for loc in &ps.structure_locations {
                                if let Some(cell) = world.constructed.contents.get(SlotCoord {
                                    cube: *loc,
                                    slot: Slot::Room,
                                }) {
                                    *counts
                                        .entry(
                                            world.constructed.structures[cell.id.as_usize()].name.clone(),
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
                                ui.horizontal(|ui| {
                                    if let Some(&tex) = icon_textures.get(&res) {
                                        ui.add(egui::Image::new(egui::load::SizedTexture::new(
                                            tex, LARGE_SIZE,
                                        )));
                                    }
                                    ui.label(format!("{}", qty));
                                });
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
                            let plans: Vec<(usize, usize)> = (0..world.constructed.stations.len())
                                .filter_map(|s_idx| {
                                    crate::station::plan_assignment(&world.constructed, cube, s_idx)
                                        .map(|plan| (s_idx, plan.pulled))
                                })
                                .collect();
                            if plans.is_empty() {
                                ui.label("No valid stations here.");
                            } else {
                                ui.label("Create station:");
                            }
                            for (s_idx, pulled) in plans {
                                if ui.button(&world.constructed.stations[s_idx].name).clicked() {
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

                    // Structures sorted by type with group headers.
                    let sorted = sorted_structure_indices(&world.constructed.structures);
                    let mut last_stype: Option<crate::materials::StructureType> = None;
                    for (display_idx, &struct_idx) in sorted.iter().enumerate() {
                        let info = &world.constructed.structures[struct_idx];
                        let stype = info.structure_type;
                        if last_stype != Some(stype) {
                            if last_stype.is_some() {
                                ui.separator();
                            }
                            ui.label(
                                egui::RichText::new(stype.label())
                                    .small()
                                    .color(egui::Color32::from_gray(140)),
                            );
                            last_stype = Some(stype);
                        }
                        let selected = build_state.selected_structure == struct_idx;
                        let label = if display_idx < 9 {
                            format!("{}. {}", display_idx + 1, info.name)
                        } else {
                            info.name.clone()
                        };
                        if ui.selectable_label(selected, &label).clicked() {
                            build_state.selected_structure = struct_idx;
                        }
                    }

                    // Material picker for the selected structure's type.
                    let selected_info = &world.constructed.structures[build_state.selected_structure];
                    let stype = selected_info.structure_type;
                    let options = material_list.for_type(stype);
                    if !options.is_empty() {
                        ui.separator();
                        ui.label("Material:");
                        let current = build_state
                            .material_per_type
                            .get(&stype)
                            .copied()
                            .unwrap_or(0);
                        let mut chosen = current;
                        for (local_idx, mat) in options.iter().enumerate() {
                            ui.radio_value(&mut chosen, local_idx, &mat.name);
                        }
                        if chosen != current {
                            build_state.material_per_type.insert(stype, chosen);
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

                    if !population.individuals.is_empty() {
                        let avg_morale = population.individuals.iter().map(|i| i.morale()).sum::<f32>()
                            / population.individuals.len() as f32;
                        ui.separator();
                        if ui
                            .button(format!("Morale: {:.2}", avg_morale))
                            .clicked()
                        {
                            ui_state.show_population = !ui_state.show_population;
                        }
                    }

                    if world.pending.num_changes() > 0 {
                        ui.separator();
                        if ui.button("Reset").clicked() {
                            let locs: Vec<_> =
                                world.pending.proposed_changes.iter().map(|(l, _)| l).collect();
                            world.pending.reset();
                            let deltas: Vec<_> = locs
                                .into_iter()
                                .map(|loc| (loc, crate::world::ProposalView::None))
                                .collect();
                            apply_proposal_changes(
                                &mut commands,
                                &mut *world.assembled,
                                &structure_list,
                                &overlay_assets,
                                deltas,
                            );
                            clear_proposed_cut_entities(&mut commands, &mut viewable);
                        }
                    }
                }
            }
        });

    if ui_state.show_population {
        egui::Window::new("Population")
            .open(&mut ui_state.show_population)
            .resizable(false)
            .show(ctx, |ui| {
                if population.individuals.is_empty() {
                    ui.label("(no individuals)");
                } else {
                    let avg_morale = population.individuals.iter().map(|i| i.morale()).sum::<f32>()
                        / population.individuals.len() as f32;
                    ui.label(format!("Avg morale: {:.2}", avg_morale));
                    ui.separator();
                    for (i, individual) in population.individuals.iter().enumerate() {
                        ui.collapsing(
                            format!("Individual {}  {:.2}", i + 1, individual.morale()),
                            |ui| {
                                need_bar(ui, "shelter", individual.shelter());
                                need_bar(ui, "food", individual.food());
                                need_bar(ui, "inspire", individual.inspiration());
                            },
                        );
                    }
                }
            });
    }

    // Apply deferred station mutations now that the panel closure has ended.
    if let Some((cube, s_idx)) = assign {
        crate::station::commit_assignment(&mut *world.constructed, cube, s_idx);
    }
    if let Some(idx) = unassign {
        crate::station::unassign_station(&mut *world.constructed, idx);
    }
    if let Some(p) = next_panel {
        ui_state.left_panel = p;
    }
    // Write only on change (set_if_neq), so the highlight system doesn't respawn
    // the overlay meshes every frame.
    station_highlight.set_if_neq(crate::world::StationHighlight(highlight));
}

fn need_bar(ui: &mut egui::Ui, label: &str, value: f32) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::ProgressBar::new(value / 1.2));
    });
}

/// Totals of all resources across every storage station, sorted for display.
/// Returns `(resource, total_quantity, precision)`.
pub(crate) fn station_resource_totals(
    constructed: &ConstructedWorld,
) -> Vec<(
    crate::resource::UniformResource,
    u32,
    crate::resource::Precision,
)> {
    use crate::resource::{round, Precision, UniformResource};
    use std::collections::HashMap;

    let mut map: HashMap<UniformResource, (u32, Precision)> = HashMap::new();
    for station in &constructed.placed_stations {
        let Some(info) = constructed.stations.get(station.station) else {
            continue;
        };
        let Some(spec) = &info.storage else {
            continue;
        };
        for (res, qty) in station.contents.uniform_totals() {
            let (rounded, precision) = round(qty, spec.accounting);
            let entry = map.entry(res).or_insert((0, Precision::Exact));
            entry.0 += rounded as u32;
            if precision != Precision::Exact {
                entry.1 = precision;
            }
        }
    }
    let mut result: Vec<_> = map.into_iter().map(|(r, (q, p))| (r, q, p)).collect();
    result.sort_by_key(|(r, _, _)| *r);
    result
}
