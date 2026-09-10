//! Deterministic, editable material recipes.
//!
//! The crate owns pixels and vector hatch geometry only. A host such as OPAL
//! owns identity, permissions, provenance, review, and publication.

use std::fmt;
use std::io::Cursor;

use image::{DynamicImage, ImageBuffer, ImageFormat, Luma, RgbImage};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const GENERATOR_VERSION: &str = env!("CARGO_PKG_VERSION");

fn default_schema() -> u32 {
    1
}

fn default_pixels() -> u32 {
    512
}

fn default_width_mm() -> f32 {
    1000.0
}

fn default_height_mm() -> f32 {
    1000.0
}

/// One versioned recipe. The physical extent is the repeat represented by the
/// output image; every generator evaluates in millimetres.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProceduralDefinition {
    #[serde(default = "default_schema")]
    pub schema: u32,
    #[serde(default = "default_pixels")]
    pub width_px: u32,
    #[serde(default = "default_pixels")]
    pub height_px: u32,
    #[serde(default = "default_width_mm")]
    pub width_mm: f32,
    #[serde(default = "default_height_mm")]
    pub height_mm: f32,
    #[serde(default)]
    pub seed: u64,
    #[serde(flatten)]
    pub recipe: Recipe,
}

/// Typed generator parameters. Unknown keys are refused rather than ignored,
/// because an ignored design control makes a recipe appear reproducible when
/// it is not.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "generator", content = "parameters", rename_all = "snake_case")]
pub enum Recipe {
    Paint(Paint),
    Masonry(Masonry),
    Timber(Timber),
    Terrazzo(Terrazzo),
    Textile(Textile),
}

impl Recipe {
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Paint(_) => "paint",
            Self::Masonry(_) => "masonry",
            Self::Timber(_) => "timber",
            Self::Terrazzo(_) => "terrazzo",
            Self::Textile(_) => "textile",
        }
    }
}

fn default_roughness() -> f32 {
    0.55
}

fn default_joint() -> f32 {
    10.0
}

fn default_edge_depth() -> f32 {
    3.0
}

fn default_surface_detail() -> f32 {
    0.22
}

fn default_unit_width() -> f32 {
    230.0
}

fn default_unit_height() -> f32 {
    76.0
}

fn default_board_width() -> f32 {
    140.0
}

fn default_board_length() -> f32 {
    1200.0
}

fn default_thread() -> f32 {
    4.0
}

fn default_chip_size() -> f32 {
    18.0
}

fn default_density() -> f32 {
    0.45
}

fn default_unit_colours() -> Vec<Colour> {
    vec![
        Colour::new(178, 89, 62),
        Colour::new(151, 69, 48),
        Colour::new(194, 112, 79),
    ]
}

fn default_timber_colours() -> Vec<Colour> {
    vec![
        Colour::new(157, 109, 64),
        Colour::new(181, 132, 79),
        Colour::new(132, 88, 53),
    ]
}

