//! MaterialX 1.39 document import and export.
//!
//! The authored graph uses `open_pbr_surface`, `image`, and `multiply` nodes.
//! Olsyn's neutral manifest is stored in a standard MaterialX property set so
//! complete provenance can survive tools which preserve unknown properties.

use std::collections::BTreeMap;
use std::io::Cursor;

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, Event};
use quick_xml::{Reader, Writer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use usd_toolbox_core::{
    Capabilities, Color3, Color4, Export, ExportError, Exporter, ImportError, Importer, Input, Loss, LossKind,
    Material, Target, TextureRef, Tier, Value, target_capabilities, validate_materials,
};

const MANIFEST_PROPERTY_SET: &str = "olsyn_neutral_manifest";
const MANIFEST_PROPERTY: &str = "json";

/// MaterialX export settings.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MaterialXExportOptions {
    /// Texture tier connected into the standard graph.
    pub graph_tier: Tier,
    /// Retain texture and auxiliary bytes inside the neutral manifest. This is
    /// useful for bytes-only round trips; disable it when external assets are
    /// already managed by a package such as USDZ.
    pub embed_payloads: bool,
    /// Include the namespaced neutral manifest which preserves provenance and
    /// data MaterialX cannot represent natively.
    pub include_neutral_manifest: bool,
}

impl Default for MaterialXExportOptions {
    fn default() -> Self {
        Self {
            graph_tier: Tier::K2,
            embed_payloads: true,
            include_neutral_manifest: true,
        }
    }
}

/// MaterialX import settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct MaterialXImportOptions {
    /// Permit documents without the neutral manifest. Generic graph parsing is
    /// intentionally rejected in this first implementation instead of silently
    /// producing a partial material.
    pub allow_partial_graph: bool,
}

/// MaterialX document exporter.
#[derive(Clone, Copy, Debug, Default)]
pub struct MaterialXExporter;

/// MaterialX document importer.
#[derive(Clone, Copy, Debug, Default)]
pub struct MaterialXImporter;

impl Exporter for MaterialXExporter {
    type Options = MaterialXExportOptions;

    fn capabilities(&self) -> Capabilities {
        target_capabilities(Target::MaterialX)
    }

