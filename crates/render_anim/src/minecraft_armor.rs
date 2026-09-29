//! The player's worn Minecraft armour on their MW2 soldier: in the
//! inventory's character window and while skating, the two places the local
//! body is seen whole. Each piece is a box of vanilla's armour sheet
//! (`HumanoidArmorLayer`'s head, body, arm and leg cubes) set along the
//! soldier's posed bones, sized to the soldier rather than to a Minecraft
//! humanoid: the helmet over the head, the chestplate from the lower spine
//! to the neck with sleeves to the elbows, the leggings round the hips and
//! down to the knees, the boots from the knees past the ankles.
use bevy::prelude::*;
use glam::{DVec3, Vec3};
use minecraft_terrain::armor_render::armor_asset;
use minecraft_terrain::mesh::{Atlas, ChunkMesh, Vertex};
use minecraft_terrain::pack::ResourceId;

use crate::anim::dobj_pose::HostDObjPoseFrame;
use crate::anim::remote_body::RemoteBodyTrees;
use crate::minecraft_world::MinecraftWorldView;

/// `HumanoidArmorLayer`'s undyed leather.
const UNDYED_LEATHER: [f32; 3] = [160.0 / 255.0, 101.0 / 255.0, 64.0 / 255.0];

/// A cube of the 64 by 32 armour sheet: its texture offset and size in
/// pixels (`ModelPart.Cube`'s layout).
#[derive(Clone, Copy)]
struct Sheet {
    u: f32,
    v: f32,
    w: f32,
    h: f32,
    d: f32,
}

const HEAD: Sheet = Sheet { u: 0., v: 0., w: 8., h: 8., d: 8. };
const BODY: Sheet = Sheet { u: 16., v: 16., w: 8., h: 12., d: 4. };
const ARM: Sheet = Sheet { u: 40., v: 16., w: 4., h: 12., d: 4. };
const LEG: Sheet = Sheet { u: 0., v: 16., w: 4., h: 12., d: 4. };

/// The soldier's joints, in map units.
struct Skeleton {
    head: Vec3,
    neck: Vec3,
    spine: Vec3,
    root: Vec3,
    shoulders: [Vec3; 2],
    elbows: [Vec3; 2],
    hips: [Vec3; 2],
    knees: [Vec3; 2],
    ankles: [Vec3; 2],
}

impl Skeleton {
    fn read(poses: &HostDObjPoseFrame, dobj: &xmodel_runtime::DObj, key: u32) -> Option<Self> {
        let at = |name: &str| -> Option<Vec3> {
            let index = dobj.find(name)?;
            let bolt = poses.resolve(key, index as i32).ok()?;
            Some(Vec3::from_array(bolt.origin))
        };
        let pair = |a: &str, b: &str| -> Option<[Vec3; 2]> { Some([at(a)?, at(b)?]) };
        Some(Self {
            head: at("j_head")?,
            neck: at("j_neck")?,
            spine: at("j_spinelower")?,
            root: at("j_mainroot")?,
            shoulders: pair("j_shoulder_ri", "j_shoulder_le")?,
            elbows: pair("j_elbow_ri", "j_elbow_le")?,
            hips: pair("j_hip_ri", "j_hip_le")?,
            knees: pair("j_knee_ri", "j_knee_le")?,
            ankles: pair("j_ankle_ri", "j_ankle_le")?,
        })
    }
}

/// An oriented box in map units: centre, right, up and forward half extents.
struct Piece {
    centre: Vec3,
    right: Vec3,
    up: Vec3,
    forward: Vec3,
    sheet: Sheet,
}

/// A box along a limb from `a` to `b` (past `b` by `beyond`), `thick` across.
fn limb(a: Vec3, b: Vec3, beyond: f32, thick: f32, forward: Vec3, sheet: Sheet) -> Piece {
    let along = (b - a).normalize_or(Vec3::NEG_Z);
    let length = a.distance(b) + beyond;
    let right = along.cross(forward).normalize_or(Vec3::X);
    let front = right.cross(along).normalize_or(Vec3::Y);
    Piece {
        centre: a + along * (length * 0.5),
        right: right * thick * 0.5,
        // The sheet's up runs from the limb's top (at `a`) down to `b`.
        up: -along * length * 0.5,
        forward: front * thick * 0.5,
        sheet,
    }
}

