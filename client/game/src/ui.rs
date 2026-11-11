//! All egui screens: login, lobby, in-game HUD, chat, scoreboard, pause and
//! the host's admin panel.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use bevy_rapier3d::prelude::Velocity;
use frontline_proto::pb::{self, client_msg::Msg as C};
use frontline_sim::{ranks, tycoon::rebirth_price};

use crate::combat::{CombatFx, Loadout};
use crate::net::{Net, SavedSession};
use crate::objects::{Interp, NetObject, RemoteAvatar, VehicleBody};
use crate::player::{Controls, Interact, Seat, Vitals};
use crate::state::{AppState, Economy, Feed, GameData, LobbyUi, LoginUi, Me, Room, request_leave};
use crate::tycoon::BuyPad;
use crate::world::MainCamera;

use egui::{Align2, Color32, FontId, RichText};

pub const AMBER: Color32 = Color32::from_rgb(0xFF, 0xB0, 0x30);
const OLIVE: Color32 = Color32::from_rgb(0x5E, 0x6B, 0x3A);
const PANEL: Color32 = Color32::from_rgba_premultiplied(18, 20, 14, 225);
const RED: Color32 = Color32::from_rgb(0xD0, 0x48, 0x40);
const GREEN: Color32 = Color32::from_rgb(0x4C, 0xD0, 0x5C);

#[derive(Resource, Default)]
pub struct UiState {
    pub pause: bool,
    pub chat_open: bool,
    pub chat_text: String,
    pub chat_focus: bool,
    pub admin_open: bool,
    pub cash_amount: String,
    pub rank_pick: u32,
    pub settings: Option<pb::RoomSettings>,
    pub styled: bool,
}

/// 0xRRGGBB of a colour.
pub fn color_u32(c: Color) -> u32 {
    let s = c.to_srgba();
    ((s.red * 255.0) as u32) << 16 | ((s.green * 255.0) as u32) << 8 | (s.blue * 255.0) as u32
}

fn c32(c: Color) -> Color32 {
    let s = c.to_srgba();
    Color32::from_rgb((s.red * 255.0) as u8, (s.green * 255.0) as u8, (s.blue * 255.0) as u8)
}

fn style(ctx: &egui::Context) {
    use egui::{CornerRadius, FontFamily, Stroke, TextStyle};
    let mut v = egui::Visuals::dark();
    let amber_dim = Color32::from_rgba_unmultiplied(0xFF, 0xB0, 0x30, 70);
    v.window_fill = PANEL;
    v.panel_fill = PANEL;
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_stroke = Stroke::new(1.0, amber_dim);
    v.selection.bg_fill = OLIVE;
    v.selection.stroke = Stroke::new(1.0, AMBER);
    v.extreme_bg_color = Color32::from_rgb(12, 14, 10);
    v.hyperlink_color = AMBER;
    let radius = CornerRadius::same(7);
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = radius;
    }
    v.widgets.inactive.bg_fill = Color32::from_rgb(0x34, 0x39, 0x28);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x34, 0x39, 0x28);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x4A, 0x50, 0x38));
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x4E, 0x56, 0x34);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x4E, 0x56, 0x34);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, AMBER);
    v.widgets.active.bg_fill = AMBER;
    v.widgets.active.weak_bg_fill = AMBER;
    v.widgets.active.fg_stroke = Stroke::new(1.5, Color32::BLACK);
    ctx.set_visuals(v);
    ctx.style_mut_of(egui::Theme::Dark, |st| {
        st.spacing.item_spacing = egui::vec2(8.0, 8.0);
        st.spacing.button_padding = egui::vec2(14.0, 6.0);
        st.spacing.window_margin = egui::Margin::same(18);
        st.spacing.interact_size.y = 26.0;
        st.text_styles.insert(TextStyle::Body, egui::FontId::new(15.0, FontFamily::Proportional));
        st.text_styles.insert(TextStyle::Button, egui::FontId::new(15.0, FontFamily::Proportional));
        st.text_styles.insert(TextStyle::Small, egui::FontId::new(12.0, FontFamily::Proportional));
    });
}

pub fn setup_style(mut contexts: EguiContexts, mut ui: ResMut<UiState>, settings: Res<crate::settings::Settings>) -> Result {
    let ctx = contexts.ctx_mut()?;
    if !ui.styled {
        style(ctx);
        ui.styled = true;
    }
    if (ctx.zoom_factor() - settings.ui_scale).abs() > 1e-3 {
        ctx.set_zoom_factor(settings.ui_scale);
    }
    Ok(())
}

/// Wide amber call-to-action button.
pub fn primary_button(ui: &mut egui::Ui, text: &str, width: f32) -> egui::Response {
    ui.add_sized(
        [width, 40.0],
        egui::Button::new(RichText::new(text).size(17.0).strong().color(Color32::from_rgb(0x1A, 0x16, 0x08))).fill(AMBER),
    )
}

/// Wide neutral menu button.
fn menu_button(ui: &mut egui::Ui, text: &str, width: f32) -> egui::Response {
    ui.add_sized([width, 40.0], egui::Button::new(RichText::new(text).size(17.0)))
}

/// Title block shared by the menu and login screens.
fn logo(ui: &mut egui::Ui, big: bool) {
    let (a, b) = if big { (58.0, 36.0) } else { (36.0, 22.0) };
    ui.label(RichText::new("FRONTLINE").size(a).color(AMBER).strong());
    ui.label(RichText::new("F O U N D R Y").size(b).color(Color32::from_rgb(0xE8, 0xE4, 0xD0)));
}

