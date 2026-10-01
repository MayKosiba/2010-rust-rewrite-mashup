//! Local Skate gameplay adapter. Rendering and match ownership remain in IW4L.
pub mod collision;
pub mod rails;
pub mod rig;
use bevy::prelude::*;
use frame::{AppScreen, SkateMode};
use skate_host::bridge::{CollisionBuilder, ControllerTransport, InputFrame, Pose, PreparedCollision, Session};
use std::sync::{Arc, Mutex, mpsc};

enum Job {
    Activate(u64, Vec3, f32, f32),
    Step(u64, f32, InputFrame, f32),
    Suspend,
}
enum Reply {
    Ready,
    Activated(u64, Pose, u128),
    Pose(u64, Pose),
    Error(String),
}
#[derive(Resource, Default)]
struct Host {
    send: Option<mpsc::Sender<Job>>,
    receive: Option<Mutex<mpsc::Receiver<Reply>>>,
    clip: Option<Arc<asset_world::ClipCollision>>,
    ready: bool,
    enter_requested: bool,
    activating: bool,
    epoch: u64,
    transport: ControllerTransport,
    previous_buttons: u16,
    input_suspended: bool,
    logged_tick: u64,
}

pub fn register(app: &mut App) {
    app.init_resource::<SkateMode>()
        .init_resource::<Host>()
        .add_systems(Startup, preload_assets)
        .add_systems(
            Update,
            update
                .after(frame::PresentedPublished)
                .before(crate::sync_camera_from_presented)
                .before(render_scene::GfxSceneAdd)
                .in_set(frame::ClientSet::Present),
        );
}

fn preload_assets(mut mode: ResMut<SkateMode>) {
    let Some(root) = std::env::var_os("IW4L_SKATE_ASSETS") else {
        return;
    };
    mode.preload_pending = true;
    if let Err(e) = std::thread::Builder::new()
        .name("skate-preload".into())
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            let start = std::time::Instant::now();
            match Session::preload(std::path::Path::new(&root)) {
                Ok(()) => diag::info!(
                    World,
                    "Skate animation banks preloaded in {}ms",
                    start.elapsed().as_millis()
                ),
                Err(e) => diag::warn!(World, "Skate preload: {e}"),
            }
        })
    {
        diag::warn!(World, "Skate preload thread: {e}");
    }
}

/// Blocks across either way of the skater that a Minecraft world's collision
/// covers, blocks up and down, and how far the skater goes before it is
/// rebuilt around them.
const BLOCK_RADIUS: i32 = 40;
const BLOCK_DEPTH: i32 = 20;
const BLOCK_RECENTRE: f32 = 14.0;

/// A Minecraft world's collision around map point `centre`, for Skate.
fn block_collision(builder: &CollisionBuilder, centre: Vec3) -> Result<(PreparedCollision, usize), String> {
    let triangles: Vec<[[f32; 3]; 3]> = sim::voxel::collision_triangles(centre.to_array(), BLOCK_RADIUS, BLOCK_DEPTH)
        .into_iter()
        .map(|t| t.map(|p| collision::to_skate(Vec3::from_array(p)).to_array()))
        .collect();
    if triangles.is_empty() {
        return Err("no blocks around the skater yet".into());
    }
    let n = triangles.len();
    Ok((builder.build(triangles, Vec::new())?, n))
}

