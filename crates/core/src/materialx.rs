//! Portable MaterialX graph data. No XML, filesystem, or renderer dependency.

use crate::TextureSource;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// An authored MaterialX element, including node graphs and interface ports.
/// Keeping the graph separate from the projected surface prevents procedural
/// connections and renderer-specific inputs from disappearing during a round trip.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MaterialXElement {
    pub category: String,
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<MaterialXElement>,
}

impl MaterialXElement {
    pub fn attribute(&self, name: &str) -> &str {
        self.attributes.get(name).map(String::as_str).unwrap_or("")
    }

    pub fn visit(&self, visitor: &mut impl FnMut(&Self)) {
        visitor(self);
        for child in &self.children {
            child.visit(visitor);
        }
    }
}

/// A native graph and its complete dependency set. Texture filenames in the
/// document are package-relative, content-addressed paths. `material_name`
/// selects one surfacematerial without discarding other shared graph nodes.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MaterialXGraph {
    pub document: MaterialXElement,
    pub material_name: String,
    #[serde(default)]
    pub assets: BTreeMap<String, TextureSource>,
}

use crate::{Channel, ColorSpace, ExportError, Material, NormalConvention, TextureRef, Tier, Value};

fn element(category: &str, name: &str, kind: &str, children: Vec<MaterialXElement>) -> MaterialXElement {
    MaterialXElement {
        category: category.into(),
        attributes: BTreeMap::from([("name".into(), name.into()), ("type".into(), kind.into())]),
        children,
    }
}
fn constant(name: &str, kind: &str, value: impl Into<String>) -> MaterialXElement {
    let mut input = element("input", name, kind, vec![]);
    input.attributes.insert("value".into(), value.into());
    input
}
fn connected(name: &str, kind: &str, node: &str) -> MaterialXElement {
    let mut input = element("input", name, kind, vec![]);
    input.attributes.insert("nodename".into(), node.into());
    input
}
fn vector(values: &[f32]) -> String {
    values.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}

struct Author {
    nodes: Vec<MaterialXElement>,
    assets: BTreeMap<String, TextureSource>,
    tier: Tier,
}
impl Author {
    fn input<T>(
        &mut self,
        name: &str,
        kind: &str,
        value: &Value<T>,
        format: impl Fn(&T) -> String,
    ) -> Result<MaterialXElement, ExportError> {
        match value {
            Value::Constant { value } => Ok(constant(name, kind, format(value))),
            Value::Texture { texture } | Value::Modulated { texture, .. } => {
                let mut output = self.texture(name, kind, texture)?;
                if let Value::Modulated { factor, .. } = value {
                    let node = format!("{name}_multiply");
                    self.nodes.push(element(
                        "multiply",
                        &node,
                        kind,
                        vec![connected("in1", kind, &output), constant("in2", kind, format(factor))],
                    ));
                    output = node;
                }
                Ok(connected(name, kind, &output))
            }
        }
    }
    fn texture(&mut self, name: &str, kind: &str, texture: &TextureRef) -> Result<String, ExportError> {
        let source = texture
            .source_for(self.tier)
            .ok_or_else(|| ExportError::InvalidModel(format!("texture `{name}` has no source")))?;
        let path = source.package_path();
        self.assets.insert(path.clone(), source.clone());
        let image_name = format!("{name}_image");
        // Scalar channels use an explicit extraction; image_float does not select
        // the green/blue/alpha channel of a packed texture.
        let image_type = if kind == "float" { "color4" } else { kind };
        let mut file = constant("file", "filename", path);
        file.attributes.insert(
            "colorspace".into(),
            match texture.color_space {
                ColorSpace::Srgb => "srgb_texture",
                ColorSpace::Raw => "none",
                ColorSpace::Linear => "lin_rec709",
            }
            .into(),
        );
        self.nodes.push(element("image", &image_name, image_type, vec![file]));
        if kind == "float" {
            let output = format!("{name}_channel");
            let mut node = element(
                "extract",
                &output,
                "float",
                vec![
                    connected("in", "color4", &image_name),
                    constant(
                        "index",
                        "integer",
                        match texture.channel {
                            Channel::Rgb | Channel::R => "0",
                            Channel::G => "1",
                            Channel::B => "2",
                            Channel::A => "3",
                        },
                    ),
                ],
            );
            node.attributes.insert("nodedef".into(), "ND_extract_color4".into());
            self.nodes.push(node);
            Ok(output)
        } else {
            Ok(image_name)
        }
    }
}

