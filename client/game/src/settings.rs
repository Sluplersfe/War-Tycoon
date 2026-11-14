//! Player preferences: stored in the config dir, applied to the window and camera.

use std::collections::BTreeMap;

use bevy::prelude::*;
use bevy::window::{MonitorSelection, PresentMode, PrimaryWindow, WindowMode};
use bevy_egui::{EguiContexts, egui};
use egui::{Align2, Color32, RichText};
use serde::{Deserialize, Serialize};

use crate::net::config_dir;
use crate::ui::{AMBER, primary_button};

#[derive(Resource, Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub sensitivity: f32,
    pub invert_y: bool,
    pub fov: f32,
    pub fullscreen: bool,
    /// Vertical render resolution in pixels (0 = native, i.e. the window size).
    pub render_height: u32,
    pub vsync: bool,
    /// Key bindings by action id (see `input::Act`); missing entries use the defaults.
    pub keys: BTreeMap<String, String>,
    /// Gamepad look sensitivity multiplier.
    pub pad_sensitivity: f32,
    pub tutorial_done: bool,
    /// Master volume, 0..=1.5.
    pub volume: f32,
    /// 0 = off, 2 or 4 samples.
    pub msaa: u32,
    pub shadows: bool,
    /// Scales how far the world is drawn (fog and far plane).
    pub draw_distance: f32,
    pub ui_scale: f32,
    pub show_fps: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { sensitivity: 1.0, invert_y: false, fov: 70.0, fullscreen: false, render_height: 0, vsync: true, keys: BTreeMap::new(), pad_sensitivity: 1.0, tutorial_done: false, volume: 0.7, msaa: 4, shadows: true, draw_distance: 1.0, ui_scale: 1.0, show_fps: false }
    }
}

impl Settings {
    pub fn key(&self, act: crate::input::Act) -> KeyCode {
        self.keys.get(act.id()).and_then(|id| crate::input::key_from_id(id)).unwrap_or_else(|| act.default_key())
    }

    fn path() -> Option<std::path::PathBuf> {
        Some(config_dir()?.join("settings.json"))
    }
    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .map(|s| s.sanitized())
            .unwrap_or_default()
    }
    fn save(&self) {
        let Some(p) = Self::path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, serde_json::to_string_pretty(self).unwrap_or_default());
    }
    fn sanitized(mut self) -> Self {
        self.sensitivity = self.sensitivity.clamp(0.1, 4.0);
        self.fov = self.fov.clamp(50.0, 120.0);
        self.ui_scale = self.ui_scale.clamp(0.7, 2.0);
        self.volume = self.volume.clamp(0.0, 1.5);
        self.pad_sensitivity = self.pad_sensitivity.clamp(0.3, 3.0);
        self.draw_distance = self.draw_distance.clamp(0.3, 2.0);
        self.msaa = match self.msaa { 0 => 0, 2 => 2, _ => 4 };
        if self.render_height != 0 {
            self.render_height = self.render_height.clamp(108, 4320);
        }
        self
    }
}

/// Whether the settings window is showing.
#[derive(Resource, Default)]
pub struct SettingsUi {
    pub open: bool,
    /// The action waiting for a key press in the controls list.
    pub rebinding: Option<crate::input::Act>,
}

/// Pushes changed settings to the window and writes them to disk once they settle.
pub fn apply_settings(
    settings: Res<Settings>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
    time: Res<Time>,
    mut pending: Local<Option<f32>>,
    mut first: Local<bool>,
) {
    if settings.is_changed() || !*first {
        let mode = if settings.fullscreen {
            WindowMode::BorderlessFullscreen(MonitorSelection::Current)
        } else {
            WindowMode::Windowed
        };
        if window.mode != mode {
            window.mode = mode;
        }
        let present = if settings.vsync { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync };
        if window.present_mode != present {
            window.present_mode = present;
        }
        if *first {
            *pending = Some(0.6);
        }
        *first = true;
    }
    if let Some(t) = pending.as_mut() {
        *t -= time.delta_secs();
        if *t <= 0.0 {
            settings.save();
            *pending = None;
        }
    }
}