/// One retained session per map. Leaving skating only pauses this worker;
/// collision, decoded animation banks, graphs and the rig remain resident.
fn preload_map(host: &mut Host, clip: Arc<asset_world::ClipCollision>) -> Result<(), String> {
    let root =
        std::env::var_os("IW4L_SKATE_ASSETS").ok_or("IW4L_SKATE_ASSETS is not configured")?;
    rig::reference().ok_or("Skate rig.json could not be loaded")?;
    assets::bot_model::local_skate_board().ok_or("Skate board.json could not be loaded")?;
    let (send, receive) = mpsc::channel();
    let (publish, results) = mpsc::channel();
    let geometry = clip.clone();
    std::thread::Builder::new()
        .name("iw4l-skate".into())
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            let result = (|| -> Result<(), String> {
                let start = std::time::Instant::now();
                let world = collision::extract(&geometry);
                let mut session = Session::new(
                    std::path::Path::new(&root),
                    world.triangles,
                    world.rails,
                    [0., 0., 0.],
                    0.,
                )?;
                diag::info!(
                    World,
                    "Skate map session preloaded in {}ms",
                    start.elapsed().as_millis()
                );
                if publish.send(Reply::Ready).is_err() {
                    return Ok(());
                }
                // On a Minecraft world the collision streams: built around
                // the skater off this thread and swapped in as they move or
                // the blocks change.
                let builder = session.collision_builder();
                let (build_send, build_jobs) = mpsc::channel::<Vec3>();
                let (built_send, built) = mpsc::channel::<(u64, Vec3, Result<(PreparedCollision, usize), String>)>();
                std::thread::Builder::new()
                    .name("iw4l-skate-blocks".into())
                    .spawn(move || {
                        while let Ok(mut centre) = build_jobs.recv() {
                            while let Ok(newer) = build_jobs.try_recv() {
                                centre = newer;
                            }
                            let revision = sim::voxel::revision();
                            let prepared = block_collision(&builder, centre);
                            if built_send.send((revision, centre, prepared)).is_err() {
                                break;
                            }
                        }
                    })
                    .map_err(|e| e.to_string())?;
                let mut blocks: Option<(u64, Vec3)> = None;
                let mut building = false;
                let mut requested = std::time::Instant::now();
                let mut skater_at: Option<Vec3> = None;
                let mut accumulated = 0.;
                let mut epoch = 0;
                while let Ok(job) = receive.recv() {
                    match job {
                        Job::Activate(new_epoch, spawn, yaw, aspect_ratio) => {
                            epoch = new_epoch;
                            accumulated = 0.;
                            let start = std::time::Instant::now();
                            session.set_aspect_ratio(aspect_ratio);
                            if sim::voxel::active() {
                                let revision = sim::voxel::revision();
                                match block_collision(&session.collision_builder(), spawn) {
                                    Ok((prepared, n)) => {
                                        session.install_collision(prepared)?;
                                        blocks = Some((revision, spawn));
                                        diag::info!(World, "Skate: {n} block collision triangles around the spawn");
                                    }
                                    Err(e) => diag::warn!(World, "Skate block collision: {e}"),
                                }
                                skater_at = Some(spawn);
                            }
                            let p = session.activate(
                                collision::to_skate(spawn).to_array(),
                                yaw.to_radians() + std::f32::consts::FRAC_PI_2,
                            )?;
                            if publish
                                .send(Reply::Activated(epoch, p, start.elapsed().as_millis()))
                                .is_err()
                            {
                                break;
                            }
                        }
                        Job::Suspend => {
                            accumulated = 0.;
                            session.suspend_input();
                        }
                        Job::Step(request, dt, input, aspect_ratio) => {
                            if request != epoch {
                                continue;
                            }
                            if let Ok((revision, centre, prepared)) = built.try_recv() {
                                building = false;
                                match prepared {
                                    Ok((prepared, _)) => {
                                        session.install_collision(prepared)?;
                                        blocks = Some((revision, centre));
                                    }
                                    Err(e) => diag::warn!(World, "Skate block collision: {e}"),
                                }
                            }
                            if sim::voxel::active()
                                && !building
                                && let Some(at) = skater_at
                            {
                                let far = blocks.is_none_or(|(_, centre)| {
                                    let d = (at - centre) / sim::voxel::BLOCK;
                                    d.truncate().length() > BLOCK_RECENTRE || d.z.abs() > BLOCK_DEPTH as f32 * 0.5
                                });
                                let changed = blocks.is_some_and(|(revision, _)| revision != sim::voxel::revision())
                                    && requested.elapsed().as_secs_f32() > 0.25;
                                if (far || changed) && build_send.send(at).is_ok() {
                                    building = true;
                                    requested = std::time::Instant::now();
                                }
                            }
                            session.set_aspect_ratio(aspect_ratio);
                            session.collect(input, dt);
                            accumulated = (accumulated + dt).min(0.15);
                            let mut advanced = false;
                            // The native camera can change the simulation period.
                            while accumulated >= session.period() {
                                accumulated -= session.period();
                                session.advance()?;
                                advanced = true;
                            }
                            if advanced {
                                let p = session.pose();
                                if !p.root.is_finite() || p.bones.iter().any(|b| !b.is_finite()) {
                                    return Err("Skate published a non-finite pose".into());
                                }
                                skater_at = Some(collision::from_skate(p.root.w_axis.truncate()));
                                if publish.send(Reply::Pose(epoch, p)).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                }
                Ok(())
            })();
            if let Err(e) = result {
                let _ = publish.send(Reply::Error(e));
            }
        })
        .map_err(|e| e.to_string())?;
    host.send = Some(send);
    host.receive = Some(Mutex::new(results));
    host.clip = Some(clip);
    host.ready = false;
    Ok(())
}

fn stop(host: &mut Host, mode: &mut SkateMode, authority: &mut net::AuthorityWorld) {
    authority
        .0
        .set_external_motion(sim::ClientId(mode.client), false);
    host.enter_requested = false;
    host.activating = false;
    host.epoch = host.epoch.wrapping_add(1);
    if let Some(send) = &host.send {
        let _ = send.send(Job::Suspend);
    }
    mode.active = false;
    mode.entering = false;
    mode.camera = None;
    mode.bones.clear();
    mode.status.clear();
    diag::info!(World, "Skate mode stopped; map session retained");
}

fn present(mode: &mut SkateMode, p: Pose, authority: &mut net::AuthorityWorld) {
    let b = collision::basis();
    let mut root = b * p.root * b.inverse();
    root.w_axis = collision::from_skate(p.root.w_axis.truncate()).extend(1.);
    mode.root = root;
    mode.bones = p.bones;
    mode.names = p.names;
    mode.tick = p.tick;
    mode.status = p.state;
    mode.camera = p.camera.map(|(position, basis, fov)| {
        (
            Transform::from_translation(collision::from_skate(position)).looking_to(
                b.transform_vector3(basis.z_axis).normalize(),
                b.transform_vector3(basis.y_axis).normalize(),
            ),
            fov,
        )
    });
    authority.0.set_origin(
        sim::ClientId(mode.client),
        root.w_axis.truncate().to_array(),
    );
}

fn update(
    time: Res<Time>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    screen: Res<AppScreen>,
    local: Res<net::LocalPresentClient>,
    presented: Res<net::PresentedSnapshot>,
    clip: Res<crate::DynEntPhysClip>,
    mut authority: Option<ResMut<net::AuthorityWorld>>,
    mut mode: ResMut<SkateMode>,
    mut host: ResMut<Host>,
    keys: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    active_pad: Option<Res<frame::ActivePad>>,
) {
    #[cfg(not(windows))]
    {
        let pad = active_pad
            .and_then(|a| a.0)
            .and_then(|e| gamepads.get(e).ok())
            .or_else(|| gamepads.iter().next());
        let typing = mode.input_blocked;
        let pad_state = match mode.script_pad {
            Some(script) => {
                let axis = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0) as i16;
                (script.buttons, script.triggers, script.left.map(axis), script.right.map(axis))
            }
            None => virtual_pad(&keys, pad, typing),
        };
        skate_host::bridge::set_virtual_pad(Some(pad_state));
    }
    #[cfg(windows)]
    let _ = (&keys, &gamepads, &active_pad);
    let Some(authority) = authority.as_deref_mut() else {
        return;
    };
    let aspect_ratio = windows
        .single()
        .map(|w| w.width() / w.height().max(1.))
        .unwrap_or(16. / 9.);
    let ps = presented.player(local.0);
    let alive = ps.is_some_and(|p| p.pm_type == 0) && *screen == AppScreen::InGame;
    let same_map = host
        .clip
        .as_ref()
        .is_none_or(|a| clip.0.as_ref().is_some_and(|b| Arc::ptr_eq(a, b)));
    if (mode.active || host.enter_requested || host.activating) && (!alive || !same_map) {
        stop(&mut host, &mut mode, authority);
    }
    if !same_map {
        host.send = None;
        host.receive = None;
        host.clip = None;
        host.ready = false;
        mode.preloaded = false;
        mode.preload_pending = std::env::var_os("IW4L_SKATE_ASSETS").is_some();
    }
    // This runs during map preparation/class selection, without waiting for J.
    if host.clip.is_none()
        && std::env::var_os("IW4L_SKATE_ASSETS").is_some()
        && let Some(geometry) = clip.0.clone()
    {
        mode.preload_pending = true;
        host.clip = Some(geometry.clone()); // A failed load retries on a new map, never every frame.
        if let Err(e) = preload_map(&mut host, geometry) {
            diag::warn!(World, "Skate map preload: {e}");
            mode.preload_pending = false;
            mode.status = e;
        }
    }
    let input = host.transport.poll();
    mode.controller = input.controller();
    host.previous_buttons = input.buttons();

    let mut replies = Vec::new();
    if let Some(receiver) = &host.receive {
        let receiver = receiver.lock().unwrap();
        loop {
            match receiver.try_recv() {
                Ok(reply) => replies.push(reply),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    replies.push(Reply::Error("Skate worker disconnected".into()));
                    break;
                }
            }
        }
    }
    for reply in replies {
        match reply {
            Reply::Ready => {
                host.ready = true;
                mode.preloaded = true;
                mode.preload_pending = false;
                diag::info!(World, "Skate ready before toggle");
            }
            Reply::Activated(epoch, p, ms) if epoch == host.epoch && host.activating && alive => {
                host.activating = false;
                mode.entering = false;
                mode.active = true;
                host.input_suspended = false;
                authority.0.set_external_motion(local.0, true);
                present(&mut mode, p, authority);
                diag::info!(World, "Skate activation from retained session: {ms}ms");
            }
            Reply::Pose(epoch, p) if epoch == host.epoch && mode.active && !mode.frozen => {
                if let Some(after) = mode.freeze_air {
                    if p.state.contains("Air") {
                        let start = *mode.air_start.get_or_insert(p.tick);
                        if p.tick >= start + after {
                            mode.frozen = true;
                            mode.freeze_air = None;
                            mode.air_start = None;
                            diag::info!(World, "Skate frozen {after} ticks into the air (tick {})", p.tick);
                        }
                    } else {
                        mode.air_start = None;
                    }
                }
                if p.tick / 120 != host.logged_tick / 120 {
                    diag::info!(
                        World,
                        "Skate tick={} speed={:.2} state={}",
                        p.tick,
                        p.velocity.length(),
                        p.state
                    );
                    host.logged_tick = p.tick;
                }
                present(&mut mode, p, authority);
            }
            Reply::Error(e) => {
                stop(&mut host, &mut mode, authority);
                host.send = None;
                host.receive = None;
                host.ready = false;
                mode.preloaded = false;
                mode.preload_pending = false;
                diag::warn!(World, "Skate stopped: {e}");
                mode.status = e;
                return;
            }
            _ => {}
        }
    }
    photo_camera(&mut mode);
    if std::mem::take(&mut mode.toggle_requested) && alive {
        if mode.active || host.enter_requested || host.activating {
            stop(&mut host, &mut mode, authority);
            return;
        }
        if host.send.is_none() {
            diag::warn!(World, "Skate session unavailable: {}", mode.status);
            return;
        }
        host.enter_requested = true;
        mode.entering = true;
        mode.client = local.0.0;
    }
    if host.enter_requested
        && host.ready
        && let Some(ps) = ps.filter(|_| alive)
    {
        host.epoch = host.epoch.wrapping_add(1);
        host.enter_requested = false;
        host.activating = true;
        if let Some(send) = &host.send {
            let _ = send.send(Job::Activate(
                host.epoch,
                Vec3::from_array(ps.origin) + Vec3::Z * 2.,
                ps.viewangles[1],
                aspect_ratio,
            ));
        }
    }
    if !mode.active {
        return;
    }
    if mode.input_blocked {
        if !host.input_suspended {
            if let Some(send) = &host.send {
                let _ = send.send(Job::Suspend);
            }
        }
        host.input_suspended = true;
        return;
    }
    host.input_suspended = false;
    if mode.frozen {
        return;
    }
    if let Some(send) = &host.send {
        if send
            .send(Job::Step(
                host.epoch,
                time.delta_secs().min(0.1),
                input,
                aspect_ratio,
            ))
            .is_err()
        {
            stop(&mut host, &mut mode, authority);
        }
    }
}

