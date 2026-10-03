//! The sprite file for the character sheet.
//!
//! [`noxel_asset::format::SpriteFile`] is a flat list of frames, so the sheet is
//! described twice: once as that list (which the engine loads), and once as an
//! `animations` array that groups the frames by name. `SpriteFile::from_json`
//! ignores unknown keys, so the file still round-trips as a `SpriteFile` while
//! staying readable to a human and to the demo's animation player.

use noxel_asset::format::{SpriteFile, SpriteFrame};
use noxel_asset::json::JsonValue;

use crate::characters;

/// Seconds per walk frame: a brisk 8 frames per second.
pub const WALK_DURATION: f32 = 0.125;
/// Seconds the idle pose is held.
pub const IDLE_DURATION: f32 = 0.25;
/// The displayed size of one character in world units (1 unit == 1 tile).
pub const SIZE: [f32; 2] = [1.0, 1.5];

/// The sprite file for `sprites/characters.json`.
///
/// The default `uv` is the down-facing idle cell, which is what a renderer
/// falls back to before an animation is playing.
#[must_use]
pub fn characters_file() -> SpriteFile {
    let mut frames = Vec::with_capacity(characters::DIRECTIONS.len() * characters::COLS as usize);
    for direction in characters::DIRECTIONS {
        for column in 0..characters::COLS as usize {
            let Some(uv) = characters::cell_uv(direction, column) else {
                continue;
            };
            let duration = if column == characters::IDLE_COLUMN {
                IDLE_DURATION
            } else {
                WALK_DURATION
            };
            frames.push(SpriteFrame {
                name: characters::frame_name(direction, column),
                uv,
                duration,
            });
        }
    }
    SpriteFile {
        name: "characters".to_string(),
        texture: "textures/characters.png".to_string(),
        uv: characters::cell_uv("down", characters::IDLE_COLUMN).unwrap_or([0, 0, 16, 24]),
        pivot: [0.5, 1.0],
        size: SIZE,
        frames,
    }
}

/// The names of the frames of one animation, in playing order.
#[must_use]
pub fn animation_frames(direction: &str) -> Vec<String> {
    let mut names = Vec::with_capacity(characters::WALK_FRAMES);
    for column in 0..characters::WALK_FRAMES {
        names.push(characters::frame_name(direction, column));
    }
    names
}

/// The file as JSON: the sprite file plus the `animations` index.
#[must_use]
pub fn characters_json() -> JsonValue {
    let sprite = characters_file();
    let mut animations = Vec::with_capacity(characters::DIRECTIONS.len() * 2);
    for direction in characters::DIRECTIONS {
        animations.push(JsonValue::object([
            ("name", JsonValue::from(format!("walk_{direction}"))),
            (
                "frames",
                JsonValue::Array(
                    animation_frames(direction)
                        .into_iter()
                        .map(JsonValue::from)
                        .collect(),
                ),
            ),
            ("loop", JsonValue::from(true)),
            (
                "duration",
                JsonValue::from(WALK_DURATION * characters::WALK_FRAMES as f32),
            ),
        ]));
        animations.push(JsonValue::object([
            ("name", JsonValue::from(format!("idle_{direction}"))),
            (
                "frames",
                JsonValue::Array(vec![JsonValue::from(characters::frame_name(
                    direction,
                    characters::IDLE_COLUMN,
                ))]),
            ),
            ("loop", JsonValue::from(true)),
            ("duration", JsonValue::from(IDLE_DURATION)),
        ]));
    }

    match sprite.to_json() {
        JsonValue::Object(mut fields) => {
            fields.push(("animations".to_string(), JsonValue::Array(animations)));
            JsonValue::Object(fields)
        }
        other => other,
    }
}