/// A box standing along `up`, facing `forward`, of width, height and depth.
fn upright(centre: Vec3, up: Vec3, forward: Vec3, [w, h, d]: [f32; 3], sheet: Sheet) -> Piece {
    let up = up.normalize_or(Vec3::Z);
    let right = forward.cross(up).normalize_or(Vec3::X);
    let front = up.cross(right).normalize_or(Vec3::Y);
    Piece { centre, right: right * w * 0.5, up: up * h * 0.5, forward: front * d * 0.5, sheet }
}

/// The pieces of the worn armour (`[head, chest, legs, feet]` item IDs) on a
/// posed soldier.
fn pieces(bones: &Skeleton, armor: &[Option<String>; 4]) -> Vec<(Piece, String, Option<[f32; 3]>)> {
    // Everything scales with the soldier's torso (the puppet is drawn small).
    let unit = bones.root.distance(bones.neck) / 20.0;
    let s = |n: f32| n * unit;
    let up = (bones.neck - bones.root).normalize_or(Vec3::Z);
    // The body faces across its hips: left hip minus right, crossed with up.
    let left = (bones.hips[1] - bones.hips[0]).normalize_or(Vec3::Y);
    let forward = left.cross(up).normalize_or(Vec3::X);
    let mut out = Vec::new();
    for (slot, item) in armor.iter().enumerate() {
        let Some(item) = item else { continue };
        let Some((asset, _)) = armor_asset(item) else { continue };
        let tint = (asset == "leather").then_some(UNDYED_LEATHER);
        let sheet_dir = if slot == 2 { "humanoid_leggings" } else { "humanoid" };
        let texture = format!("minecraft:entity/equipment/{sheet_dir}/{asset}");
        let mut add = |piece: Piece| out.push((piece, texture.clone(), tint));
        match slot {
            0 => {
                let head_up = (bones.head - bones.neck).normalize_or(up);
                add(upright(bones.head + head_up * s(4.5), head_up, forward, [s(11.5), s(11.0), s(12.5)], HEAD));
            }
            1 => {
                let centre = (bones.spine + bones.neck) * 0.5;
                let height = bones.spine.distance(bones.neck) + s(3.0);
                add(upright(centre, up, forward, [s(17.0), height, s(11.0)], BODY));
                for i in 0..2 {
                    add(limb(bones.shoulders[i], bones.elbows[i], s(1.0), s(6.5), forward, ARM));
                }
            }
            2 => {
                add(upright(bones.root + up * s(1.0), up, forward, [s(16.0), s(8.0), s(11.0)], BODY));
                for i in 0..2 {
                    add(limb(bones.hips[i], bones.knees[i], s(1.0), s(7.0), forward, LEG));
                }
            }
            _ => {
                for i in 0..2 {
                    add(limb(bones.knees[i], bones.ankles[i], s(2.5), s(6.8), forward, LEG));
                }
            }
        }
    }
    out
}

/// A map point in the Minecraft world's blocks (`sim::voxel::to_block`).
fn to_block(origin: [f64; 3], p: Vec3) -> [f32; 3] {
    let p = p.as_dvec3();
    let b = DVec3::new(origin[0] + p.x / 36.0, origin[1] + p.z / 36.0, origin[2] - p.y / 36.0);
    b.as_vec3().to_array()
}

