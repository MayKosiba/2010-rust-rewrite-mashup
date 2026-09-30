//! The Minecraft map's inventory and hotbar, between the world that owns the
//! vanilla inventory (`render_anim`) and the HUD that draws it and takes the
//! player's clicks (`hud`).
use std::collections::HashMap;

use bevy::prelude::*;

/// Slots of the vanilla player inventory: 0..9 hotbar, 9..36 main, 36..40
/// armor (feet to head), 40 offhand.
pub const MC_INVENTORY_SLOTS: usize = 41;
pub const MC_HOTBAR: usize = 9;

/// One stack as the HUD shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct McStack {
    /// The item's id (`minecraft:dirt`), or `iw4:weapon/<index>` for a gun.
    pub id: String,
    pub count: u8,
    /// The MW2 weapon this item is, for a gun.
    pub weapon: Option<u32>,
    /// Durability left of the item's maximum, for a damaged tool.
    pub durability: Option<f32>,
}

/// A slot of the inventory screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McSlot {
    Inventory(usize),
    Crafting(usize),
    Result,
    /// The crafting table's 3x3 grid and its result.
    Workbench(usize),
    WorkbenchResult,
    /// A furnace's input, fuel and output.
    Furnace(usize),
    /// A chest's or barrel's 27 slots.
    Chest(usize),
}

/// Which screen the inventory shows beside the player's slots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum McScreen {
    #[default]
    Inventory,
    Workbench,
    Furnace,
    Chest,
}

/// What the player did on the inventory screen, applied with vanilla's
/// container click rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McClick {
    /// A click on a slot; shift moves the stack across.
    Slot { slot: McSlot, right: bool, shift: bool },
    /// A click outside the window: throws the carried stack (or one of it).
    Outside { right: bool },
    /// A double click: gathers matching stacks onto the carried one.
    Gather { right: bool },
    /// A number key over a slot: swaps it with that hotbar slot.
    Swap { slot: McSlot, hotbar: usize },
    /// A drag across slots with a carried stack: shares it out.
    Spread { slots: Vec<usize>, right: bool },
    /// The screen closed: the carried stack and the crafting grid go back.
    Close,
}

#[derive(Resource, Default)]
pub struct MinecraftUi {
    /// A Minecraft world is in play and the player is alive.
    pub active: bool,
    /// The inventory screen is open (the HUD's to change).
    pub inventory_open: bool,
    /// The selected hotbar slot.
    pub selected: usize,
    pub slots: Vec<Option<McStack>>,
    pub crafting: [Option<McStack>; 4],
    pub result: Option<McStack>,
    /// A block's screen (crafting table, furnace, chest) the world opened;
    /// it goes back to the inventory when the screen closes.
    pub screen: McScreen,
    pub workbench: [Option<McStack>; 9],
    pub workbench_result: Option<McStack>,
    /// The open furnace: input, fuel, output; fuel burning and cooking
    /// done, 0 to 1; and its name (`FURNACE`, `SMOKER`...).
    pub furnace: [Option<McStack>; 3],
    pub furnace_burn: f32,
    pub furnace_cook: f32,
    pub container_title: String,
    /// The open chest's slots.
    pub chest: Vec<Option<McStack>>,
    /// A dimension the console asked to go to (`overworld`, `nether`,
    /// `end`), for the world to take.
    pub travel_request: Option<String>,
    /// An item the console asked to give (`minecraft:ender_eye`, count).
    pub give_request: Option<(String, u8)>,
    /// The console's `mcuse`: a right click (use) this frame.
    pub use_request: bool,
    /// A mob the console asked to summon in front of the player
    /// (`minecraft:blaze`).
    pub summon_request: Option<String>,
    /// A console request for the End's fight (`kill`, `reset`).
    pub dragon_request: Option<String>,
    /// The CS2 karambit's stand-in gun is in hand: its attacks are melee.
    pub knife_held: bool,
    /// A console request for Doom's bosses (`cyberdemon`, `mastermind`,
    /// `clear`).
    pub doom_request: Option<String>,
    /// A boss's name and health left, 0 to 1, for the bar at the top.
    pub boss: Option<(String, f32)>,
    /// The stack on the cursor.
    pub cursor: Option<McStack>,
    /// Item icons: one atlas, and each item's rectangle in it (s0 t0 s1 t1).
    pub icons: Option<Handle<Image>>,
    pub icon_rects: HashMap<String, [f32; 4]>,
    /// Display names by item id.
    pub names: HashMap<String, String>,
    /// Clicks the HUD took this frame, for the inventory's owner.
    pub clicks: Vec<McClick>,
    /// A hotbar slot the player picked (scroll or number key).
    pub select: Option<usize>,
    /// A drop of the selected stack (`Q`; with the whole stack on control).
    pub drop_selected: Option<bool>,
    /// The mouse in the inventory's character box, -1..1 across and down,
    /// which the character's gaze follows.
    pub gaze: [f32; 2],
    /// The character box in window pixels (centre x and y, width, height),
    /// for placing the character.
    pub character_box: Option<[f32; 4]>,
    /// The name of the item just selected and how long ago, for the hotbar.
    pub selected_name: Option<(String, f32)>,
    /// The MW2 gun the hotbar selection asks the player to raise.
    pub weapon_request: Option<u32>,
    /// The selection is not a gun: the hand or a held item shows, and the
    /// gun neither fires nor aims.
    pub holding_item: bool,
    /// The selected slot is empty: MW2's bare hands show, without the gun.
    pub empty_hand: bool,
    /// How far through its swing the hand is, 0 to 1.
    pub hand_swing: f32,
    /// The minimap's picture of the world and the map points of its
    /// north-west and south-east corners.
    pub minimap: Option<(Handle<Image>, [f32; 2], [f32; 2])>,
}

/// Whether a CS2 first-person weapon stands in the MW2 view model's place
/// (set by `render_anim`'s CS2 view model).
#[derive(Resource, Default)]
pub struct Cs2Viewmodel {
    pub covering: bool,
}

/// The player's own MW2 body, drawn standing in the inventory's character
/// box and looking towards the mouse.
#[derive(Resource, Default, Clone)]
pub struct InventoryPuppet {
    pub active: bool,
    pub client: u32,
    /// Where the body stands, in world space (in front of the camera).
    pub root: Mat4,
    /// The aim pitch its upper body and head take, in degrees.
    pub pitch: f32,
}
