//! The End's boss models, from pinned 26.3 sheets: `EnderDragonModel`'s
//! parts (head and jaw, a neck and tail of scaled segments, the body, both
//! two-jointed wings, the legs) with its wing flap, `EndCrystalModel`'s
//! turning glass around its core, the beam a crystal heals the dragon with,
//! and the dragon's fireball. Vanilla bends the neck and tail along the
//! dragon's recent flight path; here they wave about a straight line.
use crate::{
    cow_render::cube_scaled,
    mesh::{Atlas, ChunkMesh, Vertex},
    pack::ResourceId,
};
use glam::{DVec3, Quat, Vec3};

pub use minecraftoss_generator::feature::kinds::the_end::{spikes_for_seed, Spike};

/// A cuboid of a model part: from, to, texture offset, mirrored.
type Cube = ([f32; 3], [f32; 3], [f32; 2], bool);

const DRAGON_SHEET: [f32; 2] = [256.0, 256.0];

/// Where and how the dragon is drawn.
#[derive(Clone, Copy, Debug)]
pub struct DragonPose {
    /// The body's centre, in blocks.
    pub position: DVec3,
    /// Minecraft yaw and pitch in degrees: yaw 0 faces +Z, pitch up is
    /// positive.
    pub yaw: f32,
    pub pitch: f32,
    /// `EnderDragon.flapTime` in turns of the wing beat.
    pub flap: f32,
    /// Ticks lived, for the neck and tail's wave.
    pub age: f32,
    /// How open the jaw is, 0 to 1.
    pub jaw: f32,
    /// Red for a moment after a hit, 0 to 1.
    pub hurt: f32,
}

impl DragonPose {
    /// The body's turn: model forward (-Z) along the flight.
    pub fn rotation(&self) -> Quat {
        Quat::from_rotation_y(std::f32::consts::PI - self.yaw.to_radians()) * Quat::from_rotation_x(self.pitch.to_radians())
    }

    /// Unit vectors forward along the flight and to the dragon's right.
    pub fn axes(&self) -> (Vec3, Vec3) {
        let r = self.rotation();
        (r * Vec3::NEG_Z, r * Vec3::NEG_X)
    }
}

/// One posed part: its turn and origin in model pixels, and its cuboids.
struct Part {
    rotation: Quat,
    pivot: Vec3,
    cubes: Vec<Cube>,
}

fn child(parent: (Quat, Vec3), pivot: [f32; 3], rotation: Quat) -> (Quat, Vec3) {
    (parent.0 * rotation, parent.0 * Vec3::from_array(pivot) + parent.1)
}

