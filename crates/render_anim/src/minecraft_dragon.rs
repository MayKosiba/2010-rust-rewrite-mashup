//! The Ender Dragon fight on the End's main island: the exit podium at the
//! island's centre, the end crystals on the obsidian pillars (they heal the
//! dragon near them and explode when shot), and the dragon itself, which
//! circles the pillars, strafes the player with fireballs, charges them,
//! perches on the podium to breathe on them, and on death opens the exit
//! portal home and leaves the egg.
//!
//! Vanilla's fight is `EnderDragon` with its phase manager
//! (`HoldingPatternPhase`, `StrafePlayerPhase`, `ChargingPlayerPhase`,
//! `LandingApproachPhase`, `SittingScanningPhase`...) and
//! `EndDragonFight`; this keeps their shape (the phases, the crystals'
//! healing, head shots doing full damage and body shots a quarter plus one,
//! 200 health) with simpler flight: the dragon steers straight at its
//! target and its neck and tail do not trail its path.
use glam::DVec3;
use minecraft_terrain::dragon_render::{self, DragonPose};
use minecraft_terrain::mesh::{Atlas, ChunkMesh};
use minecraft_terrain::scene::Block;

pub(crate) type BlockPos = (i32, i32, i32);

const MAX_HEALTH: f32 = 200.0;
/// MW2 health (100) to Minecraft health (20), as the mobs take it.
const HEALTH_SCALE: f32 = 20.0 / 100.0;
/// Mob-box key kinds, above the mobs' own (`minecraft_entities::encode`).
const DRAGON_KEY: u64 = 0xE0 << 56;
const CRYSTAL_KEY: u64 = 0xE1 << 56;
const KIND_MASK: u64 = 0xFF << 56;
/// How far a crystal reaches to heal the dragon (`EnderDragon.checkCrystals`).
const HEAL_RANGE: f64 = 32.0;
const CIRCLE_RADIUS: f64 = 60.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Holding,
    Strafe,
    Charge,
    Landing,
    Perched,
    Takeoff,
    Dying,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Head,
    Neck,
    Body,
    Tail(u8),
    Wing(u8),
}

impl Part {
    const ALL: [Part; 8] = [Part::Head, Part::Neck, Part::Body, Part::Tail(0), Part::Tail(1), Part::Tail(2), Part::Wing(0), Part::Wing(1)];
    fn index(self) -> u64 {
        Part::ALL.iter().position(|&p| p == self).unwrap_or(0) as u64
    }
}

struct Dragon {
    pos: DVec3,
    prev: DVec3,
    vel: DVec3,
    yaw: f32,
    prev_yaw: f32,
    pitch: f32,
    prev_pitch: f32,
    flap: f32,
    prev_flap: f32,
    health: f32,
    phase: Phase,
    phase_ticks: u32,
    target: DVec3,
    circle: f64,
    turn: f64,
    hurt: u32,
    hurt_sound: u32,
    /// Damage taken while perched: enough and it takes off early.
    perched_damage: f32,
    fired: bool,
    /// The crystal healing it, by index.
    healer: Option<usize>,
}

impl Dragon {
    fn forward(&self) -> DVec3 {
        let (y, p) = (f64::from(self.yaw).to_radians(), f64::from(self.pitch).to_radians());
        DVec3::new(-y.sin() * p.cos(), p.sin(), y.cos() * p.cos())
    }
    fn right(&self) -> DVec3 {
        let y = f64::from(self.yaw).to_radians();
        DVec3::new(-y.cos(), 0.0, -y.sin())
    }
    fn head(&self) -> DVec3 {
        self.pos + self.forward() * 5.2
    }
    /// Each hit box's centre and half size.
    fn parts(&self) -> [(Part, DVec3, DVec3); 8] {
        let (f, r) = (self.forward(), self.right());
        let cube = |h: f64| DVec3::splat(h);
        [
            (Part::Head, self.head(), cube(1.0)),
            (Part::Neck, self.pos + f * 3.0, cube(1.5)),
            (Part::Body, self.pos, DVec3::new(2.5, 1.5, 2.5)),
            (Part::Tail(0), self.pos - f * 4.5, cube(1.0)),
            (Part::Tail(1), self.pos - f * 7.0, cube(1.0)),
            (Part::Tail(2), self.pos - f * 9.5, cube(1.0)),
            (Part::Wing(0), self.pos + r * 4.5 + DVec3::Y * 0.5, DVec3::new(2.0, 1.0, 2.0)),
            (Part::Wing(1), self.pos - r * 4.5 + DVec3::Y * 0.5, DVec3::new(2.0, 1.0, 2.0)),
        ]
    }
}