    fn export(&self, materials: &[Material], options: &Self::Options) -> Result<Export, ExportError> {
        if materials.is_empty() {
            return Err(ExportError::InvalidModel("at least one material is required".into()));
        }
        let issues = validate_materials(materials);
        if !issues.is_empty() {
            return Err(ExportError::InvalidModel(
                issues
                    .into_iter()
                    .map(|issue| format!("{}: {}", issue.path, issue.detail))
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        }
        validate_payload_hashes(materials)?;

        let bytes = write_document(materials, options)?;
        let mut losses = self.dry_run(materials);
        for material in materials {
            if !options.include_neutral_manifest {
                if material.provenance != Default::default() {
                    losses.push(Loss {
                        material: material.id.clone(),
                        parameter: "provenance",
                        kind: LossKind::Dropped,
                        detail: "MaterialX neutral manifest was disabled, so provenance was omitted".into(),
                    });
                }
                if !material.auxiliary.is_empty() {
                    losses.push(Loss {
                        material: material.id.clone(),
                        parameter: "auxiliary",
                        kind: LossKind::Dropped,
                        detail: "MaterialX neutral manifest was disabled, so auxiliary payloads were omitted".into(),
                    });
                }
            } else if !options.embed_payloads {
                if !material.provenance.source_assets.is_empty() {
                    losses.push(Loss {
                        material: material.id.clone(),
                        parameter: "provenance",
                        kind: LossKind::Reduced,
                        detail:
                            "MaterialX payload embedding was disabled, so retained provenance source bytes were omitted"
                                .into(),
                    });
                }
                if !material.auxiliary.is_empty() {
                    losses.push(Loss {
                        material: material.id.clone(),
                        parameter: "auxiliary",
                        kind: LossKind::Dropped,
                        detail: "MaterialX payload embedding was disabled, so auxiliary payloads were omitted".into(),
                    });
                }
            }
        }
        sort_losses(&mut losses);
        Ok(Export { bytes, losses })
    }
}

impl Importer for MaterialXImporter {
    type Options = MaterialXImportOptions;

    fn import(&self, input: Input<'_>, options: &Self::Options) -> Result<Vec<Material>, ImportError> {
        let bytes = input.single_bytes().ok_or_else(|| ImportError::Invalid {
            format: "MaterialX",
            detail: "MaterialX input must be one XML byte buffer".into(),
        })?;
        match read_manifest(bytes)? {
            Some(materials) => Ok(materials),
            None if options.allow_partial_graph => read_generic_graph(bytes),
            None => Err(ImportError::Missing(format!(
                "propertyset `{MANIFEST_PROPERTY_SET}` / property `{MANIFEST_PROPERTY}`"
            ))),
        }
    }
}

/// Conservatively imports direct constants from standard OpenPBR or Standard
/// Surface nodes. Texture connections are intentionally left unauthored unless
/// their bytes arrive through a self-contained neutral manifest/bundle.
fn read_generic_graph(bytes: &[u8]) -> Result<Vec<Material>, ImportError> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut materials = Vec::new();
    let mut current: Option<Material> = None;
    let mut unsupported_connections = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) if matches!(event.name().into_inner(), "open_pbr_surface" | "standard_surface") => {
                let name = attribute(&event, "name")?.unwrap_or_else(|| format!("material_{}", materials.len()));
                let id = name.strip_suffix("_openpbr").unwrap_or(&name).to_owned();
                let mut material = Material::new(id, name);
                material.provenance.builder = Some("usd-toolbox MaterialX partial importer".into());
                material
                    .provenance
                    .metadata
                    .insert("partial_graph_import".into(), serde_json::Value::Bool(true));
                current = Some(material);
            }
            Ok(Event::Empty(event)) if current.is_some() && event.name().into_inner() == "input" => {
                let name = attribute(&event, "name")?.unwrap_or_default();
                if let Some(node) = attribute(&event, "nodename")? {
                    unsupported_connections.push(format!("{name}->{node}"));
                } else if let Some(value) = attribute(&event, "value")? {
                    assign_generic_constant(current.as_mut().expect("checked above"), &name, &value)?;
                }
            }
            Ok(Event::End(event))
                if current.is_some()
                    && matches!(event.name().into_inner(), "open_pbr_surface" | "standard_surface") =>
            {
                let mut material = current.take().expect("checked above");
                if !unsupported_connections.is_empty() {
                    material.provenance.metadata.insert(
                        "unresolved_connections".into(),
                        serde_json::json!(std::mem::take(&mut unsupported_connections)),
                    );
                }
                materials.push(material);
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => {
                return Err(ImportError::Invalid {
                    format: "MaterialX XML",
                    detail: error.to_string(),
                });
            }
        }
    }
    if materials.is_empty() {
        return Err(ImportError::Unsupported(
            "no open_pbr_surface or standard_surface node was found".into(),
        ));
    }
    Ok(materials)
}

