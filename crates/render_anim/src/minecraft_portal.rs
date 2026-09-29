//! Nether portals on the Minecraft map: lighting an obsidian frame
//! (`PortalShape`), portals that go when their frame breaks, the four
//! seconds a player stands in one before it takes them, and where they come
//! out: the nearest portal around the scaled position, or a new one built
//! there (`PortalForcer`), as vanilla does.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use minecraft_terrain::scene::Block;
use minecraft_terrain::terrain::TerrainStream;
use minecraftoss_core::block::flags;
use minecraftoss_core::{BlockStateId, Chunk, ChunkPos, Registries};

pub(crate) type BlockPos = (i32, i32, i32);

/// `PortalShape`'s limits on the inside of a frame.
const MIN_WIDTH: i32 = 2;
const MAX_WIDTH: i32 = 21;
const MIN_HEIGHT: i32 = 3;
const MAX_HEIGHT: i32 = 21;
/// Ticks a survival player stands in a portal before it takes them
/// (`Player.getPortalWaitTime`).
pub(crate) const WAIT_TICKS: u32 = 80;
/// How far around the scaled position an existing portal is reused, and a
/// place for a new one is looked for. Vanilla reuses Overworld portals
/// within 128 blocks; each chunk searched here is generated on the spot, so
/// the Overworld's reach is less.
const SEARCH_NETHER: i32 = 16;
const SEARCH_OVERWORLD: i32 = 48;
const SEARCH_BUILD: i32 = 16;

pub(crate) fn portal_block(axis: &str) -> Block {
    let mut block = Block::new("minecraft:nether_portal");
    block.properties.insert("axis".into(), axis.into());
    block
}

/// The inside of a lit frame around `fire` (the block the flame goes in):
/// the positions to fill with portal blocks and their axis, or none when
/// `fire` is not inside a whole obsidian frame. `path` names the block at a
/// position (`obsidian`), empty for air.
pub(crate) fn light(path: impl Fn(BlockPos) -> String, fire: BlockPos) -> Option<(Vec<BlockPos>, &'static str)> {
    let empty = |p: BlockPos| {
        let name = path(p);
        name.is_empty() || name == "fire" || name == "soul_fire"
    };
    let obsidian = |p: BlockPos| path(p) == "obsidian";
    if !empty(fire) {
        return None;
    }
    for (axis, (dx, dz)) in [("x", (1, 0)), ("z", (0, 1))] {
        // Down to the frame's bottom.
        let mut bottom = fire;
        let mut steps = 0;
        while empty((bottom.0, bottom.1 - 1, bottom.2)) && steps < MAX_HEIGHT {
            bottom.1 -= 1;
            steps += 1;
        }
        if !obsidian((bottom.0, bottom.1 - 1, bottom.2)) {
            continue;
        }
        // Back to the frame's left side.
        let mut left = bottom;
        let mut steps = 0;
        while empty((left.0 - dx, left.1, left.2 - dz)) && obsidian((left.0 - dx, left.1 - 1, left.2 - dz)) && steps < MAX_WIDTH {
            left = (left.0 - dx, left.1, left.2 - dz);
            steps += 1;
        }
        if !obsidian((left.0 - dx, left.1, left.2 - dz)) {
            continue;
        }
        // Across the bottom row to the right side.
        let mut width = 0;
        while width <= MAX_WIDTH {
            let p = (left.0 + dx * width, left.1, left.2 + dz * width);
            if !empty(p) || !obsidian((p.0, p.1 - 1, p.2)) {
                break;
            }
            width += 1;
        }
        let right = (left.0 + dx * width, left.1, left.2 + dz * width);
        if !(MIN_WIDTH..=MAX_WIDTH).contains(&width) || !obsidian(right) {
            continue;
        }
        // Rows up while both sides are obsidian and the row is empty; the
        // row above the last must be obsidian all across.
        let mut height = 0;
        'rows: while height < MAX_HEIGHT {
            let y = left.1 + height;
            if !obsidian((left.0 - dx, y, left.2 - dz)) || !obsidian((right.0, y, right.2)) {
                break;
            }
            for i in 0..width {
                if !empty((left.0 + dx * i, y, left.2 + dz * i)) {
                    break 'rows;
                }
            }
            height += 1;
        }
        if !(MIN_HEIGHT..=MAX_HEIGHT).contains(&height) {
            continue;
        }
        let top = left.1 + height;
        if !(0..width).all(|i| obsidian((left.0 + dx * i, top, left.2 + dz * i))) {
            continue;
        }
        let inside = (0..height)
            .flat_map(|h| (0..width).map(move |i| (left.0 + dx * i, left.1 + h, left.2 + dz * i)))
            .collect();
        return Some((inside, axis));
    }
    None
}

