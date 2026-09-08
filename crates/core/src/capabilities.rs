//! Capability declarations and deterministic loss computation.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::{Loss, LossKind, Material, Value};

/// A neutral parameter known to the initial material model.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Parameter {
    /// Base colour.
    BaseColor,
    /// Base metalness.
    BaseMetalness,
    /// Specular roughness.
    SpecularRoughness,
    /// Specular IOR.
    SpecularIor,
    /// Specular weight.
    SpecularWeight,
    /// Specular anisotropy.
    SpecularAnisotropy,
    /// Transmission weight.
    TransmissionWeight,
    /// Transmission thickness/depth.
    TransmissionThickness,
    /// Coat weight.
    CoatWeight,
    /// Coat roughness.
    CoatRoughness,
    /// Fuzz weight.
    FuzzWeight,
    /// Fuzz roughness.
    FuzzRoughness,
    /// Subsurface scattering weight.
    SubsurfaceWeight,
    /// Emission colour.
    EmissionColor,
    /// Tangent-space normal.
    GeometryNormal,
    /// Height/displacement.
    GeometryHeight,
    /// Bump.
    GeometryBump,
    /// Opacity.
    GeometryOpacity,
    /// Ambient occlusion.
    AmbientOcclusion,
}

impl Parameter {
    /// Canonical neutral parameter spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BaseColor => "base_color",
            Self::BaseMetalness => "base_metalness",
            Self::SpecularRoughness => "specular_roughness",
            Self::SpecularIor => "specular_ior",
            Self::SpecularWeight => "specular_weight",
            Self::SpecularAnisotropy => "specular_anisotropy",
            Self::TransmissionWeight => "transmission_weight",
            Self::TransmissionThickness => "transmission_thickness",
            Self::CoatWeight => "coat_weight",
            Self::CoatRoughness => "coat_roughness",
            Self::FuzzWeight => "fuzz_weight",
            Self::FuzzRoughness => "fuzz_roughness",
            Self::SubsurfaceWeight => "subsurface_weight",
            Self::EmissionColor => "emission_color",
            Self::GeometryNormal => "geometry_normal",
            Self::GeometryHeight => "geometry_height",
            Self::GeometryBump => "geometry_bump",
            Self::GeometryOpacity => "geometry_opacity",
            Self::AmbientOcclusion => "ambient_occlusion",
        }
    }
}

/// How faithfully a target represents one neutral parameter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Support {
    /// Fully representable.
    Exact,
    /// Represented by a target parameter with different semantics.
    Approximated(&'static str),
    /// Represented with reduced precision or structure.
    Reduced,
    /// Lossless conversion whose source convention is not recoverable.
    Converted,
    /// Not representable.
    Unsupported,
}

/// Complete capability declaration for an exporter.
#[derive(Clone, Debug)]
pub struct Capabilities {
    /// Human-readable target identifier.
    pub target: &'static str,
    /// Per-parameter fidelity.
    pub parameters: BTreeMap<Parameter, Support>,
    /// Whether all texture tiers survive as separately addressable data.
    pub texture_tiers: bool,
    /// Whether physical millimetre tiling survives.
    pub physical_tiling: bool,
    /// Whether arbitrary variant axes survive.
    pub variants: bool,
    /// Whether complete provenance survives.
    pub provenance: bool,
    /// Whether auxiliary file payloads survive.
    pub auxiliary_assets: bool,
}

impl Capabilities {
    /// Starts a declaration with every parameter unsupported.
    #[must_use]
    pub fn new(target: &'static str) -> Self {
        Self {
            target,
            parameters: BTreeMap::new(),
            texture_tiers: false,
            physical_tiling: false,
            variants: false,
            provenance: false,
            auxiliary_assets: false,
        }
    }

    /// Declares support for one parameter.
    #[must_use]
    pub fn with(mut self, parameter: Parameter, support: Support) -> Self {
        self.parameters.insert(parameter, support);
        self
    }

