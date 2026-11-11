//! The gunsmith: a side panel (G) for attachments, finishes and reticles,
//! with the live weapon turned side-on in the view model next to it.
//! Everything is free; parts unlock with rank, a few finishes with rebirths.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use egui::{Align2, Color32, FontId, RichText, Sense, Stroke};
use frontline_sim::ranks;
use frontline_sim::weapons::WeaponKind;

use crate::combat::Loadout;
use crate::guns::{self, Armory, GunMods, Pattern, RETICLES, SKINS, Slot};
use crate::state::{Economy, GameData};
use crate::ui::AMBER;

#[derive(Resource, Default)]
pub struct GunsmithUi {
    pub open: bool,
}

fn c(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

const PANEL: Color32 = Color32::from_rgb(16, 18, 13);
const GREEN: Color32 = Color32::from_rgb(0x4C, 0xD0, 0x5C);
const RED: Color32 = Color32::from_rgb(0xD0, 0x48, 0x40);

/// Draws a finish swatch: a rough picture of its pattern.
fn swatch(p: &egui::Painter, r: egui::Rect, idx: usize) {
    let s = &SKINS[idx];
    let col = |i: usize| c(s.colors[i]);
    match s.pattern {
        Pattern::Factory => {
            p.rect_filled(r, 4.0, col(0));
            p.rect_filled(egui::Rect::from_min_size(r.min + egui::vec2(0.0, r.height() * 0.6), egui::vec2(r.width(), r.height() * 0.4)), 4.0, col(1));
        }
        Pattern::Solid => {
            p.rect_filled(r, 4.0, col(0));
            p.circle_filled(r.min + egui::vec2(r.width() * 0.3, r.height() * 0.3), r.width() * 0.16, Color32::from_white_alpha(70));
        }
        Pattern::Blobs | Pattern::Digital => {
            p.rect_filled(r, 4.0, col(0));
            let spots = [(0.25, 0.3, 1), (0.7, 0.25, 2), (0.55, 0.7, 3), (0.2, 0.75, 2), (0.85, 0.7, 1), (0.45, 0.45, 3)];
            for (x, y, k) in spots {
                let at = r.min + egui::vec2(r.width() * x, r.height() * y);
                if s.pattern == Pattern::Digital {
                    p.rect_filled(egui::Rect::from_center_size(at, egui::vec2(r.width() * 0.26, r.height() * 0.18)), 0.0, col(k));
                } else {
                    p.circle_filled(at, r.width() * 0.15, col(k));
                }
            }
        }
        Pattern::Tiger | Pattern::Damascus | Pattern::Hazard | Pattern::Carbon => {
            p.rect_filled(r, 4.0, col(0));
            for i in 0..5 {
                let x = r.min.x + r.width() * (i as f32 * 0.22 + 0.05);
                let (a, b) = if s.pattern == Pattern::Hazard { (egui::pos2(x, r.max.y), egui::pos2(x + r.width() * 0.3, r.min.y)) } else { (egui::pos2(x, r.min.y), egui::pos2(x + r.width() * 0.08, r.max.y)) };
                p.line_segment([a, b], Stroke::new(r.width() * 0.08, col(1)));
            }
        }
        Pattern::Grid | Pattern::Cracks => {
            p.rect_filled(r, 4.0, col(0));
            for i in 1..4 {
                let f = i as f32 / 4.0;
                if s.pattern == Pattern::Grid {
                    p.line_segment([egui::pos2(r.min.x + r.width() * f, r.min.y), egui::pos2(r.min.x + r.width() * f, r.max.y)], Stroke::new(1.5, col(1)));
                    p.line_segment([egui::pos2(r.min.x, r.min.y + r.height() * f), egui::pos2(r.max.x, r.min.y + r.height() * f)], Stroke::new(1.5, col(1)));
                } else {
                    let a = r.min + egui::vec2(r.width() * f, 0.0);
                    p.line_segment([a, a + egui::vec2(-r.width() * 0.15, r.height() * 0.5)], Stroke::new(1.5, col(1)));
                    p.line_segment([a + egui::vec2(-r.width() * 0.15, r.height() * 0.5), a + egui::vec2(r.width() * 0.05, r.height())], Stroke::new(1.5, col(1)));
                }
            }
        }
    }
}

/// One stat line: name, bar (factory vs current) and a value.
fn stat(ui: &mut egui::Ui, name: &str, base: f32, now: f32, text: String) {
    ui.horizontal(|ui| {
        ui.add_sized([92.0, 16.0], egui::Label::new(RichText::new(name).size(13.0).color(Color32::LIGHT_GRAY)));
        let (r, _) = ui.allocate_exact_size(egui::vec2(170.0, 10.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(r, 3.0, Color32::from_black_alpha(160));
        let w = |f: f32| r.width() * f.clamp(0.02, 1.0);
        let lo = base.min(now);
        p.rect_filled(egui::Rect::from_min_size(r.min, egui::vec2(w(lo), r.height())), 3.0, Color32::from_rgb(0xD8, 0xD8, 0xD0));
        if (now - base).abs() > 0.005 {
            let col = if now > base { GREEN } else { RED };
            let (a, b) = (w(lo), w(base.max(now)));
            p.rect_filled(egui::Rect::from_min_size(r.min + egui::vec2(a, 0.0), egui::vec2(b - a, r.height())), 3.0, col);
        }
        ui.label(RichText::new(text).size(13.0).color(Color32::WHITE));
    });
}

#[allow(clippy::too_many_arguments)]
pub fn gunsmith_ui(
    mut contexts: EguiContexts,
    mut gs: ResMut<GunsmithUi>,
    mut armory: ResMut<Armory>,
    mut loadout: ResMut<Loadout>,
    data: Res<GameData>,
    econ: Res<Economy>,
) -> Result {
    if !gs.open {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let id = loadout.current_id().to_string();
    let mut m = armory.get(&id);
    let mut pick = None;
    let mut close = false;
    let height = ctx.content_rect().height();
    egui::Window::new("gunsmith")
        .title_bar(false)
        .resizable(false)
        .movable(false)
        .anchor(Align2::LEFT_TOP, [0.0, 0.0])
        .fixed_size([420.0, height - 36.0])
        .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::same(18)))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("GUNSMITH").size(30.0).strong().color(AMBER));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close  (G)").clicked() {
                        close = true;
                    }
                });
            });
            ui.label(RichText::new("Attachments and finishes are free — they unlock as you rank up.").size(12.5).color(Color32::GRAY));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                for (i, w) in loadout.owned().iter().enumerate() {
                    let name = data.weapon(w).map(|d| d.name.clone()).unwrap_or_default();
                    if ui.add(egui::Button::new(RichText::new(name).size(14.0)).selected(i == loadout.current)).clicked() {
                        pick = Some(i);
                    }
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(ui.available_height() - 70.0).show(ui, |ui| {
                let mut any = false;
                for slot in Slot::ALL {
                    let opts = guns::options(&id, slot);
                    if opts.len() < 2 {
                        continue;
                    }
                    any = true;
                    ui.label(RichText::new(slot.label().to_uppercase()).size(12.0).strong().color(Color32::from_rgb(0xB8, 0xC0, 0x90)));
                    ui.horizontal_wrapped(|ui| {
                        for &o in opts {
                            let a = &guns::table(slot)[o as usize];
                            let unlocked = guns::attachment_unlocked(slot, o, econ.rank);
                            let label = if unlocked { a.name.to_string() } else { format!("🔒 {}", a.name) };
                            let r = ui.add_enabled(unlocked, egui::Button::new(RichText::new(label).size(13.5)).selected(m.get(slot) == o));
                            let tip = if unlocked { a.desc.to_string() } else { format!("{}\nUnlocks at {}", a.desc, ranks::name(a.rank)) };
                            if r.on_hover_text(tip).on_disabled_hover_text(format!("Unlocks at {}", ranks::name(a.rank))).clicked() {
                                m.set(slot, o);
                            }
                        }
                    });
                    ui.add_space(2.0);
                }
                if !any {
                    ui.label(RichText::new("No attachment points on this weapon — pick a finish on the right.").color(Color32::GRAY));
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Factory reset").clicked() {
                    m = GunMods::factory(&id);
                }
                if ui.button("🎲 Randomize").clicked() {
                    let r = |n: usize| (rand::random::<f32>() * n as f32) as usize % n.max(1);
                    for slot in Slot::ALL {
                        let opts: Vec<u8> = guns::options(&id, slot).iter().copied().filter(|&o| guns::attachment_unlocked(slot, o, econ.rank)).collect();
                        if !opts.is_empty() {
                            m.set(slot, opts[r(opts.len())]);
                        }
                    }
                    let skins: Vec<u8> = (0..SKINS.len() as u8).filter(|&i| guns::skin_unlocked(i, econ.rank, econ.rebirths)).collect();
                    m.skin = skins[r(skins.len())];
                    m.reticle = r(RETICLES.len()) as u8;
                }
            });
            ui.label(RichText::new("Hold F in game to inspect your weapon").size(11.5).color(Color32::GRAY));
        });
    egui::Window::new("gunsmith_stats")
        .title_bar(false)
        .resizable(false)
        .movable(false)
        .anchor(Align2::RIGHT_TOP, [-16.0, 16.0])
        .fixed_size([340.0, 0.0])
        .frame(egui::Frame::new().fill(PANEL).corner_radius(10).inner_margin(egui::Margin::same(14)))
        .show(ctx, |ui| {
            ui.label(RichText::new(data.weapon(&id).map(|d| d.name.to_uppercase()).unwrap_or_default()).size(18.0).strong().color(Color32::WHITE));
            ui.label(RichText::new("white = factory · green/red = your build").size(11.5).color(Color32::GRAY));
            ui.spacing_mut().item_spacing.y = 3.0;
            if let Some(def) = data.weapon(&id) {
                let (bd, bh) = guns::apply(def, &GunMods::factory(&id));
                let (nd, nh) = guns::apply(def, &m);
                let pellets = |d: &frontline_sim::weapons::WeaponDef| match d.kind {
                    WeaponKind::Hitscan { pellets, .. } => pellets.max(1) as f32,
                    _ => 1.0,
                };
                stat(ui, "Damage", bd.damage * pellets(&bd) / 220.0, nd.damage * pellets(&nd) / 220.0, format!("{:.0}", nd.damage * pellets(&nd)));
                stat(ui, "Fire rate", bd.rate / 15.0, nd.rate / 15.0, format!("{:.1}/s", nd.rate));
                if let (WeaponKind::Hitscan { range: br, spread: bs, .. }, WeaponKind::Hitscan { range: nr, spread: ns, .. }) = (bd.kind, nd.kind) {
                    stat(ui, "Range", br / 600.0, nr / 600.0, format!("{nr:.0} m"));
                    stat(ui, "Accuracy", 1.0 - bs / 0.1, 1.0 - ns / 0.1, format!("{:.0}%", (1.0 - ns / 0.1).clamp(0.0, 1.0) * 100.0));
                    stat(ui, "Hip fire", 1.0 - bh.hip * 0.5, 1.0 - nh.hip * 0.5, format!("{:.0}%", (1.0 - nh.hip * 0.5) * 100.0));
                }
                if bh.recoil > 0.0 {
                    stat(ui, "Control", 1.0 - bh.recoil / 7.0, 1.0 - nh.recoil / 7.0, format!("{:.1}°", nh.recoil));
                    stat(ui, "Aim speed", 1.0 - bh.ads_time / 0.6, 1.0 - nh.ads_time / 0.6, format!("{:.0} ms", nh.ads_time * 1000.0));
                    stat(ui, "Zoom", bh.zoom / 8.0, nh.zoom / 8.0, format!("{:.1}x", nh.zoom));
                }
                if bd.mag > 1 {
                    stat(ui, "Magazine", bd.mag as f32 / (bd.mag as f32 * 2.6), nd.mag as f32 / (bd.mag as f32 * 2.6), format!("{} rds", nd.mag));
                    stat(ui, "Reload", 1.0 - bd.reload / 6.0, 1.0 - nd.reload / 6.0, format!("{:.2} s", nd.reload));
                }
            }
            ui.add_space(4.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            let optic_has_reticle = !guns::options(&id, Slot::Optic).is_empty() && (m.optic > 0 || id == "pistol");
            if optic_has_reticle {
                ui.label(RichText::new("RETICLE").size(12.0).strong().color(Color32::from_rgb(0xB8, 0xC0, 0x90)));
                ui.horizontal(|ui| {
                    for (i, (name, col)) in RETICLES.iter().enumerate() {
                        let (r, resp) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), Sense::click());
                        let p = ui.painter();
                        p.circle_filled(r.center(), 11.0, Color32::from_black_alpha(200));
                        p.circle_filled(r.center(), 5.0, c(*col));
                        if m.reticle as usize == i {
                            p.circle_stroke(r.center(), 12.0, Stroke::new(2.0, AMBER));
                        }
                        if resp.on_hover_text(*name).clicked() {
                            m.reticle = i as u8;
                        }
                    }
                });
            }
            ui.add_space(4.0);
            ui.label(RichText::new("FINISH").size(12.0).strong().color(Color32::from_rgb(0xB8, 0xC0, 0x90)));
            egui::Grid::new("finishes").spacing([8.0, 8.0]).show(ui, |ui| {
                for (i, s) in SKINS.iter().enumerate() {
                    let unlocked = guns::skin_unlocked(i as u8, econ.rank, econ.rebirths);
                    let (r, resp) = ui.allocate_exact_size(egui::vec2(56.0, 56.0), Sense::click());
                    let p = ui.painter();
                    swatch(p, r.shrink2(egui::vec2(4.0, 10.0)).translate(egui::vec2(0.0, -6.0)), i);
                    p.text(r.center_bottom() - egui::vec2(0.0, 1.0), Align2::CENTER_BOTTOM, s.name, FontId::proportional(10.0), Color32::LIGHT_GRAY);
                    if !unlocked {
                        p.rect_filled(r, 4.0, Color32::from_black_alpha(150));
                        p.text(r.center() - egui::vec2(0.0, 6.0), Align2::CENTER_CENTER, "🔒", FontId::proportional(16.0), Color32::WHITE);
                    }
                    if m.skin as usize == i {
                        p.rect_stroke(r, 5.0, Stroke::new(2.0, AMBER), egui::StrokeKind::Inside);
                    } else if resp.hovered() {
                        p.rect_stroke(r, 5.0, Stroke::new(1.0, Color32::GRAY), egui::StrokeKind::Inside);
                    }
                    let need = if s.rebirths > 0 { format!("Unlocks after {} rebirth{}", s.rebirths, if s.rebirths > 1 { "s" } else { "" }) } else { format!("Unlocks at {}", ranks::name(s.rank)) };
                    if resp.on_hover_text(if unlocked { s.name.to_string() } else { need }).clicked() && unlocked {
                        m.skin = i as u8;
                    }
                    if i % 5 == 4 {
                        ui.end_row();
                    }
                }
            });
        });
    if let Some(i) = pick {
        loadout.current = i;
    }
    if m != armory.get(&id) {
        armory.set(&id, m);
    }
    if close {
        gs.open = false;
    }
    Ok(())
}
