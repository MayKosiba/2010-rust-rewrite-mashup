//! Doom's two bosses on the Minecraft map, summoned by the console's
//! `doomboss`: the Cyberdemon, with its rocket volleys, and the Spider
//! Mastermind, with its chaingun. They run Doom's own logic at Doom's 35
//! tics a second: the state tables of `info.c` (frames, durations, actions)
//! and the actions of `p_enemy.c` (`A_Look`, `A_Chase` with
//! `P_NewChaseDir`'s eight-way walking and `P_CheckMissileRange`,
//! `A_FaceTarget`, `A_CyberAttack`, `A_SPosAttack`, `A_SpidRefire`),
//! `P_DamageMobj`'s pain chance, and the rocket's flight and
//! `P_RadiusAttack`. Their art and sounds are Freedoom's (`doom`).
//!
//! Doom's health counts as MW2's (a Doom marine and an MW2 soldier both have
//! 100), so bullets take their MW2 damage off and Doom's damage lands as is.
//! Where the two worlds differ: they walk up whole blocks (Doom steps 24
//! units, three quarters of one), their feet are kept to at most two blocks
//! across (the Mastermind's 256-unit body would wedge on any tree), and a
//! rocket's blast reaches up and down as far as across.
use glam::DVec3;
use minecraft_terrain::doom::{self, UNITS_PER_BLOCK};
use minecraft_terrain::lighting::SkyLight;
use minecraft_terrain::mesh::{Atlas, ChunkMesh};

pub(crate) type BlockPos = (i32, i32, i32);

/// Mob-box key kind, above the dragon fight's.
const DOOM_KEY: u64 = 0xE2 << 56;
const KIND_MASK: u64 = 0xFF << 56;
const TIC: f64 = 1.0 / 35.0;
/// `MISSILERANGE`: how far a hitscan attack reaches.
const MISSILE_RANGE: f64 = 2048.0;
/// The player as Doom sees them: 16 units round, 56 tall.
const PLAYER_RADIUS: f64 = 16.0;
const PLAYER_HEIGHT: f64 = 56.0;
/// `BASETHRESHOLD`: tics a monster keeps after what last hurt it.
const BASE_THRESHOLD: i32 = 100;
/// How far a positional sound carries: `S_CLIPPING_DIST`, 1200 units, as
/// the Minecraft mixer's volume (sixteen blocks a volume).
const SOUND_VOLUME: f32 = (1200.0 / UNITS_PER_BLOCK / 16.0) as f32;

fn units(blocks: f64) -> f64 {
    blocks * UNITS_PER_BLOCK
}

fn blocks(units: f64) -> f64 {
    units / UNITS_PER_BLOCK
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Cyberdemon,
    Mastermind,
}

impl Kind {
    pub(crate) fn parse(name: &str) -> Option<Self> {
        match name {
            "cyberdemon" | "cyber" | "cyborg" => Some(Kind::Cyberdemon),
            "mastermind" | "spider" | "spidermastermind" | "spider_mastermind" => Some(Kind::Mastermind),
            _ => None,
        }
    }

    /// `mobjinfo[MT_CYBORG]`, `mobjinfo[MT_SPIDER]`.
    fn info(self) -> &'static Info {
        match self {
            Kind::Cyberdemon => &CYBORG,
            Kind::Mastermind => &SPIDER,
        }
    }
}

struct Info {
    name: &'static str,
    sprite: &'static str,
    states: &'static [State],
    spawn: usize,
    see: usize,
    pain: usize,
    missile: usize,
    death: usize,
    health: i32,
    reaction_time: i32,
    pain_chance: u8,
    speed: f64,
    radius: f64,
    height: f64,
    see_sound: &'static str,
    pain_sound: &'static str,
    death_sound: &'static str,
    active_sound: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    None,
    Look,
    Chase,
    Hoof,
    Metal,
    Face,
    CyberAttack,
    SPosAttack,
    SpidRefire,
    Pain,
    Scream,
    Fall,
}

/// A row of `states[]`: the frame (0 is `A`), whether it is drawn full
/// bright, its tics (-1 forever), its action and the next row.
#[derive(Clone, Copy)]
struct State {
    frame: u8,
    bright: bool,
    tics: i32,
    action: Action,
    next: usize,
}

const fn st(frame: u8, tics: i32, action: Action, next: usize) -> State {
    State { frame, bright: false, tics, action, next }
}

const fn lit(frame: u8, tics: i32, action: Action, next: usize) -> State {
    State { frame, bright: true, tics, action, next }
}

use Action as A;

