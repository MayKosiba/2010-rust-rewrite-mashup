//! Source-informed pinned 26.3 Zombie identity and initial lifecycle state.
//! Sources: Zombie constructor, synced baby/door/conversion state,
//! EntityTypes.ZOMBIE dimensions, LivingEntity and Mob base ticks.
use crate::{health::DamageState, movement::Body};
use glam::DVec3;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ZombieKind {
    #[default]
    Zombie,
    Drowned,
    /// `Husk`: never burns, and its bite starves.
    Husk,
    /// `ZombieVillager`: never drowns.
    ZombieVillager,
    /// `ZombifiedPiglin`: the Nether's; fire immune, never burns or drowns,
    /// neutral until hurt, when its kind nearby joins in.
    ZombifiedPiglin,
    /// `Piglin`: the Nether's gold-loving natives; hostile to players
    /// wearing no golden armour. Not undead, not fire immune. Simplified:
    /// no Overworld zombification (vanilla converts after 15 s), no
    /// bartering, admiring or crossbows.
    Piglin,
    /// `PiglinBrute`: a bastion's guard, hostile to every player. Never
    /// spawns naturally. Simplified: no Overworld zombification.
    PiglinBrute,
}

impl ZombieKind {
    /// The entity type's ID.
    pub fn type_id(self) -> &'static str {
        match self {
            Self::Zombie => "minecraft:zombie",
            Self::Drowned => "minecraft:drowned",
            Self::Husk => "minecraft:husk",
            Self::ZombieVillager => "minecraft:zombie_villager",
            Self::ZombifiedPiglin => "minecraft:zombified_piglin",
            Self::Piglin => "minecraft:piglin",
            Self::PiglinBrute => "minecraft:piglin_brute",
        }
    }

    /// Its sounds' family (`entity.<family>.ambient`).
    pub fn sound_family(self) -> &'static str {
        match self {
            Self::Zombie => "zombie",
            Self::Drowned => "drowned",
            Self::Husk => "husk",
            Self::ZombieVillager => "zombie_villager",
            Self::ZombifiedPiglin => "zombified_piglin",
            Self::Piglin => "piglin",
            Self::PiglinBrute => "piglin_brute",
        }
    }

    /// `Zombie.isSunSensitive`: all but husks burn in daylight (and the
    /// piglins, which are no zombies at all).
    pub fn burns_in_daylight(self) -> bool {
        !matches!(self, Self::Husk | Self::ZombifiedPiglin | Self::Piglin | Self::PiglinBrute)
    }

    /// Whether this is a living piglin (`AbstractPiglin`), not a zombie:
    /// not undead.
    pub fn is_piglin(self) -> bool {
        matches!(self, Self::Piglin | Self::PiglinBrute)
    }

    /// `createAttributes`' max health: `Piglin`'s 16, `PiglinBrute`'s 50,
    /// the zombies' 20.
    pub fn max_health(self) -> f32 {
        match self {
            Self::Piglin => 16.0,
            Self::PiglinBrute => 50.0,
            _ => 20.0,
        }
    }

    /// `createAttributes`' movement speed: the piglins' 0.35, a zombie's
    /// 0.23.
    pub fn movement_speed(self) -> f32 {
        if self.is_piglin() { 0.35 } else { 0.23 }
    }

    /// A baby's speed factor: `Zombie`'s +50% and `Piglin`'s +20%
    /// (`SPEED_MODIFIER_BABY`, `ADD_MULTIPLIED_BASE`).
    pub fn baby_speed_factor(self) -> f64 {
        if self.is_piglin() { 1.2 } else { 1.5 }
    }

    /// `EntityType.fireImmune`.
    pub fn fire_immune(self) -> bool {
        self == Self::ZombifiedPiglin
    }

    /// `createAttributes`' attack damage: a zombified piglin's and a
    /// piglin's is 5, a brute's 7, the others' 3.
    pub fn attack_damage(self) -> f32 {
        match self {
            Self::ZombifiedPiglin | Self::Piglin => 5.0,
            Self::PiglinBrute => 7.0,
            _ => 3.0,
        }
    }

    /// `convertsInWater` and `convertsToWhenDrowning`: a zombie becomes a
    /// drowned and a husk a zombie; the others never convert.
    pub fn drowns_into(self) -> Option<Self> {
        match self {
            Self::Zombie => Some(Self::Drowned),
            Self::Husk => Some(Self::Zombie),
            Self::Drowned | Self::ZombieVillager | Self::ZombifiedPiglin | Self::Piglin | Self::PiglinBrute => None,
        }
    }

    /// `getVoicePitch`'s centre for a baby: a zombie villager's is higher.
    pub fn baby_voice(self) -> f32 {
        if self == Self::ZombieVillager { 2.0 } else { 1.5 }
    }
}

