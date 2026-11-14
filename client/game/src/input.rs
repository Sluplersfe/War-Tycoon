//! Rebindable keys and gamepad support. Every system reads `Actions`, which
//! is rebuilt each frame from the keyboard (through the user's bindings), the
//! mouse buttons and any connected gamepad.

use bevy::input::gamepad::{Gamepad, GamepadAxis, GamepadButton};
use bevy::prelude::*;

use crate::settings::Settings;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Act {
    Forward,
    Back,
    Left,
    Right,
    Jump,
    Sprint,
    Descend,
    Reload,
    Interact,
    Inspect,
    Gunsmith,
    Challenges,
    Scoreboard,
    Chat,
}

pub const ACTS: [Act; 14] = [
    Act::Forward,
    Act::Back,
    Act::Left,
    Act::Right,
    Act::Jump,
    Act::Sprint,
    Act::Descend,
    Act::Reload,
    Act::Interact,
    Act::Inspect,
    Act::Gunsmith,
    Act::Challenges,
    Act::Scoreboard,
    Act::Chat,
];

impl Act {
    pub fn id(self) -> &'static str {
        match self {
            Act::Forward => "forward",
            Act::Back => "back",
            Act::Left => "left",
            Act::Right => "right",
            Act::Jump => "jump",
            Act::Sprint => "sprint",
            Act::Descend => "descend",
            Act::Reload => "reload",
            Act::Interact => "interact",
            Act::Inspect => "inspect",
            Act::Gunsmith => "gunsmith",
            Act::Challenges => "challenges",
            Act::Scoreboard => "scoreboard",
            Act::Chat => "chat",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Act::Forward => "Move forward",
            Act::Back => "Move back",
            Act::Left => "Move left",
            Act::Right => "Move right",
            Act::Jump => "Jump / brake / climb",
            Act::Sprint => "Sprint",
            Act::Descend => "Descend (helicopter)",
            Act::Reload => "Reload / flip vehicle",
            Act::Interact => "Interact / enter / exit",
            Act::Inspect => "Inspect weapon (hold)",
            Act::Gunsmith => "Gunsmith",
            Act::Challenges => "Challenges",
            Act::Scoreboard => "Scoreboard (hold)",
            Act::Chat => "Chat",
        }
    }
    pub fn default_key(self) -> KeyCode {
        match self {
            Act::Forward => KeyCode::KeyW,
            Act::Back => KeyCode::KeyS,
            Act::Left => KeyCode::KeyA,
            Act::Right => KeyCode::KeyD,
            Act::Jump => KeyCode::Space,
            Act::Sprint => KeyCode::ShiftLeft,
            Act::Descend => KeyCode::ControlLeft,
            Act::Reload => KeyCode::KeyR,
            Act::Interact => KeyCode::KeyE,
            Act::Inspect => KeyCode::KeyF,
            Act::Gunsmith => KeyCode::KeyG,
            Act::Challenges => KeyCode::KeyJ,
            Act::Scoreboard => KeyCode::Tab,
            Act::Chat => KeyCode::Enter,
        }
    }
}

/// Keys that may be bound (digits are reserved for weapon slots, Esc for the menu).
fn bindable() -> Vec<KeyCode> {
    use KeyCode::*;
    vec![
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS,
        KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ, Space, ShiftLeft, ShiftRight, ControlLeft, ControlRight, AltLeft, Tab,
        Enter, Backquote, CapsLock, ArrowUp, ArrowDown, ArrowLeft, ArrowRight, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11,
        F12, Insert, Delete, Home, End, PageUp, PageDown, Comma, Period, Slash, Semicolon, Quote, BracketLeft, BracketRight,
        Backslash, Minus, Equal, Numpad0, Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7, Numpad8, Numpad9,
    ]
}

pub fn key_id(k: KeyCode) -> String {
    format!("{k:?}")
}

pub fn key_from_id(id: &str) -> Option<KeyCode> {
    bindable().into_iter().find(|k| key_id(*k) == id)
}

/// Readable key name for the settings screen.
pub fn key_label(k: KeyCode) -> String {
    let id = key_id(k);
    match id.as_str() {
        "ShiftLeft" => "Left Shift".into(),
        "ShiftRight" => "Right Shift".into(),
        "ControlLeft" => "Left Ctrl".into(),
        "ControlRight" => "Right Ctrl".into(),
        "AltLeft" => "Left Alt".into(),
        _ => id.strip_prefix("Key").map(str::to_string).unwrap_or(id),
    }
}

pub fn bindable_key(k: KeyCode) -> bool {
    bindable().contains(&k)
}

