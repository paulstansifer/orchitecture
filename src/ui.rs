use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::build_ui::SandboxMode;
use crate::construction::advance_construction;
use crate::game_mode::GameMode;
use crate::materials::MaterialList;
use crate::population::Population;
use crate::resource::UniformResource;
use crate::resource_icons::{ResourceIcons, LARGE_SIZE};
use crate::structure::StructureList;
use crate::surroundings::farmstead::{
    apply_production, compute_production, preview_market, run_market, update_wanted_resources,
    FarmsResource, GameClock,
};
use crate::surroundings::map::CIRCLE_REVEAL_RADIUS;
use crate::traveler::{self, TravelerState};
use crate::city::{CityMut, ViewableWorld};
use crate::{col_format, heading_label, label, note_label};

pub fn shared_ui_system(
    mut contexts: EguiContexts,
    mut clock: ResMut<GameClock>,
    mut farms: ResMut<FarmsResource>,
    world: CityMut,
    resource_icons: Res<ResourceIcons>,
    mut next_game_mode: ResMut<NextState<GameMode>>,
    current_mode: Res<State<GameMode>>,
    mut commands: Commands,
    mut viewable: ResMut<ViewableWorld>,
    structure_list: Res<StructureList>,
    mut traveler_state: ResMut<TravelerState>,
    mut population: ResMut<Population>,
    sandbox: Res<SandboxMode>,
    material_list: Res<MaterialList>,
) {
    use egui::Color32;
    let CityMut {
        mut constructed,
        mut pending,
        mut assembled,
    } = world;

    let icon_textures_lg = resource_icons.texture_ids_large(&mut contexts);
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // Market preview for resource gain display.
    let preview = preview_market(&farms);
    let gains_map: std::collections::HashMap<UniformResource, u32> =
        preview.player_gains.iter().copied().collect();

    let station_totals = crate::build_ui::station_resource_totals(&constructed);

    // Construction cost (non-sandbox only).
    let construction_cost = if pending.num_changes() > 0 && !sandbox.enabled {
        crate::build_ui::construction_cost(
            &pending.proposed_changes,
            &structure_list,
            &material_list,
        )
    } else {
        vec![]
    };
    let cost_map: std::collections::HashMap<UniformResource, u32> =
        construction_cost.iter().copied().collect();
    let can_afford_construction = construction_cost.iter().all(|(res, qty)| {
        station_totals
            .iter()
            .find(|(r, _, _)| r == res)
            .map(|(_, q, _)| *q >= *qty)
            .unwrap_or(*qty == 0)
    });

    let mut rhs_resources: Vec<UniformResource> =
        station_totals.iter().map(|(r, _, _)| *r).collect();
    for (r, _) in &preview.player_gains {
        if !rhs_resources.contains(r) {
            rhs_resources.push(*r);
        }
    }
    for (r, _) in &construction_cost {
        if !rhs_resources.contains(r) {
            rhs_resources.push(*r);
        }
    }
    rhs_resources.sort();

    // Construction status.
    let has_project = pending.num_changes() > 0;
    let m = pending.months_waited as usize;
    let months_remaining = if has_project {
        let raw_n = pending.months_for_construction();
        (raw_n as isize - m as isize).max(1) as usize
    } else {
        0
    };
    let n_display = m + months_remaining;

    // Farm/market status.
    let market_stand_count = constructed
        .stations
        .iter()
        .position(|s| s.name == "market stand")
        .map_or(0, |idx| {
            constructed
                .placed_stations
                .iter()
                .filter(|ps| ps.station == idx)
                .count()
        });
    let invited_count = farms.farms.iter().filter(|f| f.invited).count();
    let has_farms_invited = invited_count > 0;
    let has_traveler_invited = traveler_state.invited;

    let can_afford_traveler = traveler_state.current_offer.as_ref().is_some_and(|offer| {
        traveler::can_afford_traveler(offer, &station_totals, &preview.player_gains)
    });

    let wait_id = egui::Id::new("wait_confirmation");

    let mut go_advance_month = false;
    let mut go_walk = false;
    let mut go_build = false;
    let mut go_surroundings = false;

    egui::SidePanel::right("resources")
        .min_width(130.0)
        .show(ctx, |ui| {
            let month = clock.month() + 1;
            heading_label!(ui, format!("Month {}", month));
            ui.add_space(2.0);

            if has_project || has_farms_invited || has_traveler_invited {
                ui.add_enabled_ui(can_afford_construction, |ui| {
                    if ui.button("Advance Month").clicked() {
                        go_advance_month = true;
                    }
                });
                if has_project && !can_afford_construction {
                    note_label!(ui, col_format!(problem, "Insufficient resources for construction"));
                }
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_min_width(0.0);
                        if has_project {
                            label!(
                                ui,
                                format!("Construction: {} of {} months", m, n_display)
                            );
                        } else {
                            label!(ui, "No current project");
                        }
                    });
                    ui.vertical(|ui| {
                        ui.set_min_width(0.0);
                        if market_stand_count == 0 {
                            label!(ui, "No market stalls");
                        } else {
                            label!(ui, format!("{}/{} farms invited", invited_count, market_stand_count));
                        }
                    });
                });
            } else {
                let confirmed: bool = ctx.data(|d| d.get_temp(wait_id).unwrap_or(false));
                if ui.button("Wait?").clicked() {
                    ctx.data_mut(|d| d.insert_temp(wait_id, true));
                }
                if confirmed {
                    label!(ui, col_format!(problem, "There's no ongoing construction, and no farms are invited to the next market. Wait anyways?"));
                    if ui.button("Advance Month").clicked() {
                        go_advance_month = true;
                    }
                }
            }

            ui.separator();
            ui.heading("Resources");
            if rhs_resources.is_empty() {
                ui.label("(none)");
            }
            for res in &rhs_resources {
                use crate::resource::Precision;

                let (current, precision) = station_totals
                    .iter()
                    .find(|(r, _, _)| r == res)
                    .map(|(_, q, p)| (*q, *p))
                    .unwrap_or((0, Precision::Exact));
                let gain = *gains_map.get(res).unwrap_or(&0);

                let quantity_str = match precision {
                    Precision::Exact => format!("{}", current),
                    Precision::Approximate => format!("~{}", current),
                    Precision::Conservative => format!(">{}", current),
                };

                let cost = *cost_map.get(res).unwrap_or(&0);
                let text = match (gain > 0, cost > 0) {
                    (true, true) => format!("{}: {} +{} -{}", res.label(), quantity_str, gain, cost),
                    (true, false) => format!("{}: {} +{}", res.label(), quantity_str, gain),
                    (false, true) => format!("{}: {} -{}", res.label(), quantity_str, cost),
                    (false, false) => format!("{}: {}", res.label(), quantity_str),
                };
                ui.horizontal(|ui| {
                    if let Some(&tex) = icon_textures_lg.get(res) {
                        ui.add(egui::Image::new(egui::load::SizedTexture::new(
                            tex, LARGE_SIZE,
                        )));
                    }
                    label!(ui, format!("{}", text));
                });
            }

            // Tools count (UniqueResource — tracked separately from uniform resources).
            let total_tools: usize = constructed
                .placed_stations
                .iter()
                .filter(|ps| {
                    constructed
                        .stations
                        .get(ps.station)
                        .is_some_and(|info| info.storage.is_some())
                })
                .map(|ps| ps.contents.tool_count())
                .sum();
            if total_tools > 0 {
                ui.horizontal(|ui| {
                    label!(ui, format!("Tools: {}", total_tools));
                });
            }

            ui.separator();
            heading_label!(ui, "Travelers");
            // Clone demands before the closure to avoid borrow conflict with &mut traveler_state.invited.
            let offer_demands: Option<Vec<(crate::resource::UniformResource, u16)>> =
                traveler_state
                    .current_offer
                    .as_ref()
                    .map(|o| o.demands.clone());

            if let Some(demands) = offer_demands {
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(20, 20, 30, 200))
                    .inner_margin(egui::Margin::same(4))
                    .corner_radius(4.0)
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width());
                        label!(ui, "Traveler");
                        for (res, qty) in &demands {
                            label!(ui, format!("Wants {} {}", qty, res.label()));
                        }
                        label!(ui, "Brings: 1 Tool + reveals a path");
                        ui.add_enabled(
                            can_afford_traveler || traveler_state.invited,
                            egui::Checkbox::new(&mut traveler_state.invited, "Invite"),
                        );
                        if !can_afford_traveler {
                            note_label!(ui, "(insufficient resources)");
                        }
                    });
            } else {
                label!(ui, "(no traveler this month)");
            }

            // Mode buttons pushed to the bottom of the panel.
            ui.with_layout(
                egui::Layout::bottom_up(egui::Align::LEFT),
                |ui| match *current_mode.get() {
                    GameMode::Build => {
                        if ui.button("Surroundings").clicked() {
                            go_surroundings = true;
                        }
                        if ui.button("Walk Around").clicked() {
                            go_walk = true;
                        }
                    }
                    GameMode::Walk => {
                        if ui.button("Surroundings").clicked() {
                            go_surroundings = true;
                        }
                        if ui.button("Build").clicked() {
                            go_build = true;
                        }
                    }
                    GameMode::Surroundings => {
                        if ui.button("Build").clicked() {
                            go_build = true;
                        }
                        if ui.button("Walk Around").clicked() {
                            go_walk = true;
                        }
                    }
                },
            );
        });

    // ── Apply deferred actions ────────────────────────────────────────────────
    if go_advance_month {
        clock.advance_month();
        farms.ensure_adjacency();
        let mut rng = rand::rng();
        let (gains, tools_to_return) = run_market(&mut *farms, &mut rng);
        for tool in tools_to_return {
            crate::station::deposit_tool(&mut *constructed, tool);
        }
        let plan = compute_production(&*farms, &mut rng);
        apply_production(&mut *farms, &plan);
        update_wanted_resources(&mut *farms);
        for farm in &mut farms.farms {
            farm.invited = false;
        }

        // Traveler resolution: deduct demands, deposit a Tool, reveal their path.
        if traveler_state.invited {
            if let Some(offer) = traveler_state.current_offer.take() {
                if traveler::can_afford_traveler(&offer, &station_totals, &preview.player_gains) {
                    let new_path = traveler::accept_traveler(&offer, &mut constructed);
                    farms.traveler_reveals.push(new_path);
                }
            }
        }
        traveler_state.invited = false;
        // Roll a new offer for next month.
        traveler::roll_traveler_offer(&mut traveler_state, CIRCLE_REVEAL_RADIUS, &mut rng);

        if !gains.is_empty() {
            let storage_idx = constructed.placed_stations.iter().position(|ps| {
                constructed
                    .stations
                    .get(ps.station)
                    .is_some_and(|info| info.storage.is_some())
            });
            if let Some(idx) = storage_idx {
                for (res, qty) in gains {
                    constructed.placed_stations[idx]
                        .contents
                        .add_uniform(res, qty as u16);
                }
            }
        }
        for individual in &mut population.individuals {
            individual.fed_this_month = false;
        }
        for individual in &mut population.individuals {
            if crate::station::consume_uniform(
                &mut constructed,
                crate::resource::UniformResource::Potato,
                5,
            ) {
                individual.fed_this_month = true;
            }
        }

        let construction_completed = advance_construction(
            &mut pending,
            &mut constructed,
            &mut commands,
            &mut assembled,
            &mut viewable,
            &structure_list,
        );
        if construction_completed && !sandbox.enabled {
            for (res, qty) in &construction_cost {
                crate::station::consume_uniform(&mut constructed, *res, *qty);
            }
        }
        ctx.data_mut(|d| d.remove::<bool>(wait_id));
    }
    if go_walk {
        next_game_mode.set(GameMode::Walk);
    }
    if go_build {
        next_game_mode.set(GameMode::Build);
    }
    if go_surroundings {
        next_game_mode.set(GameMode::Surroundings);
    }
}
