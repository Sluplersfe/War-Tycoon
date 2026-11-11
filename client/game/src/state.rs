//! App states, session/room/economy resources and handling of reliable
//! server messages that are not about objects.

use std::collections::{BTreeMap, HashMap};

use bevy::prelude::*;
use frontline_proto::pb::{self, client_msg::Msg as C, server_msg::Msg as S};
use frontline_sim::{defs, ranks, tycoon, weapons};

use crate::net::{Disconnected, Inbound, Net, SavedSession};

#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    #[default]
    Menu,
    Login,
    Lobby,
    InGame,
}

/// Static game data shared with the server or loaded from assets.
#[derive(Resource)]
pub struct GameData {
    pub map: defs::MapDef,
    pub vehicles: HashMap<String, defs::VehicleDef>,
    pub catalog: tycoon::Catalog,
    pub weapons: Vec<weapons::WeaponDef>,
}

impl GameData {
    pub fn load() -> Self {
        Self {
            map: defs::MapDef::load(),
            vehicles: defs::load_vehicles(),
            catalog: tycoon::Catalog::load(),
            weapons: weapons::load(),
        }
    }
    pub fn weapon(&self, id: &str) -> Option<&weapons::WeaponDef> {
        self.weapons.iter().find(|w| w.id == id)
    }
    pub fn weapon_index(&self, id: &str) -> usize {
        self.weapons.iter().position(|w| w.id == id).unwrap_or(0)
    }
}

#[derive(Resource, Default)]
pub struct Me {
    pub id: u64,
    pub name: String,
}

#[derive(Resource, Default)]
pub struct LoginUi {
    pub username: String,
    pub password: String,
    pub error: Option<String>,
    pub busy: bool,
    pub show_password: bool,
    pub saved: Option<SavedSession>,
    pub tried_resume: bool,
}

#[derive(Resource, Default)]
pub struct LobbyUi {
    pub rooms: Vec<pb::RoomSummary>,
    pub show_archived: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub create_open: bool,
    pub create_name: String,
    pub create_settings: Option<pb::RoomSettings>,
    pub busy: bool,
}

#[derive(Resource)]
pub struct Room {
    pub summary: pb::RoomSummary,
    pub settings: pb::RoomSettings,
    pub players: BTreeMap<u64, pb::PlayerInfo>,
    pub me: u64,
    pub my_plot: usize,
    pub avatar_id: u64,
    pub tick_rate: u32,
    pub host_id: u64,
    pub events: Vec<pb::EventEntry>,
}

impl Room {
    pub fn is_host(&self) -> bool {
        self.host_id == self.me
    }
    pub fn name_of(&self, uid: u64) -> String {
        self.players.get(&uid).map(|p| p.username.clone()).unwrap_or_else(|| format!("#{uid}"))
    }
    pub fn team_of(&self, uid: u64) -> Option<u32> {
        self.players.get(&uid).map(|p| p.team)
    }
    pub fn my_team(&self) -> u32 {
        self.team_of(self.me).unwrap_or(0)
    }
    pub fn team_color(&self, team: u32) -> Color {
        let c = self.settings.teams.get(team as usize).map(|t| t.color).unwrap_or(0x808080);
        rgb(c)
    }
    pub fn team_name(&self, team: u32) -> String {
        self.settings.teams.get(team as usize).map(|t| t.name.clone()).unwrap_or_default()
    }
    pub fn is_enemy(&self, uid: u64) -> bool {
        uid != self.me && (self.settings.friendly_fire || self.team_of(uid) != Some(self.my_team()))
    }
}

