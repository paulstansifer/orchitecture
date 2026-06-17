use std::f32::consts::FRAC_PI_2;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::game_mode::GameMode;
use crate::ortho_camera::WalkCameraState;

pub fn walk_ui_system(
    mut contexts: EguiContexts,
    mut next_game_mode: ResMut<NextState<GameMode>>,
    walk_state: Res<WalkCameraState>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    egui::TopBottomPanel::bottom("walk_controls_bottom").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label("Walk mode — WASD=move  Right-drag=rotate");
            ui.separator();
            if ui.button("Build").clicked() {
                next_game_mode.set(GameMode::Build);
            }
        });
    });

    // Direction indicator during right-drag: a small white square on the screen edge.
    if walk_state.is_right_dragging && walk_state.drag_delta.length_squared() > 4.0 {
        let delta = walk_state.drag_delta;
        let angle = delta.y.atan2(delta.x);
        let quadrant = ((angle / FRAC_PI_2).round() as i32).rem_euclid(4) as u8;

        let screen = ctx.content_rect();
        let margin = 20.0;
        let indicator_pos = match quadrant {
            0 => egui::pos2(screen.right() - margin, screen.center().y),
            1 => egui::pos2(screen.center().x, screen.bottom() - margin),
            2 => egui::pos2(screen.left() + margin, screen.center().y),
            _ => egui::pos2(screen.center().x, screen.top() + margin),
        };

        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("walk_drag_indicator"),
        ))
        .rect_filled(
            egui::Rect::from_center_size(indicator_pos, egui::Vec2::splat(12.0)),
            0.0,
            egui::Color32::WHITE,
        );
    }
}
