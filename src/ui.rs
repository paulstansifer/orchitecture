use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::city::City;
use crate::game_mode::{GameMode, SandboxMode};
use crate::materials::MaterialList;
use crate::month::AdvanceMonthRequested;
use crate::population::Population;
use crate::resource::Precision;
use crate::resource_icons::{ResourceIcons, LARGE_SIZE};
use crate::surroundings::farmstead::{FarmsResource, GameClock};
use crate::traveler::TravelerState;
use crate::ui_view::{month_panel_view, AdvanceState};
use crate::{col_format, heading_label, label, note_label};

pub fn shared_ui_system(
    mut contexts: EguiContexts,
    clock: Res<GameClock>,
    farms: Res<FarmsResource>,
    world: City,
    resource_icons: Res<ResourceIcons>,
    mut next_game_mode: ResMut<NextState<GameMode>>,
    current_mode: Res<State<GameMode>>,
    mut advance_month: MessageWriter<AdvanceMonthRequested>,
    mut traveler_state: ResMut<TravelerState>,
    population: Res<Population>,
    sandbox: Res<SandboxMode>,
    material_list: Res<MaterialList>,
) {
    use egui::Color32;
    let City {
        constructed,
        pending,
        assembled: _,
    } = world;

    let icon_textures_lg = resource_icons.texture_ids_large(&mut contexts);
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    let view = month_panel_view(
        clock.month() + 1,
        &farms,
        &constructed,
        &pending,
        &population,
        &traveler_state,
        &material_list,
        sandbox.enabled,
    );

    let wait_id = egui::Id::new("wait_confirmation");

    let mut go_advance_month = false;
    let mut next_mode: Option<GameMode> = None;

    egui::SidePanel::right("resources")
        .min_width(130.0)
        .show(ctx, |ui| {
            heading_label!(ui, format!("Month {}", view.month));
            ui.add_space(2.0);

            match view.advance {
                AdvanceState::Active { blocked } => {
                    ui.add_enabled_ui(!blocked, |ui| {
                        if ui.button("Advance Month").clicked() {
                            go_advance_month = true;
                        }
                    });
                    if view.has_project && blocked {
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
                            if view.has_project {
                                ui.add(
                                    egui::ProgressBar::new(view.construction_progress.unwrap_or(1.0))
                                    .desired_width(100.0)
                                    .show_percentage(),
                                );
                            } else {
                                label!(ui, "No current project");
                            }
                        });
                        ui.vertical(|ui| {
                            ui.set_min_width(0.0);
                            if view.market_stand_count == 0 {
                                label!(ui, "No market stalls");
                            } else {
                                label!(ui, format!("{}/{} farms invited", view.invited_count, view.market_stand_count));
                            }
                        });
                    });
                }
                AdvanceState::NothingPending => {
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
            }

            ui.separator();
            ui.heading("Resources");
            if view.rows.is_empty() {
                ui.label("(none)");
            }
            egui::Grid::new("resource_grid")
                .num_columns(3)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    for row in &view.rows {
                        // Icon cell.
                        if let Some(&tex) = icon_textures_lg.get(&row.resource) {
                            ui.add(egui::Image::new(egui::load::SizedTexture::new(
                                tex, LARGE_SIZE,
                            )));
                        } else {
                            ui.label("");
                        }

                        // Storage cell: current amount, plus the net change
                        // to storage this month, plus anything lost to a
                        // full store. Tooltip lives here.
                        let quantity_str = match row.precision {
                            Precision::Exact => format!("{}", row.current),
                            Precision::Approximate => format!("~{}", row.current),
                            Precision::Conservative => format!(">{}", row.current),
                        };
                        // If some storage is dedicated to this resource, show
                        // it as "current/capacity".
                        let quantity_str = match row.dedicated_capacity {
                            Some(cap) => format!("{}/{}", quantity_str, cap),
                            None => quantity_str,
                        };
                        let storage_resp = if !view.has_storage {
                            if row.lost > 0 {
                                label!(ui, format!("({} lost)", row.lost))
                            } else {
                                ui.label("")
                            }
                        } else if row.storage_delta > 0 {
                            if row.lost > 0 {
                                label!(
                                    ui,
                                    quantity_str,
                                    col_format!(preview, " +{}", row.storage_delta),
                                    format!(" ({} lost)", row.lost)
                                )
                            } else {
                                label!(ui, quantity_str, col_format!(preview, " +{}", row.storage_delta))
                            }
                        } else if row.storage_delta < 0 {
                            label!(ui, quantity_str, col_format!(problem, " –{}", -row.storage_delta))
                        } else if row.lost > 0 {
                            label!(ui, quantity_str, format!(" ({} lost)", row.lost))
                        } else {
                            label!(ui, quantity_str)
                        };

                        // Tooltip: one colored line per contributing source,
                        // e.g. "+10 (market)" or "-7 (traveler)".
                        storage_resp.on_hover_ui(|ui| {
                            let mut any = false;
                            for &(source, delta) in &row.sources {
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
                            if row.lost > 0 {
                                label!(ui, format!("({} lost)", row.lost));
                                any = true;
                            }
                            if !any {
                                label!(ui, "No activity this month.");
                            }
                        });

                        // Construction cell: total remaining need, minus
                        // what's being applied this month. Empty if nothing
                        // is needed.
                        if row.need > 0 {
                            label!(
                                ui,
                                col_format!(problem, "{}", row.need),
                                " – ",
                                col_format!(preview, "{}", row.applied)
                            );
                        } else {
                            ui.label("");
                        }

                        ui.end_row();
                    }

                    // Uncommitted storage: free capacity not dedicated to any
                    // particular resource, plus how much of it this month's
                    // projected leftover inflow will consume.
                    ui.label(egui::RichText::new("?").size(LARGE_SIZE[1]));
                    let uncommitted = &view.uncommitted_storage;
                    if uncommitted.delta < 0 {
                        label!(
                            ui,
                            format!("/{}", uncommitted.available),
                            col_format!(problem, " –{}", -uncommitted.delta)
                        );
                    } else if uncommitted.delta > 0 {
                        label!(
                            ui,
                            format!("/{}", uncommitted.available),
                            col_format!(preview, " +{}", uncommitted.delta)
                        );
                    } else {
                        label!(ui, format!("/{}", uncommitted.available));
                    }
                    ui.label("");
                    ui.end_row();
                });

            // Tools count (UniqueResource — tracked separately from uniform resources).
            if view.tool_count > 0 {
                ui.horizontal(|ui| {
                    label!(ui, format!("Tools: {}", view.tool_count));
                });
            }

            ui.separator();
            heading_label!(ui, "Travelers");

            if let Some(traveler) = &view.traveler {
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(20, 20, 30, 200))
                    .inner_margin(egui::Margin::same(4))
                    .corner_radius(4.0)
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width());
                        label!(ui, "Traveler");
                        for (res, qty) in &traveler.demands {
                            label!(ui, format!("Wants {} {}", qty, res.label()));
                        }
                        label!(ui, format!("Brings: {} + reveals a path", traveler.reward_desc));
                        ui.add_enabled(
                            traveler.affordable || traveler_state.invited,
                            egui::Checkbox::new(&mut traveler_state.invited, "Invite"),
                        );
                        if !traveler.affordable {
                            note_label!(ui, "(insufficient resources)");
                        }
                    });
            } else {
                label!(ui, "(no traveler this month)");
            }

            // Mode buttons pushed to the bottom of the panel: every mode but
            // the current one, each switchable to directly.
            const MODE_BUTTONS: [(GameMode, &str); 3] = [
                (GameMode::Build, "Build"),
                (GameMode::Walk, "Walk Around"),
                (GameMode::Surroundings, "Surroundings"),
            ];
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                for (mode, label) in MODE_BUTTONS {
                    if mode == *current_mode.get() {
                        continue;
                    }
                    if ui.button(label).clicked() {
                        next_mode = Some(mode);
                    }
                }
            });
        });

    // ── Apply deferred actions ────────────────────────────────────────────────
    if go_advance_month {
        // The actual simulation runs in `month::advance_month_system`.
        advance_month.write(AdvanceMonthRequested);
        ctx.data_mut(|d| d.remove::<bool>(wait_id));
    }
    if let Some(mode) = next_mode {
        next_game_mode.set(mode);
    }
}