fn default_chip_colours() -> Vec<Colour> {
    vec![
        Colour::new(228, 220, 207),
        Colour::new(104, 104, 101),
        Colour::new(188, 151, 119),
    ]
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Paint {
    pub colour: Colour,
    #[serde(default = "default_roughness")]
    pub roughness: f32,
    #[serde(default)]
    pub variation: f32,
    #[serde(default)]
    pub texture_depth: f32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Bond {
    Stack,
    #[default]
    Running,
    Quarter,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Masonry {
    #[serde(default = "default_unit_width")]
    pub unit_width_mm: f32,
    #[serde(default = "default_unit_height")]
    pub unit_height_mm: f32,
    #[serde(default = "default_joint")]
    pub joint_mm: f32,
    #[serde(default)]
    pub bond: Bond,
    #[serde(default = "default_unit_colours")]
    pub unit_colours: Vec<Colour>,
    #[serde(default)]
    pub joint_colour: Colour,
    #[serde(default = "default_roughness")]
    pub roughness: f32,
    #[serde(default = "default_edge_depth")]
    pub edge_depth_mm: f32,
    #[serde(default = "default_surface_detail")]
    pub surface_detail: f32,
    #[serde(default)]
    pub tone_variation: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Timber {
    #[serde(default = "default_board_width")]
    pub board_width_mm: f32,
    #[serde(default = "default_board_length")]
    pub board_length_mm: f32,
    #[serde(default = "default_joint")]
    pub joint_mm: f32,
    #[serde(default = "default_timber_colours")]
    pub colours: Vec<Colour>,
    #[serde(default = "default_roughness")]
    pub roughness: f32,
    #[serde(default)]
    pub grain_strength: f32,
    #[serde(default)]
    pub stagger: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Terrazzo {
    pub matrix_colour: Colour,
    #[serde(default = "default_chip_colours")]
    pub chip_colours: Vec<Colour>,
    #[serde(default = "default_chip_size")]
    pub chip_size_mm: f32,
    #[serde(default = "default_density")]
    pub density: f32,
    #[serde(default = "default_roughness")]
    pub roughness: f32,
    #[serde(default)]
    pub chip_depth_mm: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Textile {
    pub warp_colour: Colour,
    pub weft_colour: Colour,
    #[serde(default = "default_thread")]
    pub thread_mm: f32,
    #[serde(default = "default_roughness")]
    pub roughness: f32,
    #[serde(default)]
    pub depth_mm: f32,
    #[serde(default)]
    pub basket: bool,
}

/// An sRGB colour serialized as a CSS-style six-digit hex value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Colour(pub [u8; 3]);

impl Colour {
    #[must_use]
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self([red, green, blue])
    }
}

impl Default for Colour {
    fn default() -> Self {
        Self::new(196, 193, 184)
    }
}

impl Serialize for Colour {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("#{:02X}{:02X}{:02X}", self.0[0], self.0[1], self.0[2]))
    }
}

impl<'de> Deserialize<'de> for Colour {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;

        impl de::Visitor<'_> for Visitor {
            type Value = Colour;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a colour such as #C4C1B8")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let hex = value.strip_prefix('#').unwrap_or(value);
                if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(E::custom("colour must contain exactly six hexadecimal digits"));
                }
                let channel = |offset| u8::from_str_radix(&hex[offset..offset + 2], 16).map_err(E::custom);
                Ok(Colour::new(channel(0)?, channel(2)?, channel(4)?))
            }
        }

        deserializer.deserialize_str(Visitor)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BakedAsset {
    pub role: String,
    pub extension: String,
    pub media_type: String,
    pub sha256: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProceduralBake {
    pub schema: String,
    pub generator: String,
    pub generator_version: String,
    pub definition_digest: String,
    pub width_px: u32,
    pub height_px: u32,
    pub width_mm: f32,
    pub height_mm: f32,
    pub assets: Vec<BakedAsset>,
}

#[derive(Debug, Error)]
pub enum ProceduralError {
    #[error("unsupported procedural schema {0}; expected 1")]
    Schema(u32),
    #[error("{0}")]
    Invalid(String),
    #[error("could not encode {role}: {source}")]
    Encode {
        role: &'static str,
        #[source]
        source: image::ImageError,
    },
    #[error("could not serialize the procedural definition: {0}")]
    Serialize(#[from] serde_json::Error),
}

#[derive(Clone, Copy)]
struct Surface {
    colour: [f32; 3],
    height: f32,
    roughness: f32,
    metalness: f32,
}

/// Bake a complete, tileable PBR set and any generator-specific hatch files.
pub fn bake(definition: &ProceduralDefinition) -> Result<ProceduralBake, ProceduralError> {
    validate(definition)?;
    let pixel_count = usize::try_from(definition.width_px)
        .ok()
        .and_then(|width| {
            usize::try_from(definition.height_px)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or_else(|| ProceduralError::Invalid("output dimensions are too large".into()))?;
    let mut colours = vec![[0.0; 3]; pixel_count];
    let mut heights = vec![0.0; pixel_count];
    let mut roughness = vec![0.0; pixel_count];
    let mut metalness = vec![0.0; pixel_count];

    for y in 0..definition.height_px {
        for x in 0..definition.width_px {
            let mm_x = (x as f32 + 0.5) * definition.width_mm / definition.width_px as f32;
            let mm_y = (y as f32 + 0.5) * definition.height_mm / definition.height_px as f32;
            let surface = sample(
                &definition.recipe,
                definition.seed,
                mm_x,
                mm_y,
                definition.width_mm,
                definition.height_mm,
            );
            let index = usize::try_from(y * definition.width_px + x).unwrap_or(0);
            colours[index] = surface.colour;
            heights[index] = surface.height;
            roughness[index] = surface.roughness;
            metalness[index] = surface.metalness;
        }
    }

    let mut assets = vec![
        rgb_asset("base_color", definition.width_px, definition.height_px, &colours)?,
        normal_asset(
            definition.width_px,
            definition.height_px,
            definition.width_mm,
            definition.height_mm,
            normal_height_mm(&definition.recipe),
            &heights,
        )?,
        scalar_asset("roughness", definition.width_px, definition.height_px, &roughness)?,
        scalar_asset("height", definition.width_px, definition.height_px, &heights)?,
        scalar_asset("metallic", definition.width_px, definition.height_px, &metalness)?,
    ];
    if let Some((svg, pat)) = hatch(definition) {
        assets.push(asset("hatch_svg", "svg", "image/svg+xml", svg.into_bytes()));
        assets.push(asset("hatch_pat", "pat", "text/plain", pat.into_bytes()));
    }

    let definition_bytes = serde_json::to_vec(definition)?;
    Ok(ProceduralBake {
        schema: "usd-toolbox.procedural-bake.v1".into(),
        generator: definition.recipe.name().into(),
        generator_version: GENERATOR_VERSION.into(),
        definition_digest: hex(Sha256::digest(definition_bytes)),
        width_px: definition.width_px,
        height_px: definition.height_px,
        width_mm: definition.width_mm,
        height_mm: definition.height_mm,
        assets,
    })
}

fn validate(definition: &ProceduralDefinition) -> Result<(), ProceduralError> {
    if definition.schema != 1 {
        return Err(ProceduralError::Schema(definition.schema));
    }
    if definition.width_px == 0
        || definition.height_px == 0
        || definition.width_px > 8192
        || definition.height_px > 8192
    {
        return Err(ProceduralError::Invalid(
            "pixel dimensions must be between 1 and 8192".into(),
        ));
    }
    positive("width_mm", definition.width_mm)?;
    positive("height_mm", definition.height_mm)?;
    match &definition.recipe {
        Recipe::Paint(value) => {
            unit("roughness", value.roughness)?;
            unit("variation", value.variation)?;
            non_negative("texture_depth", value.texture_depth)?;
        }
        Recipe::Masonry(value) => {
            positive("unit_width_mm", value.unit_width_mm)?;
            positive("unit_height_mm", value.unit_height_mm)?;
            non_negative("joint_mm", value.joint_mm)?;
            non_negative("edge_depth_mm", value.edge_depth_mm)?;
            unit("surface_detail", value.surface_detail)?;
            unit("roughness", value.roughness)?;
            unit("tone_variation", value.tone_variation)?;
            colours("unit_colours", &value.unit_colours)?;
            if value.joint_mm >= value.unit_width_mm || value.joint_mm >= value.unit_height_mm {
                return Err(ProceduralError::Invalid(
                    "joint_mm must be smaller than each masonry unit".into(),
                ));
            }
            whole_repeats("width_mm", definition.width_mm, value.unit_width_mm + value.joint_mm, 1)?;
            let row_multiple = match value.bond {
                Bond::Stack => 1,
                Bond::Running => 2,
                Bond::Quarter => 4,
            };
            whole_repeats(
                "height_mm",
                definition.height_mm,
                value.unit_height_mm + value.joint_mm,
                row_multiple,
            )?;
        }
        Recipe::Timber(value) => {
            positive("board_width_mm", value.board_width_mm)?;
            positive("board_length_mm", value.board_length_mm)?;
            non_negative("joint_mm", value.joint_mm)?;
            unit("roughness", value.roughness)?;
            unit("grain_strength", value.grain_strength)?;
            colours("colours", &value.colours)?;
            whole_repeats(
                "width_mm",
                definition.width_mm,
                value.board_length_mm + value.joint_mm,
                1,
            )?;
            whole_repeats(
                "height_mm",
                definition.height_mm,
                value.board_width_mm + value.joint_mm,
                if value.stagger { 2 } else { 1 },
            )?;
        }
        Recipe::Terrazzo(value) => {
            positive("chip_size_mm", value.chip_size_mm)?;
            unit("density", value.density)?;
            unit("roughness", value.roughness)?;
            non_negative("chip_depth_mm", value.chip_depth_mm)?;
            colours("chip_colours", &value.chip_colours)?;
        }
        Recipe::Textile(value) => {
            positive("thread_mm", value.thread_mm)?;
            unit("roughness", value.roughness)?;
            non_negative("depth_mm", value.depth_mm)?;
            let repeat = value.thread_mm * 2.0;
            whole_repeats("width_mm", definition.width_mm, repeat, 1)?;
            whole_repeats("height_mm", definition.height_mm, repeat, 1)?;
        }
    }
    Ok(())
}

fn whole_repeats(name: &str, dimension: f32, pitch: f32, multiple: u32) -> Result<(), ProceduralError> {
    let repeats = dimension / pitch;
    let rounded = repeats.round();
    let aligned = (repeats - rounded).abs() <= 0.0001;
    let count = rounded.max(0.0) as u32;

    if aligned && count >= multiple && count.is_multiple_of(multiple) {
        return Ok(());
    }

    Err(ProceduralError::Invalid(format!(
        "{name} must contain a whole number of {pitch:.3} mm repeats, in multiples of {multiple}"
    )))
}

fn colours(name: &str, values: &[Colour]) -> Result<(), ProceduralError> {
    if values.is_empty() {
        Err(ProceduralError::Invalid(format!("{name} must not be empty")))
    } else if values.len() > 32 {
        Err(ProceduralError::Invalid(format!(
            "{name} may contain at most 32 colours"
        )))
    } else {
        Ok(())
    }
}

fn positive(name: &str, value: f32) -> Result<(), ProceduralError> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(ProceduralError::Invalid(format!(
            "{name} must be a positive finite number"
        )))
    }
}

fn non_negative(name: &str, value: f32) -> Result<(), ProceduralError> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(ProceduralError::Invalid(format!(
            "{name} must be a non-negative finite number"
        )))
    }
}

fn unit(name: &str, value: f32) -> Result<(), ProceduralError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(ProceduralError::Invalid(format!("{name} must be between 0 and 1")))
    }
}

