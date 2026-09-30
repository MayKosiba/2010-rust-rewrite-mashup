//! The Minecraft map at run time: MinecraftOSS generates and streams a seeded
//! world around the local player, whose chunks become block collision and
//! whose section meshes go to the renderer. The world sits with its player
//! spawn at map origin, a block to `sim::voxel::BLOCK` map units.
use std::collections::HashMap;
use std::sync::{Arc, mpsc};

use bevy::input::gamepad::GamepadButton;
use bevy::prelude::*;
use minecraft_terrain::clouds::CloudMask;
use minecraft_terrain::day_cycle::{DayCycle, Skybox};
use minecraft_terrain::environment::{DimensionEnvironment, View};
use minecraft_terrain::lighting::SkyLight;
use minecraft_terrain::mesh::{Atlas, SectionMesh, Vertex};
use minecraft_terrain::pack::PackStack;
use minecraft_terrain::scene::HandcraftedScene;
use minecraft_terrain::sections::{CullCamera, SectionPos};
use minecraft_terrain::terrain::{Dimension, TerrainStream};
use minecraftoss_core::BlockStateId;
use minecraftoss_core::registries::{DataPaths, Registries};

const VIEW_DISTANCE: i32 = 8;
const TICK_SECONDS: f64 = 1.0 / 20.0;
/// Blocks on a side of the light volume MW2 models are lit from.
pub const LIGHT_VOLUME: i32 = 64;
/// Chunk sections fade in over this long, as the viewer's default option.
const FADE_MILLIS: u64 = 750;

/// What the renderer takes from the world each frame.
#[derive(Resource, Default)]
pub struct MinecraftWorldView {
    /// A Minecraft map is loaded: the stand-in map's world is not drawn.
    pub active: bool,
    /// Block point at map origin.
    pub origin: [f64; 3],
    pub atlas: Option<Arc<Atlas>>,
    pub uploads: Vec<(SectionPos, SectionMesh)>,
    pub removed: Vec<SectionPos>,
    pub visible: Vec<(SectionPos, f32)>,
    /// Bumped when the world is replaced, so stale sections are dropped.
    pub generation: u64,
    /// The environment uniform of MinecraftOSS for this frame, in block space.
    pub environment: [[f32; 4]; 16],
    /// Sun and the eight moon phases, 32 pixels each, side by side.
    pub celestial: Option<Arc<image::RgbaImage>>,
    pub clouds: Option<Arc<(Vec<Vertex>, Vec<u32>)>>,
    /// Sky and block light around the player: origin block, then
    /// `LIGHT_VOLUME` cubed pairs, x fastest then z then y.
    pub light_volume: Option<Arc<([i32; 3], Vec<u8>)>>,
    /// Sky and block light at the eye, for the view model.
    pub eye_light: [f32; 2],
    /// Break particles as section vertices and indices, rebuilt each frame.
    pub particles: (Vec<u8>, Vec<u32>),
    /// Destroy stage cubes over blocks being mined: position, strip uv.
    pub cracks: (Vec<[f32; 5]>, Vec<u32>),
    /// The ten destroy stages side by side.
    pub crack_texture: Option<Arc<image::RgbaImage>>,
    /// Mob models (cut out, back-face culled, translucent) and entity
    /// shadows, as `mesh::Vertex` bytes and indices.
    /// shadows, and the player's armour in the inventory's window (drawn in
    /// the view model's depth band with the soldier there).
    pub entity_meshes: [(Vec<u8>, Vec<u32>); 5],
    /// The black card behind the inventory's character, in blocks.
    pub backdrop: Option<[[f32; 3]; 4]>,
    /// The first-person hand or held item: section vertex bytes in view
    /// space (x right, y up, z back) and indices, with the projection that
    /// draws them (vanilla's fixed 70 degree hand field of view).
    pub hand: (Vec<u8>, Vec<u32>),
    pub hand_clip: [f32; 16],
}

struct Loaded {
    stream: TerrainStream,
    scene: HandcraftedScene,
    packs: PackStack,
    atlas: Arc<Atlas>,
    registries: Arc<Registries>,
    seed: i64,
    environment: DimensionEnvironment,
    celestial: Arc<image::RgbaImage>,
    cloud_mask: Option<CloudMask>,
    crack_texture: Arc<image::RgbaImage>,
    dimension: Dimension,
}

/// Session world directories in the temp directory: prefix, process id, seed.
const WORLD_DIR_PREFIX: &str = "iw4l-minecraft-";

/// Removes world directories of sessions that ended without cleaning up
/// (the game quit while a Minecraft world was loaded).
fn remove_stale_world_dirs() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(rest) = name.to_str().and_then(|n| n.strip_prefix(WORLD_DIR_PREFIX)) else { continue };
        let Some(pid) = rest.split('-').next().and_then(|p| p.parse::<u32>().ok()) else { continue };
        if pid == std::process::id() {
            continue;
        }
        #[cfg(target_os = "linux")]
        let ended = !std::path::Path::new("/proc").join(pid.to_string()).exists();
        #[cfg(not(target_os = "linux"))]
        let ended = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().is_ok_and(|age| age.as_secs() > 12 * 3600));
        if ended {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Dimensions as block entities are keyed.
fn dimension_index(dimension: Dimension) -> u8 {
    match dimension {
        Dimension::Overworld => 0,
        Dimension::Nether => 1,
        Dimension::End => 2,
    }
}

/// A trip to another dimension, taken at the start of the next frame.
struct Travel {
    to: Dimension,
    /// Through a portal: come out at one (found or built); otherwise at the
    /// dimension's spawn, as a respawn does.
    portal: bool,
}

/// The hand's swing and the timers of mining and placing by hand.
#[derive(Default)]
struct HandState {
    /// Ticks into a swing, while one runs.
    swing: Option<f32>,
    /// Seconds towards the next hand-mining and placing tick.
    clock: f64,
    /// Ticks until another placement while the button is held
    /// (`rightClickDelay`).
    place_delay: u32,
}

/// The player's walk, for vanilla's step and fall sounds.
#[derive(Default)]
struct StepState {
    last: Option<[f64; 3]>,
    /// `Entity.moveDist` and `nextStep`.
    move_dist: f32,
    next_step: f32,
    /// The highest point since leaving the ground.
    air_peak: Option<f64>,
}

#[derive(Default)]
struct Runtime {
    loading: Option<mpsc::Receiver<Result<Loaded, String>>>,
    world: Option<Loaded>,
    day: DayCycle,
    environment_accumulator: f64,
    environment_primed: bool,
    light: Option<SkyLight>,
    light_volume_at: Option<[i32; 3]>,
    light_volume_age: u32,
    cloud_center: Option<(i32, i32)>,
    mining: crate::minecraft_mining::Mining,
    minimap: crate::minecraft_minimap::Minimap,
    hand: HandState,
    steps: StepState,
    sounds: Option<crate::minecraft_sounds::Sounds>,
    inventory_ui: crate::minecraft_inventory::InventoryUi,
    entities: Option<crate::minecraft_entities::Entities>,
    /// Shape id of each block state already seen.
    shapes: HashMap<BlockStateId, u16>,
    /// Boxes of each shape id, to reuse an id for a repeated shape.
    shape_ids: HashMap<Vec<[u32; 6]>, u16>,
    was_alive: bool,
    /// Seconds left to wait, after a world is put in play, for the move to
    /// its spawn to show in the player's presented position.
    settling: f64,
    /// This session's world directory: each dimension's chunks are saved
    /// here when the player leaves it, and loaded from here on return.
    world_dir: Option<std::path::PathBuf>,
    containers: crate::minecraft_containers::Containers,
    portal: crate::minecraft_portal::PortalTimer,
    travel: Option<Travel>,
    /// The player's feet in blocks last frame.
    last_feet: [f64; 3],
    /// Died outside the Overworld: the respawn is back in the Overworld.
    died_away: bool,
    /// The End's dragon fight, kept for the session.
    dragon: crate::minecraft_dragon::Fight,
    /// Doom's bosses the console summoned.
    doom: crate::minecraft_doom::Bosses,
    /// Thrown eyes of ender, and the strongholds they seek.
    eyes: crate::minecraft_eyes::Eyes,
    strongholds: crate::minecraft_eyes::Strongholds,
}

/// The player's MW2 body stands in the inventory's character window, on a
/// black card: placed from this frame's camera and projection where the
/// window shows, facing the camera, turned and aiming towards the mouse,
/// and drawn with the card in the view model's depth band so no wall comes
/// between.
pub(crate) fn place_inventory_puppet(
    ui: Res<frame::MinecraftUi>,
    mut puppet: ResMut<frame::InventoryPuppet>,
    mut view: ResMut<MinecraftWorldView>,
    local: Res<net::LocalPresentClient>,
    presented: Res<net::PresentedSnapshot>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    cameras: Query<&Transform, With<render_scene::FlyCamera>>,
    lenses: Query<&Projection, With<render_scene::FpvLens>>,
) {
    use std::sync::atomic::Ordering::Relaxed;
    puppet.active = false;
    view.backdrop = None;
    render_frame::DEPTH_HACK_SCENE_ENTNUM.store(u32::MAX, Relaxed);
    let alive = presented.player(local.0).is_some_and(|ps| ps.pm_type == 0);
    if !(ui.active && ui.inventory_open && alive) {
        return;
    }
    let (Some([cx, cy, box_w, box_h]), Ok(window), Ok(camera), Ok(Projection::Perspective(lens))) =
        (ui.character_box, windows.single(), cameras.single(), lenses.single())
    else {
        return;
    };
    let (w, h) = (window.width().max(1.0), window.height().max(1.0));
    let tan_v = (lens.fov * 0.5).tan();
    let tan_h = tan_v * if lens.aspect_ratio > 1e-3 { lens.aspect_ratio } else { w / h };
    let (eye, fwd, right, up) = (camera.translation, *camera.forward(), *camera.right(), *camera.up());
    // A point `distance` along the view at a window pixel.
    let at = |px: f32, py: f32, distance: f32| {
        let (nx, ny) = (px / w * 2.0 - 1.0, 1.0 - py / h * 2.0);
        eye + (fwd + right * (nx * tan_h) + up * (ny * tan_v)) * distance
    };
    let distance = 10.0;
    let world_h = box_h / h * 2.0 * tan_v * distance;
    // A standing MW2 player is about 72 units: most of the window.
    let scale = world_h * 0.82 / 72.0;
    let feet = at(cx, cy + box_h * 0.5, distance) + up * (world_h * 0.07);
    // Turned by the mouse as vanilla's
    // `InventoryScreen.renderEntityInInventoryFollowsMouse` turns its body.
    let turn = (ui.gaze[0] * 1.2).atan() * 0.7;
    let x_axis = (-fwd * turn.cos() + right * turn.sin()).normalize_or(-fwd);
    let y_axis = up.cross(x_axis);
    puppet.root = Mat4::from_cols(
        (x_axis * scale).extend(0.0),
        (y_axis * scale).extend(0.0),
        (up * scale).extend(0.0),
        feet.extend(1.0),
    );
    puppet.pitch = (ui.gaze[1] * 1.2).atan().to_degrees() * 0.6;
    puppet.client = local.0.0;
    puppet.active = true;
    render_frame::DEPTH_HACK_SCENE_ENTNUM.store(local.0.0, Relaxed);
    // The card: the window and a margin the panel covers, behind it.
    let corner = |dx: f32, dy: f32| {
        let p = at(cx + dx * box_w * 0.6, cy + dy * box_h * 0.6, distance * 1.4);
        let b = sim::voxel::to_block(view.origin, p.to_array());
        [b[0] as f32, b[1] as f32, b[2] as f32]
    };
    view.backdrop = Some([corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)]);
}

