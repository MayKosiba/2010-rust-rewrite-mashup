//! Eyes of ender on the Minecraft map: thrown in the Overworld they fly
//! towards the nearest stronghold (`EyeOfEnder.signalTo` and `tick`), and
//! put in an end portal frame they fill it, opening the portal once all
//! twelve frames round its three by three hold one (`EnderEyeItem.useOn`
//! with `EndPortalFrameBlock.getOrCreatePortalShape`).
use glam::{DVec3, Vec3};
use minecraft_terrain::mesh::{Atlas, ChunkMesh, Vertex};
use minecraft_terrain::pack::ResourceId;
use minecraftoss_core::ChunkPos;

pub(crate) type BlockPos = (i32, i32, i32);

/// How many stronghold ring positions to consider: the first two rings
/// (three and six strongholds), which hold the nearest for any player
/// within a few thousand blocks of the origin.
const RINGS: usize = 9;
/// `EyeOfEnder.signalTo`: it heads at most this far towards its target.
const REACH: f64 = 12.0;
/// Ticks an eye flies before it drops or shatters.
const LIFE: u32 = 80;

/// The Overworld's stronghold positions (`StructurePlacement.getLocatePos`
/// of each ring chunk), found once.
#[derive(Default)]
pub(crate) struct Strongholds {
    positions: Option<Vec<DVec3>>,
}

impl Strongholds {
    /// The stronghold nearest `from`.
    pub(crate) fn nearest(&mut self, stream: &minecraft_terrain::terrain::TerrainStream, from: DVec3) -> Option<DVec3> {
        let positions = self.positions.get_or_insert_with(|| {
            let wg = stream.world_gen();
            let structures = &wg.structures;
            let Some(set) = structures.sets().iter().position(|s| s.name == "minecraft:strongholds") else {
                return Vec::new();
            };
            let placement = structures.placement(set);
            placement
                .ring_chunks(&wg.terrain, RINGS)
                .into_iter()
                .map(|chunk: ChunkPos| {
                    let p = placement.locate_pos(chunk);
                    DVec3::new(f64::from(p.x), f64::from(p.y), f64::from(p.z))
                })
                .collect()
        });
        positions.iter().copied().min_by(|a, b| {
            let da = (a.x - from.x).hypot(a.z - from.z);
            let db = (b.x - from.x).hypot(b.z - from.z);
            da.total_cmp(&db)
        })
    }
}

struct Eye {
    pos: DVec3,
    prev: DVec3,
    vel: DVec3,
    target: DVec3,
    life: u32,
    /// `surviveAfterDeath`: four in five drop back as an item.
    survives: bool,
}

/// What a tick of the flying eyes asks of the world.
#[derive(Default)]
pub(crate) struct EyeEvents {
    /// Eyes that came down whole, as items to drop at a position.
    pub drops: Vec<DVec3>,
    /// Sound events and where.
    pub sounds: Vec<(&'static str, DVec3)>,
}

#[derive(Default)]
pub(crate) struct Eyes {
    flying: Vec<Eye>,
    clock: f64,
    rng: u64,
}

impl Eyes {
    fn random(&mut self) -> f64 {
        self.rng = self.rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.rng >> 11) as f64 / (1u64 << 53) as f64
    }

    /// `EyeOfEnder.signalTo`: from the player's eye towards the
    /// stronghold, at most `REACH` blocks off and eight above.
    pub(crate) fn throw(&mut self, from: DVec3, stronghold: DVec3) {
        let mut target = stronghold;
        let (dx, dz) = (target.x - from.x, target.z - from.z);
        let flat = dx.hypot(dz);
        if flat > REACH {
            target = DVec3::new(from.x + dx / flat * REACH, from.y + 8.0, from.z + dz / flat * REACH);
        }
        let survives = self.random() > 0.2;
        self.flying.push(Eye { pos: from, prev: from, vel: DVec3::ZERO, target, life: 0, survives });
    }