fn assign_generic_constant(material: &mut Material, name: &str, value: &str) -> Result<(), ImportError> {
    let scalar = || {
        value.trim().parse::<f32>().map_err(|error| ImportError::Invalid {
            format: "MaterialX constant",
            detail: format!("`{name}` has invalid value `{value}`: {error}"),
        })
    };
    match name {
        "base_color" => {
            let color = parse_color(value)?;
            material.surface.base_color = Value::from([color[0], color[1], color[2], 1.0]);
        }
        "base_metalness" | "metalness" => material.surface.base_metalness = Value::from(scalar()?),
        "specular_roughness" => material.surface.specular_roughness = Value::from(scalar()?),
        "specular_ior" => material.surface.specular_ior = Some(Value::from(scalar()?)),
        "specular_weight" => material.surface.specular_weight = Some(Value::from(scalar()?)),
        "specular_anisotropy" => material.surface.specular_anisotropy = Some(Value::from(scalar()?)),
        "transmission_weight" => material.surface.transmission_weight = Some(Value::from(scalar()?)),
        "transmission_depth" => material.surface.transmission_thickness = Some(Value::from(scalar()?)),
        "coat_weight" => material.surface.coat_weight = Some(Value::from(scalar()?)),
        "coat_roughness" => material.surface.coat_roughness = Some(Value::from(scalar()?)),
        "fuzz_weight" => material.surface.fuzz_weight = Some(Value::from(scalar()?)),
        "fuzz_roughness" => material.surface.fuzz_roughness = Some(Value::from(scalar()?)),
        "subsurface_weight" => material.surface.subsurface_weight = Some(Value::from(scalar()?)),
        "emission_color" => material.surface.emission_color = Some(Value::from(parse_color(value)?)),
        "geometry_height" => material.geometry.height = Some(Value::from(scalar()?)),
        "geometry_bump" => material.geometry.bump = Some(Value::from(scalar()?)),
        "geometry_opacity" | "opacity" => material.geometry.opacity = Some(Value::from(scalar()?)),
        "ambient_occlusion" => material.geometry.ambient_occlusion = Some(Value::from(scalar()?)),
        _ => {}
    }
    Ok(())
}

fn parse_color(value: &str) -> Result<[f32; 3], ImportError> {
    let components = value
        .split([',', ' '])
        .filter(|part| !part.is_empty())
        .map(str::parse::<f32>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ImportError::Invalid {
            format: "MaterialX constant",
            detail: format!("invalid color `{value}`: {error}"),
        })?;
    match components.as_slice() {
        [red, green, blue, ..] => Ok([*red, *green, *blue]),
        _ => Err(ImportError::Invalid {
            format: "MaterialX constant",
            detail: format!("color `{value}` does not have three components"),
        }),
    }
}

fn write_document(materials: &[Material], options: &MaterialXExportOptions) -> Result<Vec<u8>, ExportError> {
    let mut writer = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 2);
    writer
        .write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
        .map_err(xml_export_error)?;
    let mut root = BytesStart::new("materialx");
    root.push_attribute(("version", "1.39"));
    root.push_attribute(("xmlns:olsyn", "https://olsyn.com/ns/usd-toolbox/1"));
    writer.write_event(Event::Start(root)).map_err(xml_export_error)?;

    let mut used_names = BTreeMap::<String, usize>::new();
    for material in materials {
        let base = xml_name(&material.id.0);
        let suffix = used_names.entry(base.clone()).or_default();
        let material_name = if *suffix == 0 { base } else { format!("{base}_{suffix}") };
        *suffix += 1;
        write_material(&mut writer, material, &material_name, options.graph_tier)?;
    }

    if options.include_neutral_manifest {
        let mut manifest = materials.to_vec();
        if !options.embed_payloads {
            clear_payloads(&mut manifest);
        }
        let json = serde_json::to_string(&manifest).map_err(|error| ExportError::Encode {
            format: "MaterialX",
            detail: error.to_string(),
        })?;
        let mut property_set = BytesStart::new("propertyset");
        property_set.push_attribute(("name", MANIFEST_PROPERTY_SET));
        writer
            .write_event(Event::Start(property_set))
            .map_err(xml_export_error)?;
        let mut property = BytesStart::new("property");
        property.push_attribute(("name", MANIFEST_PROPERTY));
        property.push_attribute(("type", "string"));
        property.push_attribute(("value", json.as_str()));
        writer.write_event(Event::Empty(property)).map_err(xml_export_error)?;
        writer
            .write_event(Event::End(BytesEnd::new("propertyset")))
            .map_err(xml_export_error)?;
    }

    writer
        .write_event(Event::End(BytesEnd::new("materialx")))
        .map_err(xml_export_error)?;
    Ok(writer.into_inner().into_inner())
}