pub(crate) fn register(app: &mut App) {
    app.add_systems(
        Update,
        place_inventory_puppet
            .after(crate::sync_camera_from_presented)
            .after(frame::PresentedPublished)
            .in_set(frame::ClientSet::Present),
    );
    app.init_resource::<MinecraftWorldView>()
        .init_resource::<frame::MinecraftUi>()
        .init_resource::<frame::InventoryPuppet>()
        .insert_non_send(Runtime::default())
        .add_systems(
            Update,
            update
                .after(frame::PresentedPublished)
                .in_set(frame::ClientSet::Present),
        )
        .add_systems(
            Update,
            crate::minecraft_armor::draw_player_armor
                .after(update)
                .after(crate::occupancy::remote_body::pose_remote_bodies),
        );
}

fn load(seed: i64, world_dir: std::path::PathBuf) -> Result<Loaded, String> {
    let root = assets::minecraft_map::root().ok_or_else(assets::minecraft_setup::status)?;
    let paths = DataPaths::under(&root);
    let registries = Arc::new(Registries::load(&paths)?);
    let packs = PackStack::open(vec![root.join("resourcepacks/local/minecraft-26.3")])
        .map_err(|e| e.to_string())?;
    let stream = TerrainStream::for_dimension(
        registries.clone(),
        seed,
        VIEW_DISTANCE,
        Dimension::Overworld,
        Some(&world_dir),
    )
    .map_err(|e| e.to_string())?;
    let build = minecraft_terrain::mesh::build(&HandcraftedScene::default(), &packs)
        .map_err(|e| e.to_string())?;
    let scene = HandcraftedScene::streamed(stream.states.clone());
    let environment =
        DimensionEnvironment::load(&registries, Dimension::Overworld.dimension_type())?;
    let celestial = Arc::new(celestial_image(&packs).map_err(|e| e.to_string())?);
    let cloud_mask = CloudMask::from_pack(&packs).ok();
    let crack_texture = Arc::new(crate::minecraft_mining::crack_strip(&packs).map_err(|e| e.to_string())?);
    Ok(Loaded {
        stream,
        scene,
        packs,
        atlas: build.atlas,
        registries,
        seed,
        environment,
        celestial,
        cloud_mask,
        crack_texture,
        dimension: Dimension::Overworld,
    })
}

/// Leaves `from` for another dimension: saves its chunks, streams `to` from
/// the same world directory, and finds where the player comes out (and the
/// blocks to set there first) around `target`.
fn switch_dimension(
    mut from: Loaded,
    to: Dimension,
    world_dir: &std::path::Path,
    target: Option<(i32, i32, i32)>,
) -> Result<(Loaded, (f64, f64, f64)), String> {
    from.stream.save_all();
    let Loaded { stream, packs, atlas, registries, seed, celestial, cloud_mask, crack_texture, .. } = from;
    // Its workers finish before the next stream opens the same directory.
    drop(stream);
    let mut stream = TerrainStream::for_dimension(registries.clone(), seed, VIEW_DISTANCE, to, Some(world_dir))
        .map_err(|e| e.to_string())?;
    let arrival = match target {
        // The End is always entered on its obsidian platform
        // (`ServerPlayer.createEndPlatform`): five by five, cleared above.
        _ if to == Dimension::End => {
            let (x, y, z) = stream.player_spawn;
            let (bx, mut by, bz) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
            let mut chunks = Vec::new();
            for cx in ((bx - 2) >> 4)..=((bx + 2) >> 4) {
                for cz in ((bz - 2) >> 4)..=((bz + 2) >> 4) {
                    chunks.push(stream.load_now(minecraftoss_core::ChunkPos::new(cx, cz)));
                }
            }
            // Where the island has grown over the platform's height, the
            // platform goes on the island's top rather than in a sealed pocket.
            let solid_at = |x: i32, y: i32, z: i32| {
                chunks.iter().find(|c| c.pos.x == x >> 4 && c.pos.z == z >> 4).is_some_and(|c| {
                    y >= c.min_y() && y < c.min_y() + c.height() && !registries.blocks.is_air(c.block((x & 15) as usize, y, (z & 15) as usize))
                })
            };
            if let Some(top) = (by..by + 80).rev().find(|&h| (-2..=2).any(|dx| (-2..=2).any(|dz| solid_at(bx + dx, h, bz + dz)))) {
                by = top + 1;
            }
            let spawn = (x, f64::from(by), z);
            let obsidian = stream.states.state_of(&minecraft_terrain::scene::Block::new("minecraft:obsidian")).unwrap_or(BlockStateId::AIR);
            let mut edits = Vec::new();
            for dx in -2..=2 {
                for dz in -2..=2 {
                    edits.push((minecraftoss_core::BlockPos::new(bx + dx, by - 1, bz + dz), obsidian));
                    for dy in 0..3 {
                        edits.push((minecraftoss_core::BlockPos::new(bx + dx, by + dy, bz + dz), BlockStateId::AIR));
                    }
                }
            }
            stream.set_blocks(&edits);
            spawn
        }
        Some((x, y, z)) => {
            let range = if to == Dimension::Nether { (32, 118) } else { (-60, 250) };
            let (feet, edits) = crate::minecraft_portal::arrival(&mut stream, &registries, (x, z), y, range, to == Dimension::Nether);
            let states: Vec<_> = edits
                .iter()
                .map(|&((x, y, z), ref block)| {
                    let state = block.as_ref().and_then(|b| stream.states.state_of(b)).unwrap_or(BlockStateId::AIR);
                    (minecraftoss_core::BlockPos::new(x, y, z), state)
                })
                .collect();
            stream.set_blocks(&states);
            feet
        }
        None => stream.player_spawn,
    };
    let scene = HandcraftedScene::streamed(stream.states.clone());
    let environment = DimensionEnvironment::load(&registries, to.dimension_type())?;
    Ok((
        Loaded { stream, scene, packs, atlas, registries, seed, environment, celestial, cloud_mask, crack_texture, dimension: to },
        arrival,
    ))
}

/// The sky's sun and moon phases, laid out as MinecraftOSS lays them out.
fn celestial_image(packs: &PackStack) -> anyhow::Result<image::RgbaImage> {
    let mut celestial = image::RgbaImage::new(32 * 9, 32);
    let names = [
        "environment/celestial/sun",
        "environment/celestial/moon/full_moon",
        "environment/celestial/moon/waning_gibbous",
        "environment/celestial/moon/third_quarter",
        "environment/celestial/moon/waning_crescent",
        "environment/celestial/moon/new_moon",
        "environment/celestial/moon/waxing_crescent",
        "environment/celestial/moon/first_quarter",
        "environment/celestial/moon/waxing_gibbous",
    ];
    for (index, path) in names.iter().enumerate() {
        let id = minecraft_terrain::pack::ResourceId::parse(&format!("minecraft:{path}"))?;
        if let Some(bytes) = packs.texture(&id)? {
            let img =
                image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)?.to_rgba8();
            let tile =
                image::imageops::resize(&img, 32, 32, image::imageops::FilterType::Nearest);
            image::imageops::replace(&mut celestial, &tile, (index as i64) * 32, 0);
        }
    }
    Ok(celestial)
}

/// A controller trigger, held or just pressed: with a block or an empty hand
/// the right trigger mines and the left places, as the mouse buttons do.
fn pad_trigger(pad: Option<&bevy::input::gamepad::Gamepad>, button: GamepadButton, just: bool) -> bool {
    pad.is_some_and(|pad| if just { pad.just_pressed(button) } else { pad.pressed(button) })
}

