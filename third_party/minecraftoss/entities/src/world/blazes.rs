//! Blazes (26.3 `Blaze`) and their small fireballs (`SmallFireball` over
//! `AbstractHurtingProjectile`) in the entity world. The blaze keeps its
//! own small AI in place of the monster goal framework: `HurtByTargetGoal`
//! and `NearestAttackableTargetGoal<Player>` pick what it hunts,
//! `BlazeAttackGoal` bites within 2 blocks and fires volleys of three
//! fireballs beyond, `Blaze.customServerAiStep` lifts it towards its
//! target's eyes, `Blaze.aiStep` slows its fall, and a stroll wanders it
//! when idle. It is fire immune, takes no fall damage
//! (`Blaze.causeFallDamage`), and water and rain hurt it
//! (`isSensitiveToWater`).
use super::*;
use super::emissions::{base_tick_fluid, play_movement, MovementSounds};
use crate::blaze::{self, Blaze};
use crate::inside_blocks::Hazard;
use crate::monster_ai::{MobCandidate, Target};

/// A blaze is a `Monster`: the hostile swim and splash, and no step sound
/// of its own.
const BLAZE_SOUNDS: MovementSounds = MovementSounds::monster(None);

/// `SmallFireball`'s box: 0.3125 blocks a side.
pub const SMALL_FIREBALL_SIZE: f64 = 0.3125;

/// A blaze with its own hover, targeting and volley AI.
#[derive(Clone)]
pub struct BlazeEntity {
    pub id: u64,
    /// Its status effects (`activeEffects`).
    pub effects: crate::effects::MobEffects,
    pub blaze: Blaze,
    pub no_ai: bool,
    pub tick_count: i32,
    pub ambient_sound_time: i32,
    /// Sounds it made since the world last collected them.
    pub voices: Vec<(Voice, DVec3)>,
    pub no_action_time: i32,
    pub random: LegacyRandom,
    pub previous_position: DVec3,
    pub look_control: crate::look::LookControl,
    pub body_rotation: crate::look::BodyRotation,
    /// `Mob.getTarget`.
    pub target: Option<Target>,
    /// `LivingEntity.lastHurtByMob` with its tick count.
    pub hurt_by: Option<(Target, i32)>,
    /// Where an idle stroll heads (`WaterAvoidingRandomStrollGoal`, kept
    /// simple), and for how many more ticks.
    pub wander: Option<(DVec3, i32)>,
}

/// A blaze's small fireball: it flies along its heading, speeding up by
/// `accelerationPower` a tick under its inertia, and burns what it hits.
#[derive(Clone, Debug)]
pub struct SmallFireballEntity {
    pub id: u64,
    /// The blaze that shot it (`Projectile.getOwner`).
    pub owner: u64,
    pub position: DVec3,
    pub previous_position: DVec3,
    pub velocity: DVec3,
    /// Ticks it has flown (`tickCount`).
    pub age: i32,
    pub alive: bool,
}

/// What a blaze's AI asks of the world after its tick.
#[derive(Default)]
struct BlazeActions {
    /// A bite (`doHurtTarget`) on its target, from where it stands.
    bite: Option<(Target, DVec3)>,
    /// Fireballs to launch: where and along which heading.
    shots: Vec<(DVec3, DVec3)>,
}

/// A target's feet, eyes and height, while it can still be hunted.
#[derive(Clone, Copy)]
struct Located {
    position: DVec3,
    eye_height: f32,
    height: f32,
}

fn locate(target: Target, players: &[PlayerCandidate], mobs: &[MobCandidate]) -> Option<Located> {
    match target {
        Target::Player(id) => players
            .iter()
            .find(|p| p.id == id && p.alive && !p.spectator && p.attackable)
            .map(|p| Located { position: p.position, eye_height: p.eye_height, height: 1.8 }),
        Target::Villager(id) | Target::Mob(id) => mobs
            .iter()
            .find(|m| m.id == id && m.alive)
            .map(|m| Located { position: m.position, eye_height: m.eye_height, height: m.height }),
    }
}

impl BlazeEntity {
    /// Where it stands (its feet).
    pub fn position(&self) -> DVec3 {
        self.blaze.body.position
    }

    pub fn set_random_seed(&mut self, seed: u64) {
        self.random = LegacyRandom::new(seed);
    }