fn sample(recipe: &Recipe, seed: u64, x: f32, y: f32, width: f32, height: f32) -> Surface {
    match recipe {
        Recipe::Paint(value) => paint(value, seed, x / width, y / height),
        Recipe::Masonry(value) => masonry(value, seed, x, y),
        Recipe::Timber(value) => timber(value, seed, x, y),
        Recipe::Terrazzo(value) => terrazzo(value, seed, x, y, width, height),
        Recipe::Textile(value) => textile(value, x, y),
    }
}

fn paint(value: &Paint, seed: u64, x: f32, y: f32) -> Surface {
    let noise = periodic_noise(seed, x, y);
    Surface {
        colour: vary(value.colour, noise * value.variation),
        height: 0.5 + noise * value.texture_depth.min(1.0) * 0.35,
        roughness: clamp(value.roughness + noise * 0.08),
        metalness: 0.0,
    }
}

fn masonry(value: &Masonry, seed: u64, x: f32, y: f32) -> Surface {
    let pitch_x = value.unit_width_mm + value.joint_mm;
    let pitch_y = value.unit_height_mm + value.joint_mm;
    let row = (y / pitch_y).floor() as i64;
    let fraction = match value.bond {
        Bond::Stack => 0.0,
        Bond::Running => {
            if row.rem_euclid(2) == 0 {
                0.0
            } else {
                0.5
            }
        }
        Bond::Quarter => row.rem_euclid(4) as f32 * 0.25,
    };
    let shifted = x + pitch_x * fraction;
    let column = (shifted / pitch_x).floor() as i64;
    let local_x = shifted.rem_euclid(pitch_x);
    let local_y = y.rem_euclid(pitch_y);
    let in_joint = local_x >= value.unit_width_mm || local_y >= value.unit_height_mm;
    if in_joint {
        return Surface {
            colour: linear(value.joint_colour),
            height: 0.08,
            roughness: clamp(value.roughness + 0.2),
            metalness: 0.0,
        };
    }
    let distance = local_x
        .min(value.unit_width_mm - local_x)
        .min(local_y.min(value.unit_height_mm - local_y));
    let bevel = if value.edge_depth_mm == 0.0 {
        1.0
    } else {
        clamp(distance / value.edge_depth_mm)
    };
    let random = random(seed, column, row, 1);
    let detail_seed = seed ^ (column as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (row as u64).rotate_left(31);
    let face_detail = masonry_noise(detail_seed, local_x, local_y) * value.surface_detail;
    let colour =
        value.unit_colours[(random * value.unit_colours.len() as f32).floor() as usize % value.unit_colours.len()];
    Surface {
        colour: vary(colour, (random - 0.5) * value.tone_variation + face_detail * 0.055),
        height: clamp(0.2 + bevel * 0.72 + face_detail * 0.13),
        roughness: clamp(value.roughness + (random - 0.5) * 0.08 + face_detail * 0.04),
        metalness: 0.0,
    }
}

fn timber(value: &Timber, seed: u64, x: f32, y: f32) -> Surface {
    let pitch_y = value.board_width_mm + value.joint_mm;
    let pitch_x = value.board_length_mm + value.joint_mm;
    let row = (y / pitch_y).floor() as i64;
    let offset = if value.stagger && row.rem_euclid(2) != 0 {
        pitch_x * 0.5
    } else {
        0.0
    };
    let shifted = x + offset;
    let column = (shifted / pitch_x).floor() as i64;
    let local_x = shifted.rem_euclid(pitch_x);
    let local_y = y.rem_euclid(pitch_y);
    if local_x >= value.board_length_mm || local_y >= value.board_width_mm {
        return Surface {
            colour: linear(Colour::new(48, 39, 30)),
            height: 0.1,
            roughness: 0.72,
            metalness: 0.0,
        };
    }
    let random = random(seed, column, row, 7);
    let colour = value.colours[(random * value.colours.len() as f32).floor() as usize % value.colours.len()];
    let grain = ((x * 0.055 + random * 9.0).sin() * 0.5 + (x * 0.17 + y * 0.012).sin() * 0.25) * value.grain_strength;
    Surface {
        colour: vary(colour, grain * 0.18 + (random - 0.5) * 0.12),
        height: clamp(0.72 + grain * 0.16),
        roughness: clamp(value.roughness - grain * 0.08),
        metalness: 0.0,
    }
}

fn terrazzo(value: &Terrazzo, seed: u64, x: f32, y: f32, width: f32, height: f32) -> Surface {
    let cell = value.chip_size_mm * 1.6;
    let columns = (width / cell).ceil().max(1.0) as i64;
    let rows = (height / cell).ceil().max(1.0) as i64;
    let cell_x = (x / width * columns as f32).floor() as i64;
    let cell_y = (y / height * rows as f32).floor() as i64;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let candidate_x = (cell_x + dx).rem_euclid(columns);
            let candidate_y = (cell_y + dy).rem_euclid(rows);
            let exists = random(seed, candidate_x, candidate_y, 11);
            if exists > value.density {
                continue;
            }
            let centre_x =
                (candidate_x as f32 + 0.15 + random(seed, candidate_x, candidate_y, 12) * 0.7) * width / columns as f32;
            let centre_y =
                (candidate_y as f32 + 0.15 + random(seed, candidate_x, candidate_y, 13) * 0.7) * height / rows as f32;
            let distance_x = wrapped_distance(x, centre_x, width);
            let distance_y = wrapped_distance(y, centre_y, height);
            let radius = value.chip_size_mm * (0.22 + random(seed, candidate_x, candidate_y, 14) * 0.28);
            if distance_x * distance_x + distance_y * distance_y <= radius * radius {
                let choice =
                    (random(seed, candidate_x, candidate_y, 15) * value.chip_colours.len() as f32).floor() as usize;
                return Surface {
                    colour: linear(value.chip_colours[choice % value.chip_colours.len()]),
                    height: clamp(0.5 + value.chip_depth_mm * 0.1),
                    roughness: clamp(value.roughness - 0.04),
                    metalness: 0.0,
                };
            }
        }
    }
    Surface {
        colour: linear(value.matrix_colour),
        height: 0.5,
        roughness: value.roughness,
        metalness: 0.0,
    }
}