/// `S_CYBER_STND` .. `S_CYBER_DIE10`.
const CYBER_STATES: [State; 27] = [
    st(0, 10, A::Look, 1),
    st(1, 10, A::Look, 0),
    // RUN1..8
    st(0, 3, A::Hoof, 3),
    st(0, 3, A::Chase, 4),
    st(1, 3, A::Chase, 5),
    st(1, 3, A::Chase, 6),
    st(2, 3, A::Chase, 7),
    st(2, 3, A::Chase, 8),
    st(3, 3, A::Metal, 9),
    st(3, 3, A::Chase, 2),
    // ATK1..6
    st(4, 6, A::Face, 11),
    st(5, 12, A::CyberAttack, 12),
    st(4, 12, A::Face, 13),
    st(5, 12, A::CyberAttack, 14),
    st(4, 12, A::Face, 15),
    st(5, 12, A::CyberAttack, 2),
    // PAIN
    st(6, 10, A::Pain, 2),
    // DIE1..10
    st(7, 10, A::None, 18),
    st(8, 10, A::Scream, 19),
    st(9, 10, A::None, 20),
    st(10, 10, A::None, 21),
    st(11, 10, A::None, 22),
    st(12, 10, A::Fall, 23),
    st(13, 10, A::None, 24),
    st(14, 10, A::None, 25),
    st(15, 30, A::None, 26),
    st(15, -1, A::None, 26),
];

/// `S_SPID_STND` .. `S_SPID_DIE11`.
const SPIDER_STATES: [State; 31] = [
    st(0, 10, A::Look, 1),
    st(1, 10, A::Look, 0),
    // RUN1..12
    st(0, 3, A::Metal, 3),
    st(0, 3, A::Chase, 4),
    st(1, 3, A::Chase, 5),
    st(1, 3, A::Chase, 6),
    st(2, 3, A::Metal, 7),
    st(2, 3, A::Chase, 8),
    st(3, 3, A::Chase, 9),
    st(3, 3, A::Chase, 10),
    st(4, 3, A::Metal, 11),
    st(4, 3, A::Chase, 12),
    st(5, 3, A::Chase, 13),
    st(5, 3, A::Chase, 2),
    // ATK1..4
    lit(0, 20, A::Face, 15),
    lit(6, 4, A::SPosAttack, 16),
    lit(7, 4, A::SPosAttack, 17),
    lit(7, 1, A::SpidRefire, 15),
    // PAIN1..2
    st(8, 3, A::None, 19),
    st(8, 3, A::Pain, 2),
    // DIE1..11
    st(9, 20, A::Scream, 21),
    st(10, 10, A::Fall, 22),
    st(11, 10, A::None, 23),
    st(12, 10, A::None, 24),
    st(13, 10, A::None, 25),
    st(14, 10, A::None, 26),
    st(15, 10, A::None, 27),
    st(16, 10, A::None, 28),
    st(17, 10, A::None, 29),
    st(18, 30, A::None, 30),
    st(18, -1, A::None, 30),
];

static CYBORG: Info = Info {
    name: "CYBERDEMON",
    sprite: "CYBR",
    states: &CYBER_STATES,
    spawn: 0,
    see: 2,
    pain: 16,
    missile: 10,
    death: 17,
    health: 4000,
    reaction_time: 8,
    pain_chance: 20,
    speed: 16.0,
    radius: 40.0,
    height: 110.0,
    see_sound: "cybsit",
    pain_sound: "dmpain",
    death_sound: "cybdth",
    active_sound: "dmact",
};

static SPIDER: Info = Info {
    name: "SPIDER MASTERMIND",
    sprite: "SPID",
    states: &SPIDER_STATES,
    spawn: 0,
    see: 2,
    pain: 18,
    missile: 14,
    death: 20,
    health: 3000,
    reaction_time: 8,
    pain_chance: 40,
    speed: 12.0,
    radius: 128.0,
    height: 100.0,
    see_sound: "spisit",
    pain_sound: "dmpain",
    death_sound: "spidth",
    active_sound: "dmact",
};

/// `dirtype_t`: east, north-east, north ... south-east; 8 is none.
const NO_DIR: u8 = 8;
const OPPOSITE: [u8; 9] = [4, 5, 6, 7, 0, 1, 2, 3, NO_DIR];
const DIAGS: [u8; 4] = [3, 1, 5, 7];
const X_SPEED: [f64; 8] = [1.0, 0.70710678, 0.0, -0.70710678, -1.0, -0.70710678, 0.0, 0.70710678];
const Y_SPEED: [f64; 8] = [0.0, 0.70710678, 1.0, 0.70710678, 0.0, -0.70710678, -1.0, -0.70710678];

/// Binary angle measure: a full turn is 2^32.
const ANG90: u32 = 0x4000_0000;
const ANG270: u32 = 0xC000_0000;

fn bam_to_radians(angle: u32) -> f64 {
    f64::from(angle) / 4_294_967_296.0 * std::f64::consts::TAU
}

/// `R_PointToAngle2` in Doom's plane (x east, y north: block -z).
fn angle_between(from: DVec3, to: DVec3) -> u32 {
    let a = (-(to.z - from.z)).atan2(to.x - from.x).rem_euclid(std::f64::consts::TAU);
    (a / std::f64::consts::TAU * 4_294_967_296.0) as u64 as u32
}

/// `P_AproxDistance` in units, across the plane.
fn approx_distance(a: DVec3, b: DVec3) -> f64 {
    let (dx, dy) = (units((a.x - b.x).abs()), units((a.z - b.z).abs()));
    if dx < dy { dx + dy - dx / 2.0 } else { dx + dy - dy / 2.0 }
}

