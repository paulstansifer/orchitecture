use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiGlobalSettings};

use crate::city::{
    apply_proposal_changes, clear_proposed_cut_entities, CityMut, ConstructedCity,
    ProposalOverlayAssets, ViewableWorld,
};
use crate::construction::commit_pending_construction;
use crate::cutaway::CutawayMode;
use crate::eorf::sorted_structure_indices;
use crate::eorf::EorfList;
use crate::game_mode::SandboxMode;
use crate::input::BuildState;
use crate::map_files::{load_map, map_file_controls_ui, MapLoadRequest};
use crate::materials::MaterialList;
use crate::population::Population;
use crate::resource_icons::{ResourceIcons, LARGE_SIZE};
use crate::sparse3d::{Slot, SlotCoord};

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

pub fn enable_ui_input_absorption(mut egui_settings: ResMut<EguiGlobalSettings>) {
    egui_settings.enable_absorb_bevy_input_system = true;
}

/// The bottom control strip: keybinding hint, sandbox toggle, save/load
/// controls, cutaway-mode picker. Returns any map load the player requested
/// via [`map_file_controls_ui`], to be resolved and applied after the egui
/// closure ends.
#[allow(clippy::too_many_arguments)]
fn bottom_controls_ui(
    ctx: &egui::Context,
    commands: &mut Commands,
    world: &mut CityMut,
    viewable: &mut ViewableWorld,
    structure_list: &EorfList,
    sandbox: &mut SandboxMode,
    cutaway_mode: &mut CutawayMode,
    ui_state: &mut UiState,
) -> Option<MapLoadRequest> {
    let mut load_request = None;
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
                commit_pending_construction(
                    commands,
                    &mut world.constructed,
                    &mut world.pending,
                    &mut world.assembled,
                    viewable,
                    structure_list,
                );
            }

            load_request =
                map_file_controls_ui(ui, commands, &world.constructed, ui_state, sandbox.enabled);

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
        });
    });
    load_request
}