struct Crystal {
    /// Its feet, on the pillar's bedrock.
    pos: DVec3,
    alive: bool,
}

struct Fireball {
    pos: DVec3,
    vel: DVec3,
    age: u32,
}

/// What a tick of the fight asks of the world.
#[derive(Default)]
pub(crate) struct Effects {
    /// Damage to the player in MW2 health, and where it came from.
    pub damage: Vec<(i32, DVec3)>,
    /// Sound events, where, volume and pitch.
    pub sounds: Vec<(&'static str, DVec3, f32, f32)>,
    /// Blocks to set (the podium, the exit portal, the egg).
    pub edits: Vec<(BlockPos, Option<Block>)>,
}

impl Effects {
    fn hurt_player(&mut self, minecraft: f32, from: DVec3) {
        diag::info!(World, "Ender fight: player hurt {minecraft:.1} from {:.0?}", from.to_array());
        self.damage.push(((minecraft / HEALTH_SCALE).round() as i32, from));
    }
}

/// One End's fight, kept for the session: leaving and coming back finds it
/// as it was left (a dead dragon stays dead).
#[derive(Default)]
pub(crate) struct Fight {
    started: bool,
    killed: bool,
    podium: BlockPos,
    dragon: Option<Dragon>,
    crystals: Vec<Crystal>,
    fireballs: Vec<Fireball>,
    ticks: u64,
    clock: f64,
    rng: u64,
    /// Held for a photo (the console's `dragon stage`): where, facing, and
    /// the wing beat and pitch it is held at; its fight waits meanwhile.
    staged: Option<(DVec3, f32, f32, f32)>,
}

impl Fight {
    fn random(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64
    }

    pub(crate) fn started(&self) -> bool {
        self.started
    }

    /// Sets the fight up once the island's centre has generated: the podium
    /// where the island's top is at 0, 0 (`EndPodiumFeature`, unlit), the
    /// crystals where the pillars stand, and the dragon high over the
    /// island. `top` is the first air over the island at 0, 0.
    pub(crate) fn start(&mut self, seed: i64, top: i32) -> Effects {
        self.started = true;
        self.rng = (seed as u64) | 1;
        self.podium = (0, top, 0);
        self.crystals = dragon_render::spikes_for_seed(seed)
            .iter()
            .map(|s| Crystal {
                pos: DVec3::new(f64::from(s.center_x) + 0.5, f64::from(s.height + 1), f64::from(s.center_z) + 0.5),
                alive: true,
            })
            .collect();
        if !self.killed {
            self.dragon = Some(Dragon {
                pos: DVec3::new(0.0, 128.0, 0.0),
                prev: DVec3::new(0.0, 128.0, 0.0),
                vel: DVec3::ZERO,
                yaw: 0.0,
                prev_yaw: 0.0,
                pitch: 0.0,
                prev_pitch: 0.0,
                flap: 0.0,
                prev_flap: 0.0,
                health: MAX_HEALTH,
                phase: Phase::Holding,
                phase_ticks: 0,
                target: DVec3::new(CIRCLE_RADIUS, f64::from(top) + 30.0, 0.0),
                circle: 0.0,
                turn: 1.0,
                hurt: 0,
                hurt_sound: 0,
                perched_damage: 0.0,
                fired: false,
                healer: None,
            });
        }
        let mut fx = Effects::default();
        self.podium_blocks(self.killed, &mut fx.edits);
        fx
    }

