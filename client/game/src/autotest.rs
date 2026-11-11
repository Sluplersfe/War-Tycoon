//! Scripted login/room entry and screenshots for unattended testing.
//!
//! Enabled with `FF_AUTO=user:password`. Optional: `FF_AUTO_ROOM` (room name,
//! default "autotest"), `FF_SHOT=path.png` and `FF_SHOT_AFTER=secs` (seconds
//! after entering the game), `FF_EXIT_AFTER=secs`.
//!
//! Scenario helpers (applied once in game): `FF_AT=secs` delays the placement below, `FF_BUY_ALL=1` buys the whole base,
//! `FF_TELEPORT=x,y,z` (plot-local), `FF_LOOK=yaw_deg,pitch_deg`,
//! `FF_DEPLOY=jeep` spawns a vehicle at its pad, `FF_ENTER=jeep` drives the
//! vehicle once it exists, `FF_KEYS=W,Space` holds keys down, `FF_WORLD_TP=x,y,z`,
//! `FF_LOOKAT=x,y,z`, `FF_WEAPON=index` (or a weapon id), `FF_FIRE=1`, `FF_DEBUG=1` (log vitals),
//! `FF_ADS=1` (hold aim), `FF_INSPECT=1` (hold F), `FF_GUNSMITH=1` (open the gunsmith), `FF_CHASE=metres` (third-person camera),
//! `FF_MODS=rifle:optic,muzzle,under,mag,stock,skin,reticle;smg:...` (attachments, not saved).

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use frontline_proto::pb::{self, client_msg::Msg as C};

use crate::net::Net;
use crate::objects::NetObject;
use crate::combat::Loadout;
use crate::player::{Look, LocalPlayer, Vitals};
use crate::state::{AppState, Economy, GameData, LobbyUi, LoginUi, Room};
use crate::tycoon::SpawnVehicleRequest;

#[derive(Resource)]
pub struct Auto {
    user: String,
    pass: String,
    room: String,
    shot: Option<String>,
    shot_after: f32,
    exit_after: Option<f32>,
    step: u32,
    in_game_for: f32,
    shot_taken: bool,
    buy_timer: f32,
    deployed: bool,
    entered: bool,
    positioned: bool,
}

pub fn from_env() -> Option<Auto> {
    let creds = std::env::var("FF_AUTO").ok()?;
    let (user, pass) = creds.split_once(':')?;
    let f = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<f32>().ok());
    Some(Auto {
        user: user.into(),
        pass: pass.into(),
        room: std::env::var("FF_AUTO_ROOM").unwrap_or_else(|_| "autotest".into()),
        shot: std::env::var("FF_SHOT").ok(),
        shot_after: f("FF_SHOT_AFTER").unwrap_or(6.0),
        exit_after: f("FF_EXIT_AFTER"),
        step: 0,
        in_game_for: 0.0,
        shot_taken: false,
        buy_timer: 0.0,
        deployed: false,
        entered: false,
        positioned: false,
    })
}