fn textile(value: &Textile, x: f32, y: f32) -> Surface {
    let scale = if value.basket {
        value.thread_mm * 2.0
    } else {
        value.thread_mm
    };
    let warp = (x / scale).floor() as i64;
    let weft = (y / scale).floor() as i64;
    let over = (warp + weft).rem_euclid(2) == 0;
    let local_x = (x / scale).fract();
    let local_y = (y / scale).fract();
    let ridge = if over {
        (local_x * std::f32::consts::PI).sin()
    } else {
        (local_y * std::f32::consts::PI).sin()
    };
    Surface {
        colour: linear(if over { value.warp_colour } else { value.weft_colour }),
        height: clamp(0.35 + ridge * (0.25 + value.depth_mm * 0.08)),
        roughness: value.roughness,
        metalness: 0.0,
    }
}

fn hatch(definition: &ProceduralDefinition) -> Option<(String, String)> {
    let (spacing_x, spacing_y, label) = match &definition.recipe {
        Recipe::Masonry(value) => (
            value.unit_width_mm + value.joint_mm,
            value.unit_height_mm + value.joint_mm,
            "Masonry",
        ),
        Recipe::Timber(value) => (
            value.board_length_mm + value.joint_mm,
            value.board_width_mm + value.joint_mm,
            "Timber",
        ),
        Recipe::Textile(value) => (value.thread_mm, value.thread_mm, "Textile"),
        Recipe::Paint(_) | Recipe::Terrazzo(_) => return None,
    };
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{0}mm\" height=\"{1}mm\" viewBox=\"0 0 {0} {1}\"><path d=\"M0 0H{0}M0 0V{1}\" fill=\"none\" stroke=\"#000\" stroke-width=\"0.25\"/></svg>\n",
        spacing_x, spacing_y
    );
    let pat = format!(
        ";%UNITS=MM\n;%TYPE=MODEL\n*OPAL_{label},OPAL {label} {spacing_x:.3} x {spacing_y:.3} mm\n0, 0, 0, 0, {spacing_y:.6}\n90, 0, 0, 0, {spacing_x:.6}\n"
    );
    Some((svg, pat))
}