struct Monster {
    kind: Kind,
    dimension: u8,
    /// Feet, in blocks.
    pos: DVec3,
    prev: DVec3,
    angle: u32,
    health: i32,
    state: usize,
    tics: i32,
    move_dir: u8,
    move_count: i32,
    reaction_time: i32,
    threshold: i32,
    has_target: bool,
    just_attacked: bool,
    just_hit: bool,
    shootable: bool,
    fall: f64,
}

impl Monster {
    fn info(&self) -> &'static Info {
        self.kind.info()
    }

    fn eye(&self) -> DVec3 {
        self.pos + DVec3::Y * blocks(self.info().height * 0.75)
    }
}

/// `MT_ROCKET` (the Cyberdemon's), or its explosion (`S_EXPLODE1..3`).
struct Rocket {
    pos: DVec3,
    prev: DVec3,
    /// Units a tic.
    vel: DVec3,
    angle: u32,
    /// Frames of its explosion so far, with the tics left of the current
    /// one; none while it flies.
    exploding: Option<(u8, i32)>,
    age: u32,
}

/// `MT_PUFF`: where a bullet struck, rising a unit a tic.
struct Puff {
    pos: DVec3,
    frame: u8,
    tics: i32,
}

/// What a tic asks of the world.
#[derive(Default)]
pub(crate) struct Effects {
    /// Damage to the player in MW2 health (Doom's as is), and from where.
    pub damage: Vec<(i32, DVec3)>,
    /// Doom sounds by name (`cybsit`), where (none: everywhere, as Doom
    /// plays the bosses' sight and death), and a volume.
    pub sounds: Vec<(&'static str, Option<DVec3>, f32)>,
}

impl Effects {
    fn sound(&mut self, name: &'static str, at: DVec3) {
        self.sounds.push((name, Some(at), SOUND_VOLUME));
    }
}

/// The player, as the monsters see them this tic.
#[derive(Clone, Copy)]
struct Target {
    feet: DVec3,
}

#[derive(Default)]
pub(crate) struct Bosses {
    monsters: Vec<Monster>,
    rockets: Vec<Rocket>,
    puffs: Vec<Puff>,
    clock: f64,
    rng: u64,
}

impl Bosses {
    /// `P_Random`: 0 to 255.
    fn random(&mut self) -> i32 {
        if self.rng == 0 {
            self.rng = 0x2545_f491_4f6c_dd1d;
        }
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 56) as i32
    }

    /// Summons a boss standing at `feet`, facing `angle` degrees (Minecraft
    /// yaw), in `dimension`, as `P_SpawnMobj` does.
    pub(crate) fn spawn(&mut self, kind: Kind, dimension: u8, feet: DVec3, yaw: f32) -> Effects {
        let info = kind.info();
        // Minecraft's yaw faces -sin, +cos; Doom's angle is anticlockwise
        // from east with north -z.
        let yaw = f64::from(yaw).to_radians();
        let facing = DVec3::new(-yaw.sin(), 0.0, yaw.cos());
        let angle = angle_between(DVec3::ZERO, facing);
        let mut monster = Monster {
            kind,
            dimension,
            pos: feet,
            prev: feet,
            angle,
            health: info.health,
            state: info.spawn,
            tics: info.states[info.spawn].tics,
            move_dir: NO_DIR,
            move_count: 0,
            reaction_time: info.reaction_time,
            threshold: 0,
            has_target: false,
            just_attacked: false,
            just_hit: false,
            shootable: true,
            fall: 0.0,
        };
        // `P_SpawnMapThing` staggers the first tic.
        monster.tics = 1 + (self.random() % monster.tics.max(1));
        diag::info!(World, "Doom: {} at {:.1?}", info.name, feet.to_array());
        self.monsters.push(monster);
        Effects::default()
    }

    /// The console's `doomboss clear`.
    pub(crate) fn clear(&mut self) {
        self.monsters.clear();
        self.rockets.clear();
        self.puffs.clear();
    }

    /// The nearest live boss in `dimension`, for the boss bar.
    pub(crate) fn boss(&self, dimension: u8, player: DVec3) -> Option<(String, f32)> {
        self.monsters
            .iter()
            .filter(|m| m.dimension == dimension && m.health > 0)
            .min_by(|a, b| a.pos.distance_squared(player).total_cmp(&b.pos.distance_squared(player)))
            .map(|m| (m.info().name.to_owned(), (m.health as f32 / m.info().health as f32).clamp(0.0, 1.0)))
    }

    /// Hit boxes for bullets, in blocks: Doom's box, radius round and
    /// height tall.
    pub(crate) fn boxes(&self, dimension: u8) -> Vec<(u64, [f64; 6])> {
        self.monsters
            .iter()
            .enumerate()
            .filter(|(_, m)| m.dimension == dimension && m.shootable)
            .map(|(i, m)| {
                let (r, h, p) = (blocks(m.info().radius), blocks(m.info().height), m.pos);
                (DOOM_KEY | i as u64, [p.x - r, p.y, p.z - r, p.x + r, p.y + h, p.z + r])
            })
            .collect()
    }