fn validate_payload_hashes(materials: &[Material]) -> Result<(), ExportError> {
    for material in materials {
        let mut mismatch = None;
        material.visit_textures(|parameter, texture| {
            if mismatch.is_none() {
                mismatch = texture
                    .tiers
                    .iter()
                    .find(|(_, source)| format!("{:x}", Sha256::digest(&source.bytes)) != source.hash.0)
                    .map(|(tier, _)| format!("{parameter}.tiers.{}", tier.as_str()));
            }
        });
        if let Some(path) = mismatch {
            return Err(ExportError::InvalidModel(format!(
                "material `{}` has a payload/hash mismatch at {path}",
                material.id
            )));
        }
        for asset in &material.auxiliary {
            if format!("{:x}", Sha256::digest(&asset.bytes)) != asset.hash.0 {
                return Err(ExportError::InvalidModel(format!(
                    "material `{}` has a payload/hash mismatch for auxiliary `{}`",
                    material.id, asset.name
                )));
            }
        }
        for (hash, asset) in &material.provenance.source_assets {
            if format!("{:x}", Sha256::digest(&asset.bytes)) != hash.0 {
                return Err(ExportError::InvalidModel(format!(
                    "material `{}` has a payload/hash mismatch for provenance source `{}`",
                    material.id, asset.name
                )));
            }
        }
    }
    Ok(())
}

fn sort_losses(losses: &mut Vec<Loss>) {
    losses.sort_by(|left, right| {
        left.material
            .cmp(&right.material)
            .then_with(|| left.parameter.cmp(right.parameter))
            .then_with(|| left.detail.cmp(&right.detail))
    });
    losses.dedup();
}

