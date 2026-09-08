//! OpenPBR-aligned neutral material model.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// RGB colour with linear floating-point components unless a field says otherwise.
pub type Color3 = [f32; 3];
/// RGBA colour with linear floating-point components unless a field says otherwise.
pub type Color4 = [f32; 4];

/// A stable material identity, independent of any future geometry binding.
#[derive(Clone, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct MaterialId(pub String);

impl MaterialId {
    /// Creates an identity from an application-controlled stable string.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl fmt::Display for MaterialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A SHA-256 content digest written as 64 lowercase hexadecimal characters.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Sha256(pub String);

impl Sha256 {
    /// Returns whether this value has canonical SHA-256 syntax.
    #[must_use]
    pub fn is_canonical(&self) -> bool {
        self.0.len() == 64
            && self
                .0
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    }
}

impl fmt::Display for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A material input authored as a constant, texture, or their product.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Value<T> {
    /// A uniform value.
    Constant { value: T },
    /// A sampled texture.
    Texture { texture: TextureRef },
    /// A sampled texture multiplied by a uniform value.
    Modulated { texture: TextureRef, factor: T },
}

impl<T> Value<T> {
    /// Returns the referenced texture, if this value has one.
    #[must_use]
    pub const fn texture(&self) -> Option<&TextureRef> {
        match self {
            Self::Constant { .. } => None,
            Self::Texture { texture } | Self::Modulated { texture, .. } => Some(texture),
        }
    }
}

impl<T> From<T> for Value<T> {
    fn from(value: T) -> Self {
        Self::Constant { value }
    }
}

/// The source shading vocabulary used by a material.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadingModel {
    /// Academy Software Foundation OpenPBR Surface.
    #[default]
    OpenPbr,
    /// Autodesk Standard Surface.
    StandardSurface,
    /// glTF metallic-roughness PBR.
    GltfPbr,
}

/// OpenPBR surface parameters currently required by OPAL.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Surface {
    /// Diffuse or conductor base colour and alpha.
    pub base_color: Value<Color4>,
    /// Blend between dielectric and conductor response.
    pub base_metalness: Value<f32>,
    /// Microfacet roughness for the specular lobe.
    pub specular_roughness: Value<f32>,
    /// Index of refraction for the specular lobe.
    pub specular_ior: Option<Value<f32>>,
    /// Specular lobe weight or map.
    pub specular_weight: Option<Value<f32>>,
    /// Directional anisotropy amount.
    pub specular_anisotropy: Option<Value<f32>>,
    /// Transmission lobe weight.
    pub transmission_weight: Option<Value<f32>>,
    /// Physical transmission depth in millimetres.
    pub transmission_thickness: Option<Value<f32>>,
    /// Clear coat weight.
    pub coat_weight: Option<Value<f32>>,
    /// Clear coat roughness.
    pub coat_roughness: Option<Value<f32>>,
    /// Fuzz or sheen weight.
    pub fuzz_weight: Option<Value<f32>>,
    /// Fuzz lobe roughness.
    pub fuzz_roughness: Option<Value<f32>>,
    /// Subsurface scattering weight.
    pub subsurface_weight: Option<Value<f32>>,
    /// Emitted light colour.
    pub emission_color: Option<Value<Color3>>,
}

impl Default for Surface {
    fn default() -> Self {
        Self {
            base_color: [0.8, 0.8, 0.8, 1.0].into(),
            base_metalness: 0.0.into(),
            specular_roughness: 0.3.into(),
            specular_ior: None,
            specular_weight: None,
            specular_anisotropy: None,
            transmission_weight: None,
            transmission_thickness: None,
            coat_weight: None,
            coat_roughness: None,
            fuzz_weight: None,
            fuzz_roughness: None,
            subsurface_weight: None,
            emission_color: None,
        }
    }
}

/// Shading inputs related to a surface frame, not scene geometry.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SurfaceGeometry {
    /// Tangent-space surface normal.
    pub normal: Option<Value<Color3>>,
    /// Scalar displacement or parallax height.
    pub height: Option<Value<f32>>,
    /// Scalar bump value.
    pub bump: Option<Value<f32>>,
    /// Cutout or blend opacity.
    pub opacity: Option<Value<f32>>,
    /// Ambient-occlusion rendering convenience map.
    pub ambient_occlusion: Option<Value<f32>>,
}