#[derive(Clone, Debug)]
pub struct Zombie {
    pub kind: ZombieKind,
    pub body: Body,
    pub health: f32,
    pub damage: DamageState,
    pub baby: bool,
    pub can_break_doors: bool,
    pub underwater_converting: bool,
    pub in_water_time: i32,
    pub conversion_time: i32,
    pub persistence_required: bool,
    /// `Entity.remainingFireTicks`.
    /// An item in the head slot (`sunProtectionSlot`), and whether it can
    /// be damaged.
    pub head_item: Option<bool>,
    /// A zombie villager's `VillagerData`: its type and profession IDs.
    pub villager: Option<(String, String)>,
    /// The main hand's item ID, when it holds one.
    pub main_hand: Option<String>,
    /// The head, chest, legs and feet slots' item IDs.
    pub armor: [Option<String>; 4],
}

impl Zombie {
    pub fn new(position: DVec3) -> Self {
        Self {
            kind: ZombieKind::Zombie,
            body: Body::new(position, 0.6, 1.95),
            health: 20.0,
            damage: DamageState::default(),
            baby: false,
            can_break_doors: false,
            underwater_converting: false,
            in_water_time: 0,
            conversion_time: 0,
            persistence_required: false,
            head_item: None,
            villager: None,
            main_hand: None,
            armor: Default::default(),
        }
    }

    /// `Mob.doHurtTarget`'s damage: the type's attack damage with its main
    /// hand weapon's (`ATTACK_DAMAGE` modifiers: a sword's or shovel's
    /// damage less the base 1; a golden axe's 7 less 1).
    pub fn melee_damage(&self) -> f32 {
        let weapon = match self.main_hand.as_deref() {
            Some("minecraft:wooden_sword" | "minecraft:golden_sword") => 3.0,
            Some("minecraft:stone_sword") => 4.0,
            Some("minecraft:iron_sword") => 5.0,
            Some("minecraft:diamond_sword") => 6.0,
            Some("minecraft:netherite_sword") => 7.0,
            Some("minecraft:iron_shovel") => 3.5,
            Some("minecraft:golden_axe") => 6.0,
            _ => 0.0,
        };
        self.kind.attack_damage() + weapon
    }

    pub fn set_baby(&mut self, baby: bool) {
        self.baby = baby;
        self.body.width = if baby { 0.49 } else { 0.6 };
        self.body.height = if baby { 0.98 } else { 1.95 };
    }

    pub fn eye_height(&self) -> f32 {
        if self.baby {
            // Each type's `BABY_DIMENSIONS`.
            match self.kind {
                ZombieKind::Husk => 0.825,
                ZombieKind::ZombieVillager => 0.67,
                ZombieKind::Zombie | ZombieKind::Drowned => 0.775,
                ZombieKind::ZombifiedPiglin | ZombieKind::Piglin | ZombieKind::PiglinBrute => 0.97,
            }
        } else {
            1.74
        }
    }

    /// Pinned `ConversionTracker.tick` for the zombie's water affliction.
    /// Returns true when the countdown has expired and the caller must replace
    /// this entity with a drowned. Conversion itself belongs to world lifecycle.
    pub fn tick_drowning(&mut self, eye_in_water: bool, no_ai: bool) -> bool {
        if self.kind.drowns_into().is_none() || self.health <= 0.0 || no_ai {
            return false;
        }
        if !eye_in_water {
            self.in_water_time = -1;
            self.underwater_converting = false;
            return false;
        }
        if self.underwater_converting {
            self.conversion_time -= 1;
            return self.conversion_time < 0;
        }
        self.in_water_time += 1;
        if self.in_water_time >= 600 {
            self.conversion_time = 300;
            self.underwater_converting = true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drowning_requires_six_hundred_submerged_active_ticks_and_cancels_on_exit() {
        let mut zombie = Zombie::new(DVec3::ZERO);
        zombie.in_water_time = 598;
        assert!(!zombie.tick_drowning(true, true));
        assert_eq!(zombie.in_water_time, 598);
        assert!(!zombie.tick_drowning(true, false));
        assert!(!zombie.underwater_converting);
        assert!(!zombie.tick_drowning(true, false));
        assert!(zombie.underwater_converting);
        assert_eq!(zombie.conversion_time, 300);
        assert!(!zombie.tick_drowning(false, false));
        assert!(!zombie.underwater_converting);
        assert_eq!(zombie.in_water_time, -1);
        assert!(!zombie.tick_drowning(true, false));
        assert_eq!(zombie.in_water_time, 0);
    }

    #[test]
    fn drowning_conversion_expires_after_three_hundred_and_one_countdown_ticks() {
        let mut zombie = Zombie::new(DVec3::ZERO);
        zombie.underwater_converting = true;
        zombie.conversion_time = 300;
        for _ in 0..300 {
            assert!(!zombie.tick_drowning(true, false));
        }
        assert_eq!(zombie.conversion_time, 0);
        assert!(zombie.tick_drowning(true, false));
    }
}
