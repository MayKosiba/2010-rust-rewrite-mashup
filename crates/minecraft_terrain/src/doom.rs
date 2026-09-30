//! Doom's monsters' art from Freedoom's IWAD (`freedoom2.wad`, BSD
//! licensed; <https://freedoom.github.io>): the sprites of the bosses and
//! their shots, decoded from Doom's column-post picture format through the
//! WAD's palette, and their sounds, from Doom's 8-bit DMX format to WAV.
//!
//! The sprites join the entity atlas as `doom:entity/doom/<lump>` and are
//! drawn as Doom draws them: flat, turned to the viewer about the vertical,
//! one of eight rotations by the angle the monster is seen from.
//!
//! The WAD is found at `$IW4L_FREEDOOM`, or under `iw4l-artifacts/freedoom`
//! beside the game (the release zip unpacked there).
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use glam::{DVec3, Vec3};

use crate::mesh::{Atlas, ChunkMesh, Vertex};
use crate::pack::ResourceId;

/// The sprite families loaded: the Cyberdemon, the Spider Mastermind, the
/// rocket (and its explosion) and the bullet puff.
const SPRITES: [&str; 4] = ["CYBR", "SPID", "MISL", "PUFF"];
/// Doom's map units in a block: its 64-unit grid is two blocks, and its
/// 56-unit player stands as tall as a Minecraft one.
pub const UNITS_PER_BLOCK: f64 = 32.0;

struct Picture {
    width: u16,
    height: u16,
    left: i16,
    top: i16,
    png: Vec<u8>,
}

/// One drawable view of a sprite frame.
#[derive(Clone, Debug)]
pub struct SpriteFrame {
    pub id: ResourceId,
    pub width: f32,
    pub height: f32,
    /// Pixels from the picture's left edge to the monster's centre.
    pub left: f32,
    /// Pixels from the picture's top edge down to the monster's feet.
    pub top: f32,
    /// Drawn mirrored (the lump serves two rotations).
    pub flip: bool,
}

pub struct DoomAssets {
    pub source: PathBuf,
    pictures: HashMap<String, Picture>,
    /// (sprite, frame letter index, rotation 0-8) to (lump, mirrored).
    frames: HashMap<(String, u8, u8), (String, bool)>,
    sounds: HashMap<String, Arc<[u8]>>,
}

static ASSETS: OnceLock<Option<DoomAssets>> = OnceLock::new();

/// Freedoom's art, loaded on first use; none when the WAD is not there.
pub fn assets() -> Option<&'static DoomAssets> {
    ASSETS
        .get_or_init(|| {
            let path = find_wad()?;
            match DoomAssets::load(&path) {
                Ok(assets) => {
                    eprintln!(
                        "Freedoom: {} sprites, {} sounds from {}",
                        assets.pictures.len(),
                        assets.sounds.len(),
                        path.display()
                    );
                    Some(assets)
                }
                Err(error) => {
                    eprintln!("Freedoom: {} did not load: {error:#}", path.display());
                    None
                }
            }
        })
        .as_ref()
}

fn find_wad() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("IW4L_FREEDOOM").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(path);
    }
    let mut roots = vec![PathBuf::from("iw4l-artifacts/freedoom")];
    if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        roots.push(dir.join("iw4l-artifacts/freedoom"));
    }
    for root in roots {
        let direct = root.join("freedoom2.wad");
        if direct.is_file() {
            return Some(direct);
        }
        // The release zip unpacks into a versioned folder.
        let mut found: Vec<PathBuf> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path().join("freedoom2.wad"))
            .filter(|path| path.is_file())
            .collect();
        found.sort();
        if let Some(path) = found.pop() {
            return Some(path);
        }
    }
    None
}

/// The atlas textures of every loaded sprite lump.
pub fn texture_ids() -> Vec<ResourceId> {
    let Some(assets) = assets() else { return Vec::new() };
    assets.pictures.keys().filter_map(|lump| texture_id(lump)).collect()
}

fn texture_id(lump: &str) -> Option<ResourceId> {
    ResourceId::parse(&format!("doom:entity/doom/{}", lump.to_ascii_lowercase())).ok()
}

/// A sprite lump's PNG, for `PackStack::texture` of the `doom` namespace.
pub fn texture_png(path: &str) -> Option<Vec<u8>> {
    let lump = path.strip_prefix("entity/doom/")?.to_ascii_uppercase();
    assets()?.pictures.get(&lump).map(|p| p.png.clone())
}