/// A complete independently addressable material.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Material {
    /// Stable material identity.
    pub id: MaterialId,
    /// Human-readable display name.
    pub name: String,
    /// Source shading vocabulary.
    pub model: ShadingModel,
    /// Physical surface response.
    pub surface: Surface,
    /// Surface-frame inputs such as normal and opacity.
    pub geometry: SurfaceGeometry,
    /// Optional real-world repeat information.
    pub tiling: Option<Tiling>,
    /// Files retained beside, but never connected into, the shader.
    pub auxiliary: Vec<AuxiliaryAsset>,
    /// Lineage which must survive every operation.
    pub provenance: Provenance,
    /// Arbitrary orthogonal variant axes.
    pub variants: Vec<VariantSet>,
}

impl Material {
    /// Creates an OpenPBR material with conservative defaults.
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: MaterialId::new(id),
            name: name.into(),
            model: ShadingModel::OpenPbr,
            surface: Surface::default(),
            geometry: SurfaceGeometry::default(),
            tiling: None,
            auxiliary: Vec::new(),
            provenance: Provenance::default(),
            variants: Vec::new(),
        }
    }

    /// Visits every texture reference in a stable parameter order.
    pub fn visit_textures(&self, mut visit: impl FnMut(&'static str, &TextureRef)) {
        visit_value("base_color", &self.surface.base_color, &mut visit);
        visit_value("base_metalness", &self.surface.base_metalness, &mut visit);
        visit_value("specular_roughness", &self.surface.specular_roughness, &mut visit);
        visit_optional("specular_ior", &self.surface.specular_ior, &mut visit);
        visit_optional("specular_weight", &self.surface.specular_weight, &mut visit);
        visit_optional("specular_anisotropy", &self.surface.specular_anisotropy, &mut visit);
        visit_optional("transmission_weight", &self.surface.transmission_weight, &mut visit);
        visit_optional(
            "transmission_thickness",
            &self.surface.transmission_thickness,
            &mut visit,
        );
        visit_optional("coat_weight", &self.surface.coat_weight, &mut visit);
        visit_optional("coat_roughness", &self.surface.coat_roughness, &mut visit);
        visit_optional("fuzz_weight", &self.surface.fuzz_weight, &mut visit);
        visit_optional("fuzz_roughness", &self.surface.fuzz_roughness, &mut visit);
        visit_optional("subsurface_weight", &self.surface.subsurface_weight, &mut visit);
        visit_optional("emission_color", &self.surface.emission_color, &mut visit);
        visit_optional("geometry_normal", &self.geometry.normal, &mut visit);
        visit_optional("geometry_height", &self.geometry.height, &mut visit);
        visit_optional("geometry_bump", &self.geometry.bump, &mut visit);
        visit_optional("geometry_opacity", &self.geometry.opacity, &mut visit);
        visit_optional("ambient_occlusion", &self.geometry.ambient_occlusion, &mut visit);
    }

    /// Mutably visits every texture reference in the same stable order as
    /// [`visit_textures`](Self::visit_textures).
    pub fn visit_textures_mut(&mut self, mut visit: impl FnMut(&'static str, &mut TextureRef)) {
        visit_value_mut("base_color", &mut self.surface.base_color, &mut visit);
        visit_value_mut("base_metalness", &mut self.surface.base_metalness, &mut visit);
        visit_value_mut("specular_roughness", &mut self.surface.specular_roughness, &mut visit);
        visit_optional_mut("specular_ior", &mut self.surface.specular_ior, &mut visit);
        visit_optional_mut("specular_weight", &mut self.surface.specular_weight, &mut visit);
        visit_optional_mut("specular_anisotropy", &mut self.surface.specular_anisotropy, &mut visit);
        visit_optional_mut("transmission_weight", &mut self.surface.transmission_weight, &mut visit);
        visit_optional_mut(
            "transmission_thickness",
            &mut self.surface.transmission_thickness,
            &mut visit,
        );
        visit_optional_mut("coat_weight", &mut self.surface.coat_weight, &mut visit);
        visit_optional_mut("coat_roughness", &mut self.surface.coat_roughness, &mut visit);
        visit_optional_mut("fuzz_weight", &mut self.surface.fuzz_weight, &mut visit);
        visit_optional_mut("fuzz_roughness", &mut self.surface.fuzz_roughness, &mut visit);
        visit_optional_mut("subsurface_weight", &mut self.surface.subsurface_weight, &mut visit);
        visit_optional_mut("emission_color", &mut self.surface.emission_color, &mut visit);
        visit_optional_mut("geometry_normal", &mut self.geometry.normal, &mut visit);
        visit_optional_mut("geometry_height", &mut self.geometry.height, &mut visit);
        visit_optional_mut("geometry_bump", &mut self.geometry.bump, &mut visit);
        visit_optional_mut("geometry_opacity", &mut self.geometry.opacity, &mut visit);
        visit_optional_mut("ambient_occlusion", &mut self.geometry.ambient_occlusion, &mut visit);
    }
}

fn visit_value<T>(parameter: &'static str, value: &Value<T>, visit: &mut impl FnMut(&'static str, &TextureRef)) {
    if let Some(texture) = value.texture() {
        visit(parameter, texture);
    }
}