    pub(crate) fn owns(key: u64) -> bool {
        key & KIND_MASK == DOOM_KEY
    }

    /// A bullet into a boss with its MW2 damage (`P_DamageMobj`, the player
    /// its source).
    pub(crate) fn shot(&mut self, key: u64, damage: f32) -> Effects {
        let mut fx = Effects::default();
        let index = (key & !KIND_MASK) as usize;
        let damage = damage.round().max(1.0) as i32;
        let pain_roll = self.random();
        let death_roll = self.random();
        let Some(m) = self.monsters.get_mut(index).filter(|m| m.shootable && m.health > 0) else { return fx };
        let info = m.info();
        m.health -= damage;
        if m.health <= 0 {
            // `P_KillMobj`.
            m.shootable = false;
            diag::info!(World, "Doom: {} killed", info.name);
            set_state(m, info.death, &mut fx, None);
            m.tics = (m.tics - (death_roll & 3)).max(1);
            return fx;
        }
        if pain_roll < i32::from(info.pain_chance) {
            m.just_hit = true;
            set_state(m, info.pain, &mut fx, None);
        }
        m.reaction_time = 0;
        if m.threshold == 0 {
            m.has_target = true;
            m.threshold = BASE_THRESHOLD;
            if m.state == info.spawn {
                set_state(m, info.see, &mut fx, None);
            }
        }
        fx
    }

    /// Advances by `dt` seconds in `dimension`. `player` is the player's feet
    /// while alive; `solid` whether a block stops things and sight.
    pub(crate) fn update(&mut self, dt: f64, dimension: u8, player: Option<DVec3>, solid: impl Fn(BlockPos) -> bool) -> Effects {
        let mut fx = Effects::default();
        if self.monsters.is_empty() && self.rockets.is_empty() && self.puffs.is_empty() {
            self.clock = 0.0;
            return fx;
        }
        self.clock += dt.min(0.25);
        while self.clock >= TIC {
            self.clock -= TIC;
            self.tic(dimension, player.map(|feet| Target { feet }), &solid, &mut fx);
        }
        fx
    }

    fn tic(&mut self, dimension: u8, target: Option<Target>, solid: &impl Fn(BlockPos) -> bool, fx: &mut Effects) {
        for i in 0..self.monsters.len() {
            if self.monsters[i].dimension != dimension {
                continue;
            }
            self.monsters[i].prev = self.monsters[i].pos;
            gravity(&mut self.monsters[i], solid);
            let m = &mut self.monsters[i];
            if m.tics == -1 {
                continue;
            }
            m.tics -= 1;
            if m.tics > 0 {
                continue;
            }
            let next = m.info().states[m.state].next;
            self.enter(i, next, target, solid, fx);
        }
        self.tick_rockets(target, solid, fx);
        for puff in &mut self.puffs {
            puff.pos.y += blocks(1.0);
            puff.tics -= 1;
            if puff.tics <= 0 {
                puff.frame += 1;
                puff.tics = 4;
            }
        }
        self.puffs.retain(|p| p.frame < 4);
    }

    /// `P_SetMobjState`: enters a state and runs its action, on through any
    /// of no tics.
    fn enter(&mut self, i: usize, mut state: usize, target: Option<Target>, solid: &impl Fn(BlockPos) -> bool, fx: &mut Effects) {
        for _ in 0..16 {
            let m = &mut self.monsters[i];
            let row = m.info().states[state];
            m.state = state;
            m.tics = row.tics;
            self.act(i, row.action, target, solid, fx);
            let m = &self.monsters[i];
            if m.tics != 0 {
                return;
            }
            state = m.info().states[m.state].next;
        }
    }

    fn act(&mut self, i: usize, action: Action, target: Option<Target>, solid: &impl Fn(BlockPos) -> bool, fx: &mut Effects) {
        match action {
            Action::None => {}
            Action::Look => self.look(i, target, solid, fx),
            Action::Chase => self.chase(i, target, solid, fx),
            Action::Hoof => {
                fx.sound("hoof", self.monsters[i].pos);
                self.chase(i, target, solid, fx);
            }
            Action::Metal => {
                fx.sound("metal", self.monsters[i].pos);
                self.chase(i, target, solid, fx);
            }
            Action::Face => face(&mut self.monsters[i], target),
            Action::CyberAttack => {
                let m = &mut self.monsters[i];
                face(m, target);
                if let Some(t) = target.filter(|_| m.has_target) {
                    self.spawn_rocket(i, t, fx);
                }
            }
            Action::SPosAttack => self.chaingun(i, target, solid, fx),
            Action::SpidRefire => {
                face(&mut self.monsters[i], target);
                if self.random() < 10 {
                    return;
                }
                let m = &self.monsters[i];
                let lost = match target.filter(|_| m.has_target) {
                    Some(t) => !sight(m.eye(), t.feet + DVec3::Y * blocks(PLAYER_HEIGHT * 0.75), solid),
                    None => true,
                };
                if lost {
                    let see = m.info().see;
                    self.set(i, see, target, solid, fx);
                }
            }
            Action::Pain => {
                let m = &self.monsters[i];
                fx.sound(m.info().pain_sound, m.pos);
            }
            Action::Scream => {
                // The bosses' death is heard everywhere.
                fx.sounds.push((self.monsters[i].info().death_sound, None, 1.0));
            }
            Action::Fall => {}
        }
    }