/// A piece's six faces, textured as `ModelPart.Cube` lays its sheet out.
fn append_piece(mesh: &mut ChunkMesh, origin: [f64; 3], piece: &Piece, region: [f32; 4], tint: [f32; 3], light: [f32; 2]) {
    let Sheet { u, v, w, h, d } = piece.sheet;
    let [u0, v0, u1, v1] = region;
    let uv = |x: f32, y: f32| [u0 + (u1 - u0) * x / 64.0, v0 + (v1 - v0) * y / 32.0];
    let (c, r, up, f) = (piece.centre, piece.right, piece.up, piece.forward);
    // Each face: its outward axis, the axes across (left to right) and down
    // it as seen from outside, its sheet rectangle and its shade.
    let faces: [(Vec3, Vec3, Vec3, [f32; 4], f32); 6] = [
        (f, -r, -up, [u + d, v + d, w, h], 0.8),
        (-f, r, -up, [u + d + w + d, v + d, w, h], 0.8),
        (r, f, -up, [u + d + w, v + d, d, h], 0.6),
        (-r, -f, -up, [u, v + d, d, h], 0.6),
        (up, -r, f, [u + d, v, w, d], 1.0),
        (-up, -r, -f, [u + d + w, v, w, d], 0.5),
    ];
    for (out, across, down, [x, y, fw, fh], shade) in faces {
        let corners = [c + out - across - down, c + out + across - down, c + out + across + down, c + out - across + down];
        let uvs = [uv(x, y), uv(x + fw, y), uv(x + fw, y + fh), uv(x, y + fh)];
        let start = mesh.vertices.len() as u32;
        for (corner, coord) in corners.into_iter().zip(uvs) {
            mesh.vertices.push(Vertex {
                position: to_block(origin, corner),
                uv: coord,
                color: [tint[0] * shade, tint[1] * shade, tint[2] * shade, 1.0],
                sky_light: light[0],
                block_light: light[1],
            });
        }
        mesh.indices.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
        mesh.faces += 1;
    }
}

/// The local player's armour on their soldier, after the bodies are posed:
/// in the inventory's window (its own depth band, as the soldier is drawn
/// there) or with the other entity models while skating.
pub(crate) fn draw_player_armor(
    mut view: ResMut<MinecraftWorldView>,
    poses: Res<HostDObjPoseFrame>,
    trees: Res<RemoteBodyTrees>,
    puppet: Res<frame::InventoryPuppet>,
    skate: Res<frame::SkateMode>,
    ui: Res<frame::MinecraftUi>,
) {
    view.entity_meshes[4] = Default::default();
    let Some(atlas) = view.atlas.clone() else { return };
    if !view.active || !ui.active {
        return;
    }
    let (key, in_window) = if puppet.active {
        (puppet.client, true)
    } else if skate.active {
        (skate.client, false)
    } else {
        return;
    };
    // `MC_INVENTORY_SLOTS`: 36 feet .. 39 head.
    let slot = |i: usize| ui.slots.get(i).and_then(Option::as_ref).filter(|s| s.weapon.is_none()).map(|s| s.id.clone());
    let armor = [slot(39), slot(38), slot(37), slot(36)];
    if armor.iter().all(Option::is_none) {
        return;
    }
    let Some(dobj) = trees.get(key).and_then(|tree| tree.dobj.clone()) else { return };
    let Some(bones) = Skeleton::read(&poses, &dobj, key) else { return };
    let mut mesh = ChunkMesh::default();
    let light = if in_window { [15.0, 15.0] } else { view.eye_light };
    for (piece, texture, tint) in pieces(&bones, &armor) {
        let Ok(id) = ResourceId::parse(&texture) else { continue };
        if !atlas.contains(&id) {
            continue;
        }
        append_piece(&mut mesh, view.origin, &piece, atlas.entity_region(&id), tint.unwrap_or([1.0; 3]), light);
    }
    append(&mut view.entity_meshes[if in_window { 4 } else { 0 }], &mesh, &atlas);
}

/// Adds a mesh's vertices and indices to an entity mesh's raw buffers.
fn append(target: &mut (Vec<u8>, Vec<u32>), mesh: &ChunkMesh, _atlas: &Atlas) {
    let base = (target.0.len() / std::mem::size_of::<Vertex>()) as u32;
    target.0.extend_from_slice(bytemuck::cast_slice(&mesh.vertices));
    target.1.extend(mesh.indices.iter().map(|i| i + base));
}