fn visit_optional<T>(
    parameter: &'static str,
    value: &Option<Value<T>>,
    visit: &mut impl FnMut(&'static str, &TextureRef),
) {
    if let Some(value) = value {
        visit_value(parameter, value, visit);
    }
}

fn visit_value_mut<T>(
    parameter: &'static str,
    value: &mut Value<T>,
    visit: &mut impl FnMut(&'static str, &mut TextureRef),
) {
    match value {
        Value::Constant { .. } => {}
        Value::Texture { texture } | Value::Modulated { texture, .. } => visit(parameter, texture),
    }
}

fn visit_optional_mut<T>(
    parameter: &'static str,
    value: &mut Option<Value<T>>,
    visit: &mut impl FnMut(&'static str, &mut TextureRef),
) {
    if let Some(value) = value {
        visit_value_mut(parameter, value, visit);
    }
}

/// Texture-map role vocabulary shared with OPAL.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MapRole {
    /// Base colour or albedo.
    BaseColor,
    /// Tangent-space normal.
    Normal,
    /// Microfacet roughness.
    Roughness,
    /// Inverse roughness convention.
    Glossiness,
    /// Metalness.
    Metallic,
    /// Displacement height.
    Height,
    /// Bump map.
    Bump,
    /// Ambient occlusion.
    Ao,
    /// Cutout/blend opacity.
    Opacity,
    /// Specular weight.
    Specular,
    /// Transmission weight.
    Transmission,
    /// Emission colour.
    Emissive,
}

impl MapRole {
    /// Canonical snake-case spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BaseColor => "base_color",
            Self::Normal => "normal",
            Self::Roughness => "roughness",
            Self::Glossiness => "glossiness",
            Self::Metallic => "metallic",
            Self::Height => "height",
            Self::Bump => "bump",
            Self::Ao => "ao",
            Self::Opacity => "opacity",
            Self::Specular => "specular",
            Self::Transmission => "transmission",
            Self::Emissive => "emissive",
        }
    }
}

impl fmt::Display for MapRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MapRole {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
            "base_color" | "basecolor" | "albedo" | "diffuse" => Ok(Self::BaseColor),
            "normal" | "normal_gl" | "normal_dx" => Ok(Self::Normal),
            "roughness" | "rough" => Ok(Self::Roughness),
            "glossiness" | "gloss" => Ok(Self::Glossiness),
            "metallic" | "metalness" | "metal" => Ok(Self::Metallic),
            "height" | "displacement" | "disp" => Ok(Self::Height),
            "bump" => Ok(Self::Bump),
            "ao" | "ambient_occlusion" | "occlusion" => Ok(Self::Ao),
            "opacity" | "alpha" | "mask" => Ok(Self::Opacity),
            "specular" | "spec" => Ok(Self::Specular),
            "transmission" | "translucency" => Ok(Self::Transmission),
            "emissive" | "emission" => Ok(Self::Emissive),
            _ => Err(format!("unknown texture role `{value}`")),
        }
    }
}

/// A resolution tier, ordered from smallest to largest.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum Tier {
    /// Viewer thumbnail/preview, nominally 512 pixels on the longest edge.
    #[serde(rename = "preview")]
    Preview,
    /// Nominal 1024-pixel tier.
    #[serde(rename = "1k")]
    K1,
    /// Nominal 2048-pixel tier.
    #[serde(rename = "2k")]
    K2,
    /// Nominal 4096-pixel tier.
    #[serde(rename = "4k")]
    K4,
    /// Nominal 8192-pixel tier.
    #[serde(rename = "8k")]
    K8,
}