    /// `EndPodiumFeature`: a bedrock bowl with a pillar, the portal inside
    /// when the dragon is dead.
    fn podium_blocks(&self, open: bool, edits: &mut Vec<(BlockPos, Option<Block>)>) {
        let (px, y, pz) = self.podium;
        let bedrock = || Some(Block::new("minecraft:bedrock"));
        for dx in -4i32..=4 {
            for dz in -4i32..=4 {
                let d = dx * dx + dz * dz;
                if d > 12 {
                    continue;
                }
                let (x, z) = (px + dx, pz + dz);
                edits.push(((x, y - 1, z), bedrock()));
                let at_floor = if d == 0 {
                    bedrock()
                } else if d > 6 {
                    bedrock()
                } else if open {
                    Some(Block::new("minecraft:end_portal"))
                } else {
                    None
                };
                edits.push(((x, y, z), at_floor));
                for h in 1..=4 {
                    edits.push(((x, y + h, z), if d == 0 && h <= 3 { bedrock() } else { None }));
                }
            }
        }
        if open {
            edits.push(((px, y + 4, pz), Some(Block::new("minecraft:dragon_egg"))));
        }
    }

    /// The console's `dragon kill`: the dragon dies where it is.
    pub(crate) fn kill(&mut self, fx: &mut Effects) {
        let health = self.dragon.as_ref().map_or(0.0, |d| d.health);
        self.hurt_dragon(health + 1.0, fx);
    }

    /// The console's `dragon reset`: a new dragon and every crystal back, the
    /// exit portal closed, as a respawned fight (`EndDragonFight.respawnDragon`).
    pub(crate) fn reset(&mut self, seed: i64) -> Effects {
        let top = self.podium.1;
        self.killed = false;
        self.dragon = None;
        self.fireballs.clear();
        self.start(seed, top)
    }

    /// The boss bar's name and health left.
    pub(crate) fn boss(&self) -> Option<(String, f32)> {
        let dragon = self.dragon.as_ref()?;
        Some(("ENDER DRAGON".into(), (dragon.health / MAX_HEALTH).clamp(0.0, 1.0)))
    }

    /// Hit boxes for bullets, in blocks, keyed apart from the mobs.
    pub(crate) fn boxes(&self) -> Vec<(u64, [f64; 6])> {
        let mut out = Vec::new();
        if let Some(dragon) = self.dragon.as_ref().filter(|d| d.phase != Phase::Dying) {
            for (part, c, h) in dragon.parts() {
                out.push((DRAGON_KEY | part.index(), [c.x - h.x, c.y - h.y, c.z - h.z, c.x + h.x, c.y + h.y, c.z + h.z]));
            }
        }
        for (i, crystal) in self.crystals.iter().enumerate().filter(|(_, c)| c.alive) {
            let p = crystal.pos;
            out.push((CRYSTAL_KEY | i as u64, [p.x - 1.0, p.y, p.z - 1.0, p.x + 1.0, p.y + 2.0, p.z + 1.0]));
        }
        out
    }

    /// Whether a mob-box key is the fight's.
    pub(crate) fn owns(key: u64) -> bool {
        matches!(key & KIND_MASK, DRAGON_KEY | CRYSTAL_KEY)
    }

    /// A bullet into a dragon part or a crystal, with its MW2 damage.
    pub(crate) fn shot(&mut self, key: u64, damage: f32, player: DVec3) -> Effects {
        let mut fx = Effects::default();
        let index = (key & !KIND_MASK) as usize;
        match key & KIND_MASK {
            CRYSTAL_KEY => self.explode_crystal(index, player, &mut fx),
            DRAGON_KEY => {
                let head = Part::ALL.get(index) == Some(&Part::Head);
                let minecraft = damage * HEALTH_SCALE;
                // `EnderDragon.hurt(part, ...)`: anything but the head takes
                // a quarter, plus one.
                let taken = if head { minecraft } else { minecraft / 4.0 + minecraft.min(1.0) };
                self.hurt_dragon(taken, &mut fx);
            }
            _ => {}
        }
        fx
    }

