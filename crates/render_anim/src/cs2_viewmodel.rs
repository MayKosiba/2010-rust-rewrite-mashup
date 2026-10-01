//! CS2's first-person AK-47 and karambit in the MW2 view model's place on
//! the Minecraft map: CS2's arms holding the weapon, posed by CS2's own
//! view model animations, drawn in the first-person hand's pass.
//!
//! The models, clips and texture atlas are converted from the player's own
//! CS2 install by `tools/cs2` (Source2Viewer's glTF exports, rebound and
//! sampled at 30 frames a second) into `iw4l-artifacts/cs2` (or
//! `$IW4L_CS2`); none of it ships with the game. With them there, MW2's
//! AK-47 is drawn as the CS2 one (its animations follow MW2's weapon state:
//! drawn when raised, a shot for each shot, the reload over MW2's reload
//! time, idle otherwise, `I` to inspect) and every melee is a karambit slash.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use bevy::math::{Mat3, Mat4, Quat, Vec3};
use bevy::prelude::*;
use minecraft_terrain::mesh::{SectionVertex, Vertex};
use serde::Deserialize;
use weapon_iw4::WeaponState;

use crate::minecraft_world::MinecraftWorldView;

/// CS2's `viewmodel_fov` 68 (horizontal, at 4:3) in the hand pass's 70
/// degree vertical projection: view-space x and y grow by this much.
const FOV_SCALE: f32 = 1.384;
/// CS2's default `viewmodel_offset_x/y/z` (1, 1, -1 inches), in view space
/// (x right, y up, z back), metres.
const OFFSET: Vec3 = Vec3::new(0.0254, -0.0254, -0.0254);
/// The view model's size about the eye: the same on screen, but far enough
/// in depth that MW2's aiming depth of field (which reads the view model
/// band's depth with its own near plane, 0.1, and blurs what is nearer than
/// 8 there) leaves it sharp.
const DEPTH_SCALE: f32 = 100.0;
/// Crossfade between clips, seconds.
const BLEND: f32 = 0.12;
const INSPECT_KEY: KeyCode = KeyCode::KeyI;
/// The AK's rear sight notch and front sight post, in its model's space
/// (the legacy body's, metres; tops of the rear sight block and front post
/// on the bore's centre line).
const SIGHT_REAR: Vec3 = Vec3::new(0.0, 0.1112, 0.2253);
const SIGHT_FRONT: Vec3 = Vec3::new(0.0, 0.1125, 0.6239);
/// How far in front of the eye the rear sight sits when aiming.
const AIM_DISTANCE: f32 = 0.28;
/// CS2's view model space (glTF: x left, y up, z forward) to the hand
/// pass's view space (x right, y up, z back).
const CS2_TO_VIEW: Mat4 = Mat4::from_cols_array(&[-1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);

#[derive(Deserialize)]
struct Doc {
    skeleton: SkeletonDoc,
    surfaces: Vec<SurfaceDoc>,
    clips: HashMap<String, ClipDoc>,
}

#[derive(Deserialize)]
struct SkeletonDoc {
    parents: Vec<i32>,
    /// Translation, rotation (x y z w), scale.
    rest: Vec<[f32; 10]>,
    /// Column-major.
    inverse_bind: Vec<[f32; 16]>,
    names: Vec<String>,
}

#[derive(Deserialize)]
struct SurfaceDoc {
    positions: Vec<f32>,
    normals: Vec<f32>,
    uvs: Vec<f32>,
    bones: Vec<u32>,
    weights: Vec<f32>,
    indices: Vec<u32>,
}

#[derive(Deserialize)]
struct ClipDoc {
    fps: f32,
    frames: usize,
    bones: HashMap<String, Vec<[f32; 10]>>,
}

#[derive(Clone, Copy)]
struct Trs {
    t: Vec3,
    r: Quat,
    s: Vec3,
}

impl Trs {
    fn from(v: &[f32; 10]) -> Self {
        Self {
            t: Vec3::new(v[0], v[1], v[2]),
            r: Quat::from_xyzw(v[3], v[4], v[5], v[6]).normalize(),
            s: Vec3::new(v[7], v[8], v[9]),
        }
    }

    fn lerp(self, other: Self, f: f32) -> Self {
        Self { t: self.t.lerp(other.t, f), r: self.r.slerp(other.r, f), s: self.s.lerp(other.s, f) }
    }

    fn matrix(self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.s, self.r, self.t)
    }
}