fn read_i16(bytes: &[u8], at: usize) -> Option<i16> {
    Some(i16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

impl DoomAssets {
    fn load(path: &Path) -> anyhow::Result<Self> {
        use anyhow::Context;
        let wad = std::fs::read(path)?;
        let magic = wad.get(0..4).context("empty file")?;
        anyhow::ensure!(magic == b"IWAD" || magic == b"PWAD", "not a WAD");
        let count = read_u32(&wad, 4).context("header")? as usize;
        let directory = read_u32(&wad, 8).context("header")? as usize;
        let mut lumps = Vec::with_capacity(count);
        for i in 0..count {
            let at = directory + i * 16;
            let start = read_u32(&wad, at).context("directory")? as usize;
            let size = read_u32(&wad, at + 4).context("directory")? as usize;
            let raw = wad.get(at + 8..at + 16).context("directory")?;
            let name: String = raw.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect();
            lumps.push((name.to_ascii_uppercase(), wad.get(start..start + size).context("lump")?));
        }
        let lump = |name: &str| lumps.iter().rev().find(|(n, _)| n == name).map(|(_, bytes)| *bytes);
        let playpal = lump("PLAYPAL").context("no PLAYPAL")?;
        anyhow::ensure!(playpal.len() >= 768, "short PLAYPAL");

        // Sprites sit between S_START and S_END.
        let start = lumps.iter().position(|(n, _)| n == "S_START" || n == "SS_START").context("no S_START")?;
        let end = lumps.iter().skip(start).position(|(n, _)| n == "S_END" || n == "SS_END").map_or(lumps.len(), |i| start + i);
        let mut pictures = HashMap::new();
        let mut frames = HashMap::new();
        for (name, bytes) in &lumps[start + 1..end] {
            if name.len() < 6 || !SPRITES.contains(&&name[..4]) {
                continue;
            }
            let Some(picture) = decode_picture(bytes, playpal) else {
                eprintln!("Freedoom: sprite {name} did not decode");
                continue;
            };
            let sprite = name[..4].to_owned();
            let raw = name.as_bytes();
            let views = std::iter::once((raw[4], raw[5], false)).chain((raw.len() >= 8).then(|| (raw[6], raw[7], true)));
            for (frame, rotation, flip) in views {
                if frame.is_ascii_uppercase() && (b'0'..=b'8').contains(&rotation) {
                    frames.insert((sprite.clone(), frame - b'A', rotation - b'0'), (name.clone(), flip));
                }
            }
            pictures.insert(name.clone(), picture);
        }
        let mut sounds = HashMap::new();
        for (name, bytes) in &lumps {
            if let (Some(sound), Some(wav)) = (name.strip_prefix("DS"), dmx_to_wav(bytes)) {
                sounds.insert(sound.to_ascii_lowercase(), Arc::from(wav));
            }
        }
        Ok(Self { source: path.to_path_buf(), pictures, frames, sounds })
    }

    /// The view of `sprite`'s frame (0 is `A`) from `rotation` (1 front,
    /// counting round anticlockwise as seen from above; 0 for the frames
    /// that look the same from all round).
    pub fn frame(&self, sprite: &str, frame: u8, rotation: u8) -> Option<SpriteFrame> {
        let (lump, flip) = self
            .frames
            .get(&(sprite.to_owned(), frame, 0))
            .or_else(|| self.frames.get(&(sprite.to_owned(), frame, rotation)))?;
        let picture = self.pictures.get(lump)?;
        Some(SpriteFrame {
            id: texture_id(lump)?,
            width: f32::from(picture.width),
            height: f32::from(picture.height),
            left: f32::from(picture.left),
            top: f32::from(picture.top),
            flip: *flip,
        })
    }

    /// A sound (`cybsit` for `DSCYBSIT`) as a WAV file.
    pub fn sound(&self, name: &str) -> Option<Arc<[u8]>> {
        self.sounds.get(name).cloned()
    }
}

/// Doom's picture format: a header (size, offsets), a table of column
/// starts, and per column a run of posts (`topdelta`, length, a pad byte,
/// the palette indices, a pad byte) ended by 0xFF. A post starting no lower
/// than the last is a tall patch's, counted on from it.
fn decode_picture(bytes: &[u8], playpal: &[u8]) -> Option<Picture> {
    let width = read_u16(bytes, 0)?;
    let height = read_u16(bytes, 2)?;
    let (left, top) = (read_i16(bytes, 4)?, read_i16(bytes, 6)?);
    if width == 0 || height == 0 || width > 1024 || height > 1024 {
        return None;
    }
    let mut image = image::RgbaImage::new(u32::from(width), u32::from(height));
    for x in 0..usize::from(width) {
        let mut at = read_u32(bytes, 8 + x * 4)? as usize;
        let mut last_top: i32 = -1;
        loop {
            let delta = *bytes.get(at)?;
            if delta == 0xFF {
                break;
            }
            let row = if i32::from(delta) <= last_top { last_top + i32::from(delta) } else { i32::from(delta) };
            last_top = row;
            let length = usize::from(*bytes.get(at + 1)?);
            let pixels = bytes.get(at + 3..at + 3 + length)?;
            for (i, &index) in pixels.iter().enumerate() {
                let y = row as u32 + i as u32;
                if y < u32::from(height) {
                    let c = usize::from(index) * 3;
                    image.put_pixel(x as u32, y, image::Rgba([playpal[c], playpal[c + 1], playpal[c + 2], 255]));
                }
            }
            at += length + 4;
        }
    }
    let mut png = Vec::new();
    image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(Picture { width, height, left, top, png })
}

/// A DMX sound (format 3, the rate, the sample count, unsigned 8-bit mono
/// with 16 bytes of padding at each end) as a WAV file.
fn dmx_to_wav(bytes: &[u8]) -> Option<Vec<u8>> {
    if read_u16(bytes, 0)? != 3 {
        return None;
    }
    let rate = u32::from(read_u16(bytes, 2)?);
    let count = read_u32(bytes, 4)? as usize;
    let samples = bytes.get(8..8 + count).or_else(|| bytes.get(8..))?;
    let samples = if samples.len() > 32 { &samples[16..samples.len() - 16] } else { samples };
    let mut wav = Vec::with_capacity(44 + samples.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes()); // one byte a sample
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&8u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    wav.extend_from_slice(samples);
    Some(wav)
}

/// Doom's rotation for a thing facing `angle` (radians, anticlockwise from
/// east as seen from above) seen from `viewer`: 1 when it faces the viewer,
/// then round anticlockwise (`R_ProjectSprite`).
pub fn rotation(at: DVec3, angle: f64, viewer: DVec3) -> u8 {
    // The angle from the viewer to the thing, in Doom's plane (north is -z).
    let to = (at.x - viewer.x, -(at.z - viewer.z));
    let seen = to.1.atan2(to.0);
    let eighth = std::f64::consts::FRAC_PI_4;
    let turn = (seen - angle + eighth * 4.5).rem_euclid(std::f64::consts::TAU);
    (turn / eighth) as u8 % 8 + 1
}

/// A sprite frame stood at `feet`, turned to face `viewer` about the
/// vertical, one Doom unit to a pixel. `light` is (sky, block).
pub fn append_sprite(mesh: &mut ChunkMesh, atlas: &Atlas, frame: &SpriteFrame, feet: DVec3, viewer: DVec3, light: [f32; 2]) {
    if !atlas.contains(&frame.id) {
        return;
    }
    let [mut u0, v0, mut u1, v1] = atlas.entity_region(&frame.id);
    if frame.flip {
        std::mem::swap(&mut u0, &mut u1);
    }
    let toward = DVec3::new(viewer.x - feet.x, 0.0, viewer.z - feet.z).normalize_or(DVec3::Z).as_vec3();
    // The viewer's right, across the sprite.
    let right = Vec3::Y.cross(toward).normalize_or(Vec3::X);
    let scale = (1.0 / UNITS_PER_BLOCK) as f32;
    let base = feet.as_vec3();
    let (x0, x1) = (-frame.left * scale, (frame.width - frame.left) * scale);
    let (y1, y0) = (frame.top * scale, (frame.top - frame.height) * scale);
    let corners = [
        base + right * x0 + Vec3::Y * y1,
        base + right * x1 + Vec3::Y * y1,
        base + right * x1 + Vec3::Y * y0,
        base + right * x0 + Vec3::Y * y0,
    ];
    let uvs = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]];
    let start = mesh.vertices.len() as u32;
    for (corner, uv) in corners.into_iter().zip(uvs) {
        mesh.vertices.push(Vertex { position: corner.to_array(), uv, color: [1.0; 4], sky_light: light[0], block_light: light[1] });
    }
    mesh.indices.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
    mesh.indices.extend_from_slice(&[start, start + 2, start + 1, start, start + 3, start + 2]);
    mesh.faces += 2;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With `IW4L_FREEDOOM` set: the bosses' frames and sounds are there, and
    /// `IW4L_DOOM_DUMP` names a folder to write a few sprites to.
    #[test]
    fn loads_freedoom() {
        let Some(path) = std::env::var_os("IW4L_FREEDOOM").map(PathBuf::from) else { return };
        let assets = DoomAssets::load(&path).unwrap();
        for (sprite, frames) in [("CYBR", 16u8), ("SPID", 19)] {
            for frame in 0..frames {
                assert!(assets.frame(sprite, frame, 1).is_some(), "{sprite} {frame}");
            }
        }
        let side = assets.frame("CYBR", 0, 8).unwrap();
        assert!(side.flip && side.id.path == "entity/doom/cybra2a8");
        for sound in ["cybsit", "cybdth", "spisit", "spidth", "dmpain", "dmact", "hoof", "metal", "rlaunc", "barexp", "shotgn"] {
            let wav = assets.sound(sound).unwrap_or_else(|| panic!("{sound}"));
            assert!(wav.len() > 44 && &wav[..4] == b"RIFF");
        }
        if let Some(dir) = std::env::var_os("IW4L_DOOM_DUMP").map(PathBuf::from) {
            for lump in ["CYBRA1", "CYBRF1", "CYBRP0", "SPIDA1", "SPIDG1", "SPIDA3", "MISLB0"] {
                std::fs::write(dir.join(format!("{lump}.png")), &assets.pictures[lump].png).unwrap();
                let wav = assets.sound("cybsit").unwrap();
                std::fs::write(dir.join("cybsit.wav"), &*wav).unwrap();
            }
        }
    }
}