    fn set(&mut self, i: usize, state: usize, target: Option<Target>, solid: &impl Fn(BlockPos) -> bool, fx: &mut Effects) {
        self.enter(i, state, target, solid, fx);
    }

    /// `A_Look` with `P_LookForPlayers` (not all round: behind it, only
    /// within melee range).
    fn look(&mut self, i: usize, target: Option<Target>, solid: &impl Fn(BlockPos) -> bool, fx: &mut Effects) {
        let m = &mut self.monsters[i];
        m.threshold = 0;
        if !sees(m, target, false, solid) {
            return;
        }
        m.has_target = true;
        fx.sounds.push((m.info().see_sound, None, 1.0));
        let see = m.info().see;
        self.set(i, see, target, solid, fx);
    }

    /// `A_Chase`.
    fn chase(&mut self, i: usize, target: Option<Target>, solid: &impl Fn(BlockPos) -> bool, fx: &mut Effects) {
        let m = &mut self.monsters[i];
        if m.reaction_time > 0 {
            m.reaction_time -= 1;
        }
        if m.threshold > 0 {
            m.threshold = if target.is_none() { 0 } else { m.threshold - 1 };
        }
        // Turn toward the way it walks, an eighth at a time.
        if m.move_dir < 8 {
            m.angle &= 7 << 29;
            let delta = m.angle.wrapping_sub(u32::from(m.move_dir) << 29) as i32;
            if delta > 0 {
                m.angle = m.angle.wrapping_sub(ANG90 / 2);
            } else if delta < 0 {
                m.angle = m.angle.wrapping_add(ANG90 / 2);
            }
        }
        let Some(t) = target.filter(|_| m.has_target) else {
            // Lost its player: look all round, or stand.
            if sees(m, target, true, solid) {
                m.has_target = true;
                return;
            }
            m.has_target = false;
            let spawn = m.info().spawn;
            self.set(i, spawn, target, solid, fx);
            return;
        };
        if m.just_attacked {
            m.just_attacked = false;
            self.new_chase_dir(i, t, solid);
            return;
        }
        if m.move_count == 0 && self.check_missile_range(i, t, solid) {
            let m = &mut self.monsters[i];
            m.just_attacked = true;
            let missile = m.info().missile;
            self.set(i, missile, target, solid, fx);
            return;
        }
        let m = &mut self.monsters[i];
        m.move_count -= 1;
        if m.move_count < 0 || !step(m, solid) {
            self.new_chase_dir(i, t, solid);
        }
        if self.random() < 3 {
            let m = &self.monsters[i];
            fx.sound(m.info().active_sound, m.pos);
        }
    }

    /// `P_CheckMissileRange`, the bosses' reading of it.
    fn check_missile_range(&mut self, i: usize, t: Target, solid: &impl Fn(BlockPos) -> bool) -> bool {
        let roll = self.random();
        let m = &mut self.monsters[i];
        if !sight(m.eye(), t.feet + DVec3::Y * blocks(PLAYER_HEIGHT * 0.75), solid) {
            return false;
        }
        if m.just_hit {
            m.just_hit = false;
            return true;
        }
        if m.reaction_time > 0 {
            return false;
        }
        // No melee attack: another 128 units off.
        let mut dist = approx_distance(m.pos, t.feet) - 64.0 - 128.0;
        dist /= 2.0;
        dist = dist.min(200.0);
        if m.kind == Kind::Cyberdemon {
            dist = dist.min(160.0);
        }
        f64::from(roll) >= dist
    }

    /// `P_NewChaseDir`.
    fn new_chase_dir(&mut self, i: usize, t: Target, solid: &impl Fn(BlockPos) -> bool) {
        let swap_roll = self.random();
        let order_roll = self.random();
        let m = &mut self.monsters[i];
        let old = m.move_dir;
        let turnaround = OPPOSITE[usize::from(old)];
        let dx = units(t.feet.x - m.pos.x);
        let dy = units(-(t.feet.z - m.pos.z));
        let mut d1 = if dx > 10.0 { 0 } else if dx < -10.0 { 4 } else { NO_DIR };
        let mut d2 = if dy < -10.0 { 6 } else if dy > 10.0 { 2 } else { NO_DIR };
        let try_walk = |m: &mut Monster, dir: u8, count: i32| -> bool {
            m.move_dir = dir;
            if step(m, solid) {
                m.move_count = count;
                true
            } else {
                false
            }
        };
        let count = self.rng as i32 & 15;
        if d1 != NO_DIR && d2 != NO_DIR {
            let dir = DIAGS[(usize::from(dy < 0.0) << 1) + usize::from(dx > 0.0)];
            if dir != turnaround && try_walk(m, dir, count) {
                return;
            }
        }
        if swap_roll > 200 || dy.abs() > dx.abs() {
            std::mem::swap(&mut d1, &mut d2);
        }
        if d1 == turnaround {
            d1 = NO_DIR;
        }
        if d2 == turnaround {
            d2 = NO_DIR;
        }
        for dir in [d1, d2, old] {
            if dir != NO_DIR && try_walk(m, dir, count) {
                return;
            }
        }
        let scan: Vec<u8> = if order_roll & 1 == 1 { (0..8).collect() } else { (0..8).rev().collect() };
        for dir in scan {
            if dir != turnaround && try_walk(m, dir, count) {
                return;
            }
        }
        if turnaround != NO_DIR && try_walk(m, turnaround, count) {
            return;
        }
        m.move_dir = NO_DIR;
    }