pub fn run(
    mut auto: ResMut<Auto>,
    time: Res<Time>,
    state: Res<State<AppState>>,
    mut net: ResMut<Net>,
    mut login: ResMut<LoginUi>,
    mut lobby: ResMut<LobbyUi>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    mut scenario: Scenario,
    mut last: Local<Option<AppState>>,
    mut wait: Local<f32>,
) {
    if *last != Some(*state.get()) {
        info!("autotest: state {:?}", state.get());
        *last = Some(*state.get());
        *wait = 0.0;
    }
    *wait += time.delta_secs();
    match state.get() {
        AppState::Menu => {}
        AppState::Login => {
            if !login.busy && (auto.step == 0 || (auto.step == 1 && login.error.is_some())) {
                login.busy = true;
                login.error = None;
                net.connect();
                let (u, p) = (auto.user.clone(), auto.pass.clone());
                // First run registers; a taken name falls back to login.
                if auto.step == 0 {
                    net.send(C::Register(pb::Register { username: u, password: p }));
                } else {
                    net.send(C::Login(pb::Login { username: u, password: p }));
                }
                auto.step += 1;
            }
        }
        AppState::Lobby => {
            if let Some(e) = &lobby.error {
                warn!("autotest: lobby error: {e}");
            }
            if *wait > 1.0 && !lobby.busy && lobby.error.is_none() {
                lobby.busy = true;
                if let Some(r) = lobby.rooms.iter().find(|r| r.name == auto.room && !r.archived) {
                    net.send(C::JoinRoom(pb::JoinRoom { room_id: r.id }));
                } else {
                    let settings = pb::RoomSettings { max_players: 8, starting_cash: 1_000_000, ..default() };
                    net.send(C::CreateRoom(pb::CreateRoom { name: auto.room.clone(), settings: Some(settings) }));
                }
            }
        }
        AppState::InGame => {
            auto.in_game_for += time.delta_secs();
            scenario.run(&mut auto, time.delta_secs(), &net);
            if let Some(path) = auto.shot.clone() {
                if !auto.shot_taken && auto.in_game_for > auto.shot_after {
                    auto.shot_taken = true;
                    commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
                }
            }
            if auto.exit_after.is_some_and(|t| auto.in_game_for > t) {
                exit.write(AppExit::Success);
            }
        }
    }
}

#[derive(bevy::ecs::system::SystemParam)]
pub struct Scenario<'w, 's> {
    econ: Option<ResMut<'w, Economy>>,
    data: Res<'w, GameData>,
    room: Option<Res<'w, Room>>,
    look: ResMut<'w, Look>,
    vitals: Res<'w, Vitals>,
    loadout: ResMut<'w, Loadout>,
    mouse: ResMut<'w, ButtonInput<MouseButton>>,
    keys: ResMut<'w, ButtonInput<KeyCode>>,
    me: Query<'w, 's, &'static mut Transform, With<LocalPlayer>>,
    objects: Query<'w, 's, &'static NetObject>,
    deploy: MessageWriter<'w, SpawnVehicleRequest>,
    armory: ResMut<'w, crate::guns::Armory>,
    gunsmith: ResMut<'w, crate::gunsmith::GunsmithUi>,
}

fn env_floats(key: &str) -> Option<Vec<f32>> {
    std::env::var(key).ok().map(|v| v.split(',').filter_map(|x| x.trim().parse().ok()).collect())
}