    fn hurt_dragon(&mut self, amount: f32, fx: &mut Effects) {
        let Some(dragon) = self.dragon.as_mut() else { return };
        if dragon.phase == Phase::Dying || amount < 0.01 {
            return;
        }
        dragon.health -= amount;
        dragon.hurt = 10;
        if dragon.phase == Phase::Perched {
            dragon.perched_damage += amount;
        }
        if dragon.hurt_sound == 0 {
            fx.sounds.push(("minecraft:entity.ender_dragon.hurt", dragon.pos, 5.0, 1.0));
            dragon.hurt_sound = 8;
        }
        if dragon.health <= 0.0 {
            dragon.health = 0.0;
            dragon.phase = Phase::Dying;
            dragon.phase_ticks = 0;
            fx.sounds.push(("minecraft:entity.ender_dragon.death", dragon.pos, 5.0, 1.0));
        }
    }

    /// `EndCrystal.hurt`: it explodes (power 6); the dragon it was healing
    /// takes 10 to the head.
    fn explode_crystal(&mut self, index: usize, player: DVec3, fx: &mut Effects) {
        let Some(crystal) = self.crystals.get_mut(index).filter(|c| c.alive) else { return };
        crystal.alive = false;
        let at = crystal.pos + DVec3::Y;
        fx.sounds.push(("minecraft:entity.generic.explode", at, 4.0, 0.9));
        let distance = player.distance(at);
        if distance < 12.0 {
            fx.hurt_player((24.0 * (1.0 - distance / 12.0)) as f32, at);
        }
        if self.dragon.as_ref().is_some_and(|d| d.healer == Some(index)) {
            self.hurt_dragon(10.0, fx);
        }
    }

    /// Holds the dragon at `pos` facing `yaw` (pitched, at `flap` of its
    /// wing beat), as for a photo; `None` lets it fly on.
    pub(crate) fn stage(&mut self, staged: Option<(DVec3, f32, f32, f32)>) {
        self.staged = staged;
        if let (Some((pos, yaw, pitch, flap)), Some(dragon)) = (staged, self.dragon.as_mut()) {
            dragon.pos = pos;
            dragon.prev = pos;
            dragon.vel = DVec3::ZERO;
            dragon.yaw = yaw;
            dragon.prev_yaw = yaw;
            dragon.pitch = pitch;
            dragon.prev_pitch = pitch;
            dragon.flap = flap;
            dragon.prev_flap = flap;
        }
    }

    /// Advances the fight by `dt` seconds. `player` is the player's feet
    /// while alive; `solid` says whether a block stops a fireball.
    pub(crate) fn update(&mut self, dt: f64, player: Option<DVec3>, solid: impl Fn(BlockPos) -> bool) -> Effects {
        let mut fx = Effects::default();
        if self.staged.is_some() {
            return fx;
        }
        self.clock += dt.min(0.25);
        while self.clock >= 0.05 {
            self.clock -= 0.05;
            self.tick(player, &solid, &mut fx);
        }
        fx
    }