/// The photo camera in Skate's camera's place: about the skater, along their
/// way ahead as it was when the camera was placed.
fn photo_camera(mode: &mut SkateMode) {
    let Some(photo) = mode.photo.filter(|_| mode.active) else {
        mode.photo_forward = None;
        return;
    };
    // The board's nose, level, when the camera was placed.
    if mode.photo_forward.is_none() {
        let nose = mode.root.x_axis.truncate();
        mode.photo_forward = Some(Vec3::new(nose.x, nose.y, 0.0).normalize_or(Vec3::X));
    }
    let forward = mode.photo_forward.unwrap_or(Vec3::X);
    let out = Quat::from_rotation_z(photo.angle.to_radians()) * forward;
    let board = mode.root.w_axis.truncate();
    let eye = board + out * photo.distance + Vec3::Z * photo.up;
    let target = board + Vec3::Z * photo.look_up;
    mode.camera = Some((Transform::from_translation(eye).looking_at(target, Vec3::Z), photo.fov));
}

/// Keyboard (and SDL/evdev gamepad) as one XInput pad for the skate engine,
/// which reads only XInput. Keyboard layout:
///   WASD left stick (steer, lean)   arrows right stick (flick-it tricks)
///   Space A (push)   Shift X (brake/powerslide)   F B   R Y
///   Z / C left / right trigger (grabs)   Q / E LB / RB
///   Enter Start   Backspace Back   1-4 D-pad up/down/left/right
#[cfg(not(windows))]
fn virtual_pad(
    keys: &ButtonInput<KeyCode>,
    pad: Option<&Gamepad>,
    blocked: bool,
) -> (u16, [u8; 2], [i16; 2], [i16; 2]) {
    use bevy::input::gamepad::GamepadButton as B;
    let mut buttons = 0u16;
    let mut triggers = [0u8; 2];
    let mut left = Vec2::ZERO;
    let mut right = Vec2::ZERO;
    if let Some(pad) = pad {
        for (button, bit) in [
            (B::DPadUp, 0x0001),
            (B::DPadDown, 0x0002),
            (B::DPadLeft, 0x0004),
            (B::DPadRight, 0x0008),
            (B::Start, 0x0010),
            (B::Select, 0x0020),
            (B::LeftThumb, 0x0040),
            (B::RightThumb, 0x0080),
            (B::LeftTrigger, 0x0100),
            (B::RightTrigger, 0x0200),
            (B::South, 0x1000),
            (B::East, 0x2000),
            (B::West, 0x4000),
            (B::North, 0x8000),
        ] {
            if pad.pressed(button) {
                buttons |= bit;
            }
        }
        let trigger = |b: B| (pad.get(b).unwrap_or(0.0).clamp(0.0, 1.0) * 255.0) as u8;
        triggers = [trigger(B::LeftTrigger2), trigger(B::RightTrigger2)];
        left = pad.left_stick();
        right = pad.right_stick();
    }
    if !blocked {
        let axis = |neg: KeyCode, pos: KeyCode| {
            f32::from(keys.pressed(pos) as u8) - f32::from(keys.pressed(neg) as u8)
        };
        let stick = |v: Vec2| if v == Vec2::ZERO { v } else { v.normalize() };
        let kl = stick(Vec2::new(axis(KeyCode::KeyA, KeyCode::KeyD), axis(KeyCode::KeyS, KeyCode::KeyW)));
        let kr = stick(Vec2::new(
            axis(KeyCode::ArrowLeft, KeyCode::ArrowRight),
            axis(KeyCode::ArrowDown, KeyCode::ArrowUp),
        ));
        if kl != Vec2::ZERO {
            left = kl;
        }
        if kr != Vec2::ZERO {
            right = kr;
        }
        for (key, bit) in [
            (KeyCode::Digit1, 0x0001),
            (KeyCode::Digit2, 0x0002),
            (KeyCode::Digit3, 0x0004),
            (KeyCode::Digit4, 0x0008),
            (KeyCode::Enter, 0x0010),
            (KeyCode::Backspace, 0x0020),
            (KeyCode::KeyQ, 0x0100),
            (KeyCode::KeyE, 0x0200),
            (KeyCode::Space, 0x1000),
            (KeyCode::KeyF, 0x2000),
            (KeyCode::ShiftLeft, 0x4000),
            (KeyCode::KeyR, 0x8000),
        ] {
            if keys.pressed(key) {
                buttons |= bit;
            }
        }
        if keys.pressed(KeyCode::KeyZ) {
            triggers[0] = 255;
        }
        if keys.pressed(KeyCode::KeyC) {
            triggers[1] = 255;
        }
    }
    let axis16 = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0) as i16;
    (
        buttons,
        triggers,
        [axis16(left.x), axis16(left.y)],
        [axis16(right.x), axis16(right.y)],
    )
}