    /// Advances the flying eyes by `dt` seconds.
    pub(crate) fn update(&mut self, dt: f64) -> EyeEvents {
        let mut events = EyeEvents::default();
        self.clock += dt.min(0.25);
        while self.clock >= 0.05 {
            self.clock -= 0.05;
            let mut kept = Vec::new();
            for mut eye in std::mem::take(&mut self.flying) {
                eye.prev = eye.pos;
                eye.life += 1;
                // `EyeOfEnder.tick`: the horizontal speed eases in towards
                // the target, the height bobs up to it and hovers.
                let to = eye.target - eye.pos;
                let flat = to.x.hypot(to.z);
                let heading = to.z.atan2(to.x);
                let current = eye.vel.x.hypot(eye.vel.z);
                let speed = current + (flat * 0.0025 - current) * 0.0025 + 0.1_f64.min(flat) * 0.25;
                let speed = speed.min(0.4);
                let mut vy = eye.vel.y;
                if flat < 1.0 {
                    vy *= 0.8;
                    eye.vel.x *= 0.8;
                    eye.vel.z *= 0.8;
                } else {
                    eye.vel.x = heading.cos() * speed;
                    eye.vel.z = heading.sin() * speed;
                }
                vy += (if eye.pos.y < eye.target.y { 1.0 } else { -1.0 } - vy) * 0.015;
                eye.vel.y = vy;
                eye.pos += eye.vel;
                if eye.life > LIFE {
                    if eye.survives {
                        events.drops.push(eye.pos);
                    } else {
                        events.sounds.push(("minecraft:entity.ender_eye.death", eye.pos));
                    }
                    continue;
                }
                kept.push(eye);
            }
            self.flying = kept;
        }
        events
    }

    /// The flying eyes as crossed quads of the eye's item sprite.
    pub(crate) fn append_meshes(&self, mesh: &mut ChunkMesh, atlas: &Atlas) {
        let id = ResourceId::parse("minecraft:item/ender_eye").unwrap();
        if !atlas.contains(&id) {
            return;
        }
        let [u0, v0, u1, v1] = atlas.entity_region(&id);
        let partial = self.clock / 0.05;
        for eye in &self.flying {
            let c = eye.prev.lerp(eye.pos, partial).as_vec3();
            let h = 0.25;
            for axis in [Vec3::X, Vec3::Z] {
                let side = axis * h;
                let up = Vec3::Y * h;
                let corners = [c - side + up, c + side + up, c + side - up, c - side - up];
                let uvs = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]];
                let start = mesh.vertices.len() as u32;
                for (corner, uv) in corners.into_iter().zip(uvs) {
                    mesh.vertices.push(Vertex { position: corner.to_array(), uv, color: [1.0; 4], sky_light: 15.0, block_light: 15.0 });
                }
                mesh.indices.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
                mesh.indices.extend_from_slice(&[start, start + 2, start + 1, start, start + 3, start + 2]);
                mesh.faces += 2;
            }
        }
    }
}

/// The inside of a completed end portal around a just-filled frame: a three
/// by three whose twelve surrounding frames all hold eyes (the frame at
/// each side of it, three a side), at the frames' height.
pub(crate) fn completed_portal(frame_with_eye: impl Fn(BlockPos) -> bool, filled: BlockPos) -> Option<Vec<BlockPos>> {
    let (fx, y, fz) = filled;
    for cx in fx - 2..=fx + 2 {
        for cz in fz - 2..=fz + 2 {
            let ring = (-1..=1).flat_map(|i| [(cx + i, cz - 2), (cx + i, cz + 2), (cx - 2, cz + i), (cx + 2, cz + i)]);
            let mut ring_has_filled = false;
            let mut whole = true;
            for (x, z) in ring {
                if (x, z) == (fx, fz) {
                    ring_has_filled = true;
                }
                if !frame_with_eye((x, y, z)) {
                    whole = false;
                    break;
                }
            }
            if whole && ring_has_filled {
                return Some((-1..=1).flat_map(|dx| (-1..=1).map(move |dz| (cx + dx, y, cz + dz))).collect());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn ring(cx: i32, cz: i32) -> HashSet<BlockPos> {
        (-1..=1).flat_map(|i| [(cx + i, 0, cz - 2), (cx + i, 0, cz + 2), (cx - 2, 0, cz + i), (cx + 2, 0, cz + i)]).collect()
    }

    #[test]
    fn twelve_eyes_open_the_portal() {
        let frames = ring(10, -4);
        let inside = completed_portal(|p| frames.contains(&p), (12, 0, -4)).unwrap();
        assert_eq!(inside.len(), 9);
        assert!(inside.contains(&(10, 0, -4)) && inside.contains(&(9, 0, -5)));
    }

    #[test]
    fn eleven_do_not() {
        let mut frames = ring(0, 0);
        frames.remove(&(1, 0, 2));
        assert!(completed_portal(|p| frames.contains(&p), (2, 0, 0)).is_none());
    }

    #[test]
    fn an_eye_heads_twelve_blocks_towards_a_far_stronghold_and_ends() {
        let mut eyes = Eyes::default();
        eyes.throw(DVec3::new(0.0, 70.0, 0.0), DVec3::new(1000.0, 30.0, 0.0));
        let target = eyes.flying[0].target;
        assert!((target.x - 12.0).abs() < 1e-9 && target.y == 78.0);
        let mut ended = false;
        for _ in 0..100 {
            let events = eyes.update(0.05);
            ended |= !events.drops.is_empty() || !events.sounds.is_empty();
        }
        assert!(ended && eyes.flying.is_empty());
    }
}