/// What the player is asking for this frame.
#[derive(Resource, Default, Debug)]
pub struct Actions {
    /// +1 forward.
    pub forward: f32,
    /// +1 strafing left (same sign as the A key).
    pub strafe: f32,
    /// Vertical axis for helicopters (+1 climb).
    pub climb: f32,
    /// Right-stick look, -1..1 per axis (x right, y up).
    pub look: Vec2,
    pub fire_held: bool,
    pub fire_pressed: bool,
    pub aim: bool,
    pub jump_pressed: bool,
    pub jump_held: bool,
    pub sprint: bool,
    pub reload_pressed: bool,
    pub interact_pressed: bool,
    pub inspect: bool,
    pub gunsmith_pressed: bool,
    pub challenges_pressed: bool,
    pub scoreboard: bool,
    pub chat_pressed: bool,
    pub pause_pressed: bool,
    /// +1 next weapon, -1 previous.
    pub weapon_step: i32,
    pub pad_active: bool,
}

fn stick(v: f32) -> f32 {
    const DEAD: f32 = 0.18;
    if v.abs() < DEAD { 0.0 } else { (v.abs() - DEAD) / (1.0 - DEAD) * v.signum() }
}

pub fn gather(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    settings: Res<Settings>,
    pads: Query<&Gamepad>,
    mut out: ResMut<Actions>,
) {
    let k = |a: Act| settings.key(a);
    let held = |a: Act| keys.pressed(k(a));
    let just = |a: Act| keys.just_pressed(k(a));
    let axis = |pos: Act, neg: Act| held(pos) as i32 as f32 - held(neg) as i32 as f32;

    let mut a = Actions {
        forward: axis(Act::Forward, Act::Back),
        strafe: axis(Act::Left, Act::Right),
        climb: axis(Act::Jump, Act::Descend),
        fire_held: mouse.pressed(MouseButton::Left),
        fire_pressed: mouse.just_pressed(MouseButton::Left),
        aim: mouse.pressed(MouseButton::Right),
        jump_pressed: just(Act::Jump),
        jump_held: held(Act::Jump),
        sprint: held(Act::Sprint),
        reload_pressed: just(Act::Reload),
        interact_pressed: just(Act::Interact),
        inspect: held(Act::Inspect),
        gunsmith_pressed: just(Act::Gunsmith),
        challenges_pressed: just(Act::Challenges),
        scoreboard: held(Act::Scoreboard),
        chat_pressed: just(Act::Chat),
        ..default()
    };

    if let Some(pad) = pads.iter().next() {
        use GamepadButton as B;
        let (lx, ly) = (stick(pad.get(GamepadAxis::LeftStickX).unwrap_or(0.0)), stick(pad.get(GamepadAxis::LeftStickY).unwrap_or(0.0)));
        let (rx, ry) = (stick(pad.get(GamepadAxis::RightStickX).unwrap_or(0.0)), stick(pad.get(GamepadAxis::RightStickY).unwrap_or(0.0)));
        let used = lx != 0.0 || ly != 0.0 || rx != 0.0 || ry != 0.0 || pad.get_pressed().next().is_some();
        a.pad_active = used;
        if ly != 0.0 || lx != 0.0 {
            a.forward = ly;
            a.strafe = -lx;
        }
        a.look = Vec2::new(rx, ry);
        let rt = pad.pressed(B::RightTrigger2);
        a.fire_held |= rt;
        a.fire_pressed |= pad.just_pressed(B::RightTrigger2);
        a.aim |= pad.pressed(B::LeftTrigger2);
        a.jump_pressed |= pad.just_pressed(B::South);
        a.jump_held |= pad.pressed(B::South);
        a.sprint |= pad.pressed(B::LeftThumb);
        a.reload_pressed |= pad.just_pressed(B::West);
        a.interact_pressed |= pad.just_pressed(B::North);
        a.inspect |= pad.pressed(B::East);
        a.gunsmith_pressed |= pad.just_pressed(B::DPadUp);
        a.challenges_pressed |= pad.just_pressed(B::DPadRight);
        a.scoreboard |= pad.pressed(B::Select);
        a.chat_pressed |= pad.just_pressed(B::DPadLeft);
        a.pause_pressed = pad.just_pressed(B::Start);
        a.climb = (a.jump_held as i32 as f32 - pad.pressed(B::DPadDown) as i32 as f32).clamp(-1.0, 1.0);
        if pad.just_pressed(B::RightTrigger) {
            a.weapon_step += 1;
        }
        if pad.just_pressed(B::LeftTrigger) {
            a.weapon_step -= 1;
        }
    }
    *out = a;
}

/// While a binding row is waiting for a key, the next press becomes the binding.
pub fn capture_rebind(keys: Res<ButtonInput<KeyCode>>, mut ui: ResMut<crate::settings::SettingsUi>, mut settings: ResMut<Settings>) {
    let Some(act) = ui.rebinding else { return };
    if keys.just_pressed(KeyCode::Escape) {
        ui.rebinding = None;
        return;
    }
    let Some(key) = keys.get_just_pressed().find(|k| bindable_key(**k)).copied() else { return };
    let old = settings.key(act);
    // If another action already uses this key, give it the one being replaced.
    for other in ACTS {
        if other != act && settings.key(other) == key {
            settings.keys.insert(other.id().to_string(), key_id(old));
        }
    }
    settings.keys.insert(act.id().to_string(), key_id(key));
    ui.rebinding = None;
}