    fn tick(&mut self, player: Option<DVec3>, solid: &impl Fn(BlockPos) -> bool, fx: &mut Effects) {
        self.ticks += 1;
        self.tick_fireballs(player, solid, fx);
        let Some(mut dragon) = self.dragon.take() else { return };
        dragon.prev = dragon.pos;
        dragon.prev_yaw = dragon.yaw;
        dragon.prev_pitch = dragon.pitch;
        dragon.prev_flap = dragon.flap;
        dragon.phase_ticks += 1;
        dragon.hurt = dragon.hurt.saturating_sub(1);
        dragon.hurt_sound = dragon.hurt_sound.saturating_sub(1);

        if dragon.phase == Phase::Dying {
            // `EnderDragon.tickDeath`: rising for ten seconds, then gone.
            dragon.pos.y += 0.1;
            dragon.flap += 0.02;
            if dragon.phase_ticks >= 200 {
                self.killed = true;
                let mut edits = Vec::new();
                self.podium_blocks(true, &mut edits);
                fx.edits.extend(edits);
                let (x, y, z) = self.podium;
                fx.sounds.push(("minecraft:block.end_portal.spawn", DVec3::new(f64::from(x), f64::from(y), f64::from(z)), 1.0, 1.0));
                return;
            }
            self.dragon = Some(dragon);
            return;
        }

        // `EnderDragon.checkCrystals`: the nearest crystal in reach heals a
        // point every half second.
        dragon.healer = self
            .crystals
            .iter()
            .enumerate()
            .filter(|(_, c)| c.alive && c.pos.distance(dragon.pos) < HEAL_RANGE)
            .min_by(|a, b| a.1.pos.distance(dragon.pos).total_cmp(&b.1.pos.distance(dragon.pos)))
            .map(|(i, _)| i);
        if dragon.healer.is_some() && self.ticks % 10 == 0 {
            dragon.health = (dragon.health + 1.0).min(MAX_HEALTH);
        }

        let (px, py, pz) = self.podium;
        let perch = DVec3::new(f64::from(px) + 0.5, f64::from(py + 4) + 1.5, f64::from(pz) + 0.5);
        let alive_crystals = self.crystals.iter().filter(|c| c.alive).count();
        let near_player = player.filter(|p| DVec3::new(p.x, 0.0, p.z).length() < 150.0);
        let mut speed = 0.65;
        match dragon.phase {
            Phase::Holding => {
                if dragon.phase_ticks == 1 {
                    // Back from an attack: on round the circle from where it is.
                    dragon.circle = dragon.pos.z.atan2(dragon.pos.x);
                    let height = f64::from(py) + 20.0 + 15.0 * self.random();
                    next_waypoint(&mut dragon, height);
                }
                if dragon.pos.distance(dragon.target) < 10.0 {
                    let height = f64::from(py) + 20.0 + 15.0 * self.random();
                    next_waypoint(&mut dragon, height);
                    if self.random() < 0.08 {
                        dragon.turn = -dragon.turn;
                    }
                    // `HoldingPatternPhase.findNewTarget`: landing grows likelier
                    // as crystals go; otherwise now and then a strafe or charge.
                    if near_player.is_some() {
                        let roll = self.random();
                        let land = 1.0 / (3.0 + alive_crystals as f64);
                        if roll < land {
                            set_phase(&mut dragon, Phase::Landing);
                        } else if roll < land + 0.25 {
                            set_phase(&mut dragon, Phase::Strafe);
                        } else if roll < land + 0.35 {
                            set_phase(&mut dragon, Phase::Charge);
                        }
                    }
                }
                if self.ticks % 240 == 0 && self.random() < 0.5 {
                    fx.sounds.push(("minecraft:entity.ender_dragon.growl", dragon.pos, 5.0, 0.8 + 0.3 * self.random() as f32));
                }
            }
            Phase::Strafe => match near_player {
                Some(p) => {
                    // `StrafePlayerPhase`: a pass by the player, a little above,
                    // firing once it faces them in range.
                    if dragon.phase_ticks == 1 {
                        let across = DVec3::new(p.x - dragon.pos.x, 0.0, p.z - dragon.pos.z).normalize_or_zero();
                        dragon.target = p + across * 25.0 + DVec3::Y * 14.0;
                    }
                    let to = p + DVec3::Y - dragon.head();
                    if !dragon.fired && to.length() < 64.0 && dragon.forward().dot(to.normalize()) > 0.6 {
                        dragon.fired = true;
                        fx.sounds.push(("minecraft:entity.ender_dragon.shoot", dragon.head(), 5.0, 1.0));
                        self.fireballs.push(Fireball { pos: dragon.head(), vel: to.normalize() * 1.3, age: 0 });
                    }
                    if dragon.fired && dragon.phase_ticks > 30 || dragon.pos.distance(dragon.target) < 6.0 || dragon.phase_ticks > 240 {
                        set_phase(&mut dragon, Phase::Holding);
                    }
                }
                None => set_phase(&mut dragon, Phase::Holding),
            },
            Phase::Charge => match near_player {
                Some(p) => {
                    speed = 1.1;
                    dragon.target = p + DVec3::Y;
                    let reach = dragon.parts().iter().any(|(_, c, h)| (p + DVec3::Y - *c).abs().cmple(*h + DVec3::splat(1.0)).all());
                    if reach {
                        fx.hurt_player(10.0, dragon.pos);
                        set_phase(&mut dragon, Phase::Holding);
                    } else if dragon.phase_ticks > 160 {
                        set_phase(&mut dragon, Phase::Holding);
                    }
                }
                None => set_phase(&mut dragon, Phase::Holding),
            },
            Phase::Landing => {
                dragon.target = perch;
                speed = 0.5;
                if dragon.pos.distance(perch) < 2.0 {
                    set_phase(&mut dragon, Phase::Perched);
                    dragon.perched_damage = 0.0;
                    fx.sounds.push(("minecraft:entity.ender_dragon.growl", dragon.pos, 5.0, 0.7));
                } else if dragon.phase_ticks > 600 {
                    set_phase(&mut dragon, Phase::Holding);
                }
            }
            Phase::Perched => {
                dragon.pos = dragon.pos.lerp(perch, 0.2);
                dragon.vel = DVec3::ZERO;
                if let Some(p) = near_player {
                    let to = p - dragon.pos;
                    let want = (-to.x).atan2(to.z).to_degrees() as f32;
                    dragon.yaw = approach_angle(dragon.yaw, want, 4.0);
                    // `DragonSittingFlamingPhase`: breath in front of the head.
                    if dragon.phase_ticks % 20 == 0 {
                        let from_head = p + DVec3::Y - dragon.head();
                        if from_head.length() < 10.0 {
                            fx.hurt_player(4.0, dragon.head());
                            fx.sounds.push(("minecraft:entity.ender_dragon.growl", dragon.head(), 3.0, 1.2));
                        }
                    }
                }
                dragon.pitch = approach_angle(dragon.pitch, 0.0, 3.0);
                dragon.flap += 0.02;
                // `SittingScanningPhase`: enough hurt, or long enough, and up.
                if dragon.phase_ticks > 240 || dragon.perched_damage > 25.0 {
                    set_phase(&mut dragon, Phase::Takeoff);
                }
                self.dragon = Some(dragon);
                return;
            }
            Phase::Takeoff => {
                dragon.target = perch + DVec3::new(0.0, 30.0, 0.0) + dragon.forward() * 20.0;
                if dragon.pos.y > perch.y + 20.0 || dragon.phase_ticks > 200 {
                    set_phase(&mut dragon, Phase::Holding);
                }
            }
            Phase::Dying => unreachable!(),
        }

        // Flight: steer at the target, turning at most so far a tick.
        let to = dragon.target - dragon.pos;
        if to.length() > 0.01 {
            dragon.vel = dragon.vel * 0.9 + to.normalize() * speed * 0.1;
        }
        dragon.pos += dragon.vel;
        let horizontal = dragon.vel.x.hypot(dragon.vel.z);
        if horizontal > 0.05 {
            let want = (-dragon.vel.x).atan2(dragon.vel.z).to_degrees() as f32;
            dragon.yaw = approach_angle(dragon.yaw, want, 8.0);
        }
        let want_pitch = (dragon.vel.y.atan2(horizontal.max(0.01)).to_degrees() as f32).clamp(-40.0, 40.0);
        dragon.pitch = approach_angle(dragon.pitch, want_pitch, 3.0);
        // The wing beat: quicker climbing, slower diving.
        let before = dragon.flap;
        dragon.flap += if dragon.vel.y > 0.05 { 0.12 } else if dragon.vel.y < -0.1 { 0.06 } else { 0.09 };
        if before.floor() != dragon.flap.floor() {
            fx.sounds.push(("minecraft:entity.ender_dragon.flap", dragon.pos, 5.0, 0.8 + 0.3 * self.random() as f32));
        }
        self.dragon = Some(dragon);
    }