fn seed() -> i64 {
    if let Some(seed) = std::env::var("IW4L_MINECRAFT_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        return seed;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    (nanos as i64) ^ 0x5DEE_CE66_D1CE_4E5B
}

#[allow(clippy::too_many_arguments)]
fn update(
    time: Res<Time>,
    mut installed: MessageReader<frame::MatchInstalled>,
    mut torn_down: MessageReader<frame::MatchTornDown>,
    local: Res<net::LocalPresentClient>,
    presented: Res<net::PresentedSnapshot>,
    authority: Option<ResMut<net::AuthorityWorld>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    (mut ui, mut puppet, mut images, mut sound_queue, buttons, keys): (
        ResMut<frame::MinecraftUi>,
        ResMut<frame::InventoryPuppet>,
        ResMut<Assets<Image>>,
        ResMut<audio::McSoundQueue>,
        Res<ButtonInput<MouseButton>>,
        Res<ButtonInput<KeyCode>>,
    ),
    mut view: ResMut<MinecraftWorldView>,
    mut runtime: NonSendMut<Runtime>,
    (skate, cameras, gamepads, active_pad): (
        Res<frame::SkateMode>,
        Query<&Transform, With<render_scene::FlyCamera>>,
        Query<&bevy::input::gamepad::Gamepad>,
        Option<Res<frame::ActivePad>>,
    ),
) {
    let pad = active_pad.and_then(|active| active.0).and_then(|entity| gamepads.get(entity).ok());
    for _ in torn_down.read() {
        stop(&mut runtime, &mut view);
    }
    for match_ in installed.read() {
        stop(&mut runtime, &mut view);
        if assets::minecraft_map::is_minecraft(&match_.zone) {
            let seed = seed();
            diag::info!(World, "Minecraft world: seed {seed}");
            remove_stale_world_dirs();
            let dir = std::env::temp_dir().join(format!("{WORLD_DIR_PREFIX}{}-{seed:x}", std::process::id()));
            runtime.world_dir = Some(dir.clone());
            let (send, receive) = mpsc::channel();
            let _ = std::thread::Builder::new()
                .name("minecraft-world-load".into())
                .spawn(move || {
                    let _ = send.send(load(seed, dir));
                });
            runtime.loading = Some(receive);
            view.active = true;
        }
    }
    let Some(mut authority) = authority else {
        return;
    };

    if let Some(receive) = &runtime.loading
        && let Ok(result) = receive.try_recv()
    {
        runtime.loading = None;
        match result {
            Ok(world) => {
                runtime.day = DayCycle::default();
                // Game ticks since sunrise to start at: 6000 noon, 13000
                // dusk, 18000 midnight.
                if let Some(ticks) = std::env::var("IW4L_MINECRAFT_TIME")
                    .ok()
                    .and_then(|t| t.trim().parse::<f64>().ok())
                {
                    runtime.day.set(ticks);
                }
                let spawn = world.stream.player_spawn;
                install(&mut runtime, &mut view, world, spawn, None);
                sim::voxel::activate(authority.0.content().clip_brushes(), view.origin, vec![Vec::new()]);
            }
            Err(error) => {
                diag::warn!(World, "Minecraft world failed to load: {error}");
                view.active = false;
            }
        }
    }

    // The console's `dimension` command: straight to that dimension's spawn.
    if let Some(to) = ui.travel_request.take() {
        let to = match to.as_str() {
            "nether" => Dimension::Nether,
            "end" => Dimension::End,
            _ => Dimension::Overworld,
        };
        runtime.travel = Some(Travel { to, portal: false });
    }

    // A trip to another dimension: the world is replaced, and the player
    // keeps what they carry and comes out on the far side.
    if let Some(travel) = runtime.travel.take()
        && let Some(dir) = runtime.world_dir.clone()
        && let Some(from) = runtime.world.take()
    {
        let here = from.dimension;
        let target = travel.portal.then(|| {
            let [x, y, z] = runtime.last_feet;
            // The Nether is an eighth the Overworld's size across.
            let scale = match (here, travel.to) {
                (Dimension::Overworld, Dimension::Nether) => 0.125,
                (Dimension::Nether, Dimension::Overworld) => 8.0,
                _ => 1.0,
            };
            ((x * scale).floor() as i32, y.floor() as i32, (z * scale).floor() as i32)
        });
        let keep = runtime.entities.take().map(|mut e| (std::mem::take(&mut e.inventory), e.selected));
        match switch_dimension(from, travel.to, &dir, target) {
            Ok((world, feet)) => {
                diag::info!(World, "Minecraft travel: {here:?} to {:?}, arriving at {feet:?}", travel.to);
                install(&mut runtime, &mut view, world, feet, keep);
                sim::voxel::activate(authority.0.content().clip_brushes(), view.origin, vec![Vec::new()]);
                runtime.portal.cooldown = travel.portal;
            }
            Err(error) => {
                diag::warn!(World, "Minecraft travel failed: {error}");
                stop(&mut runtime, &mut view);
                return;
            }
        }
    }

    let origin = view.origin;
    let Runtime {
        world,
        shapes,
        shape_ids,
        was_alive,
        settling,
        day,
        environment_accumulator,
        environment_primed,
        light,
        light_volume_at,
        light_volume_age,
        cloud_center,
        mining,
        entities,
        inventory_ui,
        sounds,
        hand,
        minimap,
        steps,
        containers,
        portal,
        travel,
        last_feet,
        died_away,
        dragon,
        doom,
        eyes,
        strongholds,
        ..
    } = &mut *runtime;
    let Some(world) = world.as_mut() else {
        ui.active = false;
        ui.inventory_open = false;
        ui.screen = frame::McScreen::Inventory;
        puppet.active = false;
        return;
    };
    let Some(ps) = presented.player(local.0) else {
        ui.active = false;
        puppet.active = false;
        return;
    };

    // Every spawn lands on the Minecraft spawn once its ground exists.
    let alive = ps.pm_type == 0;
    let spawn_chunk = ((origin[0].floor() as i32) >> 4, (origin[2].floor() as i32) >> 4);
    if !alive && world.dimension != Dimension::Overworld {
        *died_away = true;
    }
    if alive && *died_away {
        // A respawn is at the Overworld's spawn, wherever the player died.
        *died_away = false;
        *travel = Some(Travel { to: Dimension::Overworld, portal: false });
        return;
    }
    if alive && !*was_alive {
        // Held at the spawn, stopped, each frame until its ground exists
        // (left where they stood before, over another dimension's void after
        // travel, they would fall meanwhile), then moved there once as a
        // teleport: its flag flips each time, so only one may be sent.
        if world.scene.generated_chunk(spawn_chunk).is_none() {
            authority.0.set_origin(local.0, [0.0, 0.0, 0.0]);
        } else if authority.0.teleport(local.0, [0.0, 0.0, 0.0]) {
            diag::info!(World, "Minecraft spawn: moved to the world spawn");
            *was_alive = true;
        }
    } else if !alive {
        *was_alive = false;
    }

    // Until the move to the spawn shows, the presented position is still
    // where the player stood before (in another dimension, after travel):
    // the world streams round its spawn meanwhile, or the chunks there (a
    // new End's platform among them) would drop from under the arrival.
    if *settling > 0.0 {
        let [x, y, z] = ps.origin;
        let arrived = *was_alive && x.hypot(y) < 72.0 && z.abs() < 144.0;
        *settling = if arrived { 0.0 } else { *settling - time.delta_secs_f64() };
    }
    let feet = if *settling > 0.0 { origin } else { sim::voxel::to_block(origin, ps.origin) };
    let block = (
        feet[0].floor() as i32,
        feet[1].floor() as i32,
        feet[2].floor() as i32,
    );
    let (loaded, forgotten) = world.stream.server_tick(block, &mut world.scene);
    for chunk in loaded {
        let blocks = &world.registries.blocks;
        let (min_y, height) = (chunk.min_y(), chunk.height());
        let mut ids = vec![0u16; (height * 256) as usize];
        for y in 0..height {
            for z in 0..16usize {
                for x in 0..16usize {
                    let state = chunk.block(x, min_y + y, z);
                    let id = *shapes.entry(state).or_insert_with(|| {
                        let boxes = blocks.collision_boxes(state);
                        if boxes.is_empty() {
                            return 0;
                        }
                        let key: Vec<[u32; 6]> =
                            boxes.iter().map(|b| b.map(|v| (v as f32).to_bits())).collect();
                        if let Some(&id) = shape_ids.get(&key) {
                            return id;
                        }
                        let boxes32 = boxes.iter().map(|b| b.map(|v| v as f32)).collect();
                        let id = sim::voxel::add_shapes(vec![boxes32]).unwrap_or(0);
                        shape_ids.insert(key, id);
                        id
                    });
                    ids[((y as usize * 16) + z) * 16 + x] = id;
                }
            }
        }
        if let Some(entities) = entities.as_mut() {
            entities.load_chunk(&chunk);
        }
        sim::voxel::set_chunk(
            chunk.pos.x,
            chunk.pos.z,
            sim::voxel::VoxelChunk {
                min_y,
                height,
                shapes: ids,
            },
        );
    }
    for pos in forgotten {
        sim::voxel::remove_chunk(pos.x, pos.z);
        if let Some(entities) = entities.as_mut() {
            entities.unload_chunk(pos);
        }
    }

    // Minecraft's yaw: 0 facing +Z (map -Y), turning towards -X.
    let yaw_rad = ps.viewangles[1].to_radians();
    let mc_yaw = (-yaw_rad.cos()).atan2(-yaw_rad.sin()).to_degrees();
    let mut all_events = sim::voxel::take_events();

    // The hand, when no gun is selected: vanilla's left click mines by hand
    // (with the hand's break speed) or punches, its right click places the
    // held block (`Player.place_selected`, vanilla's placement states).
    // Footsteps (`Entity.applyMovementEmissionAndPlaySound`): the walked
    // distance grows by 0.6 of each horizontal move on the ground, and past
    // the next step the block under the feet sounds its step at 0.15 of its
    // volume. A fall of more than three blocks lands with the block's fall
    // sound and the player's (`LivingEntity.causeFallDamage`).
    let on_ground = alive && ps.ground_entity_num != playerstate_iw4::ENTITYNUM_NONE;
    if let Some(last) = steps.last.filter(|_| alive) {
        let horizontal = ((feet[0] - last[0]).hypot(feet[2] - last[2]) * 0.6) as f32;
        let block_at = |dy: f64| {
            let pos = (feet[0].floor() as i32, (feet[1] - dy).floor() as i32, feet[2].floor() as i32);
            minecraft_terrain::scene::Scene::block(&world.scene, pos).cloned().map(|b| (pos, b))
        };
        let under = block_at(0.2);
        let centre = |pos: (i32, i32, i32)| {
            Vec3::from_array(sim::voxel::to_map(origin, [pos.0 as f64 + 0.5, pos.1 as f64 + 1.0, pos.2 as f64 + 0.5]))
        };
        if on_ground && horizontal < 2.0 {
            steps.move_dist += horizontal;
            if steps.move_dist > steps.next_step
                && let Some((pos, block)) = under.as_ref()
            {
                steps.next_step = steps.move_dist as i32 as f32 + 1.0;
                // Snow layers and carpets sound instead of what they lie on.
                let inside = block_at(-0.01).filter(|(_, b)| {
                    let p = b.id.path.as_str();
                    p == "snow" || p.ends_with("_carpet") || p == "moss_carpet"
                });
                let (pos, block) = inside.as_ref().map_or((*pos, block), |(p, b)| (*p, b));
                if let (Some(sounds), Some(kind)) = (sounds.as_mut(), world.scene.sound_type(block)) {
                    sounds.play(&world.packs, &kind.step, Some(centre(pos)), kind.volume * 0.15, kind.pitch);
                }
            }
        }
        if on_ground {
            if let Some(peak) = steps.air_peak.take() {
                let fall = peak - feet[1];
                if fall > 3.0
                    && let Some(sounds) = sounds.as_mut()
                {
                    let event = if fall > 7.0 { "minecraft:entity.player.big_fall" } else { "minecraft:entity.player.small_fall" };
                    sounds.play(&world.packs, event, None, 1.0, 1.0);
                    if let Some((pos, block)) = under.as_ref()
                        && let Some(kind) = world.scene.sound_type(block)
                    {
                        sounds.play(&world.packs, &kind.fall, Some(centre(*pos)), kind.volume * 0.5, kind.pitch * 0.75);
                    }
                }
            }
        } else {
            steps.air_peak = Some(steps.air_peak.map_or(feet[1], |p| p.max(feet[1])));
        }
    } else {
        steps.air_peak = None;
    }
    steps.last = alive.then_some(feet);

    let dt_hand = time.delta_secs_f64();
    if let Some(ticks) = hand.swing.as_mut() {
        *ticks += (dt_hand * 20.0) as f32;
        if *ticks >= crate::minecraft_hand::SWING_TICKS {
            hand.swing = None;
        }
    }
    let holding = entities.as_ref().is_some_and(|e| {
        e.inventory.slots[e.selected].as_ref().is_none_or(|s| crate::minecraft_inventory::weapon_of(s).is_none())
    });
    ui.holding_item = alive && holding;
    ui.empty_hand = ui.holding_item
        && entities.as_ref().is_some_and(|e| e.inventory.slots[e.selected].is_none());
    hand.clock += dt_hand;
    let hand_ticks = (hand.clock / TICK_SECONDS) as u32;
    hand.clock -= f64::from(hand_ticks) * TICK_SECONDS;
    // Using a block, as vanilla's use does: the right click (LT) with a hand
    // or an item, or F (MW2's use) with a gun. A crafting table, furnace or
    // chest opens its screen; flint and steel lights a portal frame; doors,
    // gates, levers and buttons go to the level; otherwise the click places.
    let mut used_block = false;
    // The console's `mcuse` stands for a right click this frame.
    let console_use = std::mem::take(&mut ui.use_request);
    // The console's `mcgive`: into the first free hotbar slot, selected.
    if let Some((id, count)) = ui.give_request.take()
        && let Some(entities) = entities.as_mut()
    {
        let mut stack = minecraftoss_player::inventory::ItemStack::new(&id, count);
        stack.max = stack.max.min(entities.inventory.recipes.max_stack(&id));
        match (0..frame::minecraft_ui::MC_HOTBAR).find(|&i| entities.inventory.slots[i].is_none()) {
            Some(slot) => {
                entities.inventory.slots[slot] = Some(stack);
                ui.select = Some(slot);
            }
            None => {
                let _ = entities.inventory.add_item(stack, entities.selected);
            }
        }
    }
    if alive && !ui.inventory_open && entities.is_some() {
        let with_hand = ui.holding_item
            && (console_use || buttons.just_pressed(MouseButton::Right) || pad_trigger(pad, GamepadButton::LeftTrigger2, true));
        let sneaking = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ControlLeft);
        if (with_hand && !sneaking) || keys.just_pressed(KeyCode::KeyF) {
            let mut player = minecraftoss_player::Player::new(glam::DVec3::from_array(feet));
            player.yaw = f64::from(mc_yaw);
            player.pitch = f64::from(ps.viewangles[0]);
            if let Some(hit) = player.target(&world.scene, 4.5)
                && let Some(block) = minecraft_terrain::scene::Scene::block(&world.scene, hit.pos).cloned()
                && let Some(entities) = entities.as_mut()
            {
                let path = block.id.path.as_str();
                let held = entities.inventory.slots[entities.selected].as_ref().map(|s| s.id.clone());
                let dim = dimension_index(world.dimension);
                if path == "crafting_table" {
                    ui.screen = frame::McScreen::Workbench;
                    ui.inventory_open = true;
                    used_block = true;
                } else if let Some(screen) = {
                    // A structure's chest or barrel holds its loot table's roll
                    // the first time it opens (`unpackLootTable`).
                    let key = (dim, hit.pos);
                    if crate::minecraft_containers::kind_of(path) == Some(frame::McScreen::Chest) && !containers.has(key) {
                        let chunk = world.stream.load_now(minecraftoss_core::ChunkPos::new(hit.pos.0 >> 4, hit.pos.2 >> 4));
                        let store = &chunk.block_entities;
                        let tag = store.entities.get(&hit.pos).or_else(|| store.pending.get(&hit.pos));
                        if let Some(table) = tag.and_then(|t| t.get("LootTable")).and_then(|t| t.as_str()) {
                            let seed = tag.and_then(|t| t.get("LootTableSeed")).and_then(|t| t.as_i64()).unwrap_or(0);
                            diag::info!(World, "Minecraft loot: {table} for the container at {:?}", hit.pos);
                            containers.stock_chest(key, entities.roll_container(table, seed, 27));
                        }
                    }
                    containers.open(key, path)
                } {
                    ui.screen = screen;
                    ui.inventory_open = true;
                    used_block = true;
                } else if with_hand && path == "end_portal_frame" && held.as_deref() == Some("minecraft:ender_eye") {
                    // `EnderEyeItem.useOn`: an eye into an empty frame; twelve
                    // round a three by three open the portal.
                    if block.properties.get("eye").is_none_or(|e| e != "true") {
                        let mut filled = block.clone();
                        filled.properties.insert("eye".into(), "true".into());
                        set_block(world, Some(&mut *entities), shapes, shape_ids, hit.pos, Some(filled));
                        if let Some(stack) = entities.inventory.slots[entities.selected].as_mut() {
                            stack.count -= 1;
                            if stack.count == 0 {
                                entities.inventory.slots[entities.selected] = None;
                            }
                        }
                        let at = |p: (i32, i32, i32)| Vec3::from_array(sim::voxel::to_map(origin, [p.0 as f64 + 0.5, p.1 as f64 + 0.5, p.2 as f64 + 0.5]));
                        if let Some(sounds) = sounds.as_mut() {
                            sounds.play(&world.packs, "minecraft:block.end_portal_frame.fill", Some(at(hit.pos)), 1.0, 1.0);
                        }
                        let frame_with_eye = |p| {
                            minecraft_terrain::scene::Scene::block(&world.scene, p)
                                .is_some_and(|b| b.id.path == "end_portal_frame" && b.properties.get("eye").is_some_and(|e| e == "true"))
                        };
                        if let Some(inside) = crate::minecraft_eyes::completed_portal(frame_with_eye, hit.pos) {
                            for pos in inside {
                                set_block(world, Some(&mut *entities), shapes, shape_ids, pos, Some(minecraft_terrain::scene::Block::new("minecraft:end_portal")));
                            }
                            if let Some(sounds) = sounds.as_mut() {
                                // Heard everywhere (`globalLevelEvent`).
                                sounds.play(&world.packs, "minecraft:block.end_portal.spawn", None, 1.0, 1.0);
                            }
                        }
                    }
                    used_block = true;
                } else if with_hand
                    && matches!(held.as_deref(), Some("minecraft:flint_and_steel" | "minecraft:fire_charge"))
                {
                    let (ox, oy, oz) = hit.face.offset();
                    let fire = (hit.pos.0 + ox, hit.pos.1 + oy, hit.pos.2 + oz);
                    let name = |p| {
                        minecraft_terrain::scene::Scene::block(&world.scene, p).map_or_else(String::new, |b| b.id.path.clone())
                    };
                    if let Some((inside, axis)) = crate::minecraft_portal::light(name, fire) {
                        for pos in inside {
                            set_block(world, Some(&mut *entities), shapes, shape_ids, pos, Some(crate::minecraft_portal::portal_block(axis)));
                        }
                        if held.as_deref() == Some("minecraft:fire_charge")
                            && let Some(stack) = entities.inventory.slots[entities.selected].as_mut()
                        {
                            stack.count -= 1;
                            if stack.count == 0 {
                                entities.inventory.slots[entities.selected] = None;
                            }
                        }
                        if let Some(sounds) = sounds.as_mut() {
                            let centre = [fire.0 as f64 + 0.5, fire.1 as f64 + 0.5, fire.2 as f64 + 0.5];
                            sounds.play(&world.packs, "minecraft:item.flintandsteel.use", Some(Vec3::from_array(sim::voxel::to_map(origin, centre))), 1.0, 1.0);
                        }
                    }
                    used_block = true;
                } else {
                    // Vanilla's `Direction.fromYRot`: 0 faces south.
                    let facing = ["south", "west", "north", "east"][((mc_yaw / 90.0 + 0.5).floor() as i32).rem_euclid(4) as usize];
                    used_block = entities.use_block(&world.scene, hit.pos, facing);
                }
                if used_block {
                    hand.swing = Some(0.0);
                    hand.place_delay = 4;
                }
            }
        }
    }
    if let Some(entities) = entities.as_mut()
        && ui.holding_item
        && !ui.inventory_open
        && !used_block
    {
        let mut player = minecraftoss_player::Player::new(glam::DVec3::from_array(feet));
        player.yaw = f64::from(mc_yaw);
        player.pitch = f64::from(ps.viewangles[0]);
        player.selected = entities.selected;
        let eye_block = glam::DVec3::from_array(feet) + glam::DVec3::Y * 1.62;
        let look = {
            let (yaw, pitch) = (f64::from(mc_yaw).to_radians(), f64::from(ps.viewangles[0]).to_radians());
            glam::DVec3::new(-yaw.sin() * pitch.cos(), -pitch.sin(), yaw.cos() * pitch.cos())
        };
        if buttons.just_pressed(MouseButton::Left) || pad_trigger(pad, GamepadButton::RightTrigger2, true) {
            hand.swing = Some(0.0);
            entities.punch(eye_block, look, mc_yaw);
        }
        if buttons.pressed(MouseButton::Left) || pad_trigger(pad, GamepadButton::RightTrigger2, false) {
            for _ in 0..hand_ticks {
                if let Some(hit) = player.target(&world.scene, 4.5) {
                    // A hand mines as vanilla's `getDestroyProgress`: a
                    // block's hardness times thirty ticks.
                    all_events.push(sim::voxel::VoxelEvent::Shot {
                        block: [hit.pos.0, hit.pos.1, hit.pos.2],
                        damage: 160.0 / 30.0,
                    });
                    if hand.swing.is_none_or(|t| t >= crate::minecraft_hand::SWING_TICKS * 0.5) {
                        hand.swing = Some(0.0);
                    }
                }
            }
        }
        hand.place_delay = hand.place_delay.saturating_sub(hand_ticks);
        let place = (console_use || buttons.just_pressed(MouseButton::Right) || pad_trigger(pad, GamepadButton::LeftTrigger2, true))
            || ((buttons.pressed(MouseButton::Right) || pad_trigger(pad, GamepadButton::LeftTrigger2, false)) && hand.place_delay == 0);
        if place {
            hand.place_delay = 4;
            // Buckets (`BucketItem.use`): an empty one scoops a water or lava
            // source; a full one pours its source against the face aimed at.
            let held = entities.inventory.slots[entities.selected].as_ref().map(|s| s.id.clone());
            let bucket_used = match held.as_deref() {
                Some("minecraft:bucket") => {
                    let scooped = player.target_source_fluid(&world.scene, 5.0).and_then(|hit| {
                        let block = minecraft_terrain::scene::Scene::block(&world.scene, hit.pos)?;
                        let source = block.properties.get("level").is_none_or(|l| l == "0");
                        match block.id.path.as_str() {
                            "water" if source => Some((hit.pos, "minecraft:water_bucket", "minecraft:item.bucket.fill")),
                            "lava" if source => Some((hit.pos, "minecraft:lava_bucket", "minecraft:item.bucket.fill_lava")),
                            _ => None,
                        }
                    });
                    if let Some((pos, filled, sound)) = scooped {
                        set_block(world, Some(&mut *entities), shapes, shape_ids, pos, None);
                        let slot = &mut entities.inventory.slots[entities.selected];
                        if slot.as_ref().is_some_and(|s| s.count > 1) {
                            if let Some(stack) = slot.as_mut() {
                                stack.count -= 1;
                            }
                            let full = minecraftoss_player::inventory::ItemStack::new(filled, 1);
                            if let Some(rest) = entities.inventory.add_item(full, entities.selected) {
                                entities.spill((feet[0].floor() as i32, feet[1].floor() as i32, feet[2].floor() as i32), vec![rest]);
                            }
                        } else {
                            *slot = Some(minecraftoss_player::inventory::ItemStack::new(filled, 1));
                        }
                        if let Some(sounds) = sounds.as_mut() {
                            let at = [pos.0 as f64 + 0.5, pos.1 as f64 + 0.5, pos.2 as f64 + 0.5];
                            sounds.play(&world.packs, sound, Some(Vec3::from_array(sim::voxel::to_map(origin, at))), 1.0, 1.0);
                        }
                        hand.swing = Some(0.0);
                    }
                    true
                }
                Some(full @ ("minecraft:water_bucket" | "minecraft:lava_bucket")) => {
                    if let Some(hit) = player.target(&world.scene, 5.0) {
                        let (ox, oy, oz) = hit.face.offset();
                        let pos = (hit.pos.0 + ox, hit.pos.1 + oy, hit.pos.2 + oz);
                        let there = minecraft_terrain::scene::Scene::block(&world.scene, pos).map(|b| b.id.path.clone());
                        let open = there.as_deref().is_none_or(|p| matches!(p, "water" | "lava" | "short_grass" | "tall_grass" | "fern" | "fire"));
                        let water = full == "minecraft:water_bucket";
                        let at = [pos.0 as f64 + 0.5, pos.1 as f64 + 0.5, pos.2 as f64 + 0.5];
                        let at = Vec3::from_array(sim::voxel::to_map(origin, at));
                        if open {
                            if water && world.dimension == Dimension::Nether {
                                // `DimensionType.ultraWarm`: water boils away.
                                if let Some(sounds) = sounds.as_mut() {
                                    sounds.play(&world.packs, "minecraft:block.fire.extinguish", Some(at), 0.5, 2.6);
                                }
                            } else {
                                let mut block = minecraft_terrain::scene::Block::new(if water { "minecraft:water" } else { "minecraft:lava" });
                                block.properties.insert("level".into(), "0".into());
                                set_block(world, Some(&mut *entities), shapes, shape_ids, pos, Some(block));
                                if let Some(sounds) = sounds.as_mut() {
                                    let event = if water { "minecraft:item.bucket.empty" } else { "minecraft:item.bucket.empty_lava" };
                                    sounds.play(&world.packs, event, Some(at), 1.0, 1.0);
                                }
                            }
                            entities.inventory.slots[entities.selected] = Some(minecraftoss_player::inventory::ItemStack::new("minecraft:bucket", 1));
                            hand.swing = Some(0.0);
                        }
                    }
                    true
                }
                // `Equippable.swapWithEquipmentSlot`: armour on (what was worn
                // comes back to the hand).
                Some(_) if entities.inventory.slots[entities.selected].as_ref().is_some_and(|stack| {
                    matches!(entities.inventory.recipes.equipment_slot(stack), Some("head" | "chest" | "legs" | "feet"))
                }) => {
                    let stack = entities.inventory.slots[entities.selected].clone().expect("checked above");
                    let slot = match entities.inventory.recipes.equipment_slot(&stack) {
                        Some("head") => 39,
                        Some("chest") => 38,
                        Some("legs") => 37,
                        _ => 36,
                    };
                    let worn = entities.inventory.slots[slot].take();
                    entities.inventory.slots[slot] = Some(stack);
                    entities.inventory.slots[entities.selected] = worn;
                    if let Some(sounds) = sounds.as_mut() {
                        sounds.play(&world.packs, "minecraft:item.armor.equip_generic", None, 1.0, 1.0);
                    }
                    hand.swing = Some(0.0);
                    true
                }
                // Gold for a piglin to barter with (`Piglin.mobInteract`).
                Some("minecraft:gold_ingot") => {
                    let eye_at = glam::DVec3::from_array(feet) + glam::DVec3::Y * 1.62;
                    let (yaw, pitch) = (f64::from(mc_yaw).to_radians(), f64::from(ps.viewangles[0]).to_radians());
                    let look = glam::DVec3::new(-yaw.sin() * pitch.cos(), -pitch.sin(), yaw.cos() * pitch.cos());
                    if entities.offer_gold(eye_at, look) {
                        if let Some(stack) = entities.inventory.slots[entities.selected].as_mut() {
                            stack.count -= 1;
                            if stack.count == 0 {
                                entities.inventory.slots[entities.selected] = None;
                            }
                        }
                        hand.swing = Some(0.0);
                    }
                    true
                }
                // `EnderEyeItem.use` in the air: in the Overworld it flies
                // off towards the nearest stronghold.
                Some("minecraft:ender_eye") => {
                    if world.dimension == Dimension::Overworld
                        && let Some(target) = strongholds.nearest(&world.stream, glam::DVec3::from_array(feet))
                    {
                        let from = glam::DVec3::from_array(feet) + glam::DVec3::Y * 1.62;
                        eyes.throw(from, target);
                        if let Some(stack) = entities.inventory.slots[entities.selected].as_mut() {
                            stack.count -= 1;
                            if stack.count == 0 {
                                entities.inventory.slots[entities.selected] = None;
                            }
                        }
                        if let Some(sounds) = sounds.as_mut() {
                            sounds.play(&world.packs, "minecraft:entity.ender_eye.launch", None, 0.5, 0.4);
                        }
                        hand.swing = Some(0.0);
                    }
                    true
                }
                _ => false,
            };
            if !bucket_used && let Some(pos) = player.place_selected(
                &mut world.scene,
                &mut entities.inventory,
                minecraftoss_player::GameMode::Survival,
            ) {
                // Not into the player's own box.
                let [fx, fy, fz] = feet;
                let inside = (fx - 0.3) < f64::from(pos.0 + 1)
                    && (fx + 0.3) > f64::from(pos.0)
                    && fy < f64::from(pos.1 + 1)
                    && (fy + 1.8) > f64::from(pos.1)
                    && (fz - 0.3) < f64::from(pos.2 + 1)
                    && (fz + 0.3) > f64::from(pos.2);
                let block = minecraft_terrain::scene::Scene::block(&world.scene, pos).cloned();
                if inside {
                    world.scene.set(pos, None);
                    if let Some(block) = block {
                        let _ = entities.inventory.add_item(
                            minecraftoss_player::inventory::ItemStack::new(block.id.key(), 1),
                            entities.selected,
                        );
                    }
                } else if let Some(block) = block {
                    let state = world.stream.states.state_of(&block);
                    let blocks = &world.registries.blocks;
                    let shape = state.map_or(0, |state| {
                        *shapes.entry(state).or_insert_with(|| {
                            let boxes = blocks.collision_boxes(state);
                            if boxes.is_empty() {
                                return 0;
                            }
                            let key: Vec<[u32; 6]> = boxes.iter().map(|b| b.map(|v| (v as f32).to_bits())).collect();
                            if let Some(&id) = shape_ids.get(&key) {
                                return id;
                            }
                            let boxes32 = boxes.iter().map(|b| b.map(|v| v as f32)).collect();
                            let id = sim::voxel::add_shapes(vec![boxes32]).unwrap_or(0);
                            shape_ids.insert(key, id);
                            id
                        })
                    });
                    sim::voxel::set_block_shape(pos.0, pos.1, pos.2, shape);
                    world.stream.record_edits(&world.scene, &[pos]);
                    world.stream.mark_edited(&world.scene, &[pos]);
                    entities.placed(&world.scene, pos);
                    hand.swing = Some(0.0);
                    if let (Some(sounds), Some(kind)) = (sounds.as_mut(), world.scene.sound_type(&block)) {
                        let centre = [pos.0 as f64 + 0.5, pos.1 as f64 + 0.5, pos.2 as f64 + 0.5];
                        let at = Vec3::from_array(sim::voxel::to_map(origin, centre));
                        sounds.play(&world.packs, &kind.place, Some(at), (kind.volume + 1.0) / 2.0, kind.pitch * 0.8);
                    }
                }
            }
        }
    }

    // Shots and explosions from the authoritative game: bullets that met a
    // mob hurt it, the rest mine.
    let (mob_shots, events): (Vec<_>, Vec<_>) = all_events
        .into_iter()
        .partition(|event| matches!(event, sim::voxel::VoxelEvent::MobShot { .. }));
    let mut fight_effects = Vec::new();
    let mut doom_effects = Vec::new();
    if let Some(entities) = entities.as_mut() {
        for shot in mob_shots {
            if let sim::voxel::VoxelEvent::MobShot { key, damage, from } = shot {
                if crate::minecraft_dragon::Fight::owns(key) {
                    fight_effects.push(dragon.shot(key, damage, glam::DVec3::from_array(feet)));
                } else if crate::minecraft_doom::Bosses::owns(key) {
                    doom_effects.push(doom.shot(key, damage));
                } else {
                    entities.shoot(key, damage, from, mc_yaw);
                }
            }
        }
    }
    // Vanilla's block sounds: a hit for each bullet into a block, then the
    // break of each block broken (`SoundType` volume and pitch as
    // `MultiPlayerGameMode` and `LevelRenderer` scale them), and blasts.
    let at = |b: [f64; 3]| Vec3::from_array(sim::voxel::to_map(origin, b));
    if let Some(sounds) = sounds.as_mut() {
        for event in &events {
            match *event {
                sim::voxel::VoxelEvent::Shot { block, .. } => {
                    let pos = (block[0], block[1], block[2]);
                    if let Some(kind) = minecraft_terrain::scene::Scene::block(&world.scene, pos)
                        .and_then(|b| world.scene.sound_type(b))
                    {
                        let centre = [block[0] as f64 + 0.5, block[1] as f64 + 0.5, block[2] as f64 + 0.5];
                        sounds.play(&world.packs, &kind.hit, Some(at(centre)), (kind.volume + 1.0) / 4.0, kind.pitch * 0.5);
                    }
                }
                sim::voxel::VoxelEvent::Explosion { center } => {
                    let pitch = (1.0 + (sounds.random() - sounds.random()) * 0.2) * 0.7;
                    sounds.play(&world.packs, "minecraft:entity.generic.explode", Some(at(center)), 4.0, pitch);
                }
                sim::voxel::VoxelEvent::MobShot { .. } | sim::voxel::VoxelEvent::Ray { .. } => {}
            }
        }
    }
    let broken = mining.apply(
        events,
        &mut crate::minecraft_mining::WorldRefs {
            stream: &mut world.stream,
            scene: &mut world.scene,
            packs: &world.packs,
            atlas: &world.atlas,
            registries: &world.registries,
        },
        time.elapsed_secs_f64(),
    );
    if let Some(sounds) = sounds.as_mut() {
        for (pos, block, blast) in &broken {
            if *blast {
                continue;
            }
            if let Some(kind) = world.scene.sound_type(block) {
                let centre = [pos.0 as f64 + 0.5, pos.1 as f64 + 0.5, pos.2 as f64 + 0.5];
                sounds.play(&world.packs, &kind.break_sound, Some(at(centre)), (kind.volume + 1.0) / 2.0, kind.pitch * 0.8);
            }
        }
    }
    if let Some(entities) = entities.as_mut() {
        let positions: Vec<_> = broken.iter().map(|(pos, ..)| *pos).collect();
        entities.broke(&world.scene, &positions);
        entities.drop_blocks(&broken);
    }

    // A broken furnace or chest spills what it held; a broken frame or
    // portal block takes the rest of the portal with it.
    let dim = dimension_index(world.dimension);
    for (pos, block, _) in &broken {
        let contents = containers.remove((dim, *pos));
        if let Some(entities) = entities.as_mut()
            && !contents.is_empty()
        {
            entities.spill(*pos, contents);
        }
        if matches!(block.id.path.as_str(), "obsidian" | "nether_portal") {
            let name = |p| minecraft_terrain::scene::Scene::block(&world.scene, p).map_or_else(String::new, |b| b.id.path.clone());
            let mut gone = Vec::new();
            for (dx, dy, dz) in crate::minecraft_portal::NEIGHBOURS {
                let next = (pos.0 + dx, pos.1 + dy, pos.2 + dz);
                if name(next) == "nether_portal" {
                    gone.extend(crate::minecraft_portal::connected(name, next));
                }
            }
            for p in gone {
                set_block(world, entities.as_mut(), shapes, shape_ids, p, None);
            }
        }
    }

    // Furnaces burn and cook each tick, lit furnaces showing it; and a
    // player standing in a portal long enough goes through.
    if let Some(entities) = entities.as_mut() {
        let recipes = entities.inventory.recipes.clone();
        for _ in 0..hand_ticks {
            for (pos, lit) in containers.tick(&recipes, dim) {
                if let Some(mut block) = minecraft_terrain::scene::Scene::block(&world.scene, pos).cloned()
                    && crate::minecraft_containers::kind_of(&block.id.path) == Some(frame::McScreen::Furnace)
                {
                    block.properties.insert("lit".into(), lit.to_string());
                    set_block(world, Some(&mut *entities), shapes, shape_ids, pos, Some(block));
                }
            }
        }
    }
    let in_portal = alive
        && [0.1, 1.0].into_iter().any(|dy| {
            let at = (feet[0].floor() as i32, (feet[1] + dy).floor() as i32, feet[2].floor() as i32);
            minecraft_terrain::scene::Scene::block(&world.scene, at).is_some_and(|b| b.id.path == "nether_portal")
        });
    if portal.step(in_portal, hand_ticks) && world.dimension != Dimension::End {
        let to = if world.dimension == Dimension::Overworld { Dimension::Nether } else { Dimension::Overworld };
        *travel = Some(Travel { to, portal: true });
    }
    // An end portal takes the player at once: to the End, or out of it to
    // the Overworld's spawn.
    let in_end_portal = alive
        && minecraft_terrain::scene::Scene::block(&world.scene, (feet[0].floor() as i32, (feet[1] + 0.1).floor() as i32, feet[2].floor() as i32))
            .is_some_and(|b| b.id.path == "end_portal");
    if in_end_portal && travel.is_none() {
        let to = if world.dimension == Dimension::End { Dimension::Overworld } else { Dimension::End };
        *travel = Some(Travel { to, portal: false });
    }
    *last_feet = feet;
    // The console's `summon`: three blocks ahead, facing the player.
    if let Some(kind) = ui.summon_request.take()
        && let Some(entities) = entities.as_mut()
    {
        let yaw = f64::from(mc_yaw).to_radians();
        let at = [feet[0] - yaw.sin() * 3.0, feet[1] + 0.5, feet[2] + yaw.cos() * 3.0];
        entities.summon(&kind, at, mc_yaw + 180.0);
    }

    // The dragon fight, in the End: set up once the island's centre has
    // generated, then its tick and what it does to the world and player.
    ui.boss = None;
    if world.dimension != Dimension::End {
        ui.dragon_request = None;
    }
    if world.dimension == Dimension::End {
        if !dragon.started() && world.scene.generated_chunk((0, 0)).is_some() {
            let top = (0..200)
                .rev()
                .find(|&y| minecraft_terrain::scene::Scene::block(&world.scene, (0, y, 0)).is_some_and(|b| b.id.path == "end_stone"))
                .map_or(64, |y| y + 1);
            fight_effects.push(dragon.start(world.seed, top));
            diag::info!(World, "Ender dragon fight: podium at y {top}");
        }
        match ui.dragon_request.take().as_deref() {
            Some("kill") if dragon.started() => {
                let mut fx = crate::minecraft_dragon::Effects::default();
                dragon.kill(&mut fx);
                fight_effects.push(fx);
            }
            Some("reset") if dragon.started() => fight_effects.push(dragon.reset(world.seed)),
            _ => {}
        }
        if dragon.started() {
            let player = alive.then(|| glam::DVec3::from_array(feet));
            let scene = &world.scene;
            fight_effects.push(dragon.update(time.delta_secs_f64(), player, |p| {
                minecraft_terrain::scene::Scene::block(scene, p).is_some_and(|b| b.is_opaque())
            }));
            ui.boss = dragon.boss();
        }
    }
    // Doom's bosses: the console's `doomboss` summons one twelve blocks
    // ahead, facing the player, on the ground there.
    let here = dimension_index(world.dimension);
    match ui.doom_request.take().as_deref() {
        Some("clear") => doom.clear(),
        Some(name) => match crate::minecraft_doom::Kind::parse(name) {
            Some(_) if minecraft_terrain::doom::assets().is_none() => {
                diag::warn!(World, "doomboss: Freedoom's freedoom2.wad was not found (iw4l-artifacts/freedoom, or IW4L_FREEDOOM)");
            }
            Some(kind) => {
                let yaw = f64::from(mc_yaw).to_radians();
                let (x, z) = (feet[0] - yaw.sin() * 12.0, feet[2] + yaw.cos() * 12.0);
                let (bx, bz) = (x.floor() as i32, z.floor() as i32);
                let top = feet[1].floor() as i32;
                let ground = (top - 24..=top + 16)
                    .rev()
                    .find(|&y| {
                        let solid = |y| minecraft_terrain::scene::Scene::block(&world.scene, (bx, y, bz)).is_some_and(|b| b.is_opaque());
                        solid(y) && !solid(y + 1)
                    })
                    .map_or(feet[1], |y| f64::from(y + 1));
                doom_effects.push(doom.spawn(kind, here, glam::DVec3::new(x, ground, z), mc_yaw + 180.0));
            }
            None => {}
        },
        None => {}
    }
    {
        let player = alive.then(|| glam::DVec3::from_array(feet));
        let scene = &world.scene;
        doom_effects.push(doom.update(time.delta_secs_f64(), here, player, |p| {
            minecraft_terrain::scene::Scene::block(scene, p).is_some_and(|b| b.is_opaque())
        }));
        if ui.boss.is_none() {
            ui.boss = doom.boss(here, glam::DVec3::from_array(feet));
        }
    }
    for fx in doom_effects {
        for (amount, from) in fx.damage {
            diag::info!(World, "Doom: player hurt {amount} from {:.0?}", from.to_array());
            let amount = entities.as_ref().map_or(amount, |e| (e.after_armor(amount as f32 * 0.2) / 0.2).round() as i32);
            sim::voxel::push_player_damage(local.0.0, amount, Some(sim::voxel::to_map(origin, from.to_array())));
        }
        if let (Some(sounds), Some(assets)) = (sounds.as_mut(), minecraft_terrain::doom::assets()) {
            for (name, at, volume) in fx.sounds {
                if let Some(bytes) = assets.sound(name) {
                    let at = at.map(|at| Vec3::from_array(sim::voxel::to_map(origin, at.to_array())));
                    sounds.play_file(format!("doom:{name}"), bytes, at, volume, 1.0);
                }
            }
        }
    }
    let eye_events = eyes.update(time.delta_secs_f64());
    if let Some(entities) = entities.as_mut() {
        for at in eye_events.drops {
            entities.spill((at.x.floor() as i32, at.y.floor() as i32, at.z.floor() as i32), vec![minecraftoss_player::inventory::ItemStack::new("minecraft:ender_eye", 1)]);
        }
    }
    if let Some(sounds) = sounds.as_mut() {
        for (event, at) in eye_events.sounds {
            sounds.play(&world.packs, event, Some(Vec3::from_array(sim::voxel::to_map(origin, at.to_array()))), 1.0, 1.0);
        }
    }
    for fx in fight_effects {
        for (amount, from) in fx.damage {
            // Back to Minecraft health (a fifth), through the worn armour.
            let amount = entities.as_ref().map_or(amount, |e| (e.after_armor(amount as f32 * 0.2) / 0.2).round() as i32);
            sim::voxel::push_player_damage(local.0.0, amount, Some(sim::voxel::to_map(origin, from.to_array())));
        }
        if let Some(sounds) = sounds.as_mut() {
            for (event, at, volume, pitch) in fx.sounds {
                sounds.play(&world.packs, event, Some(Vec3::from_array(sim::voxel::to_map(origin, at.to_array()))), volume, pitch);
            }
        }
        for (pos, block) in fx.edits {
            set_block(world, entities.as_mut(), shapes, shape_ids, pos, block);
        }
    }

    let eye = sim::voxel::to_block(origin, [ps.origin[0], ps.origin[1], ps.origin[2] + ps.view_height_current]);
    let (pitch, yaw) = (ps.viewangles[0].to_radians(), ps.viewangles[1].to_radians());
    let map_forward = [pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), -pitch.sin()];
    let forward = glam::Vec3::new(map_forward[0], map_forward[2], -map_forward[1]);
    let aspect = windows
        .single()
        .map(|w| w.width() / w.height().max(1.0))
        .unwrap_or(16.0 / 9.0);
    // Culled from the camera that draws: the player's eye, or while
    // skating the Skate camera (a frame behind, so with room to spare).
    let skate_camera = cameras.iter().next().filter(|_| skate.active).map(|t| {
        let at = sim::voxel::to_block(origin, t.translation.to_array());
        let ahead = t.rotation * Vec3::NEG_Z;
        (
            glam::DVec3::new(at[0], at[1], at[2]),
            glam::Vec3::new(ahead.x, ahead.z, -ahead.y).normalize_or(forward),
        )
    });
    let (cull_at, cull_forward) = skate_camera.unwrap_or((glam::DVec3::new(eye[0], eye[1], eye[2]), forward));
    let camera = CullCamera {
        position: cull_at,
        forward: cull_forward,
        fov_degrees: if skate_camera.is_some() { 120.0 } else { 90.0 },
        aspect,
        yaw_degrees: (-cull_forward.x).atan2(cull_forward.z).to_degrees(),
        pitch_degrees: (-cull_forward.y).asin().to_degrees(),
    };
    let update = world
        .stream
        .frame(&world.scene, &camera, FADE_MILLIS, &world.atlas, &world.packs);
    view.uploads.extend(update.uploads);
    view.removed.extend(update.removed);
    view.visible = update.visible;
    let Some(light) = light.as_mut() else {
        return;
    };
    for (chunk, column) in update.lights {
        light.set_chunk_column(chunk, column);
    }

    if let Some(sounds) = sounds.as_mut() {
        sound_queue.0.append(&mut sounds.queued);
    }
    // The minimap's picture, and its corners on the map.
    ui.minimap = minimap
        .update(time.delta_secs_f64(), feet, &world.scene, &world.packs, &world.atlas, &mut images)
        .map(|(image, [bx, bz])| {
            let corner = |x: i32, z: i32| {
                let p = sim::voxel::to_map(origin, [f64::from(x), feet[1], f64::from(z)]);
                [p[0], p[1]]
            };
            (image, corner(bx, bz), corner(bx + 256, bz + 256))
        });
    // The Overworld clock and the environment attributes of MinecraftOSS.
    let dt = time.delta_secs_f64();
    let partial = mining.tick(&world.scene, dt);
    view.particles = mining.particle_mesh(&world.atlas, forward, partial, light);

    // The mobs: a server tick when due, the blocks it changed, its hits on
    // the player, the mobs' boxes for bullets and their meshes.
    if let Some(entities) = entities.as_mut() {
        let player = crate::minecraft_entities::PlayerView {
            feet,
            alive,
            health: ps.health as f32,
            yaw: mc_yaw,
            pitch: ps.viewangles[0],
        };
        // `Level.isDay`: never in a dimension with fixed time (the Nether
        // and the End), however bright its sky; endermen there would teleport
        // from the "sun" until they found cover under the island.
        let bright_outside = world.dimension == Dimension::Overworld && world.environment.sky_light_level() > 11.0;
        let ticks_before = entities.client_ticks();
        let (changes, hits) = entities.tick(dt, day.ticks as i64, bright_outside, &player);
        let mob_ticks = (entities.client_ticks() - ticks_before) as u32;
        if !changes.is_empty() {
            let blocks = &world.registries.blocks;
            let mut positions = Vec::with_capacity(changes.len());
            for (pos, block) in changes {
                let state = block.as_ref().and_then(|b| world.stream.states.state_of(b));
                let shape = state.map_or(0, |state| {
                    *shapes.entry(state).or_insert_with(|| {
                        let boxes = blocks.collision_boxes(state);
                        if boxes.is_empty() {
                            return 0;
                        }
                        let key: Vec<[u32; 6]> = boxes.iter().map(|b| b.map(|v| (v as f32).to_bits())).collect();
                        if let Some(&id) = shape_ids.get(&key) {
                            return id;
                        }
                        let boxes32 = boxes.iter().map(|b| b.map(|v| v as f32)).collect();
                        let id = sim::voxel::add_shapes(vec![boxes32]).unwrap_or(0);
                        shape_ids.insert(key, id);
                        id
                    })
                });
                sim::voxel::set_block_shape(pos.0, pos.1, pos.2, shape);
                world.scene.set(pos, block);
                positions.push(pos);
            }
            world.stream.mark_edited(&world.scene, &positions);
        }
        for (amount, from) in hits {
            sim::voxel::push_player_damage(local.0.0, amount, from.map(|b| sim::voxel::to_map(origin, b)));
        }
        // The inventory: MW2 guns as items, the HUD's clicks, the hotbar's
        // gun, and what the HUD shows.
        let owned: Vec<u32> = ps
            .weapons
            .iter()
            .filter(|&&w| w > 0)
            .map(|&w| w as u32)
            .filter(|&w| authority.0.weapon_combat_row(w).is_some_and(|facts| facts.inventory_type == 0))
            .collect();
        ui.active = alive;
        if !alive {
            ui.inventory_open = false;
            ui.screen = frame::McScreen::Inventory;
        }
        inventory_ui.sync_weapons(&mut entities.inventory, &owned);
        let mut selected = entities.selected;
        let thrown = inventory_ui.apply_input(&mut ui, &mut entities.inventory, &mut selected, containers);
        if !ui.inventory_open {
            containers.close();
        }
        let thrower = crate::minecraft_inventory::Thrower {
            eye: glam::DVec3::from_array(eye),
            yaw: mc_yaw,
            pitch: ps.viewangles[0],
        };
        crate::minecraft_inventory::throw(&mut entities.world_items, thrown, &thrower);
        ui.weapon_request = inventory_ui.weapon_request(&entities.inventory, &mut selected, ps.weapon as u32);
        entities.selected = selected;
        inventory_ui.publish(&mut ui, &entities.inventory, selected, &world.packs, &mut images, containers);

        if let Some(sounds) = sounds.as_mut() {
            for (event, position, volume, pitch) in std::mem::take(&mut entities.sounds) {
                sounds.play(&world.packs, &event, Some(at(position.to_array())), volume, pitch);
            }
        }
        // The held item in view; an empty hand is MW2's own hands.
        view.hand = Default::default();
        let swing = hand.swing.map_or(0.0, |t| (t / crate::minecraft_hand::SWING_TICKS).clamp(0.0, 1.0));
        ui.hand_swing = swing;
        if ui.holding_item
            && !puppet.active
            && let Some(stack) = entities.inventory.slots[entities.selected].clone()
        {
            let eye_light_at = glam::Vec3::new(eye[0] as f32, eye[1] as f32, eye[2] as f32);
            let display = minecraft_terrain::pack::ResourceId::parse(&stack.id)
                .ok()
                .and_then(|id| minecraft_terrain::model::item_first_person_transform(&world.packs, &id).ok())
                .unwrap_or(glam::Mat4::IDENTITY);
            let pose = crate::minecraft_hand::item_pose(display, swing, 0.0);
            let mesh = entities.held_item_mesh(&stack.id, pose, eye_light_at, &world.packs, &world.atlas, light);
            let vertices: Vec<minecraft_terrain::mesh::SectionVertex> =
                mesh.vertices.iter().map(minecraft_terrain::mesh::SectionVertex::from_vertex).collect();
            view.hand = (bytemuck::cast_slice(&vertices).to_vec(), mesh.indices);
            // Reverse-Z with no far plane, as the scene's; 70 degrees up.
            let f = 1.0 / (35.0f32.to_radians()).tan();
            let near = 0.05;
            view.hand_clip = Mat4::from_cols(
                Vec4::new(f / aspect, 0.0, 0.0, 0.0),
                Vec4::new(0.0, f, 0.0, 0.0),
                Vec4::new(0.0, 0.0, 0.0, -1.0),
                Vec4::new(0.0, 0.0, near, 0.0),
            )
            .to_cols_array();
        }

        let mut boxes = entities.boxes();
        if world.dimension == Dimension::End {
            boxes.extend(dragon.boxes());
        }
        boxes.extend(doom.boxes(dimension_index(world.dimension)));
        sim::voxel::set_mob_boxes(boxes);
        entities.tick_scene(&world.scene, mob_ticks);
        let sky_darken = (15.0 - world.environment.sky_light_level()).clamp(0.0, 15.0) as u8;
        let mut meshes = entities.meshes(
            &world.scene,
            &world.packs,
            &world.atlas,
            light,
            forward,
            glam::DVec3::from_array(eye),
            sky_darken,
        );
        if world.dimension == Dimension::End {
            dragon.append_meshes(&mut meshes.models, &mut meshes.translucent, &world.atlas);
        }
        eyes.append_meshes(&mut meshes.models, &world.atlas);
        doom.append_meshes(dimension_index(world.dimension), &mut meshes.models, &world.atlas, light, cull_at);
        let raw = |mesh: &minecraft_terrain::mesh::ChunkMesh| {
            (bytemuck::cast_slice::<_, u8>(&mesh.vertices).to_vec(), mesh.indices.clone())
        };
        view.entity_meshes = [
            raw(&meshes.models),
            raw(&meshes.culled),
            raw(&meshes.translucent),
            raw(&meshes.shadows),
            Default::default(),
        ];
        let mesh = meshes.items;
        let (bytes, indices) = &mut view.particles;
        let base = (bytes.len() / std::mem::size_of::<minecraft_terrain::mesh::SectionVertex>()) as u32;
        let vertices: Vec<minecraft_terrain::mesh::SectionVertex> =
            mesh.vertices.iter().map(minecraft_terrain::mesh::SectionVertex::from_vertex).collect();
        bytes.extend_from_slice(bytemuck::cast_slice(&vertices));
        indices.extend(mesh.indices.iter().map(|i| i + base));
    }
    view.cracks = mining.crack_mesh();
    day.advance(dt);
    let eye_block = (eye[0].floor() as i32, eye[1].floor() as i32, eye[2].floor() as i32);
    view.eye_light = [
        f32::from(light.get(eye_block)),
        f32::from(light.get_block(eye_block)),
    ];
    world
        .environment
        .update_rain_fog(0.0, light.get(eye_block), false, (dt * 20.0) as f32);
    *environment_accumulator += dt;
    if !*environment_primed || *environment_accumulator >= TICK_SECONDS {
        *environment_accumulator = (*environment_accumulator % TICK_SECONDS).min(TICK_SECONDS);
        let scene = &world.scene;
        world.environment.tick(
            day.ticks.floor() as i64,
            0.0,
            0.0,
            eye,
            |x, y, z| scene.noise_biome((x, y, z)).map_or(0, |id| id.0),
            !*environment_primed,
        );
        *environment_primed = true;
    }
    let partial_tick = (*environment_accumulator / TICK_SECONDS).clamp(0.0, 1.0) as f32;
    let sky = world.environment.sky_state(&View {
        partial_tick,
        forward,
        camera_y: eye[1] as f32,
        render_distance: VIEW_DISTANCE as u32,
        rain_level: 0.0,
        thunder_level: 0.0,
    });
    let render_distance = VIEW_DISTANCE as f32 * 16.0;
    let right = forward.cross(glam::Vec3::Y).normalize_or(glam::Vec3::X);
    let up = right.cross(forward).normalize_or(glam::Vec3::Y);
    let put = |v: glam::Vec3| [v.x, v.y, v.z, 0.0];
    let game_time = day.ticks;
    view.environment = [
        put(forward),
        put(right),
        put(up),
        [eye[0] as f32, eye[1] as f32, eye[2] as f32, 0.0],
        put(sky.sky),
        [sky.fog.x, sky.fog.y, sky.fog.z, render_distance.min(sky.sky_fog_end)],
        [
            sky.sky_light_color.x,
            sky.sky_light_color.y,
            sky.sky_light_color.z,
            sky.sky_light_factor,
        ],
        sky.sunset,
        [sky.sun_direction.x, sky.sun_direction.y, sky.sun_direction.z, sky.rain_brightness],
        [sky.moon_direction.x, sky.moon_direction.y, sky.moon_direction.z, sky.rain_brightness],
        // The brightness option at its default.
        [sky.cloud.x, sky.cloud.y, sky.cloud.z, 0.5],
        [aspect, 0.0, sky.star_brightness, sky.star_angle],
        [sky.moon_phase as f32, (game_time as f32) * 0.03, 96.0, 160.0],
        [
            sky.fog_start,
            sky.fog_end,
            render_distance - (render_distance / 10.0).clamp(4.0, 64.0),
            render_distance,
        ],
        [
            sky.ambient.x,
            sky.ambient.y,
            sky.ambient.z,
            match sky.skybox {
                Skybox::Overworld => 0.0,
                Skybox::End => 1.0,
                _ => 2.0,
            },
        ],
        [
            sky.block_light_tint.x,
            sky.block_light_tint.y,
            sky.block_light_tint.z,
            sky.block_factor,
        ],
    ];

    // Clouds, rebuilt when the camera crosses a cloud cell.
    if let Some(mask) = &world.cloud_mask {
        let center = mask.center(eye[0] as f32, eye[2] as f32, game_time);
        if *cloud_center != Some(center) {
            *cloud_center = Some(center);
            let mesh = mask.build(center, eye[1] as f32);
            view.clouds = Some(Arc::new((mesh.vertices, mesh.indices)));
        }
    }

    // The light MW2 models stand in, around the player.
    *light_volume_age += 1;
    let half = LIGHT_VOLUME / 2;
    let corner = [block.0 - half, block.1 - half, block.2 - half];
    let moved =
        light_volume_at.is_none_or(|at| (0..3).any(|k| (at[k] - corner[k]).abs() >= 4));
    if moved || *light_volume_age >= 20 {
        *light_volume_age = 0;
        *light_volume_at = Some(corner);
        let n = LIGHT_VOLUME;
        let index = |x: i32, y: i32, z: i32| (((y * n + z) * n + x) * 2) as usize;
        let mut raw = vec![0u8; (n * n * n * 2) as usize];
        for y in 0..n {
            for z in 0..n {
                for x in 0..n {
                    let pos = (corner[0] + x, corner[1] + y, corner[2] + z);
                    let at = index(x, y, z);
                    raw[at] = light.get(pos);
                    raw[at + 1] = light.get_block(pos);
                }
            }
        }
        // Unlit cells (inside blocks) take their brightest neighbour, so a
        // model beside a block is not darkened by filtering into it. Levels
        // become unorm bytes.
        let mut data = vec![0u8; raw.len()];
        for y in 0..n {
            for z in 0..n {
                for x in 0..n {
                    let at = index(x, y, z);
                    let (mut sky, mut block) = (raw[at], raw[at + 1]);
                    if sky == 0 && block == 0 {
                        for (dx, dy, dz) in [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)] {
                            let (nx, ny, nz) = (x + dx, y + dy, z + dz);
                            if (0..n).contains(&nx) && (0..n).contains(&ny) && (0..n).contains(&nz) {
                                let near = index(nx, ny, nz);
                                sky = sky.max(raw[near]);
                                block = block.max(raw[near + 1]);
                            }
                        }
                    }
                    data[at] = sky.min(15) * 17;
                    data[at + 1] = block.min(15) * 17;
                }
            }
        }
        view.light_volume = Some(Arc::new((corner, data)));
    }
}