/// Darkens the left side of the screen behind the menu column.
fn menu_backdrop(ctx: &egui::Context) {
    let screen = ctx.content_rect();
    let p = ctx.layer_painter(egui::LayerId::background());
    let w = (screen.width() * 0.42).max(420.0).min(screen.width());
    let panel = egui::Rect::from_min_size(screen.min, egui::vec2(w, screen.height()));
    p.rect_filled(panel, 0.0, Color32::from_black_alpha(170));
    p.rect_filled(
        egui::Rect::from_min_size(egui::pos2(panel.right() - 3.0, screen.top()), egui::vec2(3.0, screen.height())),
        0.0,
        Color32::from_rgba_unmultiplied(0xFF, 0xB0, 0x30, 160),
    );
    p.text(
        egui::pos2(screen.left() + 24.0, screen.bottom() - 18.0),
        Align2::LEFT_BOTTOM,
        format!("v{}", env!("CARGO_PKG_VERSION")),
        FontId::proportional(12.0),
        Color32::from_white_alpha(90),
    );
}

// ---------- main menu ----------

pub fn menu_ui(
    mut contexts: EguiContexts,
    mut login: ResMut<LoginUi>,
    mut net: ResMut<Net>,
    mut next: ResMut<NextState<AppState>>,
    mut settings_ui: ResMut<crate::settings::SettingsUi>,
    mut exit: MessageWriter<AppExit>,
    mut commands: Commands,
    time: Res<Time>,
    mut shot_timer: Local<f32>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    // Dev aid: `FF_MENU_SHOT=path.png` saves the title screen and quits.
    if let Ok(path) = std::env::var("FF_MENU_SHOT") {
        *shot_timer += time.delta_secs();
        if *shot_timer > 3.0 && *shot_timer - time.delta_secs() <= 3.0 {
            commands
                .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
                .observe(bevy::render::view::screenshot::save_to_disk(path));
        }
        if *shot_timer > 5.0 {
            exit.write(AppExit::Success);
        }
    }
    if !login.tried_resume {
        login.tried_resume = true;
        login.saved = SavedSession::load();
        if std::env::var("FF_AUTO").is_ok() {
            next.set(AppState::Login); // scripted runs log in explicitly
            return Ok(());
        }
    }
    menu_backdrop(ctx);
    egui::Area::new(egui::Id::new("main_menu"))
        .anchor(Align2::LEFT_CENTER, [64.0, 0.0])
        .show(ctx, |ui| {
            ui.set_width(300.0);
            logo(ui, true);
            ui.label(RichText::new("build. arm. hold the line.").italics().color(Color32::GRAY));
            ui.add_space(30.0);
            ui.add_enabled_ui(!login.busy, |ui| {
                if primary_button(ui, "Play", 300.0).clicked() {
                    login.error = None;
                    match login.saved.clone().filter(|s| !s.token.is_empty()) {
                        Some(s) => {
                            login.busy = true;
                            net.connect();
                            net.send(C::Resume(pb::Resume { token: s.token }));
                        }
                        None => next.set(AppState::Login),
                    }
                }
                if menu_button(ui, "Settings", 300.0).clicked() {
                    settings_ui.open = true;
                }
                if menu_button(ui, "Quit", 300.0).clicked() {
                    exit.write(AppExit::Success);
                }
            });
            if let Some(saved) = login.saved.clone().filter(|s| !s.token.is_empty()) {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Signed in as {}", saved.username)).color(Color32::GRAY));
                    if !login.busy && ui.small_button("Switch account").clicked() {
                        SavedSession::clear();
                        login.saved = None;
                        net.disconnect();
                        next.set(AppState::Login);
                    }
                });
            }
            if login.busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Signing in…");
                    if ui.small_button("Cancel").clicked() {
                        login.busy = false;
                        net.disconnect();
                    }
                });
            }
            if let Some(e) = &login.error {
                ui.add_space(6.0);
                ui.add(egui::Label::new(RichText::new(e).color(RED)).wrap());
            }
        });
    Ok(())
}

// ---------- login ----------

pub fn login_ui(
    mut contexts: EguiContexts,
    mut login: ResMut<LoginUi>,
    mut net: ResMut<Net>,
    mut next: ResMut<NextState<AppState>>,
    keys: Res<ButtonInput<KeyCode>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    if keys.just_pressed(KeyCode::Escape) && !login.busy {
        login.error = None;
        next.set(AppState::Menu);
        return Ok(());
    }
    menu_backdrop(ctx);
    egui::Area::new(egui::Id::new("login"))
        .anchor(Align2::LEFT_CENTER, [64.0, 0.0])
        .show(ctx, |ui| {
            ui.set_width(300.0);
            logo(ui, false);
            ui.add_space(18.0);
            ui.label(RichText::new("Log in or create an account").size(16.0).color(Color32::from_rgb(0xE8, 0xE4, 0xD0)));
            ui.label(RichText::new("Pick any username and any password.").color(Color32::GRAY).size(13.0));
            ui.add_space(6.0);
            let mut go = false;
            let mut reg = false;
            ui.add_enabled_ui(!login.busy, |ui| {
                ui.label("Username");
                let user = ui.add(
                    egui::TextEdit::singleline(&mut login.username)
                        .desired_width(300.0)
                        .hint_text("username")
                        .margin(egui::vec2(10.0, 8.0)),
                );
                ui.label("Password");
                let show = login.show_password;
                let pass = ui.add(
                    egui::TextEdit::singleline(&mut login.password)
                        .password(!show)
                        .desired_width(300.0)
                        .hint_text("password")
                        .margin(egui::vec2(10.0, 8.0)),
                );
                ui.checkbox(&mut login.show_password, "Show password");
                let enter = (pass.lost_focus() || user.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(6.0);
                go = primary_button(ui, "Log in", 300.0).clicked() || enter;
                reg = menu_button(ui, "Create account", 300.0).clicked();
                if ui.add(egui::Button::new("← Back").frame(false)).clicked() {
                    login.error = None;
                    next.set(AppState::Menu);
                }
            });
            if go || reg {
                login.error = None;
                if login.username.trim().is_empty() {
                    login.error = Some("Enter a username".into());
                } else {
                    login.busy = true;
                    net.connect();
                    let (u, p) = (login.username.trim().to_string(), login.password.clone());
                    if reg {
                        net.send(C::Register(pb::Register { username: u, password: p }));
                    } else {
                        net.send(C::Login(pb::Login { username: u, password: p }));
                    }
                }
            }
            if login.busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Connecting…");
                    if ui.small_button("Cancel").clicked() {
                        login.busy = false;
                        net.disconnect();
                    }
                });
            }
            if let Some(e) = &login.error {
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(e).color(RED)).wrap());
            }
        });
    Ok(())
}