fn dragon_parts(pose: &DragonPose) -> (Vec<Part>, Vec<Part>) {
    let mut parts = Vec::new();
    let root = (Quat::IDENTITY, Vec3::ZERO);
    let segment: Vec<Cube> = vec![([-5., -5., -5.], [5., 5., 5.], [192., 104.], false), ([-1., -9., -3.], [1., -5., 3.], [48., 0.], false)];

    // The body and its back scales.
    let body = child(root, [0., 4., 8.], Quat::IDENTITY);
    parts.push(Part {
        rotation: body.0,
        pivot: body.1,
        cubes: vec![
            ([-12., 0., -16.], [12., 24., 48.], [0., 0.], false),
            ([-1., -6., -10.], [1., 0., 2.], [220., 53.], false),
            ([-1., -6., 10.], [1., 0., 22.], [220., 53.], false),
            ([-1., -6., 30.], [1., 0., 42.], [220., 53.], false),
        ],
    });

    // The neck forward from the body's front, waving.
    let wave = |i: f32, speed: f32, size: f32| (pose.age * speed + i * 0.45).sin() * size;
    let mut neck_end = Vec3::new(0., 12., -8.);
    for i in 0..5 {
        let fi = i as f32;
        let at = [0., 12. + wave(fi, 0.08, 1.5) - fi * 0.8, -13. - fi * 10.];
        neck_end = Vec3::from_array(at);
        parts.push(Part { rotation: Quat::from_rotation_x(-0.08), pivot: neck_end, cubes: segment.clone() });
    }
    // The head and its jaw.
    let head = child(root, [0., neck_end.y - 1.0, neck_end.z - 12.0], Quat::from_rotation_x(-0.1));
    let mut heads = vec![Part {
        rotation: head.0,
        pivot: head.1,
        cubes: vec![
            ([-6., -1., -24.], [6., 4., -8.], [176., 44.], false),
            ([-8., -8., -10.], [8., 8., 6.], [112., 30.], false),
            ([-5., -3., -22.], [-3., -1., -18.], [112., 0.], true),
            ([-5., -12., -4.], [-3., -8., 2.], [0., 0.], true),
            ([3., -3., -22.], [5., -1., -18.], [112., 0.], false),
            ([3., -12., -4.], [5., -8., 2.], [0., 0.], false),
        ],
    }];
    let jaw = child(head, [0., 4., -8.], Quat::from_rotation_x(0.1 + pose.jaw * 0.6));
    heads.push(Part { rotation: jaw.0, pivot: jaw.1, cubes: vec![([-6., 0., -16.], [6., 4., 0.], [176., 65.], false)] });

    // The tail back from the body, waving more the further it goes.
    for i in 0..12 {
        let fi = i as f32;
        let at = [wave(fi, 0.06, fi * 0.35), 12. + fi * 0.6 + wave(fi + 3.0, 0.05, 1.0), 61. + fi * 10.];
        parts.push(Part { rotation: Quat::from_rotation_x(0.05), pivot: Vec3::from_array(at), cubes: segment.clone() });
    }

    // The wings: `setupAnim`'s beat, the tips folding against the bones.
    let beat = pose.flap * std::f32::consts::TAU;
    for side in [-1.0f32, 1.0] {
        let mirror = side > 0.0;
        let (bone, skin, tip_bone, tip_skin) = if mirror {
            (
                ([0., -4., -4.], [56., 4., 4.]),
                ([0., 0., 2.], [56., 0., 58.]),
                ([0., -2., -2.], [56., 2., 2.]),
                ([0., 0., 2.], [56., 0., 58.]),
            )
        } else {
            (
                ([-56., -4., -4.], [0., 4., 4.]),
                ([-56., 0., 2.], [0., 0., 58.]),
                ([-56., -2., -2.], [0., 2., 2.]),
                ([-56., 0., 2.], [0., 0., 58.]),
            )
        };
        let wing_turn = Quat::from_euler(
            glam::EulerRot::ZYX,
            side * -((beat.sin() + 0.125) * 0.8),
            side * -0.25,
            0.125 - beat.cos() * 0.2,
        );
        let wing = child(root, [12. * side, 5., 2.], wing_turn);
        parts.push(Part {
            rotation: wing.0,
            pivot: wing.1,
            cubes: vec![(bone.0, bone.1, [112., 88.], mirror), (skin.0, skin.1, [-56., 88.], mirror)],
        });
        let tip = child(wing, [56. * side, 0., 0.], Quat::from_rotation_z(side * ((beat + 2.0).sin() + 0.5) * 0.75));
        parts.push(Part {
            rotation: tip.0,
            pivot: tip.1,
            cubes: vec![(tip_bone.0, tip_bone.1, [112., 136.], mirror), (tip_skin.0, tip_skin.1, [-56., 144.], mirror)],
        });
        // The legs, tucked back in flight.
        let front = child(root, [12. * side, 20., 2.], Quat::from_rotation_x(1.3));
        parts.push(Part { rotation: front.0, pivot: front.1, cubes: vec![([-4., -4., -4.], [4., 20., 4.], [112., 104.], mirror)] });
        let rear = child(root, [16. * side, 16., 42.], Quat::from_rotation_x(1.0));
        parts.push(Part { rotation: rear.0, pivot: rear.1, cubes: vec![([-8., -4., -8.], [8., 28., 8.], [0., 0.], mirror)] });
    }
    (parts, heads)
}

