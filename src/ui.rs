use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::city::{CityMut, ViewableWorld};
use crate::city_effect::{compute_month_effects, LedgerSource};
use crate::eorf::EorfList;
use crate::game_mode::{GameMode, SandboxMode};
use crate::materials::MaterialList;
use crate::population::Population;
use crate::resource::UniformResource;
use crate::resource_icons::{ResourceIcons, LARGE_SIZE};
use crate::surroundings::farmstead::{FarmsResource, GameClock};
use crate::traveler::{ResolvedReward, TravelerState};
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
    structure_list: Res<EorfList>,
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

    // This month's effects (market participation, feeding, a traveler's
    // visit, construction absorption) — computed once and shared by the
    // resource preview, the per-resource tooltip, and the traveler
    // checkbox's affordability. Same computation `advance_month` uses to
    // actually apply things, just not mutated here.
    let effects = compute_month_effects(
        &farms,
        &constructed,
        &pending,
        &population,
        &traveler_state,
        &material_list,
        sandbox.enabled,
    );
    let station_totals = crate::place::place_resource_totals(&constructed);

    // Remaining construction need (non-sandbox only).
    let remaining_need: Vec<(UniformResource, u32)> =
        if pending.num_changes() > 0 && !sandbox.enabled {
            crate::construction::remaining_construction_need(
                &pending,
                &constructed.eorfs,
                &material_list,
            )
        } else {
            vec![]
        };
    let remaining_need_map: std::collections::HashMap<UniformResource, u32> =
        remaining_need.iter().copied().collect();

    let has_storage = !crate::place::storage_ids(&constructed).is_empty();

    // Hard block: too much unpaid construction cost.
    let blocked_construction = crate::construction::construction_blocked(&remaining_need);

    let mut rhs_resources: Vec<UniformResource> =
        station_totals.iter().map(|(r, _, _)| *r).collect();
    for (r, _) in &effects.player_gains {
        if !rhs_resources.contains(r) {
            rhs_resources.push(*r);
        }
    }
    for (r, _) in &remaining_need {
        if !rhs_resources.contains(r) {
            rhs_resources.push(*r);
        }
    }
    rhs_resources.sort();

    // Construction status.
    let has_project = pending.num_changes() > 0;
    let construction_progress = crate::construction::construction_progress_fraction(
        &pending,
        &constructed.eorfs,
        &material_list,
    );

    // Farm/market status.
    let market_stand_count = crate::place::market_stand_count(&constructed);
    let invited_count = farms.invited_count();
    let has_farms_invited = invited_count > 0;
    let has_traveler_invited = traveler_state.invited;

    let can_afford_traveler = effects.traveler_affordable();

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
                ui.add_enabled_ui(!blocked_construction, |ui| {
                    if ui.button("Advance Month").clicked() {
                        go_advance_month = true;
                    }
                });
                if has_project && blocked_construction {
                    note_label!(
                        ui,
                        col_format!(
                            problem,
                            "Too much unpaid construction cost — cancel some proposed construction."
                        )
                    );
                }
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_min_width(0.0);
                        ui.set_max_width(400.0);
                        if has_project {
                            ui.add(
                                egui::ProgressBar::new(construction_progress.unwrap_or(1.0))
                                .desired_width(100.0)
                                .show_percentage(),
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
            egui::Grid::new("resource_grid")
                .num_columns(3)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    for res in &rhs_resources {
                        use crate::resource::Precision;

                        let (current, precision) = station_totals
                            .iter()
                            .find(|(r, _, _)| r == res)
                            .map(|(_, q, p)| (*q, *p))
                            .unwrap_or((0, Precision::Exact));
                        let need = *remaining_need_map.get(res).unwrap_or(&0);
                        let lost = effects.leftover.get(res).map(|f| f.lost).unwrap_or(0);
                        let storage_delta = effects.storage_delta(*res);
                        // Construction consumes (negative net); flip its sign
                        // for the "applied this month" figure.
                        let applied =
                            (-effects.ledger.net_for(LedgerSource::Construction, *res)).max(0) as u32;

                        // Icon cell.
                        if let Some(&tex) = icon_textures_lg.get(res) {
                            ui.add(egui::Image::new(egui::load::SizedTexture::new(
                                tex, LARGE_SIZE,
                            )));
                        } else {
                            ui.label("");
                        }

                        // Storage cell: current amount, plus the net change
                        // to storage this month, plus anything lost to a
                        // full store. Tooltip lives here.
                        let quantity_str = match precision {
                            Precision::Exact => format!("{}", current),
                            Precision::Approximate => format!("~{}", current),
                            Precision::Conservative => format!(">{}", current),
                        };
                        let storage_resp = if !has_storage {
                            if lost > 0 {
                                label!(ui, format!("({} lost)", lost))
                            } else {
                                ui.label("")
                            }
                        } else if storage_delta > 0 {
                            if lost > 0 {
                                label!(
                                    ui,
                                    quantity_str,
                                    col_format!(preview, " +{}", storage_delta),
                                    format!(" ({} lost)", lost)
                                )
                            } else {
                                label!(ui, quantity_str, col_format!(preview, " +{}", storage_delta))
                            }
                        } else if storage_delta < 0 {
                            label!(ui, quantity_str, col_format!(problem, " –{}", -storage_delta))
                        } else if lost > 0 {
                            label!(ui, quantity_str, format!(" ({} lost)", lost))
                        } else {
                            label!(ui, quantity_str)
                        };

                        // Tooltip: one colored line per contributing source,
                        // e.g. "+10 (market)" or "-7 (traveler)".
                        storage_resp.on_hover_ui(|ui| {
                            let mut any = false;
                            for (source, delta) in effects.ledger.sources_touching(*res) {
                                any = true;
                                if delta > 0 {
                                    label!(
                                        ui,
                                        col_format!(preview, "+{} ({})", delta, source.tag())
                                    );
                                } else {
                                    label!(
                                        ui,
                                        col_format!(problem, "{} ({})", delta, source.tag())
                                    );
                                }
                            }
                            if lost > 0 {
                                label!(ui, format!("({} lost)", lost));
                                any = true;
                            }
                            if !any {
                                label!(ui, "No activity this month.");
                            }
                        });

                        // Construction cell: total remaining need, minus
                        // what's being applied this month. Empty if nothing
                        // is needed.
                        if need > 0 {
                            label!(
                                ui,
                                col_format!(problem, "{}", need),
                                " – ",
                                col_format!(preview, "{}", applied)
                            );
                        } else {
                            ui.label("");
                        }

                        ui.end_row();
                    }
                });

            // Tools count (UniqueResource — tracked separately from uniform resources).
            let total_tools = crate::place::total_tool_count(&constructed);
            if total_tools > 0 {
                ui.horizontal(|ui| {
                    label!(ui, format!("Tools: {}", total_tools));
                });
            }

            ui.separator();
            heading_label!(ui, "Travelers");
            // Clone demands/reward before the closure to avoid borrow conflict with &mut traveler_state.invited.
            let offer_demands_reward: Option<(
                Vec<(crate::resource::UniformResource, u16)>,
                ResolvedReward,
            )> = traveler_state
                .current_offer
                .as_ref()
                .map(|o| (o.demands.clone(), o.reward.clone()));

            if let Some((demands, reward)) = offer_demands_reward {
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
                        let reward_desc = match &reward {
                            ResolvedReward::Tool(kind) => format!("1 {}", kind.label()),
                            ResolvedReward::Resource(res, qty) => {
                                format!("{} {}", qty, res.label())
                            }
                        };
                        label!(ui, format!("Brings: {} + reveals a path", reward_desc));
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
        let mut rng = rand::rng();
        let outcome = crate::month::advance_month(
            &mut clock,
            &mut farms,
            &mut constructed,
            &mut pending,
            &mut population,
            &mut traveler_state,
            &material_list,
            sandbox.enabled,
            &mut rng,
        );
        if let Some(real_changes) = outcome.construction_changes {
            crate::construction::apply_construction_completion(
                &mut commands,
                &mut assembled,
                &mut viewable,
                &structure_list,
                real_changes,
            );
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
