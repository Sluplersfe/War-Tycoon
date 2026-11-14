//! First-run onboarding: a short checklist at the bottom of the screen that
//! advances as the player actually does each thing. F9 skips it; it can be
//! shown again from Settings.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use egui::{Align2, Color32, FontId};

use crate::combat::CombatFx;
use crate::gunsmith::GunsmithUi;
use crate::input::Actions;
use crate::objects::MotionVel;
use crate::player::{LocalPlayer, Look};
use crate::settings::Settings;
use crate::state::Economy;

const STEPS: [(&str, &str); 8] = [
    ("Look around", "Move the mouse (or the right stick) to look."),
    ("Move", "Walk with W A S D (or the left stick). Hold Shift to sprint, Space to jump."),
    ("Shoot", "Click the left mouse button (or press RT) to fire. R reloads."),
    ("Aim down sights", "Hold the right mouse button (or LT) to aim; scopes zoom in."),
    ("Build", "Walk to your base and step on a glowing pad to buy a dropper. Cash flows in as ore is sold."),
    ("Customize", "Press G to open the gunsmith: attachments, finishes and reticles are all free."),
    ("Fight", "Red Raiders patrol the island and pay out when they fall. J shows your weapon mastery."),
    ("Capture", "Stand on a flag to capture it. Held flags pay income to your whole team. Good luck!"),
];

#[derive(Resource, Default)]
pub struct Tutorial {
    pub step: usize,
    looked: f32,
    walked: f32,
    base_owned: Option<usize>,
    opened_gunsmith: bool,
    step_time: f32,
    finished: bool,
}

impl Tutorial {
    pub fn restart(&mut self) {
        *self = Self::default();
    }
}

#[allow(clippy::too_many_arguments)]
pub fn tutorial_ui(
    mut contexts: EguiContexts,
    mut tut: ResMut<Tutorial>,
    mut settings: ResMut<Settings>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    actions: Res<Actions>,
    look: Res<Look>,
    fx: Res<CombatFx>,
    econ: Res<Economy>,
    gunsmith: Res<GunsmithUi>,
    me: Query<&MotionVel, With<LocalPlayer>>,
) -> Result {
    if !settings.tutorial_done && tut.finished {
        tut.restart(); // switched back on from Settings
    }
    if settings.tutorial_done || tut.step >= STEPS.len() {
        if tut.step >= STEPS.len() {
            tut.finished = true;
            settings.tutorial_done = true;
        }
        return Ok(());
    }
    let dt = time.delta_secs();
    let now = time.elapsed_secs_f64();
    tut.step_time += dt;
    if keys.just_pressed(KeyCode::F9) {
        settings.tutorial_done = true;
        return Ok(());
    }
    let base_owned = *tut.base_owned.get_or_insert(econ.base.owned.len());
    if gunsmith.open {
        tut.opened_gunsmith = true;
    }
    let walked = me.single().map(|v| Vec2::new(v.0.x, v.0.z).length()).unwrap_or(0.0);
    tut.looked += motion.delta.length() + actions.look.length() * 12.0;
    tut.walked += walked * dt;
    let done = match tut.step {
        0 => tut.looked > 300.0,
        1 => tut.walked > 12.0,
        2 => fx.fired_at > now - tut.step_time as f64,
        3 => look.ads > 0.9,
        4 => econ.base.owned.len() > base_owned,
        5 => tut.opened_gunsmith,
        6 => tut.step_time > 9.0,
        _ => tut.step_time > 10.0,
    };
    if done {
        tut.step += 1;
        tut.step_time = 0.0;
        return Ok(());
    }

    let ctx = contexts.ctx_mut()?;
    let screen = ctx.content_rect();
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("tutorial")));
    let (title, text) = STEPS[tut.step];
    let rect = egui::Rect::from_center_size(egui::pos2(screen.center().x, screen.bottom() - 150.0), egui::vec2(560.0, 84.0));
    p.rect_filled(rect, 8.0, Color32::from_black_alpha(190));
    p.rect_stroke(rect, 8.0, egui::Stroke::new(1.5, Color32::from_rgb(0xE8, 0xA3, 0x17)), egui::StrokeKind::Inside);
    p.text(
        rect.left_top() + egui::vec2(16.0, 10.0),
        Align2::LEFT_TOP,
        format!("{}  ·  {}/{}", title.to_uppercase(), tut.step + 1, STEPS.len()),
        FontId::proportional(15.0),
        Color32::from_rgb(0xE8, 0xA3, 0x17),
    );
    let galley = p.layout(text.to_string(), FontId::proportional(15.0), Color32::WHITE, rect.width() - 32.0);
    p.galley(rect.left_top() + egui::vec2(16.0, 34.0), galley, Color32::WHITE);
    p.text(rect.right_bottom() - egui::vec2(12.0, 8.0), Align2::RIGHT_BOTTOM, "F9 skip", FontId::proportional(11.0), Color32::GRAY);
    Ok(())
}