/// The Ender Dragon, full bright as the boss it is, red after a hit, its
/// eyes glowing.
pub fn append_dragon(mesh: &mut ChunkMesh, pose: &DragonPose, atlas: &Atlas) {
    let skin_id = ResourceId::parse("minecraft:entity/enderdragon/dragon").unwrap();
    if !atlas.contains(&skin_id) {
        return;
    }
    let skin = atlas.entity_region(&skin_id);
    let eyes_id = ResourceId::parse("minecraft:entity/enderdragon/dragon_eyes").unwrap();
    let eyes = atlas.contains(&eyes_id).then(|| atlas.entity_region(&eyes_id));
    let rotation = pose.rotation();
    // The body's centre sits half a block over the model's feet.
    let feet = pose.position - (rotation * Vec3::new(0.0, 0.5, 0.0)).as_dvec3();
    let tint = [1.0, 1.0 - pose.hurt * 0.6, 1.0 - pose.hurt * 0.6];
    let (parts, heads) = dragon_parts(pose);
    let mut draw = |parts: &[Part], region: [f32; 4], tint: [f32; 3]| {
        for part in parts {
            for &(from, to, uv, mirror) in &part.cubes {
                cube_scaled(mesh, feet, rotation, Vec3::ONE, region, 15.0, 15.0, from, to, uv, part.pivot.to_array(), part.rotation, tint, DRAGON_SHEET, None, mirror);
            }
        }
    };
    draw(&parts, skin, tint);
    draw(&heads, skin, tint);
    if let Some(eyes) = eyes {
        draw(&heads, eyes, [1.0; 3]);
    }
}

/// An end crystal: its base, and the glass and core turning and bobbing
/// over it (`EndCrystalRenderer`). `position` is the crystal's feet.
pub fn append_crystal(mesh: &mut ChunkMesh, position: DVec3, age: f32, atlas: &Atlas) {
    let id = ResourceId::parse("minecraft:entity/end_crystal/end_crystal").unwrap();
    if !atlas.contains(&id) {
        return;
    }
    let region = atlas.entity_region(&id);
    let sheet = [64.0, 32.0];
    let base_feet = position + DVec3::new(0.0, 24.016 / 16.0 - 0.25, 0.0);
    // The base: flat on the pillar.
    cube_scaled(mesh, base_feet, Quat::IDENTITY, Vec3::ONE, region, 15.0, 15.0, [-6., 0., -6.], [6., 4., 6.], [0., 16.], [0., 0., 0.], Quat::IDENTITY, [1.0; 3], sheet, None, false);
    // `EndCrystalRenderer.getY`: the bob.
    let bob = (age * 0.2).sin() / 2.0 + 0.5;
    let bob = (bob * bob + bob) * 0.4 - 1.4;
    let spin = Quat::from_rotation_y(age * 3.0f32.to_radians());
    let tilt = Quat::from_axis_angle(Vec3::new(1.0, 0.0, 1.0).normalize(), 60.0f32.to_radians());
    let centre = position + DVec3::new(0.0, 1.2 + f64::from(bob) + 1.4, 0.0);
    let cubes = [(1.0f32, [0., 0.], spin * tilt), (0.875, [0., 0.], spin * tilt * spin * tilt), (0.765, [32., 0.], spin * tilt * spin * tilt * spin * tilt)];
    for (scale, uv, turn) in cubes {
        let feet = centre - (turn * Vec3::new(0.0, (24.016 / 16.0) * scale, 0.0)).as_dvec3();
        cube_scaled(mesh, feet, turn, Vec3::splat(scale), region, 15.0, 15.0, [-4., -4., -4.], [4., 4., 4.], uv, [0., 0., 0.], Quat::IDENTITY, [1.0; 3], sheet, None, false);
    }
}

/// A flat quad of a whole region, seen from both sides, full bright.
fn quad(mesh: &mut ChunkMesh, corners: [Vec3; 4], [u0, v0, u1, v1]: [f32; 4]) {
    let uvs = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]];
    let start = mesh.vertices.len() as u32;
    for (corner, uv) in corners.into_iter().zip(uvs) {
        mesh.vertices.push(Vertex { position: corner.to_array(), uv, color: [1.0; 4], sky_light: 15.0, block_light: 15.0 });
    }
    mesh.indices.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
    mesh.indices.extend_from_slice(&[start, start + 2, start + 1, start, start + 3, start + 2]);
    mesh.faces += 2;
}

