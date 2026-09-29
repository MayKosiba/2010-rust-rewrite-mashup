//! Block entities with slots on the Minecraft map: furnaces (and blast
//! furnaces and smokers) and chests (and barrels). MinecraftOSS owns their
//! item rules; this keeps one per placed block, per dimension, ticks the
//! furnaces, and publishes the open one for the HUD.
use std::collections::HashMap;

use frame::{McScreen, McStack, MinecraftUi};
use minecraftoss_player::chest::Chest;
use minecraftoss_player::crafting::{CookingKind, RecipeBook};
use minecraftoss_player::furnace::Furnace;
use minecraftoss_player::inventory::{Inventory, ItemStack};

pub(crate) type BlockPos = (i32, i32, i32);
/// A block in a dimension (0 overworld, 1 nether, 2 end).
pub(crate) type Key = (u8, BlockPos);

/// What kind of container a block is, by its path (`furnace`).
pub(crate) fn kind_of(path: &str) -> Option<McScreen> {
    match path {
        "furnace" | "blast_furnace" | "smoker" => Some(McScreen::Furnace),
        "chest" | "trapped_chest" | "barrel" => Some(McScreen::Chest),
        _ => None,
    }
}

fn cooking_kind(path: &str) -> CookingKind {
    match path {
        "blast_furnace" => CookingKind::BlastFurnace,
        "smoker" => CookingKind::Smoker,
        _ => CookingKind::Furnace,
    }
}

pub(crate) enum Open<'a> {
    Furnace(&'a mut Furnace),
    Chest(&'a mut Chest),
}

#[derive(Default)]
pub(crate) struct Containers {
    furnaces: HashMap<Key, Furnace>,
    chests: HashMap<Key, Chest>,
    open: Option<Key>,
    title: String,
}

impl Containers {
    /// Opens the container at a block, making it on first use; the screen
    /// it shows, or none for a block that holds nothing.
    pub(crate) fn open(&mut self, key: Key, path: &str) -> Option<McScreen> {
        let screen = kind_of(path)?;
        match screen {
            McScreen::Furnace => {
                self.furnaces.entry(key).or_insert_with(|| Furnace::new(cooking_kind(path)));
            }
            _ => {
                self.chests.entry(key).or_default();
            }
        }
        self.open = Some(key);
        self.title = path.replace('_', " ").to_uppercase();
        Some(screen)
    }

    pub(crate) fn close(&mut self) {
        self.open = None;
    }

    pub(crate) fn opened(&mut self) -> Option<Open<'_>> {
        let key = self.open?;
        if let Some(furnace) = self.furnaces.get_mut(&key) {
            return Some(Open::Furnace(furnace));
        }
        self.chests.get_mut(&key).map(Open::Chest)
    }

    /// A broken block's contents, to drop where it stood.
    pub(crate) fn remove(&mut self, key: Key) -> Vec<ItemStack> {
        if self.open == Some(key) {
            self.open = None;
        }
        let mut out = Vec::new();
        if let Some(mut furnace) = self.furnaces.remove(&key) {
            out.extend(furnace.take_contents());
        }
        if let Some(mut chest) = self.chests.remove(&key) {
            out.extend(chest.take_contents());
        }
        out
    }

    /// One game tick of every furnace (`AbstractFurnaceBlockEntity.serverTick`
    /// runs for loaded furnaces; these run everywhere). Returns the furnaces
    /// in `dimension` whose lit state changed, with the new state.
    pub(crate) fn tick(&mut self, recipes: &RecipeBook, dimension: u8) -> Vec<(BlockPos, bool)> {
        let mut changed = Vec::new();
        for (&(dim, pos), furnace) in &mut self.furnaces {
            let was = furnace.is_lit();
            furnace.tick(recipes);
            if dim == dimension && furnace.is_lit() != was {
                changed.push((pos, furnace.is_lit()));
            }
        }
        changed
    }

    /// The open container's slots and progress, for the HUD.
    pub(crate) fn publish(&mut self, ui: &mut MinecraftUi, convert: &mut impl FnMut(&Option<ItemStack>) -> Option<McStack>) {
        ui.container_title = self.title.clone();
        match self.opened() {
            Some(Open::Furnace(furnace)) => {
                ui.furnace = std::array::from_fn(|i| convert(&furnace.slots[i]));
                ui.furnace_burn = if furnace.lit_total > 0 {
                    furnace.lit_remaining as f32 / furnace.lit_total as f32
                } else {
                    0.0
                };
                ui.furnace_cook = if furnace.cook_total > 0 {
                    furnace.cook_progress as f32 / furnace.cook_total as f32
                } else {
                    0.0
                };
            }
            Some(Open::Chest(chest)) => {
                ui.chest = chest.slots.iter().map(&mut *convert).collect();
            }
            None => {}
        }
    }
}

/// A shift click on a player slot while a container is open moves the stack
/// into the container, as its menu's `quickMoveStack` does.
pub(crate) fn quick_move(open: Open<'_>, index: usize, inventory: &mut Inventory) {
    match open {
        Open::Furnace(furnace) => furnace.quick_move_from_inventory(index, inventory),
        Open::Chest(chest) => chest.quick_move_from_inventory(index, inventory),
    }
}