impl Tier {
    /// Nominal longest-edge pixel count for this tier.
    #[must_use]
    pub const fn pixels(self) -> u32 {
        match self {
            Self::Preview => 512,
            Self::K1 => 1024,
            Self::K2 => 2048,
            Self::K4 => 4096,
            Self::K8 => 8192,
        }
    }

    /// Canonical spelling used in metadata.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Preview => "preview",
            Self::K1 => "1k",
            Self::K2 => "2k",
            Self::K4 => "4k",
            Self::K8 => "8k",
        }
    }
}

/// Transfer function/color interpretation attached to texture data.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorSpace {
    /// sRGB transfer function.
    Srgb,
    /// Uninterpreted data values.
    Raw,
    /// Linear-light RGB values.
    Linear,
}

/// Channel selected from a texture.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// RGB triplet.
    #[default]
    Rgb,
    /// Red channel.
    R,
    /// Green channel.
    G,
    /// Blue channel.
    B,
    /// Alpha channel.
    A,
}

/// Tangent-space Y-axis convention used by a normal map.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalConvention {
    /// Positive green points up (OpenGL).
    OpenGl,
    /// Negative green points up (DirectX).
    DirectX,
}

/// Self-contained bytes for one texture tier.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TextureSource {
    /// Content digest used as the package filename.
    pub hash: Sha256,
    /// Lowercase extension without a leading dot.
    pub extension: String,
    /// IANA media type.
    pub media_type: String,
    /// Decoded pixel width when known.
    pub width: Option<u32>,
    /// Decoded pixel height when known.
    pub height: Option<u32>,
    /// Complete encoded image bytes.
    #[serde(default)]
    pub bytes: Vec<u8>,
}

impl TextureSource {
    /// Returns the deterministic package-relative filename.
    #[must_use]
    pub fn package_path(&self) -> String {
        format!("textures/{}.{}", self.hash, self.extension)
    }
}

/// A texture and all available resolution tiers.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TextureRef {
    /// Semantic map role.
    pub role: MapRole,
    /// Resolution tiers in deterministic order.
    pub tiers: BTreeMap<Tier, TextureSource>,
    /// Explicit transfer-function interpretation.
    pub color_space: ColorSpace,
    /// Selected source channel.
    pub channel: Channel,
    /// Required convention for normal maps.
    pub normal_convention: Option<NormalConvention>,
}

impl TextureRef {
    /// Chooses a source at the requested tier, falling back to the nearest larger
    /// tier and then the largest smaller tier.
    #[must_use]
    pub fn source_for(&self, tier: Tier) -> Option<&TextureSource> {
        self.tiers
            .get(&tier)
            .or_else(|| self.tiers.range(tier..).next().map(|(_, source)| source))
            .or_else(|| self.tiers.values().next_back())
    }
}

/// Real-world texture repetition metadata.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Tiling {
    /// Physical repeat width in millimetres.
    pub width_mm: Option<f64>,
    /// Physical repeat height in millimetres.
    pub height_mm: Option<f64>,
    /// Repeat arrangement.
    pub repeat: Repeat,
    /// Optional domain-specific installation description.
    pub install_pattern: Option<String>,
}

/// Real-world pattern repeat.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Repeat {
    /// Aligned rows and columns.
    Straight,
    /// Every second row offset by one half.
    Halfdrop,
    /// Brick bond arrangement.
    Brick,
    /// Non-periodic placement.
    Random,
    /// No repeat.
    None,
}

/// A file retained with the package but not connected to the shader graph.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AuxiliaryAsset {
    /// Auxiliary role.
    pub role: AuxiliaryRole,
    /// Original display filename.
    pub name: String,
    /// Content digest.
    pub hash: Sha256,
    /// IANA media type.
    pub media_type: String,
    /// Complete file bytes.
    #[serde(default)]
    pub bytes: Vec<u8>,
}

/// Fixed auxiliary-file vocabulary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuxiliaryRole {
    /// Source reference photograph.
    RefImage,
    /// Offline render.
    Render,
    /// UI thumbnail.
    Thumbnail,
    /// NVIDIA MDL document.
    Mdl,
}