/// The `PlacesView::Hierarchy` panel: the ancestor chain of places containing
/// the right-clicked furniture (outermost first), each with its quality
/// breakdown, nesting restriction, contents, and (if assignable) occupant,
/// followed by the clicked furniture itself. Returns the set of cells to
/// highlight in 3D (innermost place's furniture, plus the clicked cell).
fn place_hierarchy_ui(
    ui: &mut egui::Ui,
    constructed: &mut ConstructedCity,
    population: &Population,
    icon_textures: &std::collections::HashMap<crate::resource::UniformResource, egui::TextureId>,
    loc: SlotCoord,
    next_places_view: &mut Option<PlacesView>,
) -> Vec<SlotCoord> {
    let cube = loc.cube;
    let mut highlight = Vec::new();

    ui.heading("Place");
    ui.separator();
    if ui.button("← Back").clicked() {
        *next_places_view = Some(PlacesView::List);
    }
    ui.separator();
    // Places are formed automatically; show the ancestor chain of places
    // containing the clicked furniture. `containing_chain` returns innermost
    // first; we display outermost (highest in the hierarchy) first.
    let chain = crate::place::containing_chain(constructed, cube);
    if chain.is_empty() {
        ui.label("Not part of any place.");
    } else {
        let outdoorsness =
            crate::evaluation::compute_outdoorsness(&constructed.contents, &constructed.eorfs);
        for &idx in chain.iter().rev() {
            let (place_def_idx, place_name, fulfillments, totals) = {
                let ps = &constructed.placed_places[idx];
                let def = &constructed.places[ps.place];
                (
                    ps.place,
                    def.name.clone(),
                    ps.fulfillments.clone(),
                    ps.contents.uniform_totals(),
                )
            };
            let (quality, breakdown) =
                crate::evaluation::evaluate_place_breakdown(constructed, idx, &outdoorsness);
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
                &constructed.places,
                &crate::place::Porf::Place(place_name.clone()),
            );
            if !eligible.is_empty() {
                ui.label("Nestable within:");
                restriction_dropdown(
                    ui,
                    ("place-restriction", idx),
                    &mut constructed.placed_places[idx].restriction,
                    &eligible,
                );
            }

            let mut counts: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            for f in &fulfillments {
                if let crate::place::FulfilledPorf::Furniture(loc) = f {
                    if let Some(cell) = constructed.contents.get(SlotCoord {
                        cube: *loc,
                        slot: Slot::Room,
                    }) {
                        *counts
                            .entry(constructed.eorfs[cell.id.as_usize()].name.clone())
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

            if let Some(flavor) = constructed.places[place_def_idx].assignable_for {
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
        // Highlight the innermost place's furniture in 3D. Place fulfillments
        // are always `Slot::Room` furniture.
        let innermost = &constructed.placed_places[chain[0]];
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

    // The clicked furniture itself, at the bottom (below every containing
    // place, from outermost down to innermost).
    if let Some(cell) = constructed.contents.get(loc) {
        // Always highlight the exact clicked cell at its own slot,
        // independent of place membership -- e.g. `WallPlop` furniture never
        // fulfills a place (fulfillments are Room-slot only), so it wouldn't
        // otherwise get a ring.
        if !highlight.contains(&loc) {
            highlight.push(loc);
        }
        let eorf_idx = cell.id.as_usize();
        let furniture_name = constructed.eorfs[eorf_idx].name.clone();
        ui.label(egui::RichText::new(&furniture_name).heading());

        let eligible = crate::place::eligible_parent_kinds(
            &constructed.places,
            &crate::place::Porf::Furniture(furniture_name),
        );
        if !eligible.is_empty() {
            ui.label("Nestable within:");
            let restriction = constructed.furniture_restrictions.entry(cube).or_default();
            restriction_dropdown(
                ui,
                ("furniture-restriction", eorf_idx),
                restriction,
                &eligible,
            );
        }

        if crate::place::cube_is_storage_bin(constructed, cube) {
            ui.label("Restricted to:");
            bin_restriction_dropdown(
                ui,
                ("bin-restriction", eorf_idx),
                cube,
                &mut constructed.bin_resource_restrictions,
            );
        }
    }

    highlight
}

/// The "Population" window: average morale and each individual's need bars.
fn population_window(ctx: &egui::Context, ui_state: &mut UiState, population: &Population) {
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

    // Bottom panel must be added before side panels.
    let load_request = bottom_controls_ui(
        ctx,
        &mut commands,
        &mut world,
        &mut viewable,
        &structure_list,
        &mut sandbox,
        &mut cutaway_mode,
        &mut ui_state,
    );

    // A requested load happens immediately, after the egui closure has ended.
    if let Some(new_contents) = load_request.and_then(|req| req.resolve(&world.constructed.eorfs)) {
        load_map(
            &mut commands,
            &mut world.constructed,
            &mut world.pending,
            &mut world.assembled,
            &mut viewable,
            &structure_list,
            new_contents,
        );
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
                LeftTab::Elements | LeftTab::Furniture => {
                    structure_list_ui(
                        ui,
                        &world.constructed.eorfs,
                        &mut build_state,
                        tab == LeftTab::Furniture,
                    );

                    // Material picker for the selected structure's type
                    // (elements only; furniture has a fixed cost).
                    if tab == LeftTab::Elements {
                        let selected_info =
                            &world.constructed.eorfs[build_state.selected_structure];
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
                        highlight = place_hierarchy_ui(
                            ui,
                            &mut world.constructed,
                            &population,
                            &icon_textures,
                            loc,
                            &mut next_places_view,
                        );
                    }
                },
            }
        });

    if ui_state.show_population {
        population_window(ctx, &mut ui_state, &population);
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

/// Dropdown for a bin's `UniformResource` restriction: "Any resource", or one
/// specific resource. Absence from `restrictions` means unrestricted.
fn bin_restriction_dropdown(
    ui: &mut egui::Ui,
    id_source: impl std::hash::Hash,
    cube: bevy::math::IVec3,
    restrictions: &mut std::collections::HashMap<
        bevy::math::IVec3,
        crate::resource::UniformResource,
    >,
) {
    use crate::resource::UniformResource;

    let mut current = restrictions.get(&cube).copied();
    let current_label = current.map(|r| r.label()).unwrap_or("Any resource");
    egui::ComboBox::from_id_salt(id_source)
        .selected_text(current_label)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut current, None, "Any resource");
            for &res in UniformResource::ALL {
                ui.selectable_value(&mut current, Some(res), res.label());
            }
        });
    match current {
        Some(res) => {
            restrictions.insert(cube, res);
        }
        None => {
            restrictions.remove(&cube);
        }
    }
}

fn need_bar(ui: &mut egui::Ui, label: &str, value: f32) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::ProgressBar::new(value / 1.2));
    });
}