fn write_material(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    material: &Material,
    material_name: &str,
    graph_tier: Tier,
) -> Result<(), ExportError> {
    let surface_name = format!("{material_name}_openpbr");
    let opacity = combined_opacity(material);
    let mut nodes = Vec::new();
    collect_color4_node(
        &mut nodes,
        material_name,
        "base_color",
        &material.surface.base_color,
        graph_tier,
    )?;
    collect_scalar_node(
        &mut nodes,
        material_name,
        "base_metalness",
        &material.surface.base_metalness,
        graph_tier,
    )?;
    collect_scalar_node(
        &mut nodes,
        material_name,
        "specular_roughness",
        &material.surface.specular_roughness,
        graph_tier,
    )?;
    macro_rules! scalar_node {
        ($field:expr, $name:literal) => {
            if let Some(value) = $field {
                collect_scalar_node(&mut nodes, material_name, $name, value, graph_tier)?;
            }
        };
    }
    scalar_node!(&material.surface.specular_ior, "specular_ior");
    scalar_node!(&material.surface.specular_weight, "specular_weight");
    scalar_node!(&material.surface.specular_anisotropy, "specular_anisotropy");
    scalar_node!(&material.surface.transmission_weight, "transmission_weight");
    scalar_node!(&material.surface.transmission_thickness, "transmission_depth");
    scalar_node!(&material.surface.coat_weight, "coat_weight");
    scalar_node!(&material.surface.coat_roughness, "coat_roughness");
    scalar_node!(&material.surface.fuzz_weight, "fuzz_weight");
    scalar_node!(&material.surface.fuzz_roughness, "fuzz_roughness");
    scalar_node!(&material.surface.subsurface_weight, "subsurface_weight");
    scalar_node!(&material.geometry.height, "geometry_height");
    scalar_node!(&material.geometry.bump, "geometry_bump");
    scalar_node!(opacity.as_ref(), "geometry_opacity");
    scalar_node!(&material.geometry.ambient_occlusion, "ambient_occlusion");
    if let Some(value) = &material.surface.emission_color {
        collect_color3_node(&mut nodes, material_name, "emission_color", value, graph_tier)?;
    }
    if let Some(value) = &material.geometry.normal {
        collect_color3_node(&mut nodes, material_name, "geometry_normal", value, graph_tier)?;
    }
    for node in &nodes {
        write_image_node(writer, node)?;
        if node.factor.is_some() {
            write_multiply_node(writer, node)?;
        }
    }

    let mut surface = BytesStart::new("open_pbr_surface");
    surface.push_attribute(("name", surface_name.as_str()));
    surface.push_attribute(("type", "surfaceshader"));
    writer.write_event(Event::Start(surface)).map_err(xml_export_error)?;
    write_color4_input(
        writer,
        material_name,
        "base_color",
        &material.surface.base_color,
        &nodes,
    )?;
    write_scalar_input(
        writer,
        material_name,
        "base_metalness",
        &material.surface.base_metalness,
        &nodes,
    )?;
    write_scalar_input(
        writer,
        material_name,
        "specular_roughness",
        &material.surface.specular_roughness,
        &nodes,
    )?;
    macro_rules! scalar_input {
        ($field:expr, $name:literal) => {
            if let Some(value) = $field {
                write_scalar_input(writer, material_name, $name, value, &nodes)?;
            }
        };
    }
    scalar_input!(&material.surface.specular_ior, "specular_ior");
    scalar_input!(&material.surface.specular_weight, "specular_weight");
    scalar_input!(&material.surface.specular_anisotropy, "specular_anisotropy");
    scalar_input!(&material.surface.transmission_weight, "transmission_weight");
    scalar_input!(&material.surface.transmission_thickness, "transmission_depth");
    scalar_input!(&material.surface.coat_weight, "coat_weight");
    scalar_input!(&material.surface.coat_roughness, "coat_roughness");
    scalar_input!(&material.surface.fuzz_weight, "fuzz_weight");
    scalar_input!(&material.surface.fuzz_roughness, "fuzz_roughness");
    scalar_input!(&material.surface.subsurface_weight, "subsurface_weight");
    scalar_input!(&material.geometry.height, "geometry_height");
    scalar_input!(&material.geometry.bump, "geometry_bump");
    scalar_input!(opacity.as_ref(), "geometry_opacity");
    scalar_input!(&material.geometry.ambient_occlusion, "ambient_occlusion");
    if let Some(value) = &material.surface.emission_color {
        write_color3_input(writer, material_name, "emission_color", value, &nodes)?;
    }
    if let Some(value) = &material.geometry.normal {
        write_color3_input(writer, material_name, "geometry_normal", value, &nodes)?;
    }
    writer
        .write_event(Event::End(BytesEnd::new("open_pbr_surface")))
        .map_err(xml_export_error)?;

    let mut surface_material = BytesStart::new("surfacematerial");
    surface_material.push_attribute(("name", material_name));
    surface_material.push_attribute(("type", "material"));
    writer
        .write_event(Event::Start(surface_material))
        .map_err(xml_export_error)?;
    let mut input = BytesStart::new("input");
    input.push_attribute(("name", "surfaceshader"));
    input.push_attribute(("type", "surfaceshader"));
    input.push_attribute(("nodename", surface_name.as_str()));
    writer.write_event(Event::Empty(input)).map_err(xml_export_error)?;
    writer
        .write_event(Event::End(BytesEnd::new("surfacematerial")))
        .map_err(xml_export_error)
}

#[derive(Clone, Debug)]
struct TextureNode {
    parameter: &'static str,
    image_name: String,
    multiply_name: String,
    value_type: &'static str,
    source_path: String,
    color_space: &'static str,
    channel: &'static str,
    factor: Option<String>,
}

fn collect_scalar_node(
    nodes: &mut Vec<TextureNode>,
    material_name: &str,
    parameter: &'static str,
    value: &Value<f32>,
    tier: Tier,
) -> Result<(), ExportError> {
    collect_texture_node(
        nodes,
        material_name,
        parameter,
        "float",
        value.texture(),
        tier,
        |value| value.to_string(),
        value,
    )
}

