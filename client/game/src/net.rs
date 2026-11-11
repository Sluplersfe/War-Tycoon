//! TCP (reliable, length-prefixed protobuf) on background threads and a
//! non-blocking UDP socket polled each frame.

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, unbounded};
use frontline_proto::{Message as _, pb};
use serde::{Deserialize, Serialize};

pub const DEFAULT_HOST: &str = "76.179.76.99";
pub const DEFAULT_TCP: u16 = 4002;
pub const DEFAULT_UDP: u16 = 4003;
const MAX_FRAME: usize = 1 << 20;
const LOST: &str = "Lost connection to the game server.";

pub enum NetEvent {
    Connected,
    Disconnected(String),
    Msg(pb::server_msg::Msg),
}

/// A reliable message from the server.
#[derive(Message, Clone)]
pub struct Inbound(pub pb::server_msg::Msg);

/// A realtime packet from the server.
#[derive(Message, Clone)]
pub struct UdpIn(pub pb::UdpPacket);

#[derive(Message, Clone)]
pub struct Disconnected(pub String);

#[derive(Resource)]
pub struct Net {
    pub host: String,
    pub tcp_port: u16,
    pub udp_port: u16,
    tx: Option<Sender<pb::ClientMsg>>,
    rx: Option<Receiver<NetEvent>>,
    pub connected: bool,
    pub connecting: bool,
    udp: Option<UdpSocket>,
    pub conn_id: u32,
    pub udp_key: u64,
    seq: u32,
    pub udp_ready: bool,
    hello_timer: f32,
    pub rtt_ms: f32,
    ping_timer: f32,
}

impl Default for Net {
    fn default() -> Self {
        let host = std::env::var("FF_SERVER").unwrap_or_else(|_| DEFAULT_HOST.to_string());
        Self {
            host,
            tcp_port: DEFAULT_TCP,
            udp_port: DEFAULT_UDP,
            tx: None,
            rx: None,
            connected: false,
            connecting: false,
            udp: None,
            conn_id: 0,
            udp_key: 0,
            seq: 0,
            udp_ready: false,
            hello_timer: 0.0,
            rtt_ms: 0.0,
            ping_timer: 0.0,
        }
    }
}

impl Net {
    pub fn connect(&mut self) {
        if self.connected || self.connecting {
            return;
        }
        self.connecting = true;
        let (ev_tx, ev_rx) = unbounded();
        let (out_tx, out_rx) = unbounded::<pb::ClientMsg>();
        self.rx = Some(ev_rx);
        self.tx = Some(out_tx);
        let addr = format!("{}:{}", self.host, self.tcp_port);
        thread::spawn(move || run_tcp(addr, ev_tx, out_rx));
    }

    pub fn disconnect(&mut self) {
        self.tx = None; // writer thread exits and closes the socket
        self.rx = None;
        self.connected = false;
        self.connecting = false;
        self.leave_udp();
    }

    pub fn send(&self, msg: pb::client_msg::Msg) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(pb::ClientMsg { msg: Some(msg) });
        }
    }

    pub fn bind_udp(&mut self, conn_id: u32, key: u64) {
        self.conn_id = conn_id;
        self.udp_key = key;
        self.udp_ready = false;
        self.hello_timer = 0.0;
        let target: Option<SocketAddr> =
            format!("{}:{}", self.host, self.udp_port).to_socket_addrs().ok().and_then(|mut a| a.next());
        let Some(target) = target else {
            warn!("cannot resolve UDP address");
            return;
        };
        let bind = if target.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
        match UdpSocket::bind(bind).and_then(|s| {
            s.connect(target)?;
            s.set_nonblocking(true)?;
            Ok(s)
        }) {
            Ok(s) => self.udp = Some(s),
            Err(e) => warn!("udp socket: {e}"),
        }
    }

    pub fn leave_udp(&mut self) {
        self.udp = None;
        self.udp_ready = false;
        self.conn_id = 0;
    }

    pub fn send_udp(&mut self, mut pkt: pb::UdpPacket) {
        let Some(sock) = &self.udp else { return };
        self.seq = self.seq.wrapping_add(1);
        pkt.conn_id = self.conn_id;
        pkt.key = self.udp_key;
        pkt.seq = self.seq;
        let _ = sock.send(&pkt.encode_to_vec());
    }
}

