use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiGlobalSettings};
use bevy_file_dialog::prelude::*;

use crate::city::{
    apply_changes, apply_proposal_changes, clear_proposal_entities, clear_proposed_cut_entities,
    AssembledCity, CityMut, ConstructedCity, ProposalOverlayAssets, ProposedCity, ViewableWorld,
};
use crate::construction::{construct, load_from_offline};
use crate::cutaway::CutawayMode;
use crate::eorf::sorted_structure_indices;
use crate::eorf::EorfList;
use crate::input::BuildState;
use crate::materials::MaterialList;
use crate::population::Population;
use crate::resource_icons::{ResourceIcons, LARGE_SIZE};
use crate::serialization;
use crate::sparse3d::{Slot, SlotCoord};

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

/// Which tab the left-hand panel shows.
#[derive(Default, Clone, Copy, PartialEq)]
pub enum LeftTab {
    #[default]
    Elements,
    Furniture,
    Places,
}

#[derive(Resource, Default)]
pub struct PathfindingDebugEnabled(pub bool);

/// What the "Places" tab currently shows.
#[derive(Default, Clone, Copy)]
pub enum PlacesView {
    /// Every `Place` kind and its (recursive) requirements.
    #[default]
    List,
    /// Ancestor-chain view for the furniture cell that was right-clicked.
    Hierarchy { loc: SlotCoord },
}

#[derive(Resource, Default)]
pub struct UiState {
    pub load_filename: String,
    pub example_idx: String,
    pub available_files: Vec<String>,
    pub left_tab: LeftTab,
    pub places_view: PlacesView,
    pub show_population: bool,
}

