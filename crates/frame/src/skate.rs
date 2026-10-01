use bevy::prelude::*;

/// Local skating presentation. Physics remains owned by the optional Skate host.
#[derive(Resource, Default)]
pub struct SkateMode {
    pub active: bool,
    pub entering: bool,
    pub preloaded: bool,
    pub preload_pending: bool,
    pub controller: Option<usize>,
    pub toggle_requested: bool,
    pub input_blocked: bool,
    pub client: u32,
    pub root: Mat4,
    pub bones: Vec<Mat4>,
    pub names: Vec<String>,
    pub camera: Option<(Transform, f32)>,
    pub tick: u64,
    pub status: String,
    /// Photo mode (the console's `skatepad`, `skate freeze`, `skatecam`):
    /// pad input held by a script in place of the keyboard and pad.
    pub script_pad: Option<ScriptPad>,
    /// The skater held still: the simulation is not stepped.
    pub frozen: bool,
    /// `skate freezeair <ticks>`: freeze this many ticks into the next air,
    /// and the tick that air began.
    pub freeze_air: Option<u64>,
    pub air_start: Option<u64>,
    /// A camera placed about the skater instead of Skate's own.
    pub photo: Option<PhotoCamera>,
    /// The skater's way ahead (map axes, level) when the photo camera was
    /// placed, which it and a staged dragon are placed along.
    pub photo_forward: Option<Vec3>,
}

/// One XInput pad state: buttons, triggers and sticks (-1 to 1).
#[derive(Clone, Copy, Debug, Default)]
pub struct ScriptPad {
    pub buttons: u16,
    pub triggers: [u8; 2],
    pub left: [f32; 2],
    pub right: [f32; 2],
}

/// A camera about the skater: `angle` degrees round from the board's nose
/// (anticlockwise seen from above), `distance` out and `up` over the board
/// (map units), looking at the point `look_up` over the board.
#[derive(Clone, Copy, Debug)]
pub struct PhotoCamera {
    pub angle: f32,
    pub distance: f32,
    pub up: f32,
    pub fov: f32,
    pub look_up: f32,
}