fn run_tcp(addr: String, ev: Sender<NetEvent>, out: Receiver<pb::ClientMsg>) {
    let stream = addr
        .to_socket_addrs()
        .map_err(|e| e.to_string())
        .and_then(|mut it| it.next().ok_or_else(|| "no address".to_string()))
        .and_then(|a| TcpStream::connect_timeout(&a, Duration::from_secs(6)).map_err(|e| e.to_string()));
    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            warn!("connect failed: {e}");
            let _ = ev.send(NetEvent::Disconnected("Couldn't reach the game server. Check your connection and try again.".into()));
            return;
        }
    };
    let _ = stream.set_nodelay(true);
    let _ = ev.send(NetEvent::Connected);

    let mut writer = stream.try_clone().expect("clone tcp stream");
    let wthread = thread::spawn(move || {
        for msg in out.iter() {
            let body = msg.encode_to_vec();
            let mut frame = Vec::with_capacity(4 + body.len());
            frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
            frame.extend_from_slice(&body);
            if writer.write_all(&frame).is_err() {
                break;
            }
        }
        let _ = writer.shutdown(std::net::Shutdown::Both);
    });

    let mut reader = stream;
    let reason = loop {
        let mut hdr = [0u8; 4];
        if let Err(e) = reader.read_exact(&mut hdr) {
            warn!("tcp read: {e}");
            break LOST.to_string();
        }
        let n = u32::from_be_bytes(hdr) as usize;
        if n > MAX_FRAME {
            break "Server sent an oversized frame".into();
        }
        let mut body = vec![0u8; n];
        if let Err(e) = reader.read_exact(&mut body) {
            warn!("tcp read: {e}");
            break LOST.to_string();
        }
        match pb::ServerMsg::decode(body.as_slice()) {
            Ok(pb::ServerMsg { msg: Some(m) }) => {
                if ev.send(NetEvent::Msg(m)).is_err() {
                    break String::new();
                }
            }
            Ok(_) => {}
            Err(e) => break format!("Bad message from server: {e}"),
        }
    };
    let _ = ev.send(NetEvent::Disconnected(reason));
    drop(wthread);
}

/// Drains network channels into Bevy messages; keeps UDP alive.
pub fn pump(
    mut net: ResMut<Net>,
    time: Res<Time>,
    mut inbound: MessageWriter<Inbound>,
    mut udp_in: MessageWriter<UdpIn>,
    mut lost: MessageWriter<Disconnected>,
) {
    let mut events = Vec::new();
    if let Some(rx) = &net.rx {
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
    }
    for e in events {
        match e {
            NetEvent::Connected => {
                net.connected = true;
                net.connecting = false;
            }
            NetEvent::Disconnected(reason) => {
                net.disconnect();
                lost.write(Disconnected(reason));
            }
            NetEvent::Msg(pb::server_msg::Msg::Pong(p)) => {
                let now = now_ms();
                let rtt = (now - p.client_time_ms).max(0) as f32;
                net.rtt_ms = if net.rtt_ms == 0.0 { rtt } else { net.rtt_ms * 0.8 + rtt * 0.2 };
            }
            NetEvent::Msg(m) => {
                inbound.write(Inbound(m));
            }
        }
    }

    if net.connected {
        net.ping_timer -= time.delta_secs();
        if net.ping_timer <= 0.0 {
            net.ping_timer = 5.0;
            net.send(pb::client_msg::Msg::Ping(pb::Ping { client_time_ms: now_ms() }));
        }
    }

    let mut buf = [0u8; 2048];
    let mut packets = Vec::new();
    if let Some(sock) = &net.udp {
        loop {
            match sock.recv(&mut buf) {
                Ok(n) => {
                    if let Ok(p) = pb::UdpPacket::decode(&buf[..n]) {
                        packets.push(p);
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }
    for p in packets {
        if p.hello {
            net.udp_ready = true;
        } else {
            udp_in.write(UdpIn(p));
        }
    }
    if net.udp.is_some() && !net.udp_ready {
        net.hello_timer -= time.delta_secs();
        if net.hello_timer <= 0.0 {
            net.hello_timer = 0.5;
            net.send_udp(pb::UdpPacket { hello: true, ..default() });
        }
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------- saved login ----------

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct SavedSession {
    pub username: String,
    pub token: String,
}

pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("frontline-foundry"))
}

fn session_path() -> Option<PathBuf> {
    Some(config_dir()?.join("session.json"))
}

impl SavedSession {
    pub fn load() -> Option<Self> {
        let s = std::fs::read_to_string(session_path()?).ok()?;
        serde_json::from_str(&s).ok()
    }
    pub fn save(&self) {
        let Some(p) = session_path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, serde_json::to_string(self).unwrap_or_default());
    }
    pub fn clear() {
        if let Some(p) = session_path() {
            let _ = std::fs::remove_file(p);
        }
    }
}