pub fn settings_window(
    mut contexts: EguiContexts,
    mut settings: ResMut<Settings>,
    mut open: ResMut<SettingsUi>,
) -> Result {
    if !open.open {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let mut s = settings.clone();
    let mut close = false;
    egui::Window::new("settings")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([420.0, 0.0])
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("SETTINGS").size(26.0).color(AMBER).strong());
            });
            ui.add_space(8.0);
            section(ui, "Controls");
            egui::Grid::new("set_controls").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                ui.label("Mouse sensitivity");
                ui.add(egui::Slider::new(&mut s.sensitivity, 0.1..=4.0).logarithmic(true).fixed_decimals(2).suffix("×"));
                ui.end_row();
                ui.label("Invert Y axis");
                ui.checkbox(&mut s.invert_y, "");
                ui.end_row();
                ui.label("Gamepad look speed");
                ui.add(egui::Slider::new(&mut s.pad_sensitivity, 0.3..=3.0).fixed_decimals(2).suffix("×"));
                ui.end_row();
            });
            ui.add_space(6.0);
            egui::CollapsingHeader::new("Key bindings").default_open(false).show(ui, |ui| {
                egui::Grid::new("set_keys").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                    for act in crate::input::ACTS {
                        ui.label(act.label());
                        let waiting = open.rebinding == Some(act);
                        let text = if waiting { "press a key…".to_string() } else { crate::input::key_label(s.key(act)) };
                        if ui.add_sized([130.0, 22.0], egui::Button::new(text).selected(waiting)).clicked() {
                            open.rebinding = if waiting { None } else { Some(act) };
                        }
                        ui.end_row();
                    }
                });
                ui.horizontal(|ui| {
                    if ui.small_button("Reset keys").clicked() {
                        s.keys.clear();
                        open.rebinding = None;
                    }
                    if ui.small_button("Show tutorial again").clicked() {
                        s.tutorial_done = false;
                    }
                });
                ui.label(RichText::new("Mouse: LMB fire, RMB aim, wheel switches weapons. Gamepad: sticks move/look, RT fire, LT aim, A jump, X reload, Y interact, B inspect, bumpers weapons, D-pad ↑ gunsmith, Start menu.").size(11.0).color(Color32::GRAY));
            });
            ui.add_space(10.0);
            section(ui, "Video");
            egui::Grid::new("set_video").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                ui.label("Field of view");
                ui.add(egui::Slider::new(&mut s.fov, 50.0..=120.0).fixed_decimals(0).suffix("°"));
                ui.end_row();
                ui.label("Render resolution");
                let name = |h: u32| if h == 0 { "Native".to_string() } else { format!("{h}p") };
                egui::ComboBox::from_id_salt("render_height")
                    .selected_text(name(s.render_height))
                    .show_ui(ui, |ui| {
                        for v in RENDER_HEIGHTS {
                            ui.selectable_value(&mut s.render_height, v, name(v));
                        }
                    });
                ui.end_row();
                ui.label("Fullscreen");
                ui.checkbox(&mut s.fullscreen, "");
                ui.end_row();
                ui.label("Anti-aliasing");
                egui::ComboBox::from_id_salt("msaa")
                    .selected_text(if s.msaa == 0 { "Off".to_string() } else { format!("MSAA {}×", s.msaa) })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.msaa, 0, "Off");
                        ui.selectable_value(&mut s.msaa, 2, "MSAA 2×");
                        ui.selectable_value(&mut s.msaa, 4, "MSAA 4×");
                    });
                ui.end_row();
                ui.label("Shadows");
                ui.checkbox(&mut s.shadows, "");
                ui.end_row();
                ui.label("Draw distance");
                ui.add(egui::Slider::new(&mut s.draw_distance, 0.3..=2.0).fixed_decimals(1).suffix("×"));
                ui.end_row();
                ui.label("VSync");
                ui.checkbox(&mut s.vsync, "");
                ui.end_row();
                ui.label("Volume");
                ui.add(egui::Slider::new(&mut s.volume, 0.0..=1.5).fixed_decimals(2));
                ui.end_row();
                ui.label("Show FPS");
                ui.checkbox(&mut s.show_fps, "");
                ui.end_row();
            });
            ui.add_space(10.0);
            section(ui, "Interface");
            egui::Grid::new("set_ui").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                ui.label("UI scale");
                egui::ComboBox::from_id_salt("ui_scale")
                    .selected_text(format!("{:.0}%", s.ui_scale * 100.0))
                    .show_ui(ui, |ui| {
                        for v in [0.75f32, 1.0, 1.25, 1.5, 1.75, 2.0] {
                            ui.selectable_value(&mut s.ui_scale, v, format!("{:.0}%", v * 100.0));
                        }
                    });
                ui.end_row();
            });
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if ui.button("Reset to defaults").clicked() {
                    s = Settings::default();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if primary_button(ui, "Done", 120.0).clicked() {
                        close = true;
                    }
                });
            });
        });
    if s != *settings {
        *settings = s.sanitized();
    }
    if close {
        open.open = false;
    }
    Ok(())
}

const RENDER_HEIGHTS: [u32; 9] = [0, 144, 240, 360, 480, 720, 1080, 1440, 2160];

fn section(ui: &mut egui::Ui, name: &str) {
    ui.label(RichText::new(name.to_uppercase()).size(12.0).color(Color32::from_rgb(0xA8, 0xA0, 0x80)).strong());
    ui.separator();
}

/// Small FPS counter in the top-left corner.
pub fn fps_overlay(mut contexts: EguiContexts, settings: Res<Settings>, time: Res<Time>, mut avg: Local<f32>) -> Result {
    let dt = time.delta_secs().max(1e-4);
    *avg = if *avg == 0.0 { dt } else { *avg * 0.95 + dt * 0.05 };
    if !settings.show_fps {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    egui::Area::new(egui::Id::new("fps"))
        .anchor(Align2::LEFT_TOP, [10.0, 8.0])
        .interactable(false)
        .show(ctx, |ui| {
            ui.label(RichText::new(format!("{:.0} FPS", 1.0 / *avg)).monospace().size(12.0).color(Color32::from_rgb(0xC8, 0xD8, 0xC0)));
        });
    Ok(())
}
