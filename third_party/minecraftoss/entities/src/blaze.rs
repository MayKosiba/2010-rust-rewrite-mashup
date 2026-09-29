//! Pinned Java 26.3 blaze state. Sources: `EntityTypes.BLAZE` (0.6 wide,
//! 1.8 tall, fire immune; eyes at the default 0.85 of its height),
//! `Blaze.createAttributes` (20 health, 6 attack damage, 0.23 speed, 48
//! follow range), `Blaze` (`allowedHeightOffset`,
//! `nextHeightOffsetChangeTick`, the charged flag) and
//! `Blaze.BlazeAttackGoal` (`attackStep`, `attackTime`, `lastSeen`).
use crate::{health::DamageState, movement::Body};
use glam::DVec3;

/// `Attributes.MAX_HEALTH`.
pub const MAX_HEALTH: f32 = 20.0;
/// `Attributes.ATTACK_DAMAGE`: a touch of its melee.
pub const ATTACK_DAMAGE: f32 = 6.0;
/// `Attributes.MOVEMENT_SPEED`.
pub const MOVEMENT_SPEED: f64 = 0.23_f32 as f64;
/// `Attributes.FOLLOW_RANGE`: how far it hunts and shoots.
pub const FOLLOW_RANGE: f64 = 48.0;
/// `EntityDimensions.eyeHeight` defaulting to 0.85 of 1.8.
pub const EYE_HEIGHT: f32 = 1.53;
/// `SmallFireball`'s hit (`DamageSources.fireball`).
pub const FIREBALL_DAMAGE: f32 = 5.0;

#[derive(Clone, Debug)]
pub struct Blaze {
    pub body: Body,
    pub health: f32,
    pub damage: DamageState,
    pub persistence_required: bool,
    /// Its body's facing (`getYRot`).
    pub yaw: f32,
    /// `allowedHeightOffset`: how far under its target's eyes it may hover
    /// before it rises, re-rolled every 100 ticks.
    pub allowed_height_offset: f32,
    /// `nextHeightOffsetChangeTick`.
    pub next_height_offset_change_tick: i32,
    /// `DATA_FLAGS_ID`'s charged bit (`setCharged`): set while a volley
    /// winds up and fires; the client shows it burning.
    pub charged: bool,
    /// `BlazeAttackGoal.attackStep`: 0 idle, 1 wound up, 2 to 4 shots.
    pub attack_step: i32,
    /// `BlazeAttackGoal.attackTime`: ticks until the next step or bite.
    pub attack_time: i32,
    /// `BlazeAttackGoal.lastSeen`: ticks since it last saw its target.
    pub last_seen: i32,
}

impl Blaze {
    pub fn new(position: DVec3) -> Self {
        Self {
            body: Body::new(position, 0.6, 1.8),
            health: MAX_HEALTH,
            damage: DamageState::default(),
            persistence_required: false,
            yaw: 0.0,
            allowed_height_offset: 0.5,
            next_height_offset_change_tick: 0,
            charged: false,
            attack_step: 0,
            attack_time: 0,
            last_seen: 0,
        }
    }

    pub fn eye_height(&self) -> f32 {
        EYE_HEIGHT
    }

    /// `BlazeAttackGoal.stop`: the volley ends and the charge goes out.
    pub fn stop_attack(&mut self) {
        self.attack_step = 0;
        self.charged = false;
    }
}

/// `Blaze.aiStep`'s slow fall: airborne and sinking, it keeps 60% of its
/// downward speed.
pub fn slow_fall(body: &mut Body) {
    if !body.on_ground && body.velocity.y < 0.0 {
        body.velocity.y *= 0.6;
    }
}

/// `Blaze.customServerAiStep`'s climb: under its target's eyes by more
/// than it allows, it eases its vertical speed towards 0.3.
pub fn climb_towards(body: &mut Body, target_eye_y: f64, allowed_height_offset: f32) {
    let eye = body.position.y + f64::from(EYE_HEIGHT);
    if target_eye_y > eye + f64::from(allowed_height_offset) {
        body.velocity.y += (0.3_f32 as f64 - body.velocity.y) * 0.3_f32 as f64;
        body.needs_sync = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_slowly_only_while_airborne_and_sinking() {
        let mut body = Body::new(DVec3::ZERO, 0.6, 1.8);
        body.velocity.y = -0.5;
        slow_fall(&mut body);
        assert!((body.velocity.y + 0.3).abs() < 1.0e-9);
        body.on_ground = true;
        body.velocity.y = -0.5;
        slow_fall(&mut body);
        assert_eq!(body.velocity.y, -0.5);
    }

    #[test]
    fn climbs_under_a_higher_target() {
        let mut body = Body::new(DVec3::ZERO, 0.6, 1.8);
        climb_towards(&mut body, 10.0, 0.5);
        assert!(body.velocity.y > 0.08);
        let mut level = Body::new(DVec3::ZERO, 0.6, 1.8);
        climb_towards(&mut level, 1.6, 0.5);
        assert_eq!(level.velocity.y, 0.0);
    }
}