// ---------- lobby ----------

pub fn lobby_ui(
    mut contexts: EguiContexts,
    mut lobby: ResMut<LobbyUi>,
    mut net: ResMut<Net>,
    me: Res<Me>,
    mut next: ResMut<NextState<AppState>>,
    mut login: ResMut<LoginUi>,
    time: Res<Time>,
    mut refresh: Local<f32>,
    mut settings_ui: ResMut<crate::settings::SettingsUi>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    *refresh -= time.delta_secs();
    if *refresh <= 0.0 {
        *refresh = 5.0;
        net.send(C::ListRooms(pb::ListRooms { include_archived: lobby.show_archived }));
    }
    egui::Window::new("lobby")
        .title_bar(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([640.0, 0.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("ROOMS").size(24.0).color(AMBER).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Log out").clicked() {
                        SavedSession::clear();
                        login.saved = None;
                        login.tried_resume = true;
                        net.disconnect();
                        next.set(AppState::Menu);
                    }
                    if ui.button("Settings").clicked() {
                        settings_ui.open = true;
                    }
                    ui.label(RichText::new(format!("signed in as {}", me.name)).color(Color32::GRAY));
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                let mut archived = lobby.show_archived;
                if ui.checkbox(&mut archived, "Show archived").changed() {
                    lobby.show_archived = archived;
                    net.send(C::ListRooms(pb::ListRooms { include_archived: archived }));
                }
                if ui.button("Refresh").clicked() {
                    net.send(C::ListRooms(pb::ListRooms { include_archived: lobby.show_archived }));
                }
                if ui.button(if lobby.create_open { "Cancel" } else { "New room…" }).clicked() {
                    lobby.create_open = !lobby.create_open;
                    if lobby.create_open && lobby.create_settings.is_none() {
                        lobby.create_settings = Some(pb::RoomSettings {
                            max_players: 8,
                            friendly_fire: false,
                            teams: vec![
                                pb::Team { name: "Rust".into(), color: 0xC8642D },
                                pb::Team { name: "Tide".into(), color: 0x2D9C9C },
                            ],
                            cash_multiplier: 1.0,
                            starting_cash: 100,
                            kill_reward: 150,
                            persist_vehicles: false,
                            locked: false,
                        });
                    }
                }
            });
            if lobby.create_open {
                ui.separator();
                let mut create = false;
                let mut name = lobby.create_name.clone();
                if let Some(s) = lobby.create_settings.as_mut() {
                    egui::Grid::new("create_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                        ui.label("Name");
                        ui.add(egui::TextEdit::singleline(&mut name).desired_width(240.0));
                        ui.end_row();
                        ui.label("Max players");
                        ui.add(egui::Slider::new(&mut s.max_players, 2..=8));
                        ui.end_row();
                        ui.label("Cash multiplier");
                        ui.add(egui::Slider::new(&mut s.cash_multiplier, 0.25..=10.0).logarithmic(true));
                        ui.end_row();
                        ui.label("Starting cash");
                        ui.add(egui::DragValue::new(&mut s.starting_cash).range(0..=1_000_000).speed(10.0));
                        ui.end_row();
                        ui.label("Kill reward");
                        ui.add(egui::DragValue::new(&mut s.kill_reward).range(0..=100_000).speed(5.0));
                        ui.end_row();
                        ui.label("Rules");
                        ui.vertical(|ui| {
                            ui.checkbox(&mut s.friendly_fire, "Friendly fire");
                            ui.checkbox(&mut s.persist_vehicles, "Vehicles persist when abandoned");
                            ui.checkbox(&mut s.locked, "Invite only (existing members)");
                        });
                        ui.end_row();
                    });
                    ui.add_space(4.0);
                    if ui.add_enabled(!name.trim().is_empty(), egui::Button::new("Create & enter")).clicked() {
                        create = true;
                    }
                }
                lobby.create_name = name;
                if create {
                    let settings = lobby.create_settings.clone();
                    net.send(C::CreateRoom(pb::CreateRoom { name: lobby.create_name.trim().to_string(), settings }));
                    lobby.busy = true;
                    lobby.create_open = false;
                    lobby.create_name.clear();
                }
            }
            ui.separator();
            let rooms = lobby.rooms.clone();
            if rooms.is_empty() {
                ui.label(RichText::new("No rooms yet. Create one to get started.").color(Color32::GRAY));
            }
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for r in &rooms {
                    egui::Frame::new()
                        .fill(Color32::from_rgba_unmultiplied(255, 255, 255, 10))
                        .corner_radius(8)
                        .inner_margin(egui::Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let title = RichText::new(&r.name).size(16.0).strong();
                        ui.label(if r.archived { title.color(Color32::GRAY) } else { title });
                        ui.label(RichText::new(format!("host {}", r.host_name)).color(Color32::GRAY));
                        ui.label(format!("{}/{}", r.online, r.max_players));
                        if r.member {
                            ui.label(RichText::new("joined").color(GREEN));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if r.archived {
                                if r.host_id == me.id && ui.button("Restore").clicked() {
                                    net.send(C::Admin(pb::AdminCommand {
                                        cmd: Some(pb::admin_command::Cmd::UnarchiveRoom(r.id)),
                                    }));
                                }
                            } else {
                                let label = if r.member { "Continue" } else { "Join" };
                                if ui.add_enabled(!lobby.busy, egui::Button::new(label)).clicked() {
                                    net.send(C::JoinRoom(pb::JoinRoom { room_id: r.id }));
                                    lobby.busy = true;
                                    lobby.error = None;
                                }
                            }
                        });
                    });
                        });
                }
            });
            if let Some(n) = &lobby.notice {
                ui.colored_label(AMBER, n);
            }
            if let Some(e) = &lobby.error {
                ui.colored_label(RED, e);
            }
            if lobby.busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Working…");
                });
            }
        });
    Ok(())
}