    pub fn hurt(&mut self, amount: f32) -> DamageResult {
        // `hurtServer` resets the idle clock for any hit on a living mob.
        if self.blaze.health > 0.0 && !self.blaze.damage.dead {
            self.no_action_time = 0;
        }
        let result = self.blaze.damage.hurt_generic(&mut self.blaze.health, blaze::MAX_HEALTH, amount);
        let position = self.position();
        self.blaze.damage.place_death(result, position, false);
        // A full hit plays `entity.blaze.hurt` or `.death`.
        if result.applied && result.full {
            if !result.died {
                self.ambient_sound_time = -80;
            }
            let voice = hurt_voice(&mut self.random, result.died, false);
            self.voices.push((voice, position));
        }
        result
    }

    /// `Monster.checkAnyLightMonsterSpawnRules` aside, the blaze's
    /// `Mob.serverAiStep` and `LivingEntity.aiStep` while alive with AI.
    fn tick_ai(&mut self, world: &impl World, players: &[PlayerCandidate], mobs: &[MobCandidate]) -> BlazeActions {
        let mut actions = BlazeActions::default();
        let position = self.blaze.body.position;
        // `Mob.checkDespawn` runs first: a player within 32 blocks resets
        // the idle clock.
        if players.iter().any(|p| p.alive && !p.spectator && p.position.distance_squared(position) < 32.0 * 32.0) {
            self.no_action_time = 0;
        }
        self.no_action_time += 1;
        // `Blaze.aiStep`: sinking in the air, it falls at 60%.
        blaze::slow_fall(&mut self.blaze.body);
        self.blaze.body.trim_small_velocity();
        let eye = position + DVec3::new(0.0, f64::from(blaze::EYE_HEIGHT), 0.0);
        let sees = |to: DVec3| crate::sight::line_of_sight(world, eye, to);
        // `HurtByTargetGoal`: whoever hurt it last.
        if let Some((attacker, _)) = self.hurt_by {
            if self.target != Some(attacker) && locate(attacker, players, mobs).is_some() {
                self.target = Some(attacker);
            }
        }
        // A target it can no longer hunt, or beyond its follow range, is let go.
        if let Some(target) = self.target {
            let keep = locate(target, players, mobs).is_some_and(|t| t.position.distance_squared(position) <= blaze::FOLLOW_RANGE * blaze::FOLLOW_RANGE);
            if !keep {
                self.target = None;
                self.blaze.stop_attack();
            }
        }
        // `NearestAttackableTargetGoal<Player>`: now and then, the nearest
        // player it can see within its follow range.
        if self.target.is_none() && self.random.next_int(10) == 0 {
            self.target = players
                .iter()
                .filter(|p| p.alive && !p.spectator && p.attackable)
                .filter(|p| p.position.distance_squared(position) <= blaze::FOLLOW_RANGE * blaze::FOLLOW_RANGE)
                .filter(|p| sees(p.position + DVec3::new(0.0, f64::from(p.eye_height), 0.0)))
                .min_by(|a, b| a.position.distance_squared(position).total_cmp(&b.position.distance_squared(position)))
                .map(|p| Target::Player(p.id));
        }
        // `Blaze.customServerAiStep`: the allowed height re-rolls every 100
        // ticks.
        self.blaze.next_height_offset_change_tick -= 1;
        if self.blaze.next_height_offset_change_tick <= 0 {
            self.blaze.next_height_offset_change_tick = 100;
            self.blaze.allowed_height_offset = 0.5 + self.random.next_gaussian() as f32 * 3.0;
        }
        let mut wanted = None;
        let located = self.target.and_then(|t| locate(t, players, mobs).map(|l| (t, l)));
        if let Some((target, t)) = located {
            self.wander = None;
            let target_eye = t.position + DVec3::new(0.0, f64::from(t.eye_height), 0.0);
            blaze::climb_towards(&mut self.blaze.body, target_eye.y, self.blaze.allowed_height_offset);
            // `BlazeAttackGoal.tick`.
            let blaze = &mut self.blaze;
            blaze.attack_time -= 1;
            let can_see = sees(target_eye);
            if can_see {
                blaze.last_seen = 0;
            } else {
                blaze.last_seen += 1;
            }
            let distance = position.distance_squared(t.position);
            if distance < 4.0 {
                if can_see && blaze.attack_time <= 0 {
                    blaze.attack_time = 20;
                    actions.bite = Some((target, position));
                }
                wanted = Some(t.position);
            } else if distance < blaze::FOLLOW_RANGE * blaze::FOLLOW_RANGE && can_see {
                let xd = t.position.x - position.x;
                let yd = (t.position.y + f64::from(t.height) * 0.5) - (position.y + f64::from(blaze.body.height) * 0.5);
                let zd = t.position.z - position.z;
                if blaze.attack_time <= 0 {
                    blaze.attack_step += 1;
                    if blaze.attack_step == 1 {
                        blaze.attack_time = 60;
                        blaze.charged = true;
                    } else if blaze.attack_step <= 4 {
                        blaze.attack_time = 6;
                    } else {
                        blaze.attack_time = 100;
                        blaze.attack_step = 0;
                        blaze.charged = false;
                    }
                    if blaze.attack_step > 1 {
                        let spread = distance.sqrt().sqrt() * 0.5;
                        let gx = self.random.next_gaussian();
                        let gz = self.random.next_gaussian();
                        let heading = DVec3::new(xd + gx * spread, yd, zd + gz * spread);
                        // From its middle, half a block up (`getY(0.5) + 0.5`).
                        let from = DVec3::new(position.x, position.y + f64::from(blaze.body.height) * 0.5 + 0.5, position.z);
                        actions.shots.push((from, heading));
                        // Level event 1018: `entity.blaze.shoot`.
                        let pitch = (self.random.next_float() - self.random.next_float()) * 0.2 + 1.0;
                        self.voices.push((Voice::Event("entity.blaze.shoot", 2.0, pitch), position));
                    }
                }
                // Simplified: it closes in beyond 5 blocks rather than
                // holding its ground as vanilla's goal does.
                if distance > 5.0 * 5.0 {
                    wanted = Some(t.position);
                }
            } else if self.blaze.last_seen < 5 {
                wanted = Some(t.position);
            }
            self.look_control.set_look_at_with_limits(target_eye, 10.0, 10.0);
        } else {
            self.blaze.stop_attack();
            // Idle: a stroll now and then (`randomInterval` 120).
            if let Some((_, ticks)) = &mut self.wander {
                *ticks -= 1;
            }
            if self.wander.is_some_and(|(to, ticks)| ticks <= 0 || DVec3::new(to.x - position.x, 0.0, to.z - position.z).length_squared() < 1.0) {
                self.wander = None;
            }
            if self.wander.is_none() && self.random.next_int(120) == 0 {
                let dx = self.random.next_int(21) as f64 - 10.0;
                let dz = self.random.next_int(21) as f64 - 10.0;
                self.wander = Some((position + DVec3::new(dx, 0.0, dz), 200));
            }
            wanted = self.wander.map(|(to, _)| to);
            // `RandomLookAroundGoal`, roughly.
            if self.random.next_int(50) == 0 {
                let yaw = self.random.next_float() * std::f32::consts::TAU;
                let at = eye + DVec3::new(f64::from(-yaw.sin()), 0.0, f64::from(yaw.cos()));
                self.look_control.set_look_at(at);
            }
        }
        // `MoveControl.tick` at speed 1 towards where it wants to go.
        let speed = self.effects.movement_speed(blaze::MOVEMENT_SPEED) as f32;
        let mut forward = 0.0;
        if let Some(to) = wanted {
            let (xd, zd) = (to.x - position.x, to.z - position.z);
            if xd * xd + zd * zd > 2.5000003e-7_f32 as f64 {
                let desired = (crate::control::minecraft_atan2(zd, xd) * 57.2957763671875_f64) as f32 - 90.0;
                self.blaze.yaw = crate::control::rotlerp(self.blaze.yaw, desired, 90.0);
                forward = speed;
            }
        }
        self.look_control.tick(position, blaze::EYE_HEIGHT, self.body_rotation.body_yaw, forward != 0.0);
        let body = &mut self.blaze.body;
        let fluid = FluidFrame::sample(world, body.position, body.width, body.height);
        let input = DVec3::new(0.0, 0.0, f64::from(forward));
        if fluid.in_water() {
            body.travel_water(world, input, self.blaze.yaw);
        } else if fluid.in_lava() {
            body.travel_lava(world, input, self.blaze.yaw, fluid.lava_height);
        } else {
            // `Blaze.causeFallDamage`: it never takes any.
            let _ = body.travel_air(world, input, speed, self.blaze.yaw);
        }
        play_movement(body, self.tick_count, &mut self.random, &mut self.voices, BLAZE_SOUNDS);
        self.body_rotation.tick(self.blaze.yaw, &mut self.look_control, self.previous_position, self.blaze.body.position);
        actions
    }
}