/// A right-click pick on a real furniture cell, produced by `building_input_system`
/// and consumed by `ui_system` to open the place panel.
#[derive(Resource, Default)]
pub struct FurnitureRightClick(pub Option<SlotCoord>);

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
    structure_list: Res<EorfList>,
    mut constructed: ResMut<ConstructedCity>,
    mut pending: ResMut<ProposedCity>,
    mut assembled: ResMut<AssembledCity>,
    mut viewable: ResMut<ViewableWorld>,
) {
    for ev in ev_loaded.read() {
        if let Ok(content) = std::str::from_utf8(&ev.contents) {
            let new_contents = serialization::load_from_str(content, &constructed.eorfs);
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
    structure_list: Res<EorfList>,
    mut world: CityMut,
    mut viewable: ResMut<ViewableWorld>,
    mut build_state: ResMut<BuildState>,
    mut ui_state: ResMut<UiState>,
    overlay_assets: Res<ProposalOverlayAssets>,
    mut cutaway_mode: ResMut<CutawayMode>,
    mut sandbox: ResMut<SandboxMode>,
    mut furniture_right_click: ResMut<FurnitureRightClick>,
    mut place_highlight: ResMut<crate::city::PlaceHighlight>,
    resource_icons: Res<ResourceIcons>,
    material_list: Res<MaterialList>,
    population: Res<Population>,
) {
    let icon_textures = resource_icons.texture_ids_large(&mut contexts);
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // A right-click pick (from building_input_system) opens the place panel.
    if let Some(loc) = furniture_right_click.0.take() {
        ui_state.left_tab = LeftTab::Places;
        ui_state.places_view = PlacesView::Hierarchy { loc };
    }

    // Captures a map name selected via the dropdown; handled after the egui block.
    let mut dropdown_load: Option<String> = None;

    // Bottom panel must be added before side panels.
    egui::TopBottomPanel::bottom("controls_bottom").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                "Up/Dn=layer  R=rotate  Z=undo  Y=redo  Drag=place  Ctrl+drag=erase  V=evaluate  F1/F2/F3=tabs",
            );

            ui.separator();
            let was_sandbox = sandbox.enabled;
            ui.checkbox(&mut sandbox.enabled, "Sandbox");
            if sandbox.enabled && !was_sandbox {
                // Switching into sandbox commits any pending proposals immediately.
                let real_changes =
                    construct(&mut world.constructed, &mut world.pending, &material_list);
                clear_proposal_entities(&mut commands, &mut world.assembled);
                clear_proposed_cut_entities(&mut commands, &mut viewable);
                apply_changes(
                    &mut commands,
                    &mut world.assembled,
                    &structure_list,
                    real_changes,
                );
            }

            ui.separator();
            if ui.button("Save").clicked() {
                let bytes =
                    serialization::serialize(&world.constructed.contents, &world.constructed.eorfs);
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
                            clear_proposal_entities(&mut commands, &mut world.assembled);
                            clear_proposed_cut_entities(&mut commands, &mut viewable);
                            let changes =
                                load_from_offline(&mut world.constructed, &mut world.pending, map);
                            apply_changes(
                                &mut commands,
                                &mut world.assembled,
                                &structure_list,
                                changes,
                            );
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
                &world.constructed.eorfs,
            ))
        } else {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let path = std::path::PathBuf::from(crate::paths::USER_DIR).join(&name);
                Some(serialization::load(&path, &world.constructed.eorfs))
            }
            #[cfg(target_arch = "wasm32")]
            None
        };
        if let Some(new_contents) = new_contents_opt {
            clear_proposal_entities(&mut commands, &mut world.assembled);
            clear_proposed_cut_entities(&mut commands, &mut viewable);
            let changes =
                load_from_offline(&mut world.constructed, &mut world.pending, new_contents);
            apply_changes(
                &mut commands,
                &mut world.assembled,
                &structure_list,
                changes,
            );
        }
    }

    // Deferred mutations so the panel closure only borrows `ui_state` (and
    // `wall_grid`) immutably while rendering.
    let mut next_tab: Option<LeftTab> = None;
    let mut next_places_view: Option<PlacesView> = None;
    let mut highlight: Vec<SlotCoord> = Vec::new();
    let tab = ui_state.left_tab;
    let places_view = ui_state.places_view;

    egui::SidePanel::left("controls")
        .min_width(160.0)
        .max_width(260.0)
        .default_width(190.0)
        .show(ctx, |ui| {
            ui.heading("Orchitecture");
            ui.horizontal_wrapped(|ui| {
                for (t, label) in [
                    (LeftTab::Elements, "Elements"),
                    (LeftTab::Furniture, "Furniture"),
                    (LeftTab::Places, "Places"),
                ] {
                    if ui.selectable_label(tab == t, label).clicked() {
                        next_tab = Some(t);
                    }
                }
            });
            ui.separator();

            match tab {
                LeftTab::Elements => {
                    structure_list_ui(ui, &world.constructed.eorfs, &mut build_state, false);

                    // Material picker for the selected structure's type.
                    let selected_info = &world.constructed.eorfs[build_state.selected_structure];
                    if let Some(stype) = selected_info.element_type() {
                        let options = material_list.for_type(stype);
                        if !options.is_empty() {
                            ui.separator();
                            ui.label("Material:");
                            let current = build_state.material_for_type(stype, &material_list);
                            let mut chosen = current;
                            for &(material_id, mat) in options.iter() {
                                ui.radio_value(&mut chosen, material_id, &mat.name);
                            }
                            if chosen != current {
                                build_state.material_per_type.insert(stype, chosen);
                            }
                        }
                    }

                    build_footer(
                        ui,
                        &mut commands,
                        &structure_list,
                        &mut world,
                        &mut viewable,
                        &overlay_assets,
                        &build_state,
                        &mut ui_state,
                        &population,
                    );
                }
                LeftTab::Furniture => {
                    structure_list_ui(ui, &world.constructed.eorfs, &mut build_state, true);

                    build_footer(
                        ui,
                        &mut commands,
                        &structure_list,
                        &mut world,
                        &mut viewable,
                        &overlay_assets,
                        &build_state,
                        &mut ui_state,
                        &population,
                    );
                }
                LeftTab::Places => match places_view {
                    PlacesView::List => {
                        ui.heading("Places");
                        ui.separator();
                        let mut switch_to_furniture = false;
                        for place_idx in 0..world.constructed.places.len() {
                            let name = world.constructed.places[place_idx].name.clone();
                            ui.collapsing(name, |ui| {
                                place_requirements_ui(
                                    ui,
                                    &world.constructed.places,
                                    &world.constructed.eorfs,
                                    place_idx,
                                    &mut Vec::new(),
                                    &mut build_state,
                                    &mut switch_to_furniture,
                                );
                            });
                        }
                        if switch_to_furniture {
                            next_tab = Some(LeftTab::Furniture);
                        }
                    }
                    PlacesView::Hierarchy { loc } => {
                    let cube = loc.cube;
                    ui.heading("Place");
                    ui.separator();
                    if ui.button("← Back").clicked() {
                        next_places_view = Some(PlacesView::List);
                    }
                    ui.separator();
                    // Places are formed automatically; show the ancestor chain of
                    // places containing the clicked furniture. `containing_chain`
                    // returns innermost first; we display outermost (highest in
                    // the hierarchy) first.
                    let chain = crate::place::containing_chain(&world.constructed, cube);
                    if chain.is_empty() {
                        ui.label("Not part of any place.");
                    } else {
                        let outdoorsness = crate::evaluation::compute_outdoorsness(
                            &world.constructed.contents,
                            &world.constructed.eorfs,
                        );
                        for &idx in chain.iter().rev() {
                            let (place_def_idx, place_name, fulfillments, totals) = {
                                let ps = &world.constructed.placed_places[idx];
                                let def = &world.constructed.places[ps.place];
                                (
                                    ps.place,
                                    def.name.clone(),
                                    ps.fulfillments.clone(),
                                    ps.contents.uniform_totals(),
                                )
                            };
                            let (quality, breakdown) = crate::evaluation::evaluate_place_breakdown(
                                &world.constructed,
                                idx,
                                &outdoorsness,
                            );
                            ui.label(egui::RichText::new(&place_name).heading());
                            ui.label(format!("Quality: {:.3}", quality));
                            ui.label("Quality breakdown:");
                            for factor in &breakdown {
                                match (factor.raw, factor.normalized) {
                                    (Some(raw), Some(normalized)) => {
                                        ui.label(format!(
                                            "  {}: raw {:.2}, normalized {:.2}, strength {:.2} → ×{:.3}",
                                            factor.aspect.label(),
                                            raw,
                                            normalized,
                                            factor.strength,
                                            factor.contribution
                                        ));
                                    }
                                    _ => {
                                        ui.label(format!(
                                            "  {}: n/a (no contribution)",
                                            factor.aspect.label()
                                        ));
                                    }
                                }
                            }

                            let eligible = crate::place::eligible_parent_kinds(
                                &world.constructed.places,
                                &crate::place::Porf::Place(place_name.clone()),
                            );
                            if !eligible.is_empty() {
                                ui.label("Nestable within:");
                                restriction_dropdown(
                                    ui,
                                    ("place-restriction", idx),
                                    &mut world.constructed.placed_places[idx].restriction,
                                    &eligible,
                                );
                            }

                            let mut counts: std::collections::BTreeMap<String, usize> =
                                std::collections::BTreeMap::new();
                            for f in &fulfillments {
                                if let crate::place::FulfilledPorf::Furniture(loc) = f {
                                    if let Some(cell) = world.constructed.contents.get(SlotCoord {
                                        cube: *loc,
                                        slot: Slot::Room,
                                    }) {
                                        *counts
                                            .entry(
                                                world.constructed.eorfs[cell.id.as_usize()]
                                                    .name
                                                    .clone(),
                                            )
                                            .or_default() += 1;
                                    }
                                }
                            }
                            ui.label("Structures:");
                            for (name, c) in &counts {
                                ui.label(format!("  {}: {}", name, c));
                            }

                            ui.label("Contents:");
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

                            if let Some(flavor) = world.constructed.places[place_def_idx].assignable_for
                            {
                                let occupant = population
                                    .individuals
                                    .iter()
                                    .position(|ind| ind.assigned(flavor) == Some(idx));
                                ui.label(format!(
                                    "{}: {}",
                                    flavor.label(),
                                    occupant
                                        .map(|i| format!("Individual {}", i + 1))
                                        .unwrap_or_else(|| "(unassigned)".to_string())
                                ));
                            }
                            ui.separator();
                        }
                        // Highlight the innermost place's furniture in 3D.
                        // Place fulfillments are always `Slot::Room` furniture.
                        let innermost = &world.constructed.placed_places[chain[0]];
                        highlight = innermost
                            .fulfillments
                            .iter()
                            .filter_map(|f| match f {
                                crate::place::FulfilledPorf::Furniture(cube) => Some(SlotCoord {
                                    cube: *cube,
                                    slot: Slot::Room,
                                }),
                                crate::place::FulfilledPorf::Place(_) => None,
                            })
                            .collect();
                    }

                    // The clicked furniture itself, at the bottom (below every
                    // containing place, from outermost down to innermost).
                    if let Some(cell) = world.constructed.contents.get(loc) {
                        // Always highlight the exact clicked cell at its own slot,
                        // independent of place membership -- e.g. `WallPlop`
                        // furniture never fulfills a place (fulfillments are
                        // Room-slot only), so it wouldn't otherwise get a ring.
                        if !highlight.contains(&loc) {
                            highlight.push(loc);
                        }
                        let eorf_idx = cell.id.as_usize();
                        let furniture_name = world.constructed.eorfs[eorf_idx].name.clone();
                        ui.label(egui::RichText::new(&furniture_name).heading());

                        let eligible = crate::place::eligible_parent_kinds(
                            &world.constructed.places,
                            &crate::place::Porf::Furniture(furniture_name),
                        );
                        if !eligible.is_empty() {
                            ui.label("Nestable within:");
                            let restriction = world
                                .constructed
                                .furniture_restrictions
                                .entry(cube)
                                .or_default();
                            restriction_dropdown(
                                ui,
                                ("furniture-restriction", eorf_idx),
                                restriction,
                                &eligible,
                            );
                        }
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
                    let avg_morale = population
                        .individuals
                        .iter()
                        .map(|i| i.morale())
                        .sum::<f32>()
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

    // Apply the deferred tab-navigation mutations now that the panel closure has ended.
    if let Some(t) = next_tab {
        ui_state.left_tab = t;
    }
    if let Some(v) = next_places_view {
        ui_state.places_view = v;
    }
    // Write only on change (set_if_neq), so the highlight system doesn't respawn
    // the overlay meshes every frame.
    place_highlight.set_if_neq(crate::city::PlaceHighlight(highlight));
}

/// Selectable list of structures, filtered to Elements (`want_furniture ==
/// false`) or Furniture (`want_furniture == true`), grouped by `ElementType`
/// with a group header. Numeric prefixes are assigned by position *within*
/// this filtered list, matching the per-tab digit-key shortcuts in
/// `input.rs` (each tab gets its own 1-9).
fn structure_list_ui(
    ui: &mut egui::Ui,
    eorfs: &[crate::eorf::EorfInfo],
    build_state: &mut BuildState,
    want_furniture: bool,
) {
    let sorted = sorted_structure_indices(eorfs);
    let mut last_group: Option<Option<crate::materials::ElementType>> = None;
    let mut display_idx = 0;
    for &struct_idx in sorted.iter() {
        let info = &eorfs[struct_idx];
        if info.is_furniture() != want_furniture {
            continue;
        }
        let group = info.element_type();
        if last_group != Some(group) {
            if last_group.is_some() {
                ui.separator();
            }
            let label = group.map(|g| g.label()).unwrap_or("Furniture");
            ui.label(
                egui::RichText::new(label)
                    .small()
                    .color(egui::Color32::from_gray(140)),
            );
            last_group = Some(group);
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
        display_idx += 1;
    }
}

/// Shared footer for the Elements/Furniture tabs: layer/direction readout,
/// evaluation, morale, and the pending-changes reset button.
#[allow(clippy::too_many_arguments)]
fn build_footer(
    ui: &mut egui::Ui,
    commands: &mut Commands,
    structure_list: &EorfList,
    world: &mut CityMut,
    viewable: &mut ViewableWorld,
    overlay_assets: &ProposalOverlayAssets,
    build_state: &BuildState,
    ui_state: &mut UiState,
    population: &Population,
) {
    ui.separator();
    ui.label(format!("Layer (Y): {}", build_state.cur_y));
    let selected = &world.constructed.eorfs[build_state.selected_structure];
    if selected.placement_style == crate::eorf::PlacementStyle::WallPlop {
        ui.label(format!(
            "Rotation (X-wall): {}",
            if build_state.wall_plop_flip_x {
                "flipped"
            } else {
                "normal"
            }
        ));
        ui.label(format!(
            "Rotation (Z-wall): {}",
            if build_state.wall_plop_flip_z {
                "flipped"
            } else {
                "normal"
            }
        ));
    } else {
        ui.label(format!("Direction: {}", build_state.cur_dir));
    }

    if let Some((coherence, interest)) = build_state.evaluation {
        ui.separator();
        ui.label(format!("Coherence: {:.3}", coherence));
        ui.label(format!("Interest:  {:.3}", interest));
    }

    if !population.individuals.is_empty() {
        let avg_morale = population
            .individuals
            .iter()
            .map(|i| i.morale())
            .sum::<f32>()
            / population.individuals.len() as f32;
        ui.separator();
        if ui.button(format!("Morale: {:.2}", avg_morale)).clicked() {
            ui_state.show_population = !ui_state.show_population;
        }
    }

    if world.pending.num_changes() > 0 {
        ui.separator();
        if ui.button("Reset").clicked() {
            let locs: Vec<_> = world
                .pending
                .proposed_changes
                .iter()
                .map(|(l, _)| l)
                .collect();
            world.pending.reset();
            let deltas: Vec<_> = locs
                .into_iter()
                .map(|loc| (loc, crate::city::ProposalView::None))
                .collect();
            apply_proposal_changes(
                commands,
                &mut world.assembled,
                structure_list,
                overlay_assets,
                deltas,
            );
            clear_proposed_cut_entities(commands, viewable);
        }
    }
}

/// Renders a `Place` kind's requirements, recursively (`Place` requirements
/// nest as a collapsible section for that kind's own requirements). `visited`
/// guards against a requirement cycle looping forever. `Furniture`
/// requirements render as a selectable button, like the Furniture tab's list
/// -- clicking one selects it in `build_state` and asks the caller (via
/// `switch_to_furniture`) to switch to the Furniture tab.
fn place_requirements_ui(
    ui: &mut egui::Ui,
    places: &[crate::place::Place],
    eorfs: &[crate::eorf::EorfInfo],
    place_idx: usize,
    visited: &mut Vec<usize>,
    build_state: &mut BuildState,
    switch_to_furniture: &mut bool,
) {
    if visited.contains(&place_idx) {
        ui.label("(see above)");
        return;
    }
    visited.push(place_idx);
    for req in &places[place_idx].requirements {
        let count = match req.max {
            Some(max) if max as u8 == req.min => format!("{}", req.min),
            Some(max) => format!("{}-{}", req.min, max),
            None => format!("{}+", req.min),
        };
        match &req.requirement {
            crate::place::Porf::Furniture(name) => {
                ui.horizontal(|ui| {
                    ui.label(format!("{count}×"));
                    if let Some(struct_idx) = eorfs.iter().position(|e| &e.name == name) {
                        let selected = build_state.selected_structure == struct_idx;
                        if ui.selectable_label(selected, name).clicked() {
                            build_state.selected_structure = struct_idx;
                            *switch_to_furniture = true;
                        }
                    } else {
                        ui.label(name);
                    }
                });
            }
            crate::place::Porf::Place(name) => {
                if let Some(nested_idx) = places.iter().position(|p| &p.name == name) {
                    ui.collapsing(format!("{count}× {name}"), |ui| {
                        place_requirements_ui(
                            ui,
                            places,
                            eorfs,
                            nested_idx,
                            visited,
                            build_state,
                            switch_to_furniture,
                        );
                    });
                } else {
                    ui.label(format!("{count}× {name}"));
                }
            }
        }
    }
    visited.pop();
}

/// Dropdown for a Furniture/Place kind's `ParentRestriction`: "Unrestricted",
/// "Do not include", or one of the `eligible` `Place` kinds. Not shown by the
/// caller when `eligible` is empty (nothing could ever include this kind).
fn restriction_dropdown(
    ui: &mut egui::Ui,
    id_source: impl std::hash::Hash,
    restriction: &mut crate::place::ParentRestriction,
    eligible: &[String],
) {
    use crate::place::ParentRestriction;

    let current_label = match restriction {
        ParentRestriction::Unrestricted => "Unrestricted".to_string(),
        ParentRestriction::Excluded => "Do not include".to_string(),
        ParentRestriction::RestrictedTo(name) => name.clone(),
    };
    egui::ComboBox::from_id_salt(id_source)
        .selected_text(current_label)
        .show_ui(ui, |ui| {
            ui.selectable_value(restriction, ParentRestriction::Unrestricted, "Unrestricted");
            ui.selectable_value(restriction, ParentRestriction::Excluded, "Do not include");
            for name in eligible {
                ui.selectable_value(
                    restriction,
                    ParentRestriction::RestrictedTo(name.clone()),
                    name,
                );
            }
        });
}

fn need_bar(ui: &mut egui::Ui, label: &str, value: f32) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::ProgressBar::new(value / 1.2));
    });
}

/// Totals of all resources across every storage place, sorted for display.
/// Returns `(resource, total_quantity, precision)`.
pub(crate) fn place_resource_totals(
    constructed: &ConstructedCity,
) -> Vec<(
    crate::resource::UniformResource,
    u32,
    crate::resource::Precision,
)> {
    use crate::resource::{round, Precision, UniformResource};
    use std::collections::HashMap;

    let mut map: HashMap<UniformResource, (u32, Precision)> = HashMap::new();
    for (_, place) in constructed.placed_places.iter() {
        let Some(info) = constructed.places.get(place.place) else {
            continue;
        };
        let Some(spec) = &info.storage else {
            continue;
        };
        for (res, qty) in place.contents.uniform_totals() {
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

/// Total resource cost to complete the current proposed construction.
/// Only counts `Proposal::Place` entries; removals are free. Furniture has a
/// fixed cost independent of the selected build material.
/// Returns sorted `(resource, quantity)` pairs; empty when cost is zero.
pub(crate) fn construction_cost(
    proposed: &crate::sparse3d::Sparse3D<crate::city::Proposal>,
    structure_infos: &[crate::eorf::EorfInfo],
    material_list: &crate::materials::MaterialList,
) -> Vec<(crate::resource::UniformResource, u32)> {
    use crate::city::Proposal;
    use crate::resource::UniformResource;
    use std::collections::HashMap;

    let mut totals: HashMap<UniformResource, u32> = HashMap::new();

    for (_, proposal) in proposed.iter() {
        let Proposal::Place(cell) = proposal else {
            continue;
        };
        let info = &structure_infos[cell.id.as_usize()];
        let cost = if let Some(furniture_cost) = info.furniture_cost() {
            furniture_cost.clone()
        } else {
            let Some(build_mat) = material_list.materials.get(cell.build_material.0 as usize)
            else {
                continue;
            };
            let Some(element_type) = info.element_type() else {
                continue;
            };
            let Some(cost) = build_mat.costs.get(&element_type) else {
                continue;
            };
            cost.clone()
        };
        for (res, qty) in cost {
            *totals.entry(res).or_insert(0) += qty as u32;
        }
    }

    let mut result: Vec<_> = totals.into_iter().collect();
    result.sort_by_key(|(r, _)| *r);
    result
}

/// Resource cost still owed to complete the current proposed construction:
/// `construction_cost(...)` (unchanged, total cost) minus
/// `pending.resource_progress` already applied, floored at zero. Drops zero
/// entries. Empty when there's nothing pending.
pub(crate) fn remaining_construction_need(
    pending: &crate::city::ProposedCity,
    structure_infos: &[crate::eorf::EorfInfo],
    material_list: &crate::materials::MaterialList,
) -> Vec<(crate::resource::UniformResource, u32)> {
    construction_cost(&pending.proposed_changes, structure_infos, material_list)
        .into_iter()
        .filter_map(|(res, total)| {
            let progress = pending.resource_progress.get(&res).copied().unwrap_or(0);
            let remaining = total.saturating_sub(progress.min(total));
            (remaining > 0).then_some((res, remaining))
        })
        .collect()
}

/// Fraction (0.0–1.0) of the current pending construction's total material
/// cost that's already been paid off, weighted by each material's time cost
/// (`1 / UniformResource::construct_per_month()`) so materials that are
/// slower to deliver count for more of the bar. `None` when there's no
/// pending construction (or its cost is zero).
pub(crate) fn construction_progress_fraction(
    pending: &crate::city::ProposedCity,
    structure_infos: &[crate::eorf::EorfInfo],
    material_list: &crate::materials::MaterialList,
) -> Option<f32> {
    let total_cost = construction_cost(&pending.proposed_changes, structure_infos, material_list);
    if total_cost.is_empty() {
        return None;
    }

    let time_cost = |res: crate::resource::UniformResource, qty: u32| -> f32 {
        qty as f32 / res.construct_per_month()
    };

    let total_time: f32 = total_cost
        .iter()
        .map(|(res, qty)| time_cost(*res, *qty))
        .sum();
    if total_time <= 0.0 {
        return Some(1.0);
    }
    let paid_time: f32 = total_cost
        .iter()
        .map(|(res, qty)| {
            let progress = pending
                .resource_progress
                .get(res)
                .copied()
                .unwrap_or(0)
                .min(*qty);
            time_cost(*res, progress)
        })
        .sum();
    Some((paid_time / total_time).clamp(0.0, 1.0))
}