    fn tick_fireballs(&mut self, player: Option<DVec3>, solid: &impl Fn(BlockPos) -> bool, fx: &mut Effects) {
        let mut kept = Vec::new();
        for mut ball in std::mem::take(&mut self.fireballs) {
            ball.pos += ball.vel;
            ball.age += 1;
            let block = (ball.pos.x.floor() as i32, ball.pos.y.floor() as i32, ball.pos.z.floor() as i32);
            let near = player.is_some_and(|p| (p + DVec3::Y).distance(ball.pos) < 1.5);
            if near || solid(block) || ball.age > 200 {
                // `DragonFireball.onHit`: a cloud of harming breath; here its
                // first sting, on anyone within its radius.
                fx.sounds.push(("minecraft:entity.dragon_fireball.explode", ball.pos, 1.0, 1.0));
                if let Some(p) = player
                    && (p + DVec3::Y).distance(ball.pos) < 4.0
                {
                    fx.hurt_player(6.0, ball.pos);
                }
                continue;
            }
            kept.push(ball);
        }
        self.fireballs = kept;
    }

    /// The dragon, the crystals, their beams and the fireballs.
    pub(crate) fn append_meshes(&self, mesh: &mut ChunkMesh, translucent: &mut ChunkMesh, atlas: &Atlas) {
        let partial = (self.clock / 0.05) as f32;
        let age = self.ticks as f32 + partial;
        for crystal in self.crystals.iter().filter(|c| c.alive) {
            dragon_render::append_crystal(mesh, crystal.pos, age, atlas);
        }
        for ball in &self.fireballs {
            dragon_render::append_fireball(mesh, ball.pos + ball.vel * f64::from(partial), age, atlas);
        }
        let Some(dragon) = self.dragon.as_ref() else { return };
        let position = dragon.prev.lerp(dragon.pos, f64::from(partial));
        let pose = DragonPose {
            position,
            yaw: lerp_angle(dragon.prev_yaw, dragon.yaw, partial),
            pitch: dragon.prev_pitch + (dragon.pitch - dragon.prev_pitch) * partial,
            flap: dragon.prev_flap + (dragon.flap - dragon.prev_flap) * partial,
            age,
            jaw: if self.staged.is_some() { 0.9 } else if matches!(dragon.phase, Phase::Perched | Phase::Strafe) { 0.6 } else { 0.1 },
            hurt: dragon.hurt as f32 / 10.0,
        };
        dragon_render::append_dragon(mesh, &pose, atlas);
        if dragon.phase == Phase::Dying {
            let time = (dragon.phase_ticks as f32 + partial) / 200.0;
            dragon_render::append_death_rays(translucent, position, time, atlas);
        }
        if let Some(crystal) = dragon.healer.and_then(|i| self.crystals.get(i)) {
            dragon_render::append_beam(mesh, crystal.pos + DVec3::Y * 1.5, position, atlas);
        }
    }
}

/// The next point of the circle round the pillars, `height` up.
fn next_waypoint(dragon: &mut Dragon, height: f64) {
    dragon.circle += 0.45 * dragon.turn;
    dragon.target = DVec3::new(dragon.circle.cos() * CIRCLE_RADIUS, height, dragon.circle.sin() * CIRCLE_RADIUS);
}

fn set_phase(dragon: &mut Dragon, phase: Phase) {
    diag::info!(World, "Ender dragon: {:?} -> {phase:?} at {:.0?} health {:.0}", dragon.phase, dragon.pos.to_array(), dragon.health);
    dragon.phase = phase;
    dragon.phase_ticks = 0;
    dragon.fired = false;
}

/// Moves an angle in degrees toward another by at most `step`.
fn approach_angle(from: f32, to: f32, step: f32) -> f32 {
    let delta = (to - from + 540.0).rem_euclid(360.0) - 180.0;
    from + delta.clamp(-step, step)
}

fn lerp_angle(from: f32, to: f32, t: f32) -> f32 {
    let delta = (to - from + 540.0).rem_euclid(360.0) - 180.0;
    from + delta * t
}