fn collect_color3_node(
    nodes: &mut Vec<TextureNode>,
    material_name: &str,
    parameter: &'static str,
    value: &Value<Color3>,
    tier: Tier,
) -> Result<(), ExportError> {
    collect_texture_node(
        nodes,
        material_name,
        parameter,
        "color3",
        value.texture(),
        tier,
        format_color3,
        value,
    )
}

fn collect_color4_node(
    nodes: &mut Vec<TextureNode>,
    material_name: &str,
    parameter: &'static str,
    value: &Value<Color4>,
    tier: Tier,
) -> Result<(), ExportError> {
    collect_texture_node(
        nodes,
        material_name,
        parameter,
        "color3",
        value.texture(),
        tier,
        |value| format_color3(&[value[0], value[1], value[2]]),
        value,
    )
}

#[allow(clippy::too_many_arguments)]
fn collect_texture_node<T>(
    nodes: &mut Vec<TextureNode>,
    material_name: &str,
    parameter: &'static str,
    value_type: &'static str,
    texture: Option<&TextureRef>,
    tier: Tier,
    format_factor: impl Fn(&T) -> String,
    value: &Value<T>,
) -> Result<(), ExportError> {
    let Some(texture) = texture else {
        return Ok(());
    };
    let source = texture
        .source_for(tier)
        .ok_or_else(|| ExportError::InvalidModel(format!("texture `{parameter}` has no tiers")))?;
    let factor = match value {
        Value::Modulated { factor, .. } => Some(format_factor(factor)),
        Value::Constant { .. } | Value::Texture { .. } => None,
    };
    nodes.push(TextureNode {
        parameter,
        image_name: format!("{material_name}_{parameter}_image"),
        multiply_name: format!("{material_name}_{parameter}_multiply"),
        value_type,
        source_path: source.package_path(),
        color_space: match texture.color_space {
            usd_toolbox_core::ColorSpace::Srgb => "srgb_texture",
            usd_toolbox_core::ColorSpace::Raw => "none",
            usd_toolbox_core::ColorSpace::Linear => "lin_rec709",
        },
        channel: match texture.channel {
            usd_toolbox_core::Channel::Rgb => "rgb",
            usd_toolbox_core::Channel::R => "r",
            usd_toolbox_core::Channel::G => "g",
            usd_toolbox_core::Channel::B => "b",
            usd_toolbox_core::Channel::A => "a",
        },
        factor,
    });
    Ok(())
}

fn write_image_node(writer: &mut Writer<Cursor<Vec<u8>>>, node: &TextureNode) -> Result<(), ExportError> {
    let mut image = BytesStart::new("image");
    image.push_attribute(("name", node.image_name.as_str()));
    image.push_attribute(("type", node.value_type));
    writer.write_event(Event::Start(image)).map_err(xml_export_error)?;
    let mut file = BytesStart::new("input");
    file.push_attribute(("name", "file"));
    file.push_attribute(("type", "filename"));
    file.push_attribute(("value", node.source_path.as_str()));
    file.push_attribute(("colorspace", node.color_space));
    writer.write_event(Event::Empty(file)).map_err(xml_export_error)?;
    let mut channels = BytesStart::new("input");
    channels.push_attribute(("name", "channels"));
    channels.push_attribute(("type", "string"));
    channels.push_attribute(("value", node.channel));
    writer.write_event(Event::Empty(channels)).map_err(xml_export_error)?;
    writer
        .write_event(Event::End(BytesEnd::new("image")))
        .map_err(xml_export_error)
}