pub fn rgb(c: u32) -> Color {
    Color::srgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Objects present when the room was joined; spawned on entering the game.
#[derive(Resource, Default)]
pub struct InitialObjects(pub Vec<pb::ObjectInfo>);

/// The local player's economy. The client is authoritative and saves it to the server.
#[derive(Resource, Default)]
pub struct Economy {
    pub cash: i64,
    pub bank: f64,
    pub xp: i64,
    pub rank: u32,
    pub rebirths: u32,
    pub kills: i64,
    pub deaths: i64,
    pub base: tycoon::BaseState,
    pub dirty: bool,
    pub save_in: f32,
}

impl Economy {
    pub fn from_progress(p: &pb::Progress, catalog: &tycoon::Catalog) -> Self {
        let mut base = tycoon::BaseState::from_json(&p.base_json);
        catalog.sanitize(&mut base);
        Self {
            cash: p.cash,
            bank: 0.0,
            xp: p.xp,
            rank: ranks::apply_xp(p.rank, p.xp),
            rebirths: p.rebirths,
            kills: p.kills,
            deaths: p.deaths,
            base,
            dirty: false,
            save_in: 0.0,
        }
    }

    pub fn progress(&self) -> pb::Progress {
        pb::Progress {
            cash: self.cash,
            xp: self.xp,
            rank: self.rank,
            rebirths: self.rebirths,
            base_json: self.base.to_json(),
            kills: self.kills,
            deaths: self.deaths,
        }
    }

    /// Adds XP; returns the new rank name on promotion.
    pub fn add_xp(&mut self, xp: i64) -> Option<String> {
        self.xp = (self.xp + xp).max(0);
        let new = ranks::apply_xp(self.rank, self.xp);
        self.mark_dirty();
        if new > self.rank {
            self.rank = new;
            return Some(ranks::name(new));
        }
        None
    }

    pub fn mark_dirty(&mut self) {
        if !self.dirty {
            self.dirty = true;
            self.save_in = 3.0;
        }
    }

    pub fn save_now(&mut self) {
        self.dirty = true;
        self.save_in = 0.0;
    }
}

pub struct ChatLine {
    pub from: String,
    pub text: String,
    pub system: bool,
    pub at: f64,
}

#[derive(Resource, Default)]
pub struct Feed {
    pub chat: Vec<ChatLine>,
    pub kills: Vec<(f64, String)>,
    pub toasts: Vec<(f64, String)>,
}

impl Feed {
    pub fn toast(&mut self, now: f64, text: impl Into<String>) {
        self.toasts.push((now, text.into()));
        if self.toasts.len() > 6 {
            self.toasts.remove(0);
        }
    }
    pub fn system(&mut self, now: f64, text: impl Into<String>) {
        self.chat.push(ChatLine { from: String::new(), text: text.into(), system: true, at: now });
    }
}

// ---------- systems ----------

pub fn on_disconnect(
    mut lost: MessageReader<Disconnected>,
    mut next: ResMut<NextState<AppState>>,
    mut login: ResMut<LoginUi>,
    state: Res<State<AppState>>,
) {
    for Disconnected(reason) in lost.read() {
        login.busy = false;
        if !reason.is_empty() {
            login.error = Some(reason.clone());
        }
        if *state.get() != AppState::Login {
            next.set(AppState::Menu);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn handle_session_messages(
    mut inbound: MessageReader<Inbound>,
    mut commands: Commands,
    mut net: ResMut<Net>,
    mut me: ResMut<Me>,
    mut login: ResMut<LoginUi>,
    mut lobby: ResMut<LobbyUi>,
    state: Res<State<AppState>>,
    mut next: ResMut<NextState<AppState>>,
    data: Res<GameData>,
    mut feed: ResMut<Feed>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs_f64();
    for Inbound(msg) in inbound.read() {
        match msg {
            S::AuthOk(a) => {
                me.id = a.user_id;
                me.name = a.username.clone();
                SavedSession { username: a.username.clone(), token: a.token.clone() }.save();
                login.busy = false;
                login.error = None;
                login.password.clear();
                next.set(AppState::Lobby);
                net.send(C::ListRooms(pb::ListRooms { include_archived: lobby.show_archived }));
            }
            S::Error(e) => match state.get() {
                AppState::Menu | AppState::Login => {
                    login.busy = false;
                    if e.code == "resume" {
                        SavedSession::clear();
                        login.saved = None;
                        net.disconnect();
                        login.error = Some("Your session expired. Please log in again.".into());
                        next.set(AppState::Login);
                    } else {
                        login.error = Some(e.message.clone());
                    }
                }
                AppState::Lobby => {
                    lobby.busy = false;
                    lobby.error = Some(e.message.clone());
                }
                AppState::InGame => feed.toast(now, e.message.clone()),
            },
            S::RoomList(l) => {
                lobby.rooms = l.rooms.clone();
            }
            S::RoomJoined(j) => {
                if *state.get() == AppState::InGame {
                    continue; // duplicate join reply; the world is already built
                }
                let you = j.you.clone().unwrap_or_default();
                let mut players: BTreeMap<u64, pb::PlayerInfo> =
                    j.players.iter().map(|p| (p.user_id, p.clone())).collect();
                players.insert(you.user_id, you.clone());
                let summary = j.room.clone().unwrap_or_default();
                commands.insert_resource(Room {
                    host_id: summary.host_id,
                    summary,
                    settings: j.settings.clone().unwrap_or_default(),
                    players,
                    me: me.id,
                    my_plot: you.plot as usize,
                    avatar_id: you.avatar_id,
                    tick_rate: j.tick_rate.max(5),
                    events: Vec::new(),
                });
                commands.insert_resource(Economy::from_progress(&you.progress.unwrap_or_default(), &data.catalog));
                commands.insert_resource(InitialObjects(j.objects.clone()));
                *feed = Feed::default();
                net.bind_udp(j.conn_id, j.udp_key);
                lobby.error = None;
                next.set(AppState::InGame);
            }
            S::RoomLeft(r) => {
                net.leave_udp();
                lobby.busy = false;
                lobby.notice = if r.reason.is_empty() { None } else { Some(r.reason.clone()) };
                if *state.get() == AppState::InGame {
                    next.set(AppState::Lobby);
                }
                net.send(C::ListRooms(pb::ListRooms { include_archived: lobby.show_archived }));
            }
            _ => {}
        }
    }
}

/// In-room reliable messages about players, chat, rewards and settings.
pub fn handle_room_messages(
    mut inbound: MessageReader<Inbound>,
    mut room: ResMut<Room>,
    mut econ: ResMut<Economy>,
    mut feed: ResMut<Feed>,
    data: Res<GameData>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs_f64();
    for Inbound(msg) in inbound.read() {
        match msg {
            S::PlayerJoined(p) => {
                if let Some(p) = &p.player {
                    room.players.insert(p.user_id, p.clone());
                }
            }
            S::PlayerUpdated(p) => {
                if let Some(p) = &p.player {
                    if p.user_id == room.me {
                        // Our own economy is authoritative locally; take only server-owned fields.
                        econ.kills = p.progress.as_ref().map(|x| x.kills).unwrap_or(econ.kills);
                        econ.deaths = p.progress.as_ref().map(|x| x.deaths).unwrap_or(econ.deaths);
                        let mut mine = p.clone();
                        mine.progress = Some(econ.progress());
                        room.players.insert(p.user_id, mine);
                    } else {
                        room.players.insert(p.user_id, p.clone());
                    }
                }
            }
            S::PlayerLeft(p) => {
                if let Some(pl) = room.players.get_mut(&p.user_id) {
                    pl.online = false;
                    pl.avatar_id = 0;
                }
            }
            S::Chat(c) => {
                feed.chat.push(ChatLine { from: c.from_name.clone(), text: c.text.clone(), system: c.system, at: now });
                if feed.chat.len() > 100 {
                    feed.chat.remove(0);
                }
            }
            S::Kill(k) => {
                let victim = room.name_of(k.victim);
                let weapon = data.weapon(&k.weapon).map(|w| w.name.clone()).unwrap_or_else(|| k.weapon.clone());
                let line = if k.killer == 0 || k.killer == k.victim {
                    format!("{victim} died")
                } else {
                    format!("{}  [{}]  {}", room.name_of(k.killer), weapon, victim)
                };
                feed.kills.push((now, line));
                if feed.kills.len() > 6 {
                    feed.kills.remove(0);
                }
            }
            S::Reward(r) => {
                econ.cash = (econ.cash + r.cash).max(0);
                if let Some(rank) = econ.add_xp(r.xp) {
                    feed.toast(now, format!("Promoted to {rank}!"));
                }
                econ.save_now();
                if r.cash != 0 {
                    let sign = if r.cash >= 0 { "+" } else { "-" };
                    feed.toast(now, format!("{sign}${} {}", r.cash.abs(), r.reason));
                } else {
                    feed.toast(now, r.reason.clone());
                }
            }
            S::ProgressOverride(o) => {
                if let Some(p) = &o.progress {
                    let bank = econ.bank;
                    *econ = Economy::from_progress(p, &data.catalog);
                    econ.rank = p.rank; // the host may lower a rank explicitly
                    econ.bank = if p.base_json == "{}" { 0.0 } else { bank };
                }
                feed.toast(now, o.reason.clone());
            }
            S::Settings(s) => {
                if let Some(s) = &s.settings {
                    room.settings = s.clone();
                    room.summary.max_players = s.max_players;
                }
                feed.system(now, "Room settings changed");
            }
            S::HostChanged(h) => {
                let old = room.host_id;
                room.host_id = h.host_id;
                room.summary.host_id = h.host_id;
                if let Some(p) = room.players.get_mut(&old) {
                    p.host = false;
                }
                if let Some(p) = room.players.get_mut(&h.host_id) {
                    p.host = true;
                }
            }
            S::EventLog(l) => room.events = l.events.clone(),
            _ => {}
        }
    }
}

/// Periodically pushes the local economy to the server.
pub fn save_progress(time: Res<Time>, mut econ: ResMut<Economy>, net: Res<Net>, mut room: ResMut<Room>) {
    if !econ.dirty {
        return;
    }
    econ.save_in -= time.delta_secs();
    if econ.save_in > 0.0 {
        return;
    }
    econ.dirty = false;
    let progress = econ.progress();
    let me = room.me;
    if let Some(p) = room.players.get_mut(&me) {
        p.progress = Some(progress.clone());
    }
    net.send(C::SaveProgress(pb::SaveProgress { progress: Some(progress) }));
}

/// Leaves the room on the user's request, saving first.
pub fn request_leave(net: &Net, econ: &Economy) {
    net.send(C::SaveProgress(pb::SaveProgress { progress: Some(econ.progress()) }));
    net.send(C::LeaveRoom(pb::LeaveRoom {}));
}

pub fn leave_game(mut commands: Commands, mut net: ResMut<Net>) {
    net.leave_udp();
    commands.remove_resource::<Room>();
    commands.remove_resource::<Economy>();
    commands.remove_resource::<InitialObjects>();
}