// ---------- in game ----------

/// Decides whether the UI owns the mouse (pause/chat/admin open) and handles those hotkeys.
pub fn ui_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut ui: ResMut<UiState>,
    mut controls: ResMut<Controls>,
    room: Res<Room>,
    mut settings_ui: ResMut<crate::settings::SettingsUi>,
    mut gunsmith: ResMut<crate::gunsmith::GunsmithUi>,
    seat: Res<Seat>,
) {
    if keys.just_pressed(KeyCode::KeyG) && !ui.chat_open && !ui.pause && (gunsmith.open || seat.0.is_none()) {
        gunsmith.open = !gunsmith.open;
    }
    if seat.0.is_some() {
        gunsmith.open = false;
    }
    if keys.just_pressed(KeyCode::Escape) {
        if gunsmith.open {
            gunsmith.open = false;
        } else if settings_ui.open {
            settings_ui.open = false;
        } else if ui.chat_open {
            ui.chat_open = false;
        } else if ui.admin_open {
            ui.admin_open = false;
        } else {
            ui.pause = !ui.pause;
        }
    }
    if keys.just_pressed(KeyCode::Enter) && !ui.chat_open && !ui.pause && controls.captured {
        ui.chat_open = true;
        ui.chat_focus = true;
    }
    if keys.just_pressed(KeyCode::F1) && room.is_host() {
        ui.admin_open = !ui.admin_open;
    }
    controls.ui_wants_cursor = ui.pause || ui.chat_open || ui.admin_open || settings_ui.open || gunsmith.open;
}

fn bar(p: &egui::Painter, rect: egui::Rect, frac: f32, fill: Color32) {
    p.rect_filled(rect, 3.0, Color32::from_black_alpha(140));
    let mut r = rect.shrink(2.0);
    r.set_width(r.width() * frac.clamp(0.0, 1.0));
    p.rect_filled(r, 2.0, fill);
}

fn world_to_screen(cam: &Query<(&Camera, &GlobalTransform), With<MainCamera>>, pos: Vec3) -> Option<egui::Pos2> {
    let (c, gt) = cam.single().ok()?;
    let p = c.world_to_viewport(gt, pos).ok()?;
    Some(egui::pos2(p.x, p.y))
}