    /// Computes losses in material, parameter, kind, and detail order.
    #[must_use]
    pub fn assess(&self, materials: &[Material]) -> Vec<Loss> {
        let mut losses = Vec::new();
        for material in materials {
            for parameter in authored_parameters(material) {
                match self.parameters.get(&parameter).copied().unwrap_or(Support::Unsupported) {
                    Support::Exact => {}
                    Support::Approximated(as_parameter) => losses.push(Loss {
                        material: material.id.clone(),
                        parameter: parameter.as_str(),
                        kind: LossKind::Approximated { as_parameter },
                        detail: format!(
                            "{} maps `{}` to `{as_parameter}` with different behaviour",
                            self.target,
                            parameter.as_str()
                        ),
                    }),
                    Support::Reduced => losses.push(Loss {
                        material: material.id.clone(),
                        parameter: parameter.as_str(),
                        kind: LossKind::Reduced,
                        detail: format!("{} carries a reduced form of `{}`", self.target, parameter.as_str()),
                    }),
                    Support::Converted => losses.push(Loss {
                        material: material.id.clone(),
                        parameter: parameter.as_str(),
                        kind: LossKind::Converted,
                        detail: format!(
                            "{} converts `{}` to a target convention",
                            self.target,
                            parameter.as_str()
                        ),
                    }),
                    Support::Unsupported => losses.push(Loss {
                        material: material.id.clone(),
                        parameter: parameter.as_str(),
                        kind: LossKind::Dropped,
                        detail: format!("{} cannot represent `{}`", self.target, parameter.as_str()),
                    }),
                }
            }

            if !self.texture_tiers {
                material.visit_textures(|parameter, texture| {
                    if texture.tiers.len() > 1 {
                        losses.push(Loss {
                            material: material.id.clone(),
                            parameter,
                            kind: LossKind::Reduced,
                            detail: format!("{} selects one of {} texture tiers", self.target, texture.tiers.len()),
                        });
                    }
                });
            }
            if !self.physical_tiling && material.tiling.is_some() {
                losses.push(Loss {
                    material: material.id.clone(),
                    parameter: "tiling",
                    kind: LossKind::Dropped,
                    detail: format!("{} cannot represent physical millimetre tiling", self.target),
                });
            }
            if !self.variants && !material.variants.is_empty() {
                losses.push(Loss {
                    material: material.id.clone(),
                    parameter: "variants",
                    kind: LossKind::Dropped,
                    detail: format!("{} cannot represent arbitrary variant sets", self.target),
                });
            }
            if !self.provenance && material.provenance != Default::default() {
                losses.push(Loss {
                    material: material.id.clone(),
                    parameter: "provenance",
                    kind: LossKind::Dropped,
                    detail: format!("{} has no lossless provenance field", self.target),
                });
            }
            if !self.auxiliary_assets && !material.auxiliary.is_empty() {
                losses.push(Loss {
                    material: material.id.clone(),
                    parameter: "auxiliary",
                    kind: LossKind::Dropped,
                    detail: format!("{} cannot carry auxiliary file payloads", self.target),
                });
            }
        }
        losses.sort_by(|left, right| {
            left.material
                .cmp(&right.material)
                .then_with(|| left.parameter.cmp(right.parameter))
                .then_with(|| loss_rank(&left.kind).cmp(&loss_rank(&right.kind)))
                .then_with(|| left.detail.cmp(&right.detail))
        });
        losses.dedup();
        losses
    }
}

/// Built-in target declarations available without constructing a spoke.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    /// OpenUSD/USDZ.
    Usd,
    /// MaterialX 1.39.
    MaterialX,
    /// glTF 2.0 with ratified material extensions.
    Gltf,
    /// Autodesk Revit Generic appearance asset image set.
    Revit,
    /// NVIDIA Omniverse-compatible OpenUSD package.
    Omniverse,
}

/// Computes losses without writing a target document.
#[must_use]
pub fn dry_run(materials: &[Material], target: Target) -> Vec<Loss> {
    target_capabilities(target).assess(materials)
}

/// Returns the baseline declaration shared by the matching format exporter.
#[must_use]
pub fn target_capabilities(target: Target) -> Capabilities {
    match target {
        Target::Usd => all_parameters(Capabilities::new("USD")).tap(|capabilities| {
            capabilities.texture_tiers = true;
            capabilities.physical_tiling = true;
            capabilities.variants = true;
            capabilities.provenance = true;
            capabilities.auxiliary_assets = true;
        }),
        Target::MaterialX => all_parameters(Capabilities::new("MaterialX"))
            .with(Parameter::AmbientOcclusion, Support::Approximated("ambient_occlusion"))
            .tap(|capabilities| {
                capabilities.provenance = true;
                capabilities.auxiliary_assets = true;
            }),
        Target::Gltf => {
            let mut capabilities = Capabilities::new("glTF 2.0")
                .with(Parameter::BaseColor, Support::Exact)
                .with(Parameter::BaseMetalness, Support::Exact)
                .with(Parameter::SpecularRoughness, Support::Exact)
                .with(Parameter::SpecularIor, Support::Exact)
                .with(Parameter::SpecularWeight, Support::Exact)
                .with(Parameter::SpecularAnisotropy, Support::Exact)
                .with(Parameter::TransmissionWeight, Support::Exact)
                .with(Parameter::TransmissionThickness, Support::Exact)
                .with(Parameter::CoatWeight, Support::Exact)
                .with(Parameter::CoatRoughness, Support::Exact)
                .with(
                    Parameter::FuzzWeight,
                    Support::Approximated("KHR_materials_sheen.sheenColorFactor"),
                )
                .with(Parameter::FuzzRoughness, Support::Exact)
                .with(Parameter::EmissionColor, Support::Exact)
                .with(Parameter::GeometryNormal, Support::Exact)
                .with(Parameter::GeometryOpacity, Support::Exact)
                .with(Parameter::AmbientOcclusion, Support::Exact);
            capabilities.provenance = true; // Stored under namespaced `extras`.
            capabilities
        }
        Target::Revit => {
            let mut capabilities = Capabilities::new("Revit Generic appearance asset")
                .with(Parameter::BaseColor, Support::Reduced)
                .with(Parameter::SpecularRoughness, Support::Converted)
                .with(Parameter::GeometryNormal, Support::Approximated("generic_bump_map"))
                .with(Parameter::GeometryHeight, Support::Approximated("generic_bump_map"))
                .with(Parameter::GeometryBump, Support::Exact)
                .with(Parameter::GeometryOpacity, Support::Reduced);
            capabilities.physical_tiling = true;
            capabilities
        }
        Target::Omniverse => all_parameters(Capabilities::new("NVIDIA Omniverse OpenUSD")).tap(|capabilities| {
            capabilities.texture_tiers = true;
            capabilities.physical_tiling = true;
            capabilities.variants = true;
            capabilities.provenance = true;
            capabilities.auxiliary_assets = true;
        }),
    }
}