fn rgb_asset(role: &'static str, width: u32, height: u32, values: &[[f32; 3]]) -> Result<BakedAsset, ProceduralError> {
    let mut image = RgbImage::new(width, height);
    for (pixel, value) in image.pixels_mut().zip(values) {
        *pixel = image::Rgb(value.map(to_byte));
    }
    encode(role, DynamicImage::ImageRgb8(image))
}

fn scalar_asset(role: &'static str, width: u32, height: u32, values: &[f32]) -> Result<BakedAsset, ProceduralError> {
    let mut image = ImageBuffer::<Luma<u8>, Vec<u8>>::new(width, height);
    for (pixel, value) in image.pixels_mut().zip(values) {
        *pixel = Luma([to_byte(*value)]);
    }
    encode(role, DynamicImage::ImageLuma8(image))
}

fn normal_asset(
    width: u32,
    height: u32,
    width_mm: f32,
    height_mm: f32,
    height_scale_mm: f32,
    values: &[f32],
) -> Result<BakedAsset, ProceduralError> {
    let mut image = RgbImage::new(width, height);
    let pixel_width_mm = width_mm / width as f32;
    let pixel_height_mm = height_mm / height as f32;
    let at = |x: i64, y: i64| {
        let x = x.rem_euclid(i64::from(width)) as u32;
        let y = y.rem_euclid(i64::from(height)) as u32;
        values[usize::try_from(y * width + x).unwrap_or(0)]
    };
    for y in 0..height {
        for x in 0..width {
            let dx = (at(i64::from(x) + 1, i64::from(y)) - at(i64::from(x) - 1, i64::from(y))) * height_scale_mm
                / (2.0 * pixel_width_mm);
            let dy = (at(i64::from(x), i64::from(y) + 1) - at(i64::from(x), i64::from(y) - 1)) * height_scale_mm
                / (2.0 * pixel_height_mm);
            let length = (dx * dx + dy * dy + 1.0).sqrt();
            let normal = [
                -dx / length * 0.5 + 0.5,
                -dy / length * 0.5 + 0.5,
                1.0 / length * 0.5 + 0.5,
            ];
            image.put_pixel(x, y, image::Rgb(normal.map(to_byte)));
        }
    }
    encode("normal", DynamicImage::ImageRgb8(image))
}