    /// `A_CyberAttack`'s `P_SpawnMissile(MT_ROCKET)`: 32 units up, aimed
    /// at the target's feet, 20 units a tic.
    fn spawn_rocket(&mut self, i: usize, t: Target, fx: &mut Effects) {
        let m = &self.monsters[i];
        let from = m.pos + DVec3::Y * blocks(32.0);
        let angle = m.angle;
        let a = bam_to_radians(angle);
        let dist = (approx_distance(from, t.feet) / 20.0).max(1.0);
        let vel = DVec3::new(a.cos() * 20.0, units(t.feet.y - from.y) / dist, -a.sin() * 20.0);
        fx.sound("rlaunc", from);
        // `P_CheckMissileSpawn`: half a tic out.
        let pos = from + vel * (0.5 / UNITS_PER_BLOCK);
        self.rockets.push(Rocket { pos, prev: pos, vel, angle, exploding: None, age: 0 });
    }

    /// `A_SPosAttack`: three pellets, each 3 to 15, spread by
    /// `(P_Random() - P_Random()) << 20` about the aim.
    fn chaingun(&mut self, i: usize, target: Option<Target>, solid: &impl Fn(BlockPos) -> bool, fx: &mut Effects) {
        let Some(t) = target.filter(|_| self.monsters[i].has_target) else { return };
        let m = &mut self.monsters[i];
        fx.sound("shotgn", m.pos);
        face(m, Some(t));
        // `shootz`: half its height and 8 units up.
        let from = m.pos + DVec3::Y * blocks(m.info().height / 2.0 + 8.0);
        let base = m.angle;
        let aim = t.feet + DVec3::Y * blocks(PLAYER_HEIGHT / 2.0);
        let across = DVec3::new(aim.x - from.x, 0.0, aim.z - from.z).length().max(0.01);
        let slope = (aim.y - from.y) / across;
        for _ in 0..3 {
            let spread = (self.random() - self.random()) << 20;
            let damage = (self.random() % 5 + 1) * 3;
            let a = bam_to_radians(base.wrapping_add(spread as u32));
            let dir = DVec3::new(a.cos(), slope, -a.sin());
            match line_attack(from, dir, t, solid) {
                Hit::Player => fx.damage.push((damage, from)),
                Hit::Wall(at) => {
                    let tics = (4 - (self.random() & 3)).max(1);
                    self.puffs.push(Puff { pos: at, frame: 0, tics });
                }
                Hit::Nothing => {}
            }
        }
    }

    fn tick_rockets(&mut self, target: Option<Target>, solid: &impl Fn(BlockPos) -> bool, fx: &mut Effects) {
        for rocket in &mut self.rockets {
            rocket.prev = rocket.pos;
            rocket.age += 1;
            if let Some((frame, tics)) = rocket.exploding.as_mut() {
                *tics -= 1;
                if *tics <= 0 {
                    // `S_EXPLODE1..3`: 8, 6 and 4 tics.
                    *frame += 1;
                    *tics = [8, 6, 4].get(usize::from(*frame)).copied().unwrap_or(0);
                }
                continue;
            }
            // Two half moves, so a 20-unit tic does not skip a player.
            let mut hit = None;
            for _ in 0..2 {
                rocket.pos += rocket.vel * (0.5 / UNITS_PER_BLOCK);
                let block = (rocket.pos.x.floor() as i32, rocket.pos.y.floor() as i32, rocket.pos.z.floor() as i32);
                if let Some(t) = target
                    && touches_player(rocket.pos, 11.0, t)
                {
                    hit = Some(true);
                    break;
                }
                if solid(block) || rocket.age > 35 * 10 {
                    rocket.pos -= rocket.vel * (0.25 / UNITS_PER_BLOCK);
                    hit = Some(false);
                    break;
                }
            }
            let Some(direct) = hit else { continue };
            // `P_ExplodeMissile`, then `A_Explode`'s `P_RadiusAttack(128)`.
            rocket.exploding = Some((0, 8));
            fx.sound("barexp", rocket.pos);
            if let Some(t) = target {
                if direct {
                    // `((P_Random() % 8) + 1) * damage`.
                    let roll = {
                        self.rng ^= self.rng << 13;
                        self.rng ^= self.rng >> 7;
                        self.rng ^= self.rng << 17;
                        (self.rng >> 56) as i32
                    };
                    fx.damage.push(((roll % 8 + 1) * 20, rocket.pos));
                }
                let blast = radius_damage(rocket.pos, t);
                if blast > 0 && sight(rocket.pos, t.feet + DVec3::Y * blocks(PLAYER_HEIGHT / 2.0), solid) {
                    fx.damage.push((blast, rocket.pos));
                }
            }
        }
        self.rockets.retain(|r| r.exploding.map_or(true, |(frame, _)| frame < 3));
    }