/// Author a standards-based OpenPBR graph from the neutral surface. Both XML
/// and USD consume these nodes; there is no second shader implementation.
/// Native graphs are returned intact, including procedural connections.
pub fn materialx_graph(material: &Material, tier: Tier) -> Result<MaterialXGraph, ExportError> {
    if let Some(graph) = &material.materialx {
        return Ok(graph.clone());
    }
    let mut author = Author {
        nodes: vec![],
        assets: BTreeMap::new(),
        tier,
    };
    let mut inputs = vec![
        author.input("base_color", "color3", &material.surface.base_color, |v| {
            vector(&v[..3])
        })?,
        author.input(
            "base_metalness",
            "float",
            &material.surface.base_metalness,
            ToString::to_string,
        )?,
        author.input(
            "specular_roughness",
            "float",
            &material.surface.specular_roughness,
            ToString::to_string,
        )?,
    ];
    macro_rules! scalar {
        ($field:expr, $name:literal) => {
            if let Some(value) = $field {
                inputs.push(author.input($name, "float", value, ToString::to_string)?);
            }
        };
    }
    scalar!(&material.surface.specular_ior, "specular_ior");
    scalar!(&material.surface.specular_weight, "specular_weight");
    scalar!(&material.surface.specular_anisotropy, "specular_roughness_anisotropy");
    scalar!(&material.surface.transmission_weight, "transmission_weight");
    scalar!(&material.surface.transmission_thickness, "transmission_depth");
    scalar!(&material.surface.coat_weight, "coat_weight");
    scalar!(&material.surface.coat_roughness, "coat_roughness");
    scalar!(&material.surface.fuzz_weight, "fuzz_weight");
    scalar!(&material.surface.fuzz_roughness, "fuzz_roughness");
    scalar!(&material.surface.subsurface_weight, "subsurface_weight");
    if let Some(emission) = &material.surface.emission_color {
        inputs.push(author.input("emission_color", "color3", emission, |v| vector(v))?);
        inputs.push(constant("emission_luminance", "float", "1"));
    }
    let alpha = match &material.surface.base_color {
        Value::Constant { value } => value[3],
        Value::Modulated { factor, .. } => factor[3],
        _ => 1.0,
    };
    let opacity = match &material.geometry.opacity {
        Some(Value::Constant { value }) => Some(Value::from(value * alpha)),
        Some(Value::Texture { texture }) => Some(Value::Modulated {
            texture: texture.clone(),
            factor: alpha,
        }),
        Some(Value::Modulated { texture, factor }) => Some(Value::Modulated {
            texture: texture.clone(),
            factor: factor * alpha,
        }),
        None if alpha != 1.0 => Some(Value::from(alpha)),
        _ => None,
    };
    scalar!(opacity.as_ref(), "geometry_opacity");
    let mut normal = None;
    if let Some(value) = &material.geometry.normal {
        let sampled = author.input("geometry_normal", "vector3", value, |v| vector(v))?;
        if value.texture().is_some() {
            let mut source = sampled.attribute("nodename").to_owned();
            if value
                .texture()
                .is_some_and(|t| t.normal_convention == Some(NormalConvention::DirectX))
            {
                author.nodes.push(element(
                    "multiply",
                    "normal_flip",
                    "vector3",
                    vec![
                        connected("in1", "vector3", &source),
                        constant("in2", "vector3", "1, -1, 1"),
                    ],
                ));
                author.nodes.push(element(
                    "add",
                    "normal_offset",
                    "vector3",
                    vec![
                        connected("in1", "vector3", "normal_flip"),
                        constant("in2", "vector3", "0, 1, 0"),
                    ],
                ));
                source = "normal_offset".into();
            }
            let mut node = element(
                "normalmap",
                "NormalMap",
                "vector3",
                vec![connected("in", "vector3", &source)],
            );
            node.attributes.insert("nodedef".into(), "ND_normalmap_float".into());
            author.nodes.push(node);
            normal = Some(connected("geometry_normal", "vector3", "NormalMap"));
        } else {
            normal = Some(sampled);
        }
    }
    if let Some(value) = &material.geometry.bump {
        let mut bump = author.input("bump", "float", value, ToString::to_string)?;
        bump.attributes.insert("name".into(), "height".into());
        let mut bump_inputs = vec![bump];
        if let Some(mut normal_input) = normal.take() {
            normal_input.attributes.insert("name".into(), "normal".into());
            bump_inputs.push(normal_input);
        }
        author.nodes.push(element("bump", "Bump", "vector3", bump_inputs));
        normal = Some(connected("geometry_normal", "vector3", "Bump"));
    }
    if let Some(normal) = normal {
        inputs.push(normal);
    }
    author
        .nodes
        .push(element("open_pbr_surface", "OpenPBR", "surfaceshader", inputs));
    let mut binding = vec![connected("surfaceshader", "surfaceshader", "OpenPBR")];
    if let Some(value) = &material.geometry.height {
        let mut height = author.input("height", "float", value, ToString::to_string)?;
        height.attributes.insert("name".into(), "displacement".into());
        let mut node = element("displacement", "Displacement", "displacementshader", vec![height]);
        node.attributes.insert("nodedef".into(), "ND_displacement_float".into());
        author.nodes.push(node);
        binding.push(connected("displacementshader", "displacementshader", "Displacement"));
    }
    author
        .nodes
        .push(element("surfacematerial", "Material", "material", binding));
    Ok(MaterialXGraph {
        document: MaterialXElement {
            category: "materialx".into(),
            attributes: BTreeMap::from([("version".into(), "1.39".into())]),
            children: author.nodes,
        },
        material_name: "Material".into(),
        assets: author.assets,
    })
}
