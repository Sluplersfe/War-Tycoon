//! Automatic reconnect: when the connection drops mid-game, keep the world on
//! screen, retry with backoff, resume the session, rejoin the same room and
//! restore the local economy (the client is authoritative for it).

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use frontline_proto::pb::{self, client_msg::Msg as C, server_msg::Msg as S};

use crate::net::{Inbound, Net, SavedSession};
use crate::state::{AppState, Economy, Feed, LoginUi};

const MAX_ATTEMPTS: u32 = 10;

#[derive(Resource, Default)]
pub struct Reconnect {
    pub active: bool,
    pub room_id: u64,
    pub attempts: u32,
    wait: f32,
    resumed: bool,
    /// The server's reply to our rejoin, replayed once the old world is torn down.
    pub joined: Option<pb::RoomJoined>,
    replayed: bool,
    keep: Option<(pb::Progress, f64)>,
}

impl Reconnect {
    pub fn begin(&mut self, room_id: u64, econ: Option<&Economy>) {
        *self = Self { active: true, room_id, wait: 0.5, ..default() };
        self.keep = econ.map(|e| (e.progress(), e.bank));
    }
    /// Registers a failed attempt; returns false when it's time to give up.
    pub fn failed(&mut self) -> bool {
        self.attempts += 1;
        self.resumed = false;
        self.wait = (1u32 << self.attempts.min(4)) as f32;
        self.attempts < MAX_ATTEMPTS
    }
    pub fn give_up(&mut self) {
        *self = Self::default();
    }
}

/// Connects, resumes the session and asks to rejoin the room.
pub fn drive(mut rc: ResMut<Reconnect>, mut net: ResMut<Net>, time: Res<Time>, mut login: ResMut<LoginUi>, mut next: ResMut<NextState<AppState>>) {
    if !rc.active || rc.joined.is_some() {
        return;
    }
    if !net.connected && !net.connecting {
        rc.wait -= time.delta_secs();
        if rc.wait <= 0.0 {
            rc.wait = 1.0e6; // until the attempt fails
            net.connect();
        }
    }
    if net.connected && !rc.resumed {
        match SavedSession::load().filter(|s| !s.token.is_empty()) {
            Some(s) => {
                rc.resumed = true;
                net.send(C::Resume(pb::Resume { token: s.token }));
            }
            None => {
                rc.give_up();
                net.disconnect();
                login.error = Some("Connection lost.".into());
                next.set(AppState::Menu);
            }
        }
    }
}

/// Once the rejoin reply arrived and the old world is gone, feed it back through
/// the normal join path so the world is rebuilt from the server's state.
pub fn replay_join(mut rc: ResMut<Reconnect>, state: Res<State<AppState>>, mut out: MessageWriter<Inbound>) {
    if rc.active && !rc.replayed && *state.get() == AppState::Lobby {
        if let Some(j) = rc.joined.clone() {
            rc.replayed = true;
            out.write(Inbound(S::RoomJoined(j)));
        }
    }
}

/// Puts our own economy back over what the server saved last.
pub fn restore(
    mut rc: ResMut<Reconnect>,
    state: Res<State<AppState>>,
    econ: Option<ResMut<Economy>>,
    mut feed: ResMut<Feed>,
    time: Res<Time>,
    data: Res<crate::state::GameData>,
) {
    if !(rc.active && rc.replayed && *state.get() == AppState::InGame) {
        return;
    }
    let Some(mut econ) = econ else { return };
    if let Some((p, bank)) = rc.keep.take() {
        let rank = p.rank;
        let kills = (econ.kills, econ.deaths);
        econ.cash = p.cash;
        econ.xp = p.xp;
        econ.rank = rank;
        econ.rebirths = p.rebirths;
        let mut base = frontline_sim::tycoon::BaseState::from_json(&p.base_json);
        data.catalog.sanitize(&mut base);
        econ.base = base;
        econ.bank = bank;
        (econ.kills, econ.deaths) = kills;
        econ.save_now();
    }
    feed.toast(time.elapsed_secs_f64(), "Reconnected");
    rc.give_up();
}

pub fn overlay(mut contexts: EguiContexts, rc: Res<Reconnect>) -> Result {
    if !rc.active {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    egui::Area::new(egui::Id::new("reconnect")).anchor(egui::Align2::CENTER_TOP, [0.0, 60.0]).order(egui::Order::Foreground).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Connection lost. Reconnecting… (attempt {} of {})", rc.attempts + 1, MAX_ATTEMPTS));
            });
        });
    });
    Ok(())
}