fn write_multiply_node(writer: &mut Writer<Cursor<Vec<u8>>>, node: &TextureNode) -> Result<(), ExportError> {
    let mut multiply = BytesStart::new("multiply");
    multiply.push_attribute(("name", node.multiply_name.as_str()));
    multiply.push_attribute(("type", node.value_type));
    writer.write_event(Event::Start(multiply)).map_err(xml_export_error)?;
    let mut input1 = BytesStart::new("input");
    input1.push_attribute(("name", "in1"));
    input1.push_attribute(("type", node.value_type));
    input1.push_attribute(("nodename", node.image_name.as_str()));
    writer.write_event(Event::Empty(input1)).map_err(xml_export_error)?;
    let mut input2 = BytesStart::new("input");
    input2.push_attribute(("name", "in2"));
    input2.push_attribute(("type", node.value_type));
    input2.push_attribute(("value", node.factor.as_deref().unwrap_or("1")));
    writer.write_event(Event::Empty(input2)).map_err(xml_export_error)?;
    writer
        .write_event(Event::End(BytesEnd::new("multiply")))
        .map_err(xml_export_error)
}

fn write_scalar_input(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    material_name: &str,
    parameter: &'static str,
    value: &Value<f32>,
    nodes: &[TextureNode],
) -> Result<(), ExportError> {
    write_input(writer, material_name, parameter, "float", value, nodes, |value| {
        value.to_string()
    })
}

fn write_color3_input(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    material_name: &str,
    parameter: &'static str,
    value: &Value<Color3>,
    nodes: &[TextureNode],
) -> Result<(), ExportError> {
    write_input(writer, material_name, parameter, "color3", value, nodes, format_color3)
}

fn write_color4_input(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    material_name: &str,
    parameter: &'static str,
    value: &Value<Color4>,
    nodes: &[TextureNode],
) -> Result<(), ExportError> {
    write_input(writer, material_name, parameter, "color3", value, nodes, |value| {
        format_color3(&[value[0], value[1], value[2]])
    })
}

fn write_input<T>(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    _material_name: &str,
    parameter: &'static str,
    value_type: &'static str,
    value: &Value<T>,
    nodes: &[TextureNode],
    format_constant: impl Fn(&T) -> String,
) -> Result<(), ExportError> {
    let mut input = BytesStart::new("input");
    input.push_attribute(("name", parameter));
    input.push_attribute(("type", value_type));
    match value {
        Value::Constant { value } => {
            let formatted = format_constant(value);
            input.push_attribute(("value", formatted.as_str()));
            writer.write_event(Event::Empty(input)).map_err(xml_export_error)
        }
        Value::Texture { .. } | Value::Modulated { .. } => {
            let node = nodes
                .iter()
                .find(|node| node.parameter == parameter)
                .expect("texture node was collected");
            let node_name = if node.factor.is_some() {
                &node.multiply_name
            } else {
                &node.image_name
            };
            input.push_attribute(("nodename", node_name.as_str()));
            writer.write_event(Event::Empty(input)).map_err(xml_export_error)
        }
    }
}

fn read_manifest(bytes: &[u8]) -> Result<Option<Vec<Material>>, ImportError> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut in_manifest_set = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) if event.name().into_inner() == "propertyset" => {
                in_manifest_set = attribute(&event, "name")?.as_deref() == Some(MANIFEST_PROPERTY_SET);
            }
            Ok(Event::End(event)) if event.name().into_inner() == "propertyset" => in_manifest_set = false,
            Ok(Event::Empty(event)) if in_manifest_set && event.name().into_inner() == "property" => {
                if attribute(&event, "name")?.as_deref() == Some(MANIFEST_PROPERTY) {
                    let json = attribute(&event, "value")?
                        .ok_or_else(|| ImportError::Missing("MaterialX manifest property value".into()))?;
                    let materials: Vec<Material> =
                        serde_json::from_str(&json).map_err(|error| ImportError::Invalid {
                            format: "MaterialX manifest",
                            detail: error.to_string(),
                        })?;
                    let issues = validate_materials(&materials);
                    if !issues.is_empty() {
                        return Err(ImportError::Invalid {
                            format: "MaterialX manifest",
                            detail: issues
                                .into_iter()
                                .map(|issue| format!("{}: {}", issue.path, issue.detail))
                                .collect::<Vec<_>>()
                                .join("; "),
                        });
                    }
                    validate_import_payload_hashes(&materials)?;
                    return Ok(Some(materials));
                }
            }
            Ok(Event::Eof) => return Ok(None),
            Ok(_) => {}
            Err(error) => {
                return Err(ImportError::Invalid {
                    format: "MaterialX XML",
                    detail: error.to_string(),
                });
            }
        }
    }
}