/// Portal blocks joined to `start`, for a portal whose frame broke.
pub(crate) fn connected(path: impl Fn(BlockPos) -> String, start: BlockPos) -> Vec<BlockPos> {
    let mut seen = HashSet::new();
    let mut stack = vec![start];
    while let Some(p) = stack.pop() {
        if seen.len() > 21 * 21 || seen.contains(&p) || path(p) != "nether_portal" {
            continue;
        }
        seen.insert(p);
        for (dx, dy, dz) in NEIGHBOURS {
            stack.push((p.0 + dx, p.1 + dy, p.2 + dz));
        }
    }
    seen.into_iter().collect()
}

pub(crate) const NEIGHBOURS: [(i32, i32, i32); 6] = [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)];

/// The player's time in a portal.
#[derive(Default)]
pub(crate) struct PortalTimer {
    ticks: u32,
    /// Just came through: the player must step out before a portal takes
    /// them again.
    pub(crate) cooldown: bool,
}

impl PortalTimer {
    /// Advances by `ticks` game ticks; true when the portal takes the player.
    pub(crate) fn step(&mut self, in_portal: bool, ticks: u32) -> bool {
        if !in_portal {
            self.ticks = 0;
            self.cooldown = false;
            return false;
        }
        if self.cooldown {
            return false;
        }
        self.ticks += ticks;
        if self.ticks >= WAIT_TICKS {
            self.ticks = 0;
            self.cooldown = true;
            return true;
        }
        false
    }
}

/// Blocks of freshly loaded chunks, before the scene has them.
struct Area<'a> {
    chunks: HashMap<(i32, i32), Arc<Chunk>>,
    registries: &'a Registries,
}

impl Area<'_> {
    fn state(&self, (x, y, z): BlockPos) -> Option<BlockStateId> {
        let chunk = self.chunks.get(&(x >> 4, z >> 4))?;
        let (min, height) = (chunk.min_y(), chunk.height() as i32);
        if y < min || y >= min + height {
            return None;
        }
        Some(chunk.block((x & 15) as usize, y, (z & 15) as usize))
    }
    fn name(&self, p: BlockPos) -> String {
        self.state(p).map_or_else(String::new, |s| {
            let blocks = &self.registries.blocks;
            let name = blocks.block(blocks.block_of(s)).name.to_string();
            name.strip_prefix("minecraft:").map_or_else(|| name.clone(), str::to_owned)
        })
    }
    fn air(&self, p: BlockPos) -> bool {
        self.state(p).is_some_and(|s| self.registries.blocks.is_air(s))
    }
    fn solid(&self, p: BlockPos) -> bool {
        self.state(p).is_some_and(|s| self.registries.blocks.is(s, flags::SOLID_RENDER))
    }
}