/// A general variant axis.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct VariantSet {
    /// Stable axis name, such as `colourway` or `lod`.
    pub name: String,
    /// Name selected when no explicit selection is made.
    pub default: String,
    /// Ordered variant definitions.
    pub variants: Vec<Variant>,
}

/// One named variant with sparse parameter overrides.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Variant {
    /// Stable selection name.
    pub name: String,
    /// Sparse overrides keyed by neutral parameter name.
    pub overrides: BTreeMap<String, ParameterValue>,
}

/// Dynamically typed value used by sparse variant overrides.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "value")]
pub enum ParameterValue {
    /// Scalar input.
    Scalar(Value<f32>),
    /// Three-component colour/vector input.
    Color3(Value<Color3>),
    /// Four-component colour input.
    Color4(Value<Color4>),
    /// Real-world tiling override.
    Tiling(Tiling),
}

/// Complete lineage attached to a material.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Provenance {
    /// Library or application which built the material.
    pub builder: Option<String>,
    /// Builder version.
    pub builder_version: Option<String>,
    /// Reproducible build timestamp supplied by the caller, never invented by exporters.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub built_at: Option<OffsetDateTime>,
    /// Original source identity or URI.
    pub source_identifier: Option<String>,
    /// Supplier-specific source record.
    pub supplier_source: Option<String>,
    /// Material revision/version.
    pub version: Option<String>,
    /// Application-specific lineage which must survive format round trips.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, serde_json::Value>,
    /// Per-content derivation records, keyed by result hash.
    pub derivations: BTreeMap<Sha256, Derivation>,
    /// Source artifacts retained so every derivation parent can be resolved.
    pub source_assets: BTreeMap<Sha256, ProvenanceAsset>,
    /// Material-level conversion history.
    pub events: Vec<ProvenanceEvent>,
}

/// An immutable source artifact retained solely to keep lineage self-contained.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProvenanceAsset {
    /// Original display filename.
    pub name: String,
    /// Lowercase extension without a leading dot.
    pub extension: String,
    /// IANA media type.
    pub media_type: String,
    /// Complete source bytes.
    #[serde(default)]
    pub bytes: Vec<u8>,
}

impl ProvenanceAsset {
    /// Deterministic path used when the asset is packaged.
    #[must_use]
    pub fn package_path(&self, hash: &Sha256) -> String {
        format!("provenance/sources/{hash}.{}", self.extension)
    }
}

/// Lineage for one texture result.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Derivation {
    /// Parent image digest, if this result was derived from another image.
    pub parent: Option<Sha256>,
    /// Transformation category.
    pub operation: Operation,
    /// Producing tool.
    pub tool: String,
    /// Producing tool version.
    pub tool_version: String,
    /// Optional model identity for generated content.
    pub model: Option<String>,
    /// Stable operation parameters.
    pub parameters: serde_json::Value,
    /// Timestamp supplied by the ingest caller.
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
}

/// Material-level provenance event.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProvenanceEvent {
    /// Transformation category.
    pub operation: Operation,
    /// Human-readable deterministic detail.
    pub detail: String,
    /// Timestamp supplied by the caller.
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
}

/// Provenance operation vocabulary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// Created directly by a human authoring workflow.
    Authored,
    /// Captured from the physical world.
    Scanned,
    /// Resampled to fewer pixels.
    Downscale,
    /// Resampled to more pixels.
    Upscale,
    /// Produced by a generative model or procedural tool.
    Generated,
    /// Byte-for-byte copied.
    Copied,
    /// Converted between semantic or encoding conventions.
    Converted,
    /// Manually adjusted from an existing source image.
    Retouched,
}

/// One declared fidelity loss.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Loss {
    /// Affected material.
    pub material: MaterialId,
    /// Canonical neutral parameter name.
    pub parameter: &'static str,
    /// Loss category.
    pub kind: LossKind,
    /// Deterministic diagnostic detail.
    pub detail: String,
}

/// Fidelity loss category.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum LossKind {
    /// Not representable and omitted.
    Dropped,
    /// Mapped to a parameter with meaningfully different behaviour.
    Approximated {
        /// Target parameter used for the approximation.
        as_parameter: &'static str,
    },
    /// Represented with less information.
    Reduced,
    /// Lossless output whose original convention cannot be recovered.
    Converted,
}