fn validate_import_payload_hashes(materials: &[Material]) -> Result<(), ImportError> {
    validate_payload_hashes(materials).map_err(|error| ImportError::Invalid {
        format: "MaterialX manifest",
        detail: error.to_string(),
    })
}

fn attribute(event: &BytesStart<'_>, name: &str) -> Result<Option<String>, ImportError> {
    for attribute in event.attributes() {
        let attribute = attribute.map_err(|error| ImportError::Invalid {
            format: "MaterialX XML",
            detail: error.to_string(),
        })?;
        if attribute.key.into_inner() == name {
            return attribute
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map(|value| Some(value.into_owned()))
                .map_err(|error| ImportError::Invalid {
                    format: "MaterialX XML",
                    detail: error.to_string(),
                });
        }
    }
    Ok(None)
}

fn clear_payloads(materials: &mut [Material]) {
    for material in materials {
        material.visit_textures_mut(|_, texture| {
            for source in texture.tiers.values_mut() {
                source.bytes.clear();
            }
        });
        for asset in &mut material.auxiliary {
            asset.bytes.clear();
        }
        for asset in material.provenance.source_assets.values_mut() {
            asset.bytes.clear();
        }
    }
}

fn combined_opacity(material: &Material) -> Option<Value<f32>> {
    let base_alpha = match &material.surface.base_color {
        Value::Constant { value } => value[3],
        Value::Texture { .. } => 1.0,
        Value::Modulated { factor, .. } => factor[3],
    };
    match &material.geometry.opacity {
        Some(Value::Constant { value }) => Some(Value::from(base_alpha * value)),
        Some(Value::Texture { texture }) if base_alpha != 1.0 => Some(Value::Modulated {
            texture: texture.clone(),
            factor: base_alpha,
        }),
        Some(Value::Texture { texture }) => Some(Value::Texture {
            texture: texture.clone(),
        }),
        Some(Value::Modulated { texture, factor }) => Some(Value::Modulated {
            texture: texture.clone(),
            factor: base_alpha * factor,
        }),
        None if base_alpha != 1.0 => Some(Value::from(base_alpha)),
        None => None,
    }
}

fn format_color3(value: &Color3) -> String {
    format!("{}, {}, {}", value[0], value[1], value[2])
}

fn xml_name(value: &str) -> String {
    let mut result: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if result.is_empty() {
        result.push_str("material");
    }
    if result.as_bytes()[0].is_ascii_digit() {
        result.insert(0, '_');
    }
    result
}

fn xml_export_error(error: impl std::fmt::Display) -> ExportError {
    ExportError::Encode {
        format: "MaterialX",
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_names_are_stable() {
        assert_eq!(xml_name("1 / red"), "_1___red");
    }

    #[test]
    fn generic_openpbr_constants_can_be_inspected_without_a_toolbox_manifest() {
        let bytes = br#"<?xml version="1.0"?><materialx version="1.39">
          <open_pbr_surface name="external_surface" type="surfaceshader">
            <input name="base_color" type="color3" value="0.1, 0.2, 0.3"/>
            <input name="specular_roughness" type="float" value="0.7"/>
          </open_pbr_surface>
        </materialx>"#;
        let materials = MaterialXImporter
            .import(
                Input::Bytes(bytes),
                &MaterialXImportOptions {
                    allow_partial_graph: true,
                },
            )
            .unwrap();
        assert_eq!(materials[0].id.0, "external_surface");
        assert_eq!(materials[0].surface.specular_roughness, Value::from(0.7));
    }
}