/// Where a player coming through a portal to `target` (x, z in the new
/// dimension, `near_y` the height to prefer) stands, and the blocks to set
/// there first: an existing portal within reach, or a new frame on the
/// nearest spot with room, or a new frame in a cleared pocket.
pub(crate) fn arrival(
    stream: &mut TerrainStream,
    registries: &Registries,
    target: (i32, i32),
    near_y: i32,
    y_range: (i32, i32),
    nether: bool,
) -> ((f64, f64, f64), Vec<(BlockPos, Option<Block>)>) {
    let reach = if nether { SEARCH_NETHER } else { SEARCH_OVERWORLD };
    let mut chunks = HashMap::new();
    for cx in ((target.0 - reach - 2) >> 4)..=((target.0 + reach + 2) >> 4) {
        for cz in ((target.1 - reach - 2) >> 4)..=((target.1 + reach + 2) >> 4) {
            chunks.insert((cx, cz), stream.load_now(ChunkPos::new(cx, cz)));
        }
    }
    let area = Area { chunks, registries };
    let (lo, hi) = y_range;
    let mut heights: Vec<i32> = (lo..=hi).collect();
    heights.sort_by_key(|y| (y - near_y).abs());
    // A new Overworld portal goes on the surface, not in the cave nearest
    // the Nether's height as vanilla's can: highest spot with room first.
    let build_heights: Vec<i32> = if nether { heights.clone() } else { (lo..=hi).rev().collect() };

    // An existing portal: stand in its bottom block nearest the target.
    let mut best: Option<(i64, BlockPos)> = None;
    for x in target.0 - reach..=target.0 + reach {
        for z in target.1 - reach..=target.1 + reach {
            for &y in &heights {
                if area.name((x, y, z)) == "nether_portal" && area.name((x, y - 1, z)) != "nether_portal" {
                    let d = i64::from(x - target.0).pow(2) + i64::from(z - target.1).pow(2) + i64::from(y - near_y).pow(2);
                    if best.is_none_or(|(b, _)| d < b) {
                        best = Some((d, (x, y, z)));
                    }
                }
            }
        }
    }
    if let Some((_, (x, y, z))) = best {
        return ((f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5), Vec::new());
    }

    // A new portal along x: inside x..=x+1, y..=y+2 at z, on solid ground,
    // with room in front and behind.
    let fits = |x: i32, y: i32, z: i32| {
        (0..2).all(|i| area.solid((x + i, y - 1, z)))
            && (-1..3).all(|i| (0..4).all(|h| (-1..=1).all(|dz| area.air((x + i, y + h, z + dz)))))
    };
    let mut spot = None;
    'search: for r in 0..=SEARCH_BUILD {
        for &y in &build_heights {
            for x in target.0 - r..=target.0 + r {
                for z in target.1 - r..=target.1 + r {
                    if (x - target.0).abs().max((z - target.1).abs()) != r {
                        continue;
                    }
                    if fits(x, y, z) {
                        spot = Some((x, y, z));
                        break 'search;
                    }
                }
            }
        }
    }
    let carve = spot.is_none();
    let (x, y, z) = spot.unwrap_or((target.0, near_y.clamp(lo + 1, hi - 4), target.1));
    let mut edits: Vec<(BlockPos, Option<Block>)> = Vec::new();
    let obsidian = || Some(Block::new("minecraft:obsidian"));
    if carve {
        // No room: a pocket the size of the frame and a step either side.
        for i in -1..3 {
            for h in 0..4 {
                for dz in -1..=1 {
                    edits.push(((x + i, y + h, z + dz), None));
                }
            }
        }
    }
    // Floor either side of the portal, so the player lands on something.
    for i in 0..2 {
        for dz in [-1, 1] {
            if carve || !area.solid((x + i, y - 1, z + dz)) {
                edits.push(((x + i, y - 1, z + dz), obsidian()));
            }
        }
    }
    // The frame: sides, bottom and top, then the portal inside.
    for h in -1..4 {
        edits.push(((x - 1, y + h, z), obsidian()));
        edits.push(((x + 2, y + h, z), obsidian()));
    }
    for i in 0..2 {
        edits.push(((x + i, y - 1, z), obsidian()));
        edits.push(((x + i, y + 3, z), obsidian()));
        for h in 0..3 {
            edits.push(((x + i, y + h, z), Some(portal_block("x"))));
        }
    }
    ((f64::from(x) + 1.0, f64::from(y), f64::from(z) + 0.5), edits)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame along x or z: inside `w` by `h` from (0, 1, 0).
    fn frame(w: i32, h: i32, along_z: bool, gap: Option<BlockPos>) -> HashSet<BlockPos> {
        let at = |i: i32, y: i32| if along_z { (0, y, i) } else { (i, y, 0) };
        let mut out = HashSet::new();
        for y in 0..=h + 1 {
            out.insert(at(-1, y));
            out.insert(at(w, y));
        }
        for i in 0..w {
            out.insert(at(i, 0));
            out.insert(at(i, h + 1));
        }
        if let Some(gap) = gap {
            out.remove(&gap);
        }
        out
    }

    fn lookup(frame: HashSet<BlockPos>) -> impl Fn(BlockPos) -> String {
        move |p| if frame.contains(&p) { "obsidian".into() } else { String::new() }
    }

    #[test]
    fn lights_a_minimal_frame_either_way() {
        let (inside, axis) = light(lookup(frame(2, 3, false, None)), (1, 2, 0)).unwrap();
        assert_eq!((inside.len(), axis), (6, "x"));
        let (inside, axis) = light(lookup(frame(2, 3, true, None)), (0, 1, 1)).unwrap();
        assert_eq!((inside.len(), axis), (6, "z"));
        let (inside, _) = light(lookup(frame(5, 7, false, None)), (4, 7, 0)).unwrap();
        assert_eq!(inside.len(), 35);
    }

    #[test]
    fn needs_a_whole_frame_of_size() {
        assert!(light(lookup(frame(2, 3, false, Some((0, 4, 0)))), (0, 1, 0)).is_none());
        assert!(light(lookup(frame(2, 3, false, Some((2, 2, 0)))), (0, 1, 0)).is_none());
        assert!(light(lookup(frame(1, 3, false, None)), (0, 1, 0)).is_none());
        assert!(light(lookup(frame(2, 2, false, None)), (0, 1, 0)).is_none());
        // Outside the frame.
        assert!(light(lookup(frame(2, 3, false, None)), (0, 1, 3)).is_none());
    }

    #[test]
    fn portal_goes_with_its_blocks() {
        let blocks: HashSet<BlockPos> = [(0, 1, 0), (1, 1, 0), (0, 2, 0), (5, 5, 5)].into();
        let found = connected(move |p| if blocks.contains(&p) { "nether_portal".into() } else { String::new() }, (0, 1, 0));
        assert_eq!(found.len(), 3);
    }

    #[test]
    fn waits_four_seconds_then_needs_stepping_out() {
        let mut timer = PortalTimer::default();
        assert!(!timer.step(true, WAIT_TICKS - 1));
        assert!(timer.step(true, 1));
        assert!(!timer.step(true, WAIT_TICKS * 2));
        assert!(!timer.step(false, 1));
        assert!(timer.step(true, WAIT_TICKS));
    }
}