fn normal_height_mm(recipe: &Recipe) -> f32 {
    match recipe {
        Recipe::Masonry(value) => value.edge_depth_mm.max(value.surface_detail * 0.5),
        Recipe::Paint(_) | Recipe::Timber(_) | Recipe::Terrazzo(_) | Recipe::Textile(_) => 1.0,
    }
}

fn encode(role: &'static str, image: DynamicImage) -> Result<BakedAsset, ProceduralError> {
    let mut bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .map_err(|source| ProceduralError::Encode { role, source })?;
    Ok(asset(role, "png", "image/png", bytes))
}

fn asset(role: &str, extension: &str, media_type: &str, bytes: Vec<u8>) -> BakedAsset {
    BakedAsset {
        role: role.into(),
        extension: extension.into(),
        media_type: media_type.into(),
        sha256: hex(Sha256::digest(&bytes)),
        bytes,
    }
}

fn linear(colour: Colour) -> [f32; 3] {
    colour.0.map(|channel| channel as f32 / 255.0)
}

fn vary(colour: Colour, amount: f32) -> [f32; 3] {
    linear(colour).map(|channel| clamp(channel + amount))
}

fn clamp(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

fn to_byte(value: f32) -> u8 {
    (clamp(value) * 255.0).round() as u8
}

fn random(seed: u64, x: i64, y: i64, salt: u64) -> f32 {
    let mut value = seed ^ (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).rotate_left(31) ^ salt;
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    ((value ^ (value >> 31)) >> 40) as f32 / 16_777_215.0
}

fn periodic_noise(seed: u64, x: f32, y: f32) -> f32 {
    let phase = random(seed, 0, 0, 29) * std::f32::consts::TAU;
    ((x * std::f32::consts::TAU * 7.0 + phase).sin() * (y * std::f32::consts::TAU * 5.0 + phase).cos()
        + (x * std::f32::consts::TAU * 13.0 - phase).sin() * 0.35)
        / 1.35
}

fn masonry_noise(seed: u64, x_mm: f32, y_mm: f32) -> f32 {
    value_noise(seed, x_mm / 34.0, y_mm / 34.0, 41) * 0.5
        + value_noise(seed, x_mm / 12.0, y_mm / 12.0, 42) * 0.32
        + value_noise(seed, x_mm / 4.0, y_mm / 4.0, 43) * 0.18
}

fn value_noise(seed: u64, x: f32, y: f32, salt: u64) -> f32 {
    let x0 = x.floor() as i64;
    let y0 = y.floor() as i64;
    let tx = smooth(x.fract());
    let ty = smooth(y.fract());
    let at = |dx, dy| random(seed, x0 + dx, y0 + dy, salt) * 2.0 - 1.0;
    let top = at(0, 0) + (at(1, 0) - at(0, 0)) * tx;
    let bottom = at(0, 1) + (at(1, 1) - at(0, 1)) * tx;

    top + (bottom - top) * ty
}

fn smooth(value: f32) -> f32 {
    value * value * (3.0 - 2.0 * value)
}

fn wrapped_distance(left: f32, right: f32, period: f32) -> f32 {
    let direct = (left - right).abs();
    direct.min(period - direct)
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|byte| format!("{byte:02x}")).collect()
}