impl EntityWorld {
    /// A blaze facing its yaw (NoAI blazes keep still but still sound and
    /// take hits).
    pub fn spawn_blaze(&mut self, blaze: Blaze, no_ai: bool) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let yaw = blaze.yaw;
        self.blazes.push(BlazeEntity {
            id,
            effects: Default::default(),
            previous_position: blaze.body.position,
            blaze,
            no_ai,
            tick_count: 0,
            ambient_sound_time: 0,
            voices: Vec::new(),
            no_action_time: 0,
            random: LegacyRandom::new(0),
            look_control: crate::look::LookControl::new(yaw),
            body_rotation: crate::look::BodyRotation::new(yaw),
            target: None,
            hurt_by: None,
            wander: None,
        });
        self.order.push(EntityKey::Blaze(id));
        self.file_in_section(EntityKey::Blaze(id));
        id
    }

    pub fn blazes(&self) -> &[BlazeEntity] {
        &self.blazes
    }

    pub fn blaze_mut(&mut self, id: u64) -> Option<&mut BlazeEntity> {
        self.blazes.iter_mut().find(|entity| entity.id == id)
    }

    /// The small fireballs in flight.
    pub fn blaze_fireballs(&self) -> &[SmallFireballEntity] {
        &self.blaze_fireballs
    }

    /// One blaze's tick (`Blaze.aiStep` over the living tick), then its
    /// bite and the fireballs it fired.
    pub(super) fn tick_blaze(&mut self, id: u64, world: &impl World, players: &[PlayerCandidate], ticks: &dyn Fn(DVec3) -> bool) {
        let game_time = self.game_time;
        let mobs = self.mob_candidates();
        let Some(entity) = self.blazes.iter_mut().find(|e| e.id == id) else { return };
        entity.previous_position = entity.blaze.body.position;
        entity.tick_count += 1;
        // `Entity.baseTick`'s fluid interaction (the first tick never splashes).
        base_tick_fluid(&mut entity.blaze.body, world, entity.tick_count == 1, &mut entity.random, &mut entity.voices, BLAZE_SOUNDS);
        super::hazards::burn(entity, world, game_time);
        super::hazards::suffocate(entity, world, blaze::EYE_HEIGHT, game_time);
        if entity.blaze.health > 0.0 && super::living::breathe(&mut entity.blaze.body, world, blaze::EYE_HEIGHT, false) {
            entity.hurt(2.0);
        }
        let removed = entity.blaze.damage.tick();
        // `LivingEntity.baseTick` forgets an attacker after 100 ticks.
        if entity.hurt_by.is_some_and(|(_, when)| entity.tick_count - when > 100) {
            entity.hurt_by = None;
        }
        // `tickEffects`, after the hurt and death clocks.
        for work in entity.effects.tick(false) {
            match work.resolve(entity.blaze.health, blaze::MAX_HEALTH) {
                Some(EffectWork::Heal(amount)) => heal(&mut entity.blaze.health, blaze::MAX_HEALTH, amount),
                Some(EffectWork::HurtMagic(amount)) => {
                    entity.hurt(amount);
                }
                _ => {}
            }
        }
        // `Mob.baseTick`'s ambient sound (`entity.blaze.ambient`).
        if entity.blaze.health > 0.0 {
            if (entity.random.next_int(1000) as i32) < entity.ambient_sound_time {
                entity.ambient_sound_time = -80;
                let voice = Voice::Ambient(voice_pitch(&mut entity.random, false));
                entity.voices.push((voice, entity.position()));
            } else {
                entity.ambient_sound_time += 1;
            }
            // `Blaze.aiStep`'s crackle, played client side in vanilla.
            if entity.random.next_int(24) == 0 {
                let volume = 1.0 + entity.random.next_float();
                let pitch = entity.random.next_float() * 0.7 + 0.3;
                entity.voices.push((Voice::Event("entity.blaze.burn", volume, pitch), entity.position()));
            }
        }
        let mut actions = BlazeActions::default();
        if !removed {
            if entity.blaze.health <= 0.0 && !entity.no_ai {
                let yaw = entity.blaze.yaw;
                let _ = super::living::dying_travel(&mut entity.blaze.body, world, yaw, entity.tick_count, &mut entity.random, &mut entity.voices, BLAZE_SOUNDS, Some(true));
            }
            if entity.blaze.health > 0.0 {
                // `LivingEntity.aiStep`: `isSensitiveToWater` and in water
                // or rain, 1 drowning damage a tick.
                let p = entity.blaze.body.position;
                let feet = (p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32);
                let top = (feet.0, (p.y + f64::from(entity.blaze.body.height)).floor() as i32, feet.2);
                if entity.blaze.body.touching_water || world.rain_at(feet) || world.rain_at(top) {
                    entity.hurt(1.0);
                }
            }
            if entity.blaze.health > 0.0 && !entity.no_ai {
                actions = entity.tick_ai(world, players, &mobs);
            } else if entity.no_ai {
                entity.blaze.body.trim_small_velocity();
            }
            let old = entity.previous_position;
            super::hazards::blocks_act(entity, world, old, game_time);
            self.push_entities(EntityKey::Blaze(id), world, game_time, ticks);
        }
        if let Some((target, attacker)) = actions.bite {
            match target {
                Target::Player(player_id) => {
                    self.player_hits.push(PlayerHit { player_id, damage: blaze::ATTACK_DAMAGE, kind: PlayerHitKind::Melee { attacker, hunger_ticks: 0, lift: 0.0 }, source: Some(id) });
                }
                Target::Villager(victim) | Target::Mob(victim) => {
                    self.mob_hits_mob(id, victim, blaze::ATTACK_DAMAGE, attacker, 0.0);
                }
            }
        }
        for (from, heading) in actions.shots {
            self.spawn_small_fireball(id, from, heading);
        }
    }

    /// `SmallFireball(level, owner, movement)`: its heading normalised to
    /// `accelerationPower` (0.1) as its first motion.
    pub fn spawn_small_fireball(&mut self, owner: u64, position: DVec3, heading: DVec3) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let velocity = heading.try_normalize().unwrap_or(DVec3::Y) * 0.1;
        self.blaze_fireballs.push(SmallFireballEntity { id, owner, position, previous_position: position, velocity, age: 0, alive: true });
        id
    }

    /// `AbstractHurtingProjectile.tick` for every small fireball: the hit
    /// along its motion (the first block, else the nearest player or mob
    /// box inflated by 0.3, never its owner nor a blaze), then the move
    /// and the speed-up (`accelerationPower` 0.1 along its motion, inertia
    /// 0.95). `SmallFireball.onHitEntity` sets what it hits alight for 5
    /// seconds and deals 5 fireball damage; a mob that shrugs it off stops
    /// burning again. It dies after 200 ticks. Simplified: a hit block
    /// catches no fire, water does not slow it, and the player's side gets
    /// the hit as a projectile's without the burning.
    pub(super) fn tick_blaze_fireballs(&mut self, world: &impl World, players: &[PlayerCandidate]) {
        if self.blaze_fireballs.is_empty() {
            return;
        }
        let game_time = self.game_time;
        let mobs: Vec<MobCandidate> = self.mob_candidates().into_iter().filter(|m| m.alive && m.kind != "minecraft:blaze").collect();
        let mut fireballs = std::mem::take(&mut self.blaze_fireballs);
        for ball in fireballs.iter_mut().filter(|b| b.alive) {
            ball.previous_position = ball.position;
            ball.age += 1;
            if ball.age > 200 {
                ball.alive = false;
                continue;
            }
            let from = ball.position;
            let mut to = from + ball.velocity;
            let block = crate::sight::clip(world, from, to);
            if let Some((_, at)) = block {
                to = at;
            }
            let mut nearest: Option<(Target, f64)> = None;
            let mut consider = |target: Target, min: DVec3, max: DVec3| {
                let margin = DVec3::splat(0.3);
                if let Some(at) = crate::sight::clip_box(min - margin, max + margin, from, to) {
                    let distance = at.distance_squared(from);
                    if nearest.is_none_or(|(_, best)| distance < best) {
                        nearest = Some((target, distance));
                    }
                }
            };
            if self.players_pickable {
                for p in players.iter().filter(|p| p.alive && !p.spectator) {
                    consider(Target::Player(p.id), p.position - DVec3::new(0.3, 0.0, 0.3), p.position + DVec3::new(0.3, 1.8, 0.3));
                }
            }
            for m in mobs.iter().filter(|m| m.id != ball.owner) {
                let half = f64::from(m.width) / 2.0;
                consider(Target::Mob(m.id), m.position - DVec3::new(half, 0.0, half), m.position + DVec3::new(half, f64::from(m.height), half));
            }
            match nearest {
                Some((Target::Player(player_id), _)) => {
                    if players.iter().any(|p| p.id == player_id && p.attackable) {
                        self.player_hits.push(PlayerHit {
                            player_id,
                            damage: blaze::FIREBALL_DAMAGE,
                            kind: PlayerHitKind::Arrow { velocity: ball.velocity, effect: None },
                            source: Some(ball.owner),
                        });
                    }
                    ball.alive = false;
                }
                Some((Target::Mob(victim) | Target::Villager(victim), _)) => {
                    let key = self.order.iter().copied().find(|key| key.id() == victim);
                    if let Some(mob) = key.and_then(|key| self.exposed_mut(key)) {
                        // `igniteForSeconds(5)`, undone when the hurt fails.
                        let before = mob.body().fire_ticks;
                        if !mob.fire_immune() {
                            mob.body().fire_ticks = before.max(100);
                        }
                        if !mob.hurt_hazard(Hazard::InFire, blaze::FIREBALL_DAMAGE, world, game_time) {
                            mob.body().fire_ticks = before;
                        }
                    }
                    ball.alive = false;
                }
                None if block.is_some() => ball.alive = false,
                None => {
                    ball.position = to;
                    let direction = ball.velocity.try_normalize().unwrap_or(DVec3::ZERO);
                    ball.velocity = (ball.velocity + direction * 0.1) * 0.95;
                }
            }
        }
        fireballs.retain(|b| b.alive);
        // Any shot during this pass joined the (now empty) list.
        fireballs.append(&mut self.blaze_fireballs);
        self.blaze_fireballs = fireballs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EmptyWorld;
    impl World for EmptyWorld {
        fn block(&self, _: minecraftoss_player::Pos) -> Option<minecraftoss_player::Block> {
            None
        }
        fn set_block(&mut self, _: minecraftoss_player::Pos, _: Option<minecraftoss_player::Block>) {}
    }

    fn player(position: DVec3) -> PlayerCandidate {
        PlayerCandidate {
            id: 7,
            position,
            eye_height: 1.62,
            main_hand_cow_food: false,
            offhand_cow_food: false,
            main_hand_pig_food: false,
            offhand_pig_food: false,
            main_hand_chicken_food: false,
            offhand_chicken_food: false,
            main_hand_carrot_on_a_stick: false,
            offhand_carrot_on_a_stick: false,
            main_hand_wolf_interest: false,
            offhand_wolf_interest: false,
            main_hand_horse_tempt: false,
            offhand_horse_tempt: false,
            alive: true,
            spectator: false,
            attackable: true,
            wears_gold: false,
        }
    }

    /// A blaze finds a player in range, winds up and fires a volley of
    /// small fireballs at it, and hovers rather than dropping like a stone.
    #[test]
    fn a_blaze_hunts_and_fires_volleys() {
        let mut world = EntityWorld::default();
        world.set_players_pickable(true);
        let id = world.spawn_blaze(Blaze::new(DVec3::new(0.5, 64.0, 0.5)), false);
        let players = [player(DVec3::new(12.5, 64.0, 0.5))];
        let mut most = 0;
        for _ in 0..200 {
            world.tick_with_players(&mut EmptyWorld, &players);
            most = most.max(world.blaze_fireballs().len());
        }
        let blaze = world.blazes().iter().find(|e| e.id == id).unwrap();
        assert_eq!(blaze.target, Some(Target::Player(7)));
        assert!(most >= 1, "it fired");
        assert!(blaze.blaze.body.velocity.y > -1.0, "it falls slowly: {}", blaze.blaze.body.velocity.y);
        let sounds = world.take_sounds();
        assert!(sounds.iter().any(|s| s.event == "entity.blaze.shoot"));
    }

    /// A small fireball speeds up along its heading and hits the player in
    /// its way for 5, credited to its blaze; blazes never stop it.
    #[test]
    fn a_small_fireball_hits_the_player_in_its_way() {
        let mut world = EntityWorld::default();
        world.set_players_pickable(true);
        let owner = world.spawn_blaze(Blaze::new(DVec3::new(0.5, 64.0, 0.5)), true);
        world.spawn_blaze(Blaze::new(DVec3::new(0.5, 64.0, 3.5)), true);
        world.spawn_small_fireball(owner, DVec3::new(0.5, 65.0, 1.5), DVec3::Z);
        let players = [player(DVec3::new(0.5, 64.0, 8.5))];
        let mut hits = Vec::new();
        for _ in 0..20 {
            world.tick_with_players(&mut EmptyWorld, &players);
            hits.extend(world.take_player_hits());
        }
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].damage, 5.0);
        assert_eq!(hits[0].source, Some(owner));
        assert!(world.blaze_fireballs().is_empty());
    }
}