fn all_parameters(mut capabilities: Capabilities) -> Capabilities {
    for parameter in ALL_PARAMETERS {
        capabilities.parameters.insert(parameter, Support::Exact);
    }
    capabilities
}

trait Tap: Sized {
    fn tap(mut self, update: impl FnOnce(&mut Self)) -> Self {
        update(&mut self);
        self
    }
}

impl<T> Tap for T {}

const ALL_PARAMETERS: [Parameter; 19] = [
    Parameter::BaseColor,
    Parameter::BaseMetalness,
    Parameter::SpecularRoughness,
    Parameter::SpecularIor,
    Parameter::SpecularWeight,
    Parameter::SpecularAnisotropy,
    Parameter::TransmissionWeight,
    Parameter::TransmissionThickness,
    Parameter::CoatWeight,
    Parameter::CoatRoughness,
    Parameter::FuzzWeight,
    Parameter::FuzzRoughness,
    Parameter::SubsurfaceWeight,
    Parameter::EmissionColor,
    Parameter::GeometryNormal,
    Parameter::GeometryHeight,
    Parameter::GeometryBump,
    Parameter::GeometryOpacity,
    Parameter::AmbientOcclusion,
];

fn authored_parameters(material: &Material) -> Vec<Parameter> {
    let mut parameters = vec![
        Parameter::BaseColor,
        Parameter::BaseMetalness,
        Parameter::SpecularRoughness,
    ];
    macro_rules! optional {
        ($field:expr, $parameter:expr) => {
            if $field.is_some() {
                parameters.push($parameter);
            }
        };
    }
    optional!(material.surface.specular_ior, Parameter::SpecularIor);
    optional!(material.surface.specular_weight, Parameter::SpecularWeight);
    optional!(material.surface.specular_anisotropy, Parameter::SpecularAnisotropy);
    optional!(material.surface.transmission_weight, Parameter::TransmissionWeight);
    optional!(
        material.surface.transmission_thickness,
        Parameter::TransmissionThickness
    );
    optional!(material.surface.coat_weight, Parameter::CoatWeight);
    optional!(material.surface.coat_roughness, Parameter::CoatRoughness);
    optional!(material.surface.fuzz_weight, Parameter::FuzzWeight);
    optional!(material.surface.fuzz_roughness, Parameter::FuzzRoughness);
    optional!(material.surface.subsurface_weight, Parameter::SubsurfaceWeight);
    optional!(material.surface.emission_color, Parameter::EmissionColor);
    optional!(material.geometry.normal, Parameter::GeometryNormal);
    optional!(material.geometry.height, Parameter::GeometryHeight);
    optional!(material.geometry.bump, Parameter::GeometryBump);
    optional!(material.geometry.opacity, Parameter::GeometryOpacity);
    optional!(material.geometry.ambient_occlusion, Parameter::AmbientOcclusion);
    parameters
}

fn loss_rank(kind: &LossKind) -> u8 {
    match kind {
        LossKind::Dropped => 0,
        LossKind::Approximated { .. } => 1,
        LossKind::Reduced => 2,
        LossKind::Converted => 3,
    }
}

#[allow(dead_code)]
fn _value_is_textured<T>(value: &Value<T>) -> bool {
    value.texture().is_some()
}
