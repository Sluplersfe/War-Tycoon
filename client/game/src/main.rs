//! Frontline Foundry client.

mod art;
mod autotest;
mod combat;
mod guns;
mod gunsmith;
mod net;
mod objects;
mod player;
mod render;
mod settings;
mod state;
mod tycoon;
mod ui;
mod vehicles;
mod viewmodel;
mod world;

use bevy::prelude::*;
use bevy::window::WindowResolution;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use bevy_rapier3d::plugin::PhysicsSet;
use bevy_rapier3d::prelude::*;

use state::AppState;

fn in_game(state: Res<State<AppState>>) -> bool {
    *state.get() == AppState::InGame
}

fn begin_game(mut commands: Commands) {
    commands.insert_resource(player::Vitals::default());
    commands.insert_resource(player::Look::default());
    commands.insert_resource(player::Seat::default());
    commands.insert_resource(player::Interact::default());
    commands.insert_resource(objects::OutShots::default());
    commands.insert_resource(combat::CombatFx::default());
    commands.insert_resource(ui::UiState { styled: true, ..default() });
    commands.insert_resource(gunsmith::GunsmithUi::default());
}

fn main() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Frontline Foundry".into(),
            resolution: WindowResolution::new(1280, 720),
            ..default()
        }),
        ..default()
    }))
    .add_plugins(RapierPhysicsPlugin::<NoUserData>::default())
    .add_plugins(EguiPlugin::default())
    .init_state::<AppState>();

    app.add_message::<net::Inbound>()
        .add_message::<net::UdpIn>()
        .add_message::<net::Disconnected>()
        .add_message::<objects::Explosion>()
        .add_message::<tycoon::RebirthRequest>()
        .add_message::<tycoon::SpawnVehicleRequest>();

    app.insert_resource(state::GameData::load())
        .init_resource::<net::Net>()
        .init_resource::<art::Art>()
        .init_resource::<state::Me>()
        .init_resource::<state::LoginUi>()
        .init_resource::<state::LobbyUi>()
        .init_resource::<state::Feed>()
        .init_resource::<objects::NetObjects>()
        .init_resource::<objects::OutShots>()
        .insert_resource(objects::SendTimer(Timer::from_seconds(0.05, TimerMode::Repeating)))
        .init_resource::<player::Look>()
        .init_resource::<player::Vitals>()
        .init_resource::<player::Seat>()
        .init_resource::<player::Controls>()
        .init_resource::<player::Interact>()
        .init_resource::<combat::Loadout>()
        .init_resource::<combat::CombatFx>()
        .init_resource::<guns::GunCache>()
        .insert_resource(guns::Armory::load())
        .init_resource::<viewmodel::Vm>()
        .init_resource::<gunsmith::GunsmithUi>()
        .init_resource::<tycoon::Production>()
        .init_resource::<tycoon::PadSig>()
        .init_resource::<tycoon::Cooldowns>()
        .init_resource::<ui::UiState>()
        .init_resource::<settings::SettingsUi>()
        .insert_resource(settings::Settings::load());

    if let Some(auto) = autotest::from_env() {
        app.insert_resource(auto).add_systems(Update, autotest::run);
    }

    app.add_systems(Startup, (art::setup_art, world::setup_camera, render::setup_blit, world::spawn_world).chain());

    app.add_systems(
        Update,
        (
            net::pump,
            state::on_disconnect,
            state::handle_session_messages,
            art::spin_parts,
            settings::apply_settings,
            render::apply_render_scale,
            world::menu_camera.run_if(not(in_state(AppState::InGame))),
        )
            .chain(),
    );

    app.add_systems(
        OnEnter(AppState::InGame),
        (
            begin_game,
            objects::spawn_initial,
            combat::reset_loadout,
            tycoon::reset_state,
        )
            .chain(),
    );
    app.add_systems(OnExit(AppState::InGame), (player::release_cursor, state::leave_game, viewmodel::hide_rig));

    app.add_systems(
        Update,
        (
            (
                player::clear_interact,
                objects::handle_object_messages,
                objects::apply_udp,
                state::handle_room_messages,
                player::attach_local_player,
                objects::sync_body_modes,
                player::track_seat,
                ui::ui_input,
                player::grab_cursor,
                player::mouse_look,
                combat::sync_racks,
                combat::select_weapon,
                combat::tick_weapons,
                player::move_player,
                vehicles::drive,
            )
                .chain(),
            (
                combat::fire,
                combat::update_projectiles,
                combat::remote_shots,
                combat::handle_hits,
            )
                .chain(),
            (
                tycoon::sync_bases,
                tycoon::sync_pads,
                tycoon::pad_colors,
                tycoon::buy_on_pads,
                tycoon::production,
                tycoon::claim_pad,
                tycoon::offer_actions,
                tycoon::turrets,
                tycoon::support_buildings,
                tycoon::handle_requests,
                player::interact,
                player::life_cycle,
            )
                .chain(),
            (
                state::save_progress,
                objects::interpolate,
                objects::avatar_visuals,
                objects::send_owned,
                vehicles::animate_turrets,
                art::animate_soldiers,
                player::vehicle_spinners,
                guns::third_person_guns,
                viewmodel::update_viewmodel,
                viewmodel::update_shells,
                combat::spawn_explosions,
                combat::update_fx,
            )
                .chain(),
        )
            .chain()
            .run_if(in_game),
    );

    app.add_systems(
        PostUpdate,
        (player::follow_seat, player::camera_follow)
            .chain()
            .after(PhysicsSet::Writeback)
            .before(TransformSystems::Propagate)
            .run_if(in_game),
    );

    app.add_systems(
        EguiPrimaryContextPass,
        (
            ui::setup_style,
            ui::menu_ui.run_if(in_state(AppState::Menu)),
            ui::login_ui.run_if(in_state(AppState::Login)),
            ui::lobby_ui.run_if(in_state(AppState::Lobby)),
            (ui::hud, ui::scoreboard, ui::chat_input, ui::pause_menu, ui::admin_panel, gunsmith::gunsmith_ui).run_if(in_game),
            (settings::settings_window, settings::fps_overlay),
        ),
    );

    app.run();
}