impl Scenario<'_, '_> {
    fn run(&mut self, auto: &mut Auto, dt: f32, net: &Net) {
        let (Some(econ), Some(room)) = (self.econ.as_deref_mut(), self.room.as_deref()) else { return };
        if std::env::var("FF_BUY_ALL").is_ok() {
            econ.rank = econ.rank.max(10);
            auto.buy_timer -= dt;
            if auto.buy_timer <= 0.0 {
                auto.buy_timer = 0.05;
                let next = self.data.catalog.available(&econ.base).map(|i| i.id.clone()).next();
                if let Some(id) = next {
                    let cash = econ.cash.max(10_000_000);
                    econ.cash = cash;
                    let rank = econ.rank;
                    if let Ok(rest) = self.data.catalog.buy(&id, &mut econ.base.clone(), cash, rank) {
                        econ.base.owned.push(id);
                        econ.cash = rest;
                    }
                }
            }
        }
        let at = env_floats("FF_AT").and_then(|v| v.first().copied()).unwrap_or(1.0);
        if std::env::var("FF_DEBUG").is_ok() && (auto.in_game_for * 2.0) as u32 != ((auto.in_game_for - dt) * 2.0) as u32 {
            info!("vitals hp={:.0} armor={:.0} alive={} cash={} bank={:.0} kills={} owned={} rank={}", self.vitals.hp, self.vitals.armor, self.vitals.alive(), econ.cash, econ.bank, econ.kills, econ.base.owned.len(), econ.rank);
        }
        if !auto.positioned && auto.in_game_for > at {
            auto.positioned = true;
            if let Some(t) = env_floats("FF_TELEPORT") {
                if let (Ok(mut tr), [x, y, z]) = (self.me.single_mut(), t.as_slice()) {
                    tr.translation = self.data.map.plot_to_world(room.my_plot, Vec3::new(*x, *y, *z));
                }
            }
            if let Some(t) = env_floats("FF_WORLD_TP") {
                if let (Ok(mut tr), [x, y, z]) = (self.me.single_mut(), t.as_slice()) {
                    tr.translation = Vec3::new(*x, *y, *z);
                }
            }
            if let Some(t) = env_floats("FF_LOOKAT") {
                if let (Ok(tr), [x, y, z]) = (self.me.single(), t.as_slice()) {
                    let d = Vec3::new(*x, *y, *z) - tr.translation;
                    self.look.yaw = d.x.atan2(d.z);
                    self.look.pitch = (d.y / d.xz().length().max(0.1)).atan();
                }
            }
            if let Ok(w) = std::env::var("FF_WEAPON") {
                if let Ok(i) = w.parse::<usize>() {
                    self.loadout.current = i;
                } else if let Some(i) = self.loadout.owned().iter().position(|o| *o == w) {
                    self.loadout.current = i;
                }
            }
            if let Ok(spec) = std::env::var("FF_MODS") {
                for part in spec.split(';') {
                    let Some((w, nums)) = part.split_once(':') else { continue };
                    let n: Vec<u8> = nums.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                    if let [optic, muzzle, under, mag, stock, skin, reticle] = n.as_slice() {
                        let m = crate::guns::GunMods { optic: *optic, muzzle: *muzzle, under: *under, mag: *mag, stock: *stock, skin: *skin, reticle: *reticle };
                        self.armory.set(w.trim(), m);
                    }
                }
            }
            if std::env::var("FF_ADS").is_ok() {
                self.mouse.press(MouseButton::Right);
            }
            if std::env::var("FF_INSPECT").is_ok() {
                self.keys.press(KeyCode::KeyF);
            }
            if let Some(d) = env_floats("FF_CHASE").and_then(|v| v.first().copied()) {
                self.look.chase = d;
            }
            if std::env::var("FF_GUNSMITH").is_ok() {
                self.gunsmith.open = true;
            }
            if std::env::var("FF_FIRE").is_ok() {
                self.mouse.press(MouseButton::Left);
            }
            if let Some(l) = env_floats("FF_LOOK") {
                if let [yaw, pitch] = l.as_slice() {
                    let (_, plot_yaw) = self.data.map.plot(room.my_plot);
                    self.look.yaw = plot_yaw + yaw.to_radians();
                    self.look.pitch = pitch.to_radians();
                }
            }
            if let Ok(keys) = std::env::var("FF_KEYS") {
                for k in keys.split(',') {
                    let code = match k.trim() {
                        "W" => KeyCode::KeyW,
                        "A" => KeyCode::KeyA,
                        "S" => KeyCode::KeyS,
                        "D" => KeyCode::KeyD,
                        "Space" => KeyCode::Space,
                        "Ctrl" => KeyCode::ControlLeft,
                        "Shift" => KeyCode::ShiftLeft,
                        _ => continue,
                    };
                    self.keys.press(code);
                }
            }
        }
        if let Ok(def) = std::env::var("FF_DEPLOY") {
            if !auto.deployed && auto.in_game_for > 3.0 {
                auto.deployed = true;
                if let Some(item) = self
                    .data
                    .catalog
                    .items
                    .iter()
                    .find(|i| matches!(&i.kind, frontline_sim::tycoon::ItemKind::VehiclePad { vehicle } if *vehicle == def))
                {
                    let (_, yaw) = self.data.map.plot(room.my_plot);
                    let pos = self.data.map.plot_to_world(room.my_plot, item.pos());
                    self.deploy.write(SpawnVehicleRequest { def, pos, yaw });
                }
            }
        }
        if let Ok(def) = std::env::var("FF_ENTER") {
            if !auto.entered && auto.in_game_for > 5.0 {
                if let Some(o) = self.objects.iter().find(|o| o.def == def && o.spawned_by == room.me) {
                    auto.entered = true;
                    net.send(frontline_proto::pb::client_msg::Msg::Claim(frontline_proto::pb::ClaimObject { id: o.id, seat: 0 }));
                }
            }
        }
    }
}
