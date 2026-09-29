//! Pinned Java 26.3 skeleton base state. Sources: EntityTypes.SKELETON,
//! AbstractSkeleton.createAttributes, and inherited Monster lifecycle.
use crate::{health::DamageState, movement::Body};
use glam::DVec3;
use minecraftoss_player::survival::EffectKind;

/// The skeletons the entity world runs (`AbstractSkeleton`'s subclasses).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SkeletonKind {
    #[default]
    Skeleton,
    /// `Stray`: its arrows slow.
    Stray,
    /// `Bogged`: its arrows poison; it can be sheared of its mushrooms.
    Bogged,
    /// `Parched`: its arrows weaken.
    Parched,
    /// `WitherSkeleton`: the Nether fortress's; fire immune, taller, with a
    /// stone sword in place of a bow.
    WitherSkeleton,
}

impl SkeletonKind {
    /// The entity type's ID.
    pub fn type_id(self) -> &'static str {
        match self {
            Self::Skeleton => "minecraft:skeleton",
            Self::Stray => "minecraft:stray",
            Self::Bogged => "minecraft:bogged",
            Self::Parched => "minecraft:parched",
            Self::WitherSkeleton => "minecraft:wither_skeleton",
        }
    }

    /// Its sounds' family (`entity.<family>.ambient`).
    pub fn sound_family(self) -> &'static str {
        match self {
            Self::Skeleton => "skeleton",
            Self::Stray => "stray",
            Self::Bogged => "bogged",
            Self::Parched => "parched",
            Self::WitherSkeleton => "wither_skeleton",
        }
    }

    /// `#minecraft:burn_in_daylight`: all but the parched.
    pub fn burns_in_daylight(self) -> bool {
        !matches!(self, Self::Parched | Self::WitherSkeleton)
    }

    /// `EntityType.fireImmune`.
    pub fn fire_immune(self) -> bool {
        self == Self::WitherSkeleton
    }

    /// `WitherSkeleton.createAttributes`' 4 attack damage and its stone
    /// sword's 4; the archers never strike.
    pub fn melee_damage(self) -> f32 {
        if self == Self::WitherSkeleton { 8.0 } else { 2.0 }
    }

    /// `createAttributes`: bogged and parched have 16 health.
    pub fn max_health(self) -> f32 {
        match self {
            Self::Bogged | Self::Parched => 16.0,
            Self::Skeleton | Self::Stray | Self::WitherSkeleton => 20.0,
        }
    }

    /// `getAttackInterval`, or `getHardAttackInterval` on hard.
    pub fn attack_interval(self, hard: bool) -> i32 {
        match (self, hard) {
            (Self::Bogged | Self::Parched, false) => 70,
            (Self::Bogged | Self::Parched, true) => 50,
            (_, false) => 40,
            (_, true) => 20,
        }
    }

    /// `getArrow`'s effect on the arrows it looses (its full duration: a
    /// plain arrow's `POTION_DURATION_SCALE` is 1).
    pub fn arrow_effect(self) -> Option<(EffectKind, u32)> {
        match self {
            Self::Skeleton | Self::WitherSkeleton => None,
            Self::Stray => Some((EffectKind::Slowness, 600)),
            Self::Bogged => Some((EffectKind::Poison, 100)),
            Self::Parched => Some((EffectKind::Weakness, 600)),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Skeleton {
    pub kind: SkeletonKind,
    /// A bogged's `sheared` flag (its mushrooms gone).
    pub sheared: bool,
    pub body: Body,
    pub health: f32,
    pub damage: DamageState,
    pub persistence_required: bool,
    /// `Entity.remainingFireTicks`.
    /// An item in the head slot (`sunProtectionSlot`), and whether it can
    /// be damaged.
    pub head_item: Option<bool>,
    /// Its bow in the main hand (`populateDefaultEquipmentSlots`).
    pub holds_bow: bool,
    /// The head, chest, legs and feet slots' item IDs.
    pub armor: [Option<String>; 4],
    /// The main hand's drop chance (`Mob.dropChances`); above 1 the bow
    /// drops whoever killed it, unworn (`isPreserved`).
    pub bow_drop_chance: f32,
}

impl Skeleton {
    pub fn new(position: DVec3) -> Self {
        Self {
            kind: SkeletonKind::Skeleton,
            sheared: false,
            body: Body::new(position, 0.6, 1.99),
            health: 20.0,
            damage: DamageState::default(),
            persistence_required: false,
            head_item: None,
            holds_bow: true,
            armor: Default::default(),
            bow_drop_chance: 0.085,
        }
    }

    /// A skeleton of `kind` at full health.
    pub fn of_kind(kind: SkeletonKind, position: DVec3) -> Self {
        let mut skeleton = Self::new(position);
        skeleton.kind = kind;
        skeleton.health = kind.max_health();
        if kind == SkeletonKind::WitherSkeleton {
            // `EntityTypes.WITHER_SKELETON`: 0.7 by 2.4; a sword, no bow.
            skeleton.body = Body::new(position, 0.7, 2.4);
            skeleton.holds_bow = false;
        }
        skeleton
    }

    pub fn eye_height(&self) -> f32 {
        if self.kind == SkeletonKind::WitherSkeleton { 2.1 } else { 1.74 }
    }
}