/// The beam from a crystal to the dragon it heals: two crossed ribbons of
/// the beam sheet along the line.
pub fn append_beam(mesh: &mut ChunkMesh, from: DVec3, to: DVec3, atlas: &Atlas) {
    let id = ResourceId::parse("minecraft:entity/end_crystal/end_crystal_beam").unwrap();
    if !atlas.contains(&id) {
        return;
    }
    let region = atlas.entity_region(&id);
    let (a, b) = (from.as_vec3(), to.as_vec3());
    let along = b - a;
    if along.length() < 0.1 {
        return;
    }
    let dir = along.normalize();
    let side = dir.any_orthonormal_vector() * 0.2;
    let other = dir.cross(side).normalize() * 0.2;
    for offset in [side, other] {
        quad(mesh, [a - offset, a + offset, b + offset, b - offset], region);
    }
}

/// The dragon's fireball: three crossed squares of its sheet, turning.
pub fn append_fireball(mesh: &mut ChunkMesh, position: DVec3, age: f32, atlas: &Atlas) {
    let id = ResourceId::parse("minecraft:entity/enderdragon/dragon_fireball").unwrap();
    if !atlas.contains(&id) {
        return;
    }
    let region = atlas.entity_region(&id);
    let turn = Quat::from_rotation_y(age * 0.3) * Quat::from_rotation_x(age * 0.2);
    let c = position.as_vec3();
    let h = 0.5;
    for (u, v) in [(Vec3::X, Vec3::Y), (Vec3::Y, Vec3::Z), (Vec3::Z, Vec3::X)] {
        let (u, v) = (turn * u * h, turn * v * h);
        quad(mesh, [c - u - v, c + u - v, c + u + v, c - u + v], region);
    }
}

/// `EnderDragonRenderer.renderRays`: the light bursting from a dying
/// dragon. `time` runs 0 to 1 over the ten-second death; more rays come as
/// it goes, each a thin pyramid white at the heart and purple and clear at
/// its tip, all turning, fading out at the end. Drawn blended, unlit.
pub fn append_death_rays(mesh: &mut ChunkMesh, centre: DVec3, time: f32, atlas: &Atlas) {
    let white = ResourceId::parse("minecraft:block/white_concrete").unwrap();
    if !atlas.contains(&white) || time <= 0.0 {
        return;
    }
    // One texel's colour for every vertex: the rays are untextured.
    let [u0, v0, u1, v1] = atlas.region(&white);
    let uv = [(u0 + u1) * 0.5, (v0 + v1) * 0.5];
    let fade = if time > 0.8 { ((time - 0.8) / 0.2).min(1.0) } else { 0.0 };
    let count = (((time + time * time) / 2.0) * 60.0) as u32;
    let c = centre.as_vec3();
    // The renderer's fixed seed: the same rays every frame.
    let mut seed: u64 = 432;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((seed >> 40) as f32) / ((1u64 << 24) as f32)
    };
    let half_sqrt3 = 3.0f32.sqrt() / 2.0;
    for _ in 0..count.min(60) {
        let turn = Quat::from_rotation_x(next() * std::f32::consts::TAU)
            * Quat::from_rotation_y(next() * std::f32::consts::TAU)
            * Quat::from_rotation_z(next() * std::f32::consts::TAU)
            * Quat::from_rotation_x(next() * std::f32::consts::TAU)
            * Quat::from_rotation_y(next() * std::f32::consts::TAU)
            * Quat::from_rotation_z((next() + time * 0.5) * std::f32::consts::TAU);
        let length = next() * 20.0 + 5.0 + fade * 10.0;
        let width = next() * 2.0 + 1.0 + fade * 2.0;
        let tip = [
            Vec3::new(-half_sqrt3 * width, length, -0.5 * width),
            Vec3::new(half_sqrt3 * width, length, -0.5 * width),
            Vec3::new(0.0, length, width),
        ]
        .map(|p| c + turn * p);
        let start = mesh.vertices.len() as u32;
        mesh.vertices.push(Vertex { position: c.to_array(), uv, color: [1.0, 1.0, 1.0, 1.0 - fade], sky_light: 15.0, block_light: 15.0 });
        for p in tip {
            mesh.vertices.push(Vertex { position: p.to_array(), uv, color: [1.0, 0.0, 1.0, 0.0], sky_light: 15.0, block_light: 15.0 });
        }
        for (a, b) in [(1, 2), (2, 3), (3, 1)] {
            mesh.indices.extend_from_slice(&[start, start + a, start + b]);
        }
        mesh.faces += 3;
    }
}