#[allow(clippy::too_many_arguments)]
pub fn hud(
    mut contexts: EguiContexts,
    time: Res<Time>,
    data: Res<GameData>,
    room: Res<Room>,
    econ: Res<Economy>,
    vitals: Res<Vitals>,
    seat: Res<Seat>,
    loadout: Res<Loadout>,
    (feed, fx, inter, net, ui_state, controls): (Res<Feed>, Res<CombatFx>, Res<Interact>, Res<Net>, Res<UiState>, Res<Controls>),
    (look, armory, gunsmith): (Res<crate::player::Look>, Res<crate::guns::Armory>, Res<crate::gunsmith::GunsmithUi>),
    cam: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    pads: Query<(&BuyPad, &GlobalTransform)>,
    avatars: Query<(&NetObject, &GlobalTransform), With<RemoteAvatar>>,
    vehicles: Query<(&NetObject, &VehicleBody, Option<&Velocity>, &Interp)>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let now = time.elapsed_secs_f64();
    let screen = ctx.content_rect();
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::new("hud")));
    let font = |s: f32| FontId::proportional(s);
    let shadow = |p: &egui::Painter, pos: egui::Pos2, a: Align2, text: String, size: f32, color: Color32| {
        p.text(pos + egui::vec2(1.5, 1.5), a, &text, font(size), Color32::from_black_alpha(200));
        p.text(pos, a, text, font(size), color);
    };

    // Damage flash.
    let hurt = (1.0 - (now - fx.hurt_at) / 0.5).clamp(0.0, 1.0) as f32;
    if hurt > 0.0 {
        p.rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(200, 20, 20, (hurt * 70.0) as u8));
    }

    // Scope, crosshair and hit marker.
    let c = screen.center();
    if look.scoped {
        let r = screen.height() * 0.46;
        let big = screen.width().max(screen.height()) * 2.0;
        p.circle_stroke(c, r + big / 2.0, egui::Stroke::new(big, Color32::BLACK));
        p.circle_stroke(c, r, egui::Stroke::new(10.0, Color32::from_black_alpha(230)));
        p.circle_stroke(c, r - 12.0, egui::Stroke::new(14.0, Color32::from_black_alpha(70)));
        let ink = Color32::from_black_alpha(235);
        p.line_segment([c - egui::vec2(r, 0.0), c - egui::vec2(18.0, 0.0)], egui::Stroke::new(1.5, ink));
        p.line_segment([c + egui::vec2(18.0, 0.0), c + egui::vec2(r, 0.0)], egui::Stroke::new(1.5, ink));
        p.line_segment([c + egui::vec2(0.0, 18.0), c + egui::vec2(0.0, r)], egui::Stroke::new(1.5, ink));
        p.line_segment([c - egui::vec2(0.0, r), c - egui::vec2(0.0, 18.0)], egui::Stroke::new(1.5, ink));
        for (a, b, w) in [(-r, -r * 0.55, 5.0), (r * 0.55, r, 5.0), (r * 0.55, r, 5.0)] {
            p.line_segment([c + egui::vec2(a, 0.0), c + egui::vec2(b, 0.0)], egui::Stroke::new(w, ink));
        }
        p.line_segment([c + egui::vec2(0.0, r * 0.55), c + egui::vec2(0.0, r)], egui::Stroke::new(5.0, ink));
        for i in 1..6 {
            let d = i as f32 * 26.0;
            for (dx, dy) in [(d, 0.0), (-d, 0.0), (0.0, d)] {
                p.circle_filled(c + egui::vec2(dx, dy), 2.2, ink);
            }
        }
        let ret = crate::guns::RETICLES[armory.get(loadout.current_id()).reticle as usize % crate::guns::RETICLES.len()].1;
        let rc = Color32::from_rgb((ret >> 16) as u8, (ret >> 8) as u8, ret as u8);
        p.circle_filled(c, 2.6, rc);
    } else if !gunsmith.open {
        let a = (1.0 - look.ads * 1.6).clamp(0.0, 1.0);
        let cross = Color32::from_white_alpha(((if controls.captured { 220.0 } else { 90.0 }) * a) as u8);
        let gap = 5.0 + (1.0 - look.ads) * 2.0;
        for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            p.line_segment([c + egui::vec2(dx * gap, dy * gap), c + egui::vec2(dx * (gap + 6.0), dy * (gap + 6.0))], egui::Stroke::new(2.0, cross));
        }
    }
    let hit = (1.0 - (now - fx.hit_at) / 0.25).clamp(0.0, 1.0) as f32;
    if hit > 0.0 {
        let col = Color32::from_rgba_unmultiplied(255, 70, 60, (hit * 255.0) as u8);
        for (dx, dy) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
            p.line_segment([c + egui::vec2(dx * 6.0, dy * 6.0), c + egui::vec2(dx * 13.0, dy * 13.0)], egui::Stroke::new(2.5, col));
        }
    }

    // Name tags and buy-pad labels.
    for (obj, gt) in &avatars {
        if obj.health <= 0.0 || obj.extra.get(2).copied().unwrap_or(0.0) > 0.5 {
            continue;
        }
        let pos = gt.translation() + Vec3::Y * 1.35;
        let dist = pos.distance(cam.single().map(|c| c.1.translation()).unwrap_or_default());
        if dist > 90.0 {
            continue;
        }
        if let Some(sp) = world_to_screen(&cam, pos) {
            let team = room.team_of(obj.owner).unwrap_or(0);
            shadow(&p, sp, Align2::CENTER_BOTTOM, room.name_of(obj.owner), 14.0, c32(room.team_color(team)));
        }
    }
    let cam_pos = cam.single().map(|c| c.1.translation()).unwrap_or_default();
    for (pad, gt) in &pads {
        let pos = gt.translation();
        if pos.distance(cam_pos) > 26.0 {
            continue;
        }
        let Some(item) = data.catalog.get(&pad.item) else { continue };
        let Some(sp) = world_to_screen(&cam, pos + Vec3::Y * 1.6) else { continue };
        let locked = econ.rank < item.rank;
        let color = if locked {
            RED
        } else if econ.cash < item.cost {
            AMBER
        } else {
            GREEN
        };
        shadow(&p, sp, Align2::CENTER_BOTTOM, item.name.clone(), 15.0, Color32::WHITE);
        let sub = if locked {
            format!("requires {}", ranks::name(item.rank))
        } else if item.cost == 0 {
            "free".to_string()
        } else {
            format!("${}", item.cost)
        };
        shadow(&p, sp + egui::vec2(0.0, 16.0), Align2::CENTER_BOTTOM, sub, 14.0, color);
    }

    // Top-left economy (the gunsmith panel covers it).
    let x = if gunsmith.open { -1000.0 } else { 16.0 };
    shadow(&p, egui::pos2(x, 14.0), Align2::LEFT_TOP, format!("${}", econ.cash), 28.0, GREEN);
    let income = crate::tycoon::income(&data, &econ, &room);
    shadow(&p, egui::pos2(x, 46.0), Align2::LEFT_TOP, format!("bank ${}   +${:.1}/s", econ.bank as i64, income), 15.0, AMBER);
    let rank_name = ranks::name(econ.rank);
    let (lo, hi) = (ranks::xp_for(econ.rank), ranks::xp_for(econ.rank + 1));
    shadow(&p, egui::pos2(x, 68.0), Align2::LEFT_TOP, format!("{rank_name}   ·   rebirths {}", econ.rebirths), 15.0, Color32::WHITE);
    bar(&p, egui::Rect::from_min_size(egui::pos2(x, 90.0), egui::vec2(200.0, 10.0)), (econ.xp - lo) as f32 / (hi - lo).max(1) as f32, OLIVE);
    if econ.base.owns("hq") {
        shadow(&p, egui::pos2(x, 106.0), Align2::LEFT_TOP, format!("rebirth costs ${}", rebirth_price(econ.rebirths)), 13.0, Color32::GRAY);
    }

    // Health and armor, bottom-left.
    let base = egui::pos2(16.0, screen.bottom() - 52.0);
    bar(&p, egui::Rect::from_min_size(base, egui::vec2(240.0, 18.0)), vitals.hp / vitals.max_hp.max(1.0), RED);
    shadow(&p, base + egui::vec2(120.0, 9.0), Align2::CENTER_CENTER, format!("{:.0}", vitals.hp.max(0.0)), 13.0, Color32::WHITE);
    if vitals.max_armor > 0.0 {
        let a = base + egui::vec2(0.0, 22.0);
        bar(&p, egui::Rect::from_min_size(a, egui::vec2(240.0, 10.0)), vitals.armor / vitals.max_armor, Color32::from_rgb(0x4D, 0xA6, 0xFF));
    }

    // Weapon, bottom-right.
    if let Some((name, ammo, mag, reloading)) = loadout.hud(&data, &seat) {
        let pos = egui::pos2(screen.right() - 20.0, screen.bottom() - 70.0);
        shadow(&p, pos, Align2::RIGHT_TOP, name, 16.0, Color32::WHITE);
        let text = if reloading {
            "reloading…".to_string()
        } else if mag == 0 {
            "∞".to_string()
        } else {
            format!("{ammo} / {mag}")
        };
        shadow(&p, pos + egui::vec2(0.0, 20.0), Align2::RIGHT_TOP, text, 26.0, if reloading { AMBER } else { Color32::WHITE });
        if seat.0.is_none() {
            let slots: Vec<String> = loadout
                .owned()
                .iter()
                .enumerate()
                .map(|(i, id)| format!("{}{}", i + 1, if i == loadout.current { "▪" } else { "" }) + &data.weapon(id).map(|w| format!(" {}", w.name)).unwrap_or_default())
                .collect();
            let _ = slots;
        }
    }
    // Weapon bar, centre bottom.
    if seat.0.is_none() || crate::combat::mounted_weapon(&seat).is_none() {
        let n = loadout.owned().len();
        let w = 96.0;
        let start = screen.center().x - w * n as f32 / 2.0;
        for (i, id) in loadout.owned().iter().enumerate() {
            let r = egui::Rect::from_min_size(egui::pos2(start + w * i as f32 + 2.0, screen.bottom() - 34.0), egui::vec2(w - 4.0, 24.0));
            p.rect_filled(r, 3.0, if i == loadout.current { Color32::from_rgba_unmultiplied(0xFF, 0xB0, 0x30, 150) } else { Color32::from_black_alpha(120) });
            let name = data.weapon(id).map(|w| w.name.clone()).unwrap_or_default();
            p.text(r.center(), Align2::CENTER_CENTER, format!("{} {}", i + 1, name), font(11.0), Color32::WHITE);
        }
    }

    // Vehicle gauges.
    if let Some(s) = seat.0 {
        if let Ok((obj, vb, vel, interp)) = vehicles.get(s.vehicle) {
            let speed = if obj.owner == room.me {
                vel.map(|v| v.linear.length()).unwrap_or(0.0)
            } else {
                interp.0.latest().map(|x| x.vel.length()).unwrap_or(0.0)
            };
            let pos = egui::pos2(screen.center().x, screen.bottom() - 92.0);
            shadow(&p, pos, Align2::CENTER_BOTTOM, format!("{} · {:.0} km/h", obj.def.to_uppercase(), speed * 3.6), 16.0, Color32::WHITE);
            bar(&p, egui::Rect::from_center_size(pos + egui::vec2(0.0, 12.0), egui::vec2(220.0, 10.0)), obj.health / vb.max_health.max(1.0), AMBER);
        }
    }

    // Prompt.
    if let Some((_, label, _)) = &inter.best {
        shadow(&p, egui::pos2(screen.center().x, screen.center().y + 90.0), Align2::CENTER_CENTER, format!("[E] {label}"), 20.0, AMBER);
    }

    // Kill feed and toasts.
    for (i, (t, line)) in feed.kills.iter().rev().filter(|(t, _)| now - t < 8.0).enumerate() {
        let _ = t;
        shadow(&p, egui::pos2(screen.right() - 16.0, 16.0 + i as f32 * 20.0), Align2::RIGHT_TOP, line.clone(), 15.0, Color32::WHITE);
    }
    for (i, (_, text)) in feed.toasts.iter().rev().filter(|(t, _)| now - t < 4.0).enumerate() {
        shadow(&p, egui::pos2(screen.center().x, 70.0 + i as f32 * 24.0), Align2::CENTER_TOP, text.clone(), 18.0, AMBER);
    }

    // Chat log.
    let visible: Vec<_> = feed.chat.iter().rev().take(8).filter(|l| ui_state.chat_open || now - l.at < 14.0).collect();
    for (i, line) in visible.iter().enumerate() {
        let y = screen.bottom() - 130.0 - i as f32 * 18.0;
        if line.system {
            shadow(&p, egui::pos2(16.0, y), Align2::LEFT_BOTTOM, line.text.clone(), 14.0, AMBER);
        } else {
            let from = format!("{}: ", line.from);
            let g = p.layout_no_wrap(from.clone(), font(14.0), Color32::LIGHT_BLUE);
            shadow(&p, egui::pos2(16.0, y), Align2::LEFT_BOTTOM, from, 14.0, Color32::LIGHT_BLUE);
            shadow(&p, egui::pos2(16.0 + g.size().x, y), Align2::LEFT_BOTTOM, line.text.clone(), 14.0, Color32::WHITE);
        }
    }

    // Death screen.
    if let Some(until) = vitals.dead_until {
        p.rect_filled(screen, 0.0, Color32::from_black_alpha(120));
        shadow(&p, screen.center(), Align2::CENTER_CENTER, "ELIMINATED".into(), 44.0, RED);
        shadow(&p, screen.center() + egui::vec2(0.0, 44.0), Align2::CENTER_CENTER, format!("redeploying in {:.0}", (until - now).max(0.0).ceil()), 20.0, Color32::WHITE);
    }

    // Net stats.
    shadow(&p, egui::pos2(screen.right() - 16.0, screen.bottom() - 16.0), Align2::RIGHT_BOTTOM, format!("{:.0} ms  ·  {}", net.rtt_ms, room.summary.name), 12.0, Color32::GRAY);

    // Scoreboard while Tab is held is drawn by `scoreboard`.
    Ok(())
}