struct Surface {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    uvs: Vec<[f32; 2]>,
    bones: Vec<[u16; 4]>,
    weights: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

struct Clip {
    fps: f32,
    frames: usize,
    tracks: Vec<(usize, Vec<Trs>)>,
}

impl Clip {
    fn duration(&self) -> f32 {
        (self.frames.max(2) - 1) as f32 / self.fps
    }
}

struct Model {
    /// The weapon's root bone, which carries its sights.
    weapon_bone: Option<usize>,
    parents: Vec<i32>,
    rest: Vec<Trs>,
    inverse_bind: Vec<Mat4>,
    surfaces: Vec<Surface>,
    clips: HashMap<String, Clip>,
}

impl Model {
    fn load(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let doc: Doc = serde_json::from_slice(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let index: HashMap<&str, usize> = doc.skeleton.names.iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();
        let surfaces = doc
            .surfaces
            .into_iter()
            .map(|s| Surface {
                positions: s.positions.chunks_exact(3).map(|p| Vec3::new(p[0], p[1], p[2])).collect(),
                normals: s.normals.chunks_exact(3).map(|n| Vec3::new(n[0], n[1], n[2])).collect(),
                uvs: s.uvs.chunks_exact(2).map(|uv| [uv[0], uv[1]]).collect(),
                bones: s.bones.chunks_exact(4).map(|b| [b[0] as u16, b[1] as u16, b[2] as u16, b[3] as u16]).collect(),
                weights: s.weights.chunks_exact(4).map(|w| [w[0], w[1], w[2], w[3]]).collect(),
                indices: s.indices,
            })
            .collect();
        let clips = doc
            .clips
            .into_iter()
            .map(|(name, clip)| {
                let tracks = clip
                    .bones
                    .iter()
                    .filter_map(|(bone, frames)| Some((*index.get(bone.as_str())?, frames.iter().map(Trs::from).collect())))
                    .collect();
                (name, Clip { fps: clip.fps, frames: clip.frames, tracks })
            })
            .collect();
        Ok(Self {
            weapon_bone: index.get("weapon").copied(),
            parents: doc.skeleton.parents,
            rest: doc.skeleton.rest.iter().map(Trs::from).collect(),
            inverse_bind: doc.skeleton.inverse_bind.iter().map(Mat4::from_cols_array).collect(),
            surfaces,
            clips,
        })
    }

    /// Each bone's local pose in `clip` at `time` seconds.
    fn locals(&self, clip: &str, time: f32) -> Vec<Trs> {
        let mut locals = self.rest.clone();
        let Some(clip) = self.clips.get(clip) else { return locals };
        let at = (time * clip.fps).clamp(0.0, (clip.frames - 1) as f32);
        let (frame, f) = (at.floor() as usize, at.fract());
        for (bone, frames) in &clip.tracks {
            let a = frames[frame.min(frames.len() - 1)];
            let b = frames[(frame + 1).min(frames.len() - 1)];
            locals[*bone] = a.lerp(b, f);
        }
        locals
    }

    /// Skinning matrices for bone locals.
    fn skin(&self, locals: &[Trs]) -> Vec<Mat4> {
        let mut world: Vec<Mat4> = Vec::with_capacity(locals.len());
        for (i, local) in locals.iter().enumerate() {
            let m = local.matrix();
            let parent = self.parents[i];
            world.push(if parent >= 0 { world[parent as usize] * m } else { m });
        }
        world.iter().zip(&self.inverse_bind).map(|(w, b)| *w * *b).collect()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Which {
    Ak,
    Karambit,
}

struct Assets {
    ak: Model,
    karambit: Model,
    atlas: Arc<Vec<image::RgbaImage>>,
}

impl Assets {
    fn model(&self, which: Which) -> &Model {
        match which {
            Which::Ak => &self.ak,
            Which::Karambit => &self.karambit,
        }
    }

    fn load(dir: PathBuf) -> Result<Self, String> {
        let atlas = image::open(dir.join("atlas.png")).map_err(|e| format!("atlas.png: {e}"))?.to_rgba8();
        // Mip levels down to 16 texels, opaque: the texture's alpha is the
        // hand shader's cutout.
        let mut levels = vec![atlas];
        levels[0].pixels_mut().for_each(|p| p.0[3] = 255);
        while levels.last().is_some_and(|l| l.width() > 16 && l.height() > 16) {
            let last = levels.last().unwrap();
            let next = image::imageops::resize(last, last.width() / 2, last.height() / 2, image::imageops::FilterType::Triangle);
            levels.push(next);
        }
        Ok(Self { ak: Model::load(&dir.join("ak47.json"))?, karambit: Model::load(&dir.join("karambit.json"))?, atlas: Arc::new(levels) })
    }
}

enum LoadState {
    Idle,
    Loading,
    Ready(Arc<Assets>),
    Missing,
}

/// The converted assets, loaded in the background on first use.
fn assets() -> Option<Arc<Assets>> {
    static STATE: OnceLock<Mutex<LoadState>> = OnceLock::new();
    let state = STATE.get_or_init(|| Mutex::new(LoadState::Idle));
    let mut guard = state.lock().ok()?;
    match &*guard {
        LoadState::Ready(assets) => return Some(assets.clone()),
        LoadState::Loading | LoadState::Missing => return None,
        LoadState::Idle => {}
    }
    let dir = std::env::var_os("IW4L_CS2").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("iw4l-artifacts/cs2"));
    if !dir.join("atlas.png").is_file() {
        *guard = LoadState::Missing;
        return None;
    }
    *guard = LoadState::Loading;
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let loaded = Assets::load(dir);
        let mut guard = state.lock().expect("cs2 view model state");
        *guard = match loaded {
            Ok(assets) => {
                diag::info!(World, "CS2 view models loaded in {:.2}s", started.elapsed().as_secs_f32());
                LoadState::Ready(Arc::new(assets))
            }
            Err(error) => {
                diag::warn!(World, "CS2 view models did not load: {error}");
                LoadState::Missing
            }
        };
    });
    None
}

/// A clip playing on one of the weapons.
#[derive(Clone, Copy, Debug)]
struct Playing {
    which: Which,
    clip: &'static str,
    time: f32,
    speed: f32,
    looping: bool,
}

#[derive(Default)]
pub(crate) struct State {
    playing: Option<Playing>,
    /// The clip faded out of, and how far the fade has gone (0 to 1).
    from: Option<(Playing, f32)>,
    weapon: u32,
    shots: i32,
    melee: bool,
    reloading: bool,
    slash: u32,
}

impl State {
    fn play(&mut self, which: Which, clip: &'static str, speed: f32, looping: bool) {
        if std::env::var_os("IW4L_DEBUG_CS2").is_some() {
            diag::info!(World, "CS2 view model: {which:?} {clip} x{speed:.2}");
        }
        if let Some(current) = self.playing {
            self.from = Some((current, 0.0));
        }
        self.playing = Some(Playing { which, clip, time: 0.0, speed, looping });
    }

