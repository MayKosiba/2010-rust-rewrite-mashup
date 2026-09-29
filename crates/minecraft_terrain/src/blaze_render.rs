//! Pack-backed pinned 26.3 blaze: `BlazeModel.createBodyLayer` (a head and
//! twelve rods) and its `setupAnim` (the head's turn and pitch; the rods
//! circling in three rings of four, each bobbing on its own phase), drawn
//! full bright as `BlazeRenderer.getBlockLightLevel` asks (block light
//! 15). The model's parts sit where vanilla puts them: like every living
//! model its y = 24 is the feet, so the head (−4 to 4 about y = 0) tops out
//! at 1.75 blocks under the 1.8-block box and the lowest rods hang about a
//! quarter block above the ground; no offset is added.
use crate::{
    client_mobs::ClientMobs,
    cow_render::cube_scaled,
    lighting::SkyLight,
    mesh::{Atlas, ChunkMesh},
    pack::ResourceId,
};
use glam::{Quat, Vec3};
use minecraftoss_entities::world::BlazeEntity;
use std::f32::consts::PI;

/// `BlazeModel`'s head: texOffs(0, 0), an 8-pixel cube about its pivot.
const HEAD: ([f32; 3], [f32; 3], [f32; 2]) = ([-4., -4., -4.], [4., 4., 4.], [0., 0.]);
/// A rod: texOffs(0, 16), 2×8×2 from its pivot.
const ROD: ([f32; 3], [f32; 3], [f32; 2]) = ([0., 0., 0.], [2., 8., 2.], [0., 16.]);

/// `BlazeModel.setupAnim`'s rod pivots at `age` ticks: the upper ring of
/// four (radius 9) turning one way, the middle (radius 7) the other, the
/// lower (radius 5) the first way again, each rod bobbing.
pub fn rod_pivots(age: f32) -> [[f32; 3]; 12] {
    let mut out = [[0.0; 3]; 12];
    let mut f = age * PI * -0.1;
    for (i, rod) in out.iter_mut().enumerate().take(4) {
        let y = -2.0 + (((i * 2) as f32 + age) * 0.25).cos();
        *rod = [f.cos() * 9.0, y, f.sin() * 9.0];
        f += PI * 0.5;
    }
    f = PI * 0.25 + age * PI * 0.03;
    for (i, rod) in out.iter_mut().enumerate().skip(4).take(4) {
        let y = 2.0 + (((i * 2) as f32 + age) * 0.25).cos();
        *rod = [f.cos() * 7.0, y, f.sin() * 7.0];
        f += PI * 0.5;
    }
    f = 0.47123894 + age * PI * -0.05;
    for (i, rod) in out.iter_mut().enumerate().skip(8) {
        let y = 11.0 + ((i as f32 * 1.5 + age) * 0.5).cos();
        *rod = [f.cos() * 5.0, y, f.sin() * 5.0];
        f += PI * 0.5;
    }
    out
}

pub fn append_blazes<'a>(
    mesh: &mut ChunkMesh,
    blazes: impl IntoIterator<Item = &'a BlazeEntity>,
    poses: &ClientMobs,
    atlas: &Atlas,
    light: &SkyLight,
    partial: f32,
) {
    let texture = ResourceId::parse("minecraft:entity/blaze/blaze").unwrap();
    if !atlas.contains(&texture) {
        return;
    }
    let region = atlas.entity_region(&texture);
    let partial = partial.clamp(0.0, 1.0);
    // Each mob's first vertex and overlay (`getOverlayCoords`).
    let mut marks = Vec::new();
    for entity in blazes {
        let Some(mob) = poses.pose(entity.id, partial) else { continue };
        let feet = mob.feet;
        // `BlazeRenderer.getBlockLightLevel`: always 15.
        let sky = light.get(mob.light_block()) as f32;
        let block = 15.0;
        let rotation = mob.body_rotation(90.0);
        let head = Quat::from_euler(glam::EulerRot::ZYX, 0.0, mob.head_yaw.to_radians(), mob.head_pitch.to_radians());
        marks.push((mesh.vertices.len(), mob.overlay(0.0)));
        let tint = [1.0; 3];
        let (from, to, uv) = HEAD;
        cube_scaled(mesh, feet, rotation, Vec3::ONE, region, sky, block, from, to, uv, [0., 0., 0.], head, tint, [64., 32.], None, false);
        let (from, to, uv) = ROD;
        for pivot in rod_pivots(mob.age_in_ticks) {
            cube_scaled(mesh, feet, rotation, Vec3::ONE, region, sky, block, from, to, uv, pivot, Quat::IDENTITY, tint, [64., 32.], None, false);
        }
    }
    crate::cow_render::apply_overlays(mesh, &marks);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rods_circle_in_three_rings() {
        let rods = rod_pivots(0.0);
        let radius = |p: [f32; 3]| (p[0] * p[0] + p[2] * p[2]).sqrt();
        for (i, rod) in rods.iter().enumerate() {
            let expected = [9.0, 7.0, 5.0][i / 4];
            assert!((radius(*rod) - expected).abs() < 1.0e-4, "{i}: {rod:?}");
        }
        // The lowest rods hang lowest but stay above the feet (y 24).
        assert!(rods[8..].iter().all(|r| r[1] + 8.0 < 24.0 && r[1] > rods[4][1]));
        assert_ne!(rod_pivots(0.0), rod_pivots(10.0));
    }
}