pub fn scoreboard(mut contexts: EguiContexts, keys: Res<ButtonInput<KeyCode>>, room: Res<Room>, econ: Res<Economy>) -> Result {
    if !keys.pressed(KeyCode::Tab) {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    egui::Window::new("scores")
        .title_bar(false)
        .resizable(false)
        .anchor(Align2::CENTER_TOP, [0.0, 90.0])
        .interactable(false)
        .show(ctx, |ui| {
            let teams = room.settings.teams.len().max(1);
            for team in 0..teams as u32 {
                ui.label(RichText::new(room.team_name(team)).size(18.0).strong().color(c32(room.team_color(team))));
                egui::Grid::new(("score", team)).striped(true).num_columns(5).min_col_width(80.0).show(ui, |ui| {
                    for h in ["Player", "Rank", "Kills", "Deaths", "Rebirths"] {
                        ui.label(RichText::new(h).color(Color32::GRAY));
                    }
                    ui.end_row();
                    for p in room.players.values().filter(|p| p.team == team && p.online) {
                        let pr = if p.user_id == room.me { econ.progress() } else { p.progress.clone().unwrap_or_default() };
                        let name = if p.host { format!("{} ★", p.username) } else { p.username.clone() };
                        ui.label(name);
                        ui.label(ranks::name(pr.rank));
                        ui.label(pr.kills.to_string());
                        ui.label(pr.deaths.to_string());
                        ui.label(pr.rebirths.to_string());
                        ui.end_row();
                    }
                });
                ui.add_space(6.0);
            }
        });
    Ok(())
}

pub fn chat_input(mut contexts: EguiContexts, mut ui_state: ResMut<UiState>, net: Res<Net>) -> Result {
    if !ui_state.chat_open {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let mut send = false;
    egui::Area::new(egui::Id::new("chat_input")).anchor(Align2::LEFT_BOTTOM, [16.0, -100.0]).show(ctx, |ui| {
        let mut text = std::mem::take(&mut ui_state.chat_text);
        let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(360.0).hint_text("say something…"));
        if ui_state.chat_focus {
            r.request_focus();
            ui_state.chat_focus = false;
        }
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            send = true;
        }
        ui_state.chat_text = text;
    });
    if send {
        let text = ui_state.chat_text.trim().to_string();
        if !text.is_empty() {
            net.send(C::Chat(pb::SendChat { text }));
        }
        ui_state.chat_text.clear();
        ui_state.chat_open = false;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn pause_menu(
    mut contexts: EguiContexts,
    mut ui_state: ResMut<UiState>,
    room: Res<Room>,
    mut econ: ResMut<Economy>,
    net: Res<Net>,
    mut exit: MessageWriter<AppExit>,
    mut settings_ui: ResMut<crate::settings::SettingsUi>,
    mut gunsmith: ResMut<crate::gunsmith::GunsmithUi>,
) -> Result {
    if !ui_state.pause {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    egui::Window::new("pause")
        .title_bar(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([320.0, 0.0])
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(&room.summary.name).size(22.0).color(AMBER).strong());
                ui.label(RichText::new(format!("{} players online", room.players.values().filter(|p| p.online).count())).color(Color32::GRAY));
            });
            ui.add_space(8.0);
            if ui.add_sized([300.0, 28.0], egui::Button::new("Resume")).clicked() {
                ui_state.pause = false;
            }
            ui.add_space(4.0);
            ui.label("Switch team");
            ui.horizontal(|ui| {
                for (i, t) in room.settings.teams.iter().enumerate() {
                    let mine = room.my_team() == i as u32;
                    if ui.add(egui::Button::new(RichText::new(&t.name).color(c32(crate::state::rgb(t.color)))).selected(mine)).clicked() && !mine {
                        net.send(C::ChooseTeam(pb::ChooseTeam { team: i as u32 }));
                    }
                }
            });
            if room.is_host() {
                ui.add_space(4.0);
                if ui.add_sized([300.0, 28.0], egui::Button::new("Admin panel  (F1)")).clicked() {
                    ui_state.admin_open = true;
                    ui_state.pause = false;
                }
            }
            ui.add_space(4.0);
            if ui.add_sized([300.0, 28.0], egui::Button::new("Gunsmith  (G)")).clicked() {
                gunsmith.open = true;
                ui_state.pause = false;
            }
            if ui.add_sized([300.0, 28.0], egui::Button::new("Settings")).clicked() {
                settings_ui.open = true;
            }
            if ui.add_sized([300.0, 28.0], egui::Button::new("Leave room")).clicked() {
                econ.bank = econ.bank.floor();
                let bank = econ.bank as i64;
                econ.cash += bank;
                econ.bank = 0.0;
                request_leave(&net, &econ);
                ui_state.pause = false;
            }
            if ui.add_sized([300.0, 28.0], egui::Button::new("Quit game")).clicked() {
                request_leave(&net, &econ);
                exit.write(AppExit::Success);
            }
            ui.separator();
            ui.label(RichText::new("WASD move · Shift sprint · Space jump · LMB fire · RMB aim down sights · F inspect\n1-9/scroll weapons · R reload · G gunsmith · E interact/enter/exit · Enter chat · Tab scores\nVehicles: WASD drive, Space brake (heli: Space up, Ctrl down), R flip, mouse aims").size(12.0).color(Color32::GRAY));
        });
    Ok(())
}