/// The collision shape id of a block state, made on first sight.
fn shape_of(
    state: Option<BlockStateId>,
    registries: &Registries,
    shapes: &mut HashMap<BlockStateId, u16>,
    shape_ids: &mut HashMap<Vec<[u32; 6]>, u16>,
) -> u16 {
    let Some(state) = state else { return 0 };
    *shapes.entry(state).or_insert_with(|| {
        let boxes = registries.blocks.collision_boxes(state);
        if boxes.is_empty() {
            return 0;
        }
        let key: Vec<[u32; 6]> = boxes.iter().map(|b| b.map(|v| (v as f32).to_bits())).collect();
        if let Some(&id) = shape_ids.get(&key) {
            return id;
        }
        let boxes32 = boxes.iter().map(|b| b.map(|v| v as f32)).collect();
        let id = sim::voxel::add_shapes(vec![boxes32]).unwrap_or(0);
        shape_ids.insert(key, id);
        id
    })
}

/// A block the game itself changed (a portal lit or gone, a furnace lit):
/// the scene, its collision, the chunk store, the section meshes and the
/// mob server's level.
fn set_block(
    world: &mut Loaded,
    entities: Option<&mut crate::minecraft_entities::Entities>,
    shapes: &mut HashMap<BlockStateId, u16>,
    shape_ids: &mut HashMap<Vec<[u32; 6]>, u16>,
    pos: (i32, i32, i32),
    block: Option<minecraft_terrain::scene::Block>,
) {
    let state = block.as_ref().and_then(|b| world.stream.states.state_of(b));
    let placed = block.is_some();
    world.scene.set(pos, block);
    let shape = shape_of(state, &world.registries, shapes, shape_ids);
    sim::voxel::set_block_shape(pos.0, pos.1, pos.2, shape);
    world.stream.record_edits(&world.scene, &[pos]);
    world.stream.mark_edited(&world.scene, &[pos]);
    if let Some(entities) = entities {
        if placed {
            entities.placed(&world.scene, pos);
        } else {
            entities.broke(&world.scene, &[pos]);
        }
    }
}