    fn reset(&mut self) {
        self.playing = None;
        self.from = None;
    }
}

fn playing_done(assets: &Assets, playing: &Playing) -> bool {
    !playing.looping && assets.model(playing.which).clips.get(playing.clip).is_none_or(|c| playing.time >= c.duration())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    presented: Res<net::PresentedSnapshot>,
    local: Res<net::LocalPresentClient>,
    weapons: Option<Res<assets::PreparedWeapons>>,
    (mut ui, puppet, skate): (ResMut<frame::MinecraftUi>, Res<frame::InventoryPuppet>, Res<frame::SkateMode>),
    mut covering: ResMut<frame::Cs2Viewmodel>,
    mut view: ResMut<MinecraftWorldView>,
    mut state: Local<State>,
) {
    view.cs2_hand = Default::default();
    covering.covering = false;
    ui.knife_held = false;
    let shown = !ui.hide_gun && view.active && ui.active && !skate.active && !puppet.active && !(ui.holding_item && !ui.empty_hand);
    // Hidden a moment (the view weapon can drop out for a frame, as in a
    // melee), the clip keeps its place; dead, it starts over.
    let Some(ps) = presented.viewweapon_player(local.0).filter(|ps| shown && ps.pm_type == 0) else {
        if presented.player(local.0).is_none_or(|ps| ps.pm_type != 0) {
            state.reset();
        }
        return;
    };
    let (Some(assets), Some(weapons)) = (assets(), weapons) else { return };
    view.cs2_texture.get_or_insert_with(|| assets.atlas.clone());

    let name = weapons.0.name_of(ps.weapon);
    let is_ak = name.starts_with("ak47");
    // The karambit held as a weapon: MW2's USP .45 with its tactical knife
    // stands in (its attacks are made melee as it is held).
    let is_knife = name.starts_with("usp") && name.contains("tactical");
    ui.knife_held = is_knife;
    let weapon_state = WeaponState::from_i32(ps.weaponstate_primary).ok();
    let melee = matches!(weapon_state, Some(WeaponState::MeleeInit | WeaponState::MeleeFire));
    let reloading = matches!(
        weapon_state,
        Some(WeaponState::Reloading | WeaponState::ReloadStart | WeaponState::ReloadEnd)
    );
    // A slash plays out whatever is in hand (not the karambit's own draw,
    // idle or inspect).
    let knifing = state
        .playing
        .is_some_and(|p| p.which == Which::Karambit && p.clip.starts_with("light_hit") && !playing_done(&assets, &p));
    if std::env::var_os("IW4L_DEBUG_CS2").is_some() {
        diag::info!(World, "CS2 frame: weapon {} {name} state {:?} melee {melee} knifing {knifing} playing {:?}", ps.weapon, weapon_state, state.playing.map(|p| (p.which, p.clip, p.time)));
    }

    if melee && !state.melee {
        // A slash, alternating as CS's light attacks do.
        state.slash += 1;
        let clip = if state.slash % 2 == 1 { "light_hit1" } else { "light_hit2" };
        state.play(Which::Karambit, clip, 1.0, false);
    } else if knifing {
        // The slash plays out.
    } else if is_ak {
        let current = state.playing.filter(|p| p.which == Which::Ak);
        if current.is_none() || ps.weapon != state.weapon {
            state.play(Which::Ak, "draw", 1.0, false);
        } else if reloading && !state.reloading {
            // Over MW2's reload time, as the reload's own length.
            let length = assets.ak.clips.get("reload").map_or(2.5, Clip::duration);
            let reload = (ps.weapon_time as f32 / 1000.0).max(0.5);
            state.play(Which::Ak, "reload", length / reload, false);
        } else if ps.weapon_shot_count != state.shots && !reloading {
            state.play(Which::Ak, "shoot", 1.0, false);
        } else if keys.just_pressed(INSPECT_KEY) && !reloading {
            state.play(Which::Ak, "inspect", 1.0, false);
        } else if current.is_some_and(|p| playing_done(&assets, &p)) {
            state.play(Which::Ak, "idle", 1.0, true);
        }
    } else if is_knife {
        let current = state.playing.filter(|p| p.which == Which::Karambit);
        if current.is_none() || ps.weapon != state.weapon {
            state.play(Which::Karambit, "draw", 1.0, false);
        } else if keys.just_pressed(INSPECT_KEY) {
            state.play(Which::Karambit, "inspect", 1.0, false);
        } else if current.is_some_and(|p| playing_done(&assets, &p)) {
            state.play(Which::Karambit, "idle", 1.0, true);
        }
    } else {
        state.reset();
    }
    state.weapon = ps.weapon;
    state.shots = ps.weapon_shot_count;
    state.melee = melee;
    state.reloading = reloading;

    let Some(mut playing) = state.playing else { return };
    let dt = time.delta_secs();
    playing.time += dt * playing.speed;
    if playing.looping
        && let Some(clip) = assets.model(playing.which).clips.get(playing.clip)
    {
        playing.time %= clip.duration().max(0.01);
    }
    state.playing = Some(playing);
    let model = assets.model(playing.which);
    let mut locals = model.locals(playing.clip, playing.time);
    if let Some((mut from, fade)) = state.from {
        let fade = fade + dt / BLEND;
        if fade >= 1.0 || from.which != playing.which {
            state.from = None;
        } else {
            from.time += dt * from.speed;
            let old = model.locals(from.clip, from.time);
            for (bone, value) in locals.iter_mut().enumerate() {
                *value = old[bone].lerp(*value, fade);
            }
            state.from = Some((from, fade));
        }
    }
    covering.covering = true;

    // Sprinting lowers and turns the gun (CS has no sprint).
    let sprint = if ps.pm_flags & 0x4000 != 0 { 1.0 } else { 0.0 };
    // Aiming down the sights (CS has none for the AK), over MW2's own aim
    // in and out: the view model turned and moved so that the idle pose's
    // sights lie on the view's centre line, the clips still moving it
    // about that.
    let ads = if playing.which == Which::Ak { ps.f_weapon_pos_frac.clamp(0.0, 1.0) } else { 0.0 };
    let aim = model
        .weapon_bone
        .filter(|_| ads > 0.0)
        .map(|bone| {
            let idle = CS2_TO_VIEW * model.skin(&model.locals("idle", 0.0))[bone];
            let (rear, front) = (idle.transform_point3(SIGHT_REAR), idle.transform_point3(SIGHT_FRONT));
            let turn = Quat::from_rotation_arc((front - rear).normalize_or(Vec3::NEG_Z), Vec3::NEG_Z);
            let shift = Vec3::new(0.0, 0.0, -AIM_DISTANCE) - turn * rear;
            Mat4::from_rotation_translation(Quat::IDENTITY.slerp(turn, ads), shift * ads)
        })
        .unwrap_or(Mat4::IDENTITY);
    let place = Mat4::from_translation(OFFSET * (1.0 - ads) + Vec3::new(0.0, -0.05 * sprint, 0.0))
        * Mat4::from_rotation_x(-0.5 * sprint)
        * Mat4::from_rotation_y(0.35 * sprint)
        * aim
        * CS2_TO_VIEW;
    let fov = Mat4::from_scale(Vec3::new(FOV_SCALE, FOV_SCALE, 1.0) * DEPTH_SCALE);
    let skin = model.skin(&locals);
    let light_dir = Vec3::new(0.35, 0.8, 0.5).normalize();
    let [sky, block] = view.eye_light;
    let (mut bytes, mut indices) = (Vec::new(), Vec::new());
    for surface in &model.surfaces {
        let base = (bytes.len() / std::mem::size_of::<SectionVertex>()) as u32;
        for v in 0..surface.positions.len() {
            let (mut p, mut n) = (Vec3::ZERO, Vec3::ZERO);
            for k in 0..4 {
                let w = surface.weights[v][k];
                if w <= 0.0 {
                    continue;
                }
                let m = skin[surface.bones[v][k] as usize];
                p += w * m.transform_point3(surface.positions[v]);
                n += w * Mat3::from_mat4(m).mul_vec3(surface.normals[v]);
            }
            let p = place.transform_point3(p);
            let n = Mat3::from_mat4(place).mul_vec3(n).normalize_or_zero();
            let shade = 0.5 + 0.5 * n.dot(light_dir).max(0.0);
            let vertex = Vertex {
                position: fov.transform_point3(p).to_array(),
                uv: surface.uvs[v],
                color: [shade, shade, shade, 1.0],
                sky_light: sky,
                block_light: block,
            };
            bytes.extend_from_slice(bytemuck::bytes_of(&SectionVertex::from_vertex(&vertex)));
        }
        indices.extend(surface.indices.iter().map(|i| i + base));
    }
    view.cs2_hand = (bytes, indices);
}