fn admin(cmd: pb::admin_command::Cmd) -> C {
    C::Admin(pb::AdminCommand { cmd: Some(cmd) })
}

#[allow(clippy::too_many_arguments)]
pub fn admin_panel(
    mut contexts: EguiContexts,
    mut ui_state: ResMut<UiState>,
    room: Res<Room>,
    net: Res<Net>,
    objects: Query<&NetObject, With<VehicleBody>>,
    mut asked: Local<bool>,
) -> Result {
    if !ui_state.admin_open || !room.is_host() {
        *asked = false;
        return Ok(());
    }
    if !*asked {
        *asked = true;
        net.send(C::ListEvents(pb::ListEvents {}));
        ui_state.settings = Some(room.settings.clone());
    }
    let ctx = contexts.ctx_mut()?;
    let mut open = true;
    egui::Window::new("Host admin").open(&mut open).default_width(520.0).show(ctx, |ui| {
        ui.label(RichText::new("Players").strong().color(AMBER));
        let cash_amount = ui_state.cash_amount.clone();
        let mut cash_edit = cash_amount;
        let mut rank_pick = ui_state.rank_pick;
        ui.horizontal(|ui| {
            ui.label("Cash");
            ui.add(egui::TextEdit::singleline(&mut cash_edit).desired_width(90.0));
            ui.label("Rank");
            ui.add(egui::DragValue::new(&mut rank_pick).range(0..=40));
        });
        ui_state.cash_amount = cash_edit.clone();
        ui_state.rank_pick = rank_pick;
        egui::ScrollArea::vertical().max_height(220.0).id_salt("admin_players").show(ui, |ui| {
            for p in room.players.values() {
                ui.horizontal_wrapped(|ui| {
                    let status = if p.banned { " (banned)" } else if !p.online { " (offline)" } else { "" };
                    ui.label(RichText::new(format!("{}{}", p.username, status)).color(c32(room.team_color(p.team))));
                    if p.user_id == room.me {
                        ui.label("(you)");
                    }
                    if p.user_id != room.me {
                        if p.online && ui.small_button("Kick").clicked() {
                            net.send(admin(pb::admin_command::Cmd::Kick(p.user_id)));
                        }
                        if p.banned {
                            if ui.small_button("Unban").clicked() {
                                net.send(admin(pb::admin_command::Cmd::Unban(p.user_id)));
                            }
                        } else if ui.small_button("Ban").clicked() {
                            net.send(admin(pb::admin_command::Cmd::Ban(p.user_id)));
                        }
                        if ui.small_button("Make host").clicked() {
                            net.send(admin(pb::admin_command::Cmd::TransferHost(p.user_id)));
                        }
                    }
                    if ui.small_button("Give cash").clicked() {
                        if let Ok(amount) = cash_edit.trim().parse::<i64>() {
                            net.send(admin(pb::admin_command::Cmd::GiveCash(pb::GiveCash { user_id: p.user_id, amount })));
                        }
                    }
                    if ui.small_button("Set rank").clicked() {
                        net.send(admin(pb::admin_command::Cmd::SetRank(pb::SetRank { user_id: p.user_id, rank: rank_pick })));
                    }
                    if ui.small_button("Switch team").clicked() {
                        let next = (p.team + 1) % room.settings.teams.len().max(1) as u32;
                        net.send(admin(pb::admin_command::Cmd::SetTeam(pb::SetTeam { user_id: p.user_id, team: next })));
                    }
                    if ui.small_button("Wipe base").clicked() {
                        net.send(admin(pb::admin_command::Cmd::ResetPlayer(p.user_id)));
                    }
                });
            }
        });
        ui.separator();
        ui.label(RichText::new("Rules").strong().color(AMBER));
        let mut apply = false;
        if let Some(s) = ui_state.settings.as_mut() {
            ui.horizontal(|ui| {
                ui.checkbox(&mut s.friendly_fire, "Friendly fire");
                ui.checkbox(&mut s.persist_vehicles, "Persist vehicles");
                ui.checkbox(&mut s.locked, "Locked");
            });
            ui.horizontal(|ui| {
                ui.label("Max");
                ui.add(egui::DragValue::new(&mut s.max_players).range(1..=8));
                ui.label("Cash x");
                ui.add(egui::DragValue::new(&mut s.cash_multiplier).range(0.1..=100.0).speed(0.05));
                ui.label("Start $");
                ui.add(egui::DragValue::new(&mut s.starting_cash).range(0..=1_000_000));
                ui.label("Kill $");
                ui.add(egui::DragValue::new(&mut s.kill_reward).range(0..=100_000));
            });
            if ui.button("Apply rules").clicked() {
                apply = true;
            }
        }
        if apply {
            if let Some(s) = ui_state.settings.clone() {
                net.send(admin(pb::admin_command::Cmd::UpdateSettings(s)));
            }
        }
        ui.separator();
        ui.label(RichText::new("Vehicles").strong().color(AMBER));
        ui.horizontal_wrapped(|ui| {
            for o in &objects {
                if ui.small_button(format!("remove {} #{}", o.def, o.id)).clicked() {
                    net.send(admin(pb::admin_command::Cmd::Despawn(o.id)));
                }
            }
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Audit log").strong().color(AMBER));
            if ui.small_button("Refresh").clicked() {
                net.send(C::ListEvents(pb::ListEvents {}));
            }
        });
        egui::ScrollArea::vertical().max_height(140.0).id_salt("admin_log").show(ui, |ui| {
            for e in &room.events {
                ui.label(RichText::new(format!("{}  {}  {}", e.actor_name, e.action, e.detail)).size(12.0).color(Color32::GRAY));
            }
        });
        ui.separator();
        if ui.add(egui::Button::new(RichText::new("Archive this room").color(RED))).clicked() {
            net.send(admin(pb::admin_command::Cmd::Archive(true)));
        }
    });
    if !open {
        ui_state.admin_open = false;
    }
    Ok(())
}