/// Puts a loaded world in play with the player's feet at map origin: the
/// renderer starts over, and a new mob and item server runs for its
/// dimension. `keep` is the inventory and hotbar slot carried in.
fn install(
    runtime: &mut Runtime,
    view: &mut MinecraftWorldView,
    world: Loaded,
    (x, y, z): (f64, f64, f64),
    keep: Option<(minecraftoss_player::inventory::Inventory, usize)>,
) {
    view.origin = [x, y, z];
    view.atlas = Some(world.atlas.clone());
    view.celestial = Some(world.celestial.clone());
    view.crack_texture = Some(world.crack_texture.clone());
    view.uploads.clear();
    view.removed.clear();
    view.visible.clear();
    view.clouds = None;
    view.light_volume = None;
    view.generation += 1;
    runtime.mining = Default::default();
    runtime.minimap = Default::default();
    runtime.steps = Default::default();
    if runtime.sounds.is_none() {
        runtime.sounds = Some(crate::minecraft_sounds::Sounds::load(&world.packs));
    }
    let mut entities = crate::minecraft_entities::Entities::new(&world.stream, world.seed, world.dimension.dimension_type());
    if let Some((inventory, selected)) = keep {
        entities.inventory = inventory;
        entities.selected = selected;
    }
    runtime.entities = Some(entities);
    runtime.environment_accumulator = 0.0;
    runtime.environment_primed = false;
    runtime.light = Some(SkyLight::streamed());
    runtime.light_volume_at = None;
    runtime.cloud_center = None;
    runtime.containers.close();
    diag::info!(World, "Minecraft world ready: seed {} {:?} at {:?}", world.seed, world.dimension, (x, y, z));
    runtime.world = Some(world);
    runtime.shapes.clear();
    runtime.shape_ids.clear();
    runtime.was_alive = false;
    runtime.settling = 10.0;
}

fn stop(runtime: &mut Runtime, view: &mut MinecraftWorldView) {
    runtime.entities = None;
    runtime.travel = None;
    runtime.containers = Default::default();
    runtime.dragon = Default::default();
    runtime.doom = Default::default();
    runtime.eyes = Default::default();
    runtime.strongholds = Default::default();
    if let Some(dir) = runtime.world_dir.take() {
        // Streams and their savers stop first.
        runtime.world = None;
        let _ = std::fs::remove_dir_all(dir);
    }
    if runtime.world.take().is_some() || runtime.loading.take().is_some() || view.active {
        sim::voxel::deactivate();
        view.active = false;
        view.atlas = None;
        view.uploads.clear();
        view.removed.clear();
        view.visible.clear();
        view.celestial = None;
        view.crack_texture = None;
        view.particles = Default::default();
        view.entity_meshes = Default::default();
        view.hand = Default::default();
        view.cracks = Default::default();
        view.clouds = None;
        view.light_volume = None;
        view.generation += 1;
    }
}