    /// Everything as sprites, facing `viewer`.
    pub(crate) fn append_meshes(&self, dimension: u8, mesh: &mut ChunkMesh, atlas: &Atlas, light: &SkyLight, viewer: DVec3) {
        let Some(assets) = doom::assets() else { return };
        let partial = self.clock / TIC;
        let light_at = |p: DVec3, bright: bool| {
            let at = (p.x.floor() as i32, (p.y + 0.5).floor() as i32, p.z.floor() as i32);
            let sky = f32::from(light.get(at));
            let block = if bright { 15.0 } else { f32::from(light.get_block(at)) };
            [sky, block]
        };
        for m in self.monsters.iter().filter(|m| m.dimension == dimension) {
            let info = m.info();
            let row = info.states[m.state];
            let at = m.prev.lerp(m.pos, partial);
            let rotation = doom::rotation(at, bam_to_radians(m.angle), viewer);
            if let Some(frame) = assets.frame(info.sprite, row.frame, rotation) {
                doom::append_sprite(mesh, atlas, &frame, at, viewer, light_at(at, row.bright));
            }
        }
        for r in &self.rockets {
            let at = r.prev.lerp(r.pos, partial);
            // Doom stands a sprite on its thing's height by its offsets.
            let frame = match r.exploding {
                None => assets.frame("MISL", 0, doom::rotation(at, bam_to_radians(r.angle), viewer)),
                Some((f, _)) => assets.frame("MISL", f + 1, 0),
            };
            if let Some(frame) = frame {
                doom::append_sprite(mesh, atlas, &frame, at, viewer, light_at(at, true));
            }
        }
        for p in &self.puffs {
            if let Some(frame) = assets.frame("PUFF", p.frame, 0) {
                doom::append_sprite(mesh, atlas, &frame, p.pos, viewer, light_at(p.pos, p.frame == 0));
            }
        }
    }
}

fn set_state(m: &mut Monster, state: usize, fx: &mut Effects, _target: Option<Target>) {
    // Pain and death rows begin with no action but `A_Pain` and
    // `A_Scream`, which the bosses' first pain and death rows carry.
    let row = m.info().states[state];
    m.state = state;
    m.tics = row.tics;
    match row.action {
        Action::Pain => fx.sound(m.info().pain_sound, m.pos),
        Action::Scream => fx.sounds.push((m.info().death_sound, None, 1.0)),
        _ => {}
    }
}

/// `A_FaceTarget`.
fn face(m: &mut Monster, target: Option<Target>) {
    if let Some(t) = target.filter(|_| m.has_target) {
        m.angle = angle_between(m.pos, t.feet);
    }
}

/// `P_LookForPlayers`: the player, alive and in sight, and (unless all
/// round) not behind it past melee range.
fn sees(m: &Monster, target: Option<Target>, all_round: bool, solid: &impl Fn(BlockPos) -> bool) -> bool {
    let Some(t) = target else { return false };
    if !sight(m.eye(), t.feet + DVec3::Y * blocks(PLAYER_HEIGHT * 0.75), solid) {
        return false;
    }
    if !all_round {
        let an = angle_between(m.pos, t.feet).wrapping_sub(m.angle);
        if an > ANG90 && an < ANG270 && approx_distance(m.pos, t.feet) > 64.0 {
            return false;
        }
    }
    true
}

/// `P_CheckSight`, through the blocks: nothing solid on the line.
fn sight(from: DVec3, to: DVec3, solid: &impl Fn(BlockPos) -> bool) -> bool {
    let length = from.distance(to);
    let steps = (length / 0.25).ceil().max(1.0) as usize;
    (1..steps).all(|s| {
        let p = from.lerp(to, s as f64 / steps as f64);
        !solid((p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32))
    })
}

enum Hit {
    Player,
    Wall(DVec3),
    Nothing,
}

/// `P_LineAttack` against the player and the blocks, out to
/// `MISSILERANGE`.
fn line_attack(from: DVec3, dir: DVec3, t: Target, solid: &impl Fn(BlockPos) -> bool) -> Hit {
    // Units along the plane per step; the slope rides along.
    let step = 0.125;
    let flat = DVec3::new(dir.x, 0.0, dir.z).length().max(1e-6);
    let dir = dir / flat;
    let mut p = from;
    for _ in 0..(blocks(MISSILE_RANGE) / step) as usize {
        p += dir * step;
        if touches_player(p, 0.0, t) {
            return Hit::Player;
        }
        if solid((p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32)) {
            return Hit::Wall(p - dir * step);
        }
    }
    Hit::Nothing
}

/// Whether a point (with `radius` units of its own) is inside the player.
fn touches_player(p: DVec3, radius: f64, t: Target) -> bool {
    let reach = blocks(PLAYER_RADIUS + radius);
    (p.x - t.feet.x).abs() < reach
        && (p.z - t.feet.z).abs() < reach
        && p.y > t.feet.y - blocks(radius)
        && p.y < t.feet.y + blocks(PLAYER_HEIGHT + radius)
}

/// `PIT_RadiusAttack` with 128 damage: 128 less the distance past the
/// player's radius, the distance the largest of across, along and (here)
/// up or down to them.
fn radius_damage(at: DVec3, t: Target) -> i32 {
    let dx = units((at.x - t.feet.x).abs());
    let dz = units((at.z - t.feet.z).abs());
    let top = t.feet.y + blocks(PLAYER_HEIGHT);
    let dy = units(if at.y < t.feet.y { t.feet.y - at.y } else if at.y > top { at.y - top } else { 0.0 });
    let dist = (dx.max(dz).max(dy) - PLAYER_RADIUS).max(0.0);
    if dist >= 128.0 { 0 } else { (128.0 - dist) as i32 }
}

/// The half width of a monster's feet, in blocks (see the module's note).
fn foot(m: &Monster) -> f64 {
    blocks(m.info().radius).min(1.0)
}

/// The floor under a body's feet at `pos`: the top of the highest solid
/// block within a step up and eight down, across its footprint; none when
/// its body would meet a wall there.
fn floor_at(m: &Monster, pos: DVec3, solid: &impl Fn(BlockPos) -> bool) -> Option<f64> {
    let r = foot(m);
    let (x0, x1) = ((pos.x - r).floor() as i32, (pos.x + r - 1e-3).floor() as i32);
    let (z0, z1) = ((pos.z - r).floor() as i32, (pos.z + r - 1e-3).floor() as i32);
    let feet = pos.y.floor() as i32;
    let mut floor = i32::MIN;
    for x in x0..=x1 {
        for z in z0..=z1 {
            if let Some(y) = (feet - 8..=feet).rev().find(|&y| solid((x, y, z))) {
                floor = floor.max(y + 1);
            }
        }
    }
    // A step up of one block.
    let floor = if (x0..=x1).any(|x| (z0..=z1).any(|z| solid((x, feet, z)))) { feet + 1 } else { floor };
    if floor == i32::MIN {
        return Some(f64::from(feet - 8));
    }
    // Room for its body over that floor.
    let height = blocks(m.info().height).ceil() as i32;
    let clear = (x0..=x1).all(|x| (z0..=z1).all(|z| (floor..floor + height).all(|y| !solid((x, y, z)))));
    clear.then_some(f64::from(floor))
}

/// `P_Move`: a step of its speed in its direction, if the ground allows.
fn step(m: &mut Monster, solid: &impl Fn(BlockPos) -> bool) -> bool {
    if m.move_dir == NO_DIR {
        return false;
    }
    let d = usize::from(m.move_dir);
    let speed = blocks(m.info().speed);
    let to = m.pos + DVec3::new(X_SPEED[d] * speed, 0.0, -Y_SPEED[d] * speed);
    let Some(floor) = floor_at(m, to, solid) else { return false };
    if floor > m.pos.y + 1.01 {
        return false;
    }
    m.pos.x = to.x;
    m.pos.z = to.z;
    if floor > m.pos.y {
        m.pos.y = floor;
        m.fall = 0.0;
    }
    true
}

/// Falls to the floor under it (Doom's gravity, a unit a tic faster each
/// tic).
fn gravity(m: &mut Monster, solid: &impl Fn(BlockPos) -> bool) {
    let r = foot(m);
    let (x0, x1) = ((m.pos.x - r).floor() as i32, (m.pos.x + r - 1e-3).floor() as i32);
    let (z0, z1) = ((m.pos.z - r).floor() as i32, (m.pos.z + r - 1e-3).floor() as i32);
    let below = (m.pos.y - 0.01).floor() as i32;
    let standing = (x0..=x1).any(|x| (z0..=z1).any(|z| solid((x, below, z))));
    if standing || m.pos.y < -128.0 {
        m.fall = 0.0;
        return;
    }
    m.fall += blocks(1.0);
    let target = m.pos.y - m.fall;
    // Land on the first floor on the way down.
    let mut y = m.pos.y;
    while y > target {
        let next = (y - 0.25).max(target);
        let cell = next.floor() as i32;
        if (x0..=x1).any(|x| (z0..=z1).any(|z| solid((x, cell, z)))) {
            m.pos.y = f64::from(cell + 1);
            m.fall = 0.0;
            return;
        }
        y = next;
    }
    m.pos.y = target;
}
