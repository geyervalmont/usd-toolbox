//! OpenUSD and USDZ material import/export.
//!
//! USD layers are authored and parsed through the pure-Rust `openusd` crate.
//! USDZ archives use its 64-byte-aligned, uncompressed writer. A compact neutral
//! manifest is embedded as layer metadata and packaged separately so this crate's
//! own round trips preserve every field even when another USD implementation does
//! not understand Olsyn metadata.

use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Read};

use openusd::gf;
use openusd::sdf::{
    self, AbstractData, AssetPath, ChildrenKey, FieldKey, ListOp, Path, SpecType, Specifier, Value as UsdValue,
};
use openusd::tf::Token;
use openusd::usda::TextWriter;
use openusd::usdc::{CrateData, CrateWriter, MAGIC as USDC_MAGIC};
use openusd::usdz::{Archive, ArchiveWriter};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256 as Sha256Hasher};
use usd_toolbox_core::{
    Capabilities, Color3, Color4, Export, ExportError, Exporter, ImportError, Importer, Input, Material,
    ParameterValue, Sha256, ShadingModel, Target, TextureRef, Tier, Value, target_capabilities, validate_materials,
};
use zip::CompressionMethod;

const MANIFEST_VERSION: u32 = 1;
const PROVENANCE_ENTRY: &str = "provenance/provenance.usda";
const ROOT_LAYER_USDA: &str = "material.usda";

/// USD output container.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsdFormat {
    /// Human-readable USDA layer.
    Usda,
    /// Binary crate layer.
    Usdc,
    /// Aligned, uncompressed USDZ package with a USDA default layer.
    #[default]
    Usdz,
}

/// USD export settings.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UsdExportOptions {
    /// Output container.
    pub format: UsdFormat,
    /// Texture tier connected into the authored shader graph. Every tier is
    /// retained in a USDZ package and the closest available tier is selected.
    pub graph_tier: Tier,
    /// Write a tool-friendly provenance mirror as a second USDA layer. JSON is
    /// not an allowed USDZ entry type, so the mirror remains valid USDZ.
    pub include_provenance_mirror: bool,
}

impl Default for UsdExportOptions {
    fn default() -> Self {
        Self {
            format: UsdFormat::Usdz,
            graph_tier: Tier::K2,
            include_provenance_mirror: true,
        }
    }
}

/// Defensive USD/USDZ reader limits.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct UsdImportOptions {
    /// Maximum number of package entries accepted.
    pub max_entries: usize,
    /// Maximum sum of declared uncompressed entry bytes.
    pub max_uncompressed_bytes: u64,
    /// Require allowed USDZ file types, a USD default layer first, uncompressed
    /// entries, and 64-byte-aligned payloads.
    pub verify_usdz_layout: bool,
    /// Recompute every content-addressed payload hash.
    pub verify_content_hashes: bool,
    /// Conservatively inspect standard external shader graphs when no toolbox
    /// neutral manifest is present. Direct constants are imported; unresolved
    /// texture connections are recorded in provenance metadata.
    pub allow_partial_graph: bool,
}

impl Default for UsdImportOptions {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            max_uncompressed_bytes: 2 * 1024 * 1024 * 1024,
            verify_usdz_layout: true,
            verify_content_hashes: true,
            allow_partial_graph: false,
        }
    }
}

/// USD/USDZ exporter.
#[derive(Clone, Copy, Debug, Default)]
pub struct UsdExporter;

/// USD/USDZ importer.
#[derive(Clone, Copy, Debug, Default)]
pub struct UsdImporter;

impl Exporter for UsdExporter {
    type Options = UsdExportOptions;

    fn capabilities(&self) -> Capabilities {
        target_capabilities(Target::Usd)
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

        let manifest = Manifest::from_materials(materials, options.format == UsdFormat::Usdz);
        let manifest_json = serde_json::to_vec(&manifest).map_err(encode_error)?;
        let data = author_layer(materials, &manifest_json, options.graph_tier)?;
        let losses = self.dry_run(materials);
        let bytes = match options.format {
            UsdFormat::Usda => TextWriter::write_to_string(&data)
                .map(String::into_bytes)
                .map_err(|error| encode_error(error.to_string()))?,
            UsdFormat::Usdc => {
                let mut cursor = Cursor::new(Vec::new());
                CrateWriter::write(&data, &mut cursor).map_err(|error| encode_error(error.to_string()))?;
                cursor.into_inner()
            }
            UsdFormat::Usdz => write_usdz(materials, &data, options.include_provenance_mirror)?,
        };
        Ok(Export { bytes, losses })
    }
}

impl Importer for UsdImporter {
    type Options = UsdImportOptions;

    fn import(&self, input: Input<'_>, options: &Self::Options) -> Result<Vec<Material>, ImportError> {
        let bytes = input.single_bytes().ok_or_else(|| ImportError::Invalid {
            format: "USD",
            detail: "USD input must be a single layer or USDZ byte buffer".into(),
        })?;
        if bytes.starts_with(b"PK\x03\x04") {
            import_usdz(bytes, options)
        } else {
            import_layer(bytes, options)
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Manifest {
    schema_version: u32,
    materials: Vec<Material>,
}

impl Manifest {
    fn from_materials(materials: &[Material], for_usdz: bool) -> Self {
        let mut materials = materials.to_vec();
        if for_usdz {
            for material in &mut materials {
                material.visit_textures_mut(|_, texture| {
                    for source in texture.tiers.values_mut() {
                        source.bytes.clear();
                    }
                });
                for asset in &mut material.auxiliary {
                    if is_usdz_allowed_extension(path_extension(&asset.name)) {
                        asset.bytes.clear();
                    }
                }
                for asset in material.provenance.source_assets.values_mut() {
                    if is_usdz_allowed_extension(&asset.extension) {
                        asset.bytes.clear();
                    }
                }
            }
        }
        Self {
            schema_version: MANIFEST_VERSION,
            materials,
        }
    }
}

fn author_layer(materials: &[Material], manifest_json: &[u8], graph_tier: Tier) -> Result<sdf::Data, ExportError> {
    let manifest_text = std::str::from_utf8(manifest_json).expect("JSON is UTF-8");
    let manifest_hash = digest(manifest_json);
    let mut data = sdf::Data::new();
    let root = Path::abs_root();
    let root_spec = data.create_spec(root, SpecType::PseudoRoot);
    root_spec.add(FieldKey::DefaultPrim, UsdValue::Token("Materials".into()));
    root_spec.add("metersPerUnit", UsdValue::Double(1.0));
    root_spec.add("upAxis", UsdValue::Token("Y".into()));
    root_spec.add(ChildrenKey::PrimChildren, UsdValue::TokenVec(vec!["Materials".into()]));
    let mut layer_data = HashMap::from([
        ("olsyn_builder".into(), UsdValue::String("usd-toolbox".into())),
        (
            "olsyn_builderVersion".into(),
            UsdValue::String(env!("CARGO_PKG_VERSION").into()),
        ),
        ("olsyn_manifestSha256".into(), UsdValue::String(manifest_hash.0)),
        ("olsyn_manifestJson".into(), UsdValue::String(manifest_text.into())),
    ]);
    if materials.len() == 1 {
        let provenance = &materials[0].provenance;
        if let Some(created) = provenance.built_at {
            layer_data.insert("olsyn_buildTime".into(), UsdValue::String(created.to_string()));
        }
        if let Some(source) = &provenance.source_identifier {
            layer_data.insert("olsyn_sourceIdentifier".into(), UsdValue::String(source.clone()));
        }
    }
    data.spec_mut(&Path::abs_root())
        .expect("pseudo-root exists")
        .add(FieldKey::CustomLayerData, UsdValue::Dictionary(layer_data));

    let materials_path = path("/Materials")?;
    let materials_spec = data.create_spec(materials_path.clone(), SpecType::Prim);
    materials_spec.add(FieldKey::Specifier, UsdValue::Specifier(Specifier::Def));
    materials_spec.add(FieldKey::TypeName, UsdValue::Token("Scope".into()));

    let mut used_names = BTreeMap::<String, usize>::new();
    let mut material_names = Vec::new();
    for material in materials {
        let base = usd_identifier(&material.id.0);
        let suffix = used_names.entry(base.clone()).or_default();
        let prim_name = if *suffix == 0 { base } else { format!("{base}_{suffix}") };
        *suffix += 1;
        material_names.push(Token::from(prim_name.clone()));
        author_material(&mut data, &materials_path, &prim_name, material, graph_tier)?;
    }
    data.spec_mut(&materials_path)
        .expect("materials scope exists")
        .add(ChildrenKey::PrimChildren, UsdValue::TokenVec(material_names));
    Ok(data)
}

fn author_material(
    data: &mut sdf::Data,
    parent: &Path,
    prim_name: &str,
    material: &Material,
    graph_tier: Tier,
) -> Result<(), ExportError> {
    let material_path = parent.append_path(prim_name).map_err(path_error)?;
    let mut children = vec![Token::new("OpenPBR")];
    let mut properties = vec![Token::new("outputs:surface")];
    let mut asset_info = HashMap::from([
        ("identifier".into(), UsdValue::String(material.id.0.clone())),
        ("name".into(), UsdValue::String(material.name.clone())),
    ]);
    if let Some(source) = &material.provenance.supplier_source {
        asset_info.insert("olsyn_supplierSource".into(), UsdValue::String(source.clone()));
    }
    if let Some(version) = &material.provenance.version {
        asset_info.insert("version".into(), UsdValue::String(version.clone()));
    }
    let material_spec = data.create_spec(material_path.clone(), SpecType::Prim);
    material_spec.add(FieldKey::Specifier, UsdValue::Specifier(Specifier::Def));
    material_spec.add(FieldKey::TypeName, UsdValue::Token("Material".into()));
    material_spec.add(FieldKey::AssetInfo, UsdValue::Dictionary(asset_info));
    material_spec.add(
        FieldKey::CustomData,
        UsdValue::Dictionary(HashMap::from([
            (
                "olsyn_tiling".into(),
                UsdValue::String(serde_json::to_string(&material.tiling).map_err(encode_error)?),
            ),
            (
                "olsyn_provenance".into(),
                UsdValue::String(serde_json::to_string(&material.provenance).map_err(encode_error)?),
            ),
        ])),
    );

    let shader_path = material_path.append_path("OpenPBR").map_err(path_error)?;
    let mut shader_properties = vec![Token::new("info:id"), Token::new("outputs:out")];
    create_attribute(
        data,
        &shader_path,
        "info:id",
        "token",
        Some(UsdValue::Token("ND_open_pbr_surface_surfaceshader".into())),
        None,
        None,
    )?;
    create_attribute(data, &shader_path, "outputs:out", "token", None, None, None)?;

    author_color4(
        data,
        &material_path,
        &shader_path,
        "base_color",
        &material.surface.base_color,
        graph_tier,
        &mut children,
        &mut shader_properties,
    )?;
    author_scalar(
        data,
        &material_path,
        &shader_path,
        "base_metalness",
        &material.surface.base_metalness,
        graph_tier,
        &mut children,
        &mut shader_properties,
    )?;
    author_scalar(
        data,
        &material_path,
        &shader_path,
        "specular_roughness",
        &material.surface.specular_roughness,
        graph_tier,
        &mut children,
        &mut shader_properties,
    )?;
    macro_rules! scalar {
        ($field:expr, $name:literal) => {
            if let Some(value) = $field {
                author_scalar(
                    data,
                    &material_path,
                    &shader_path,
                    $name,
                    value,
                    graph_tier,
                    &mut children,
                    &mut shader_properties,
                )?;
            }
        };
    }
    scalar!(&material.surface.specular_ior, "specular_ior");
    scalar!(&material.surface.specular_weight, "specular_weight");
    scalar!(&material.surface.specular_anisotropy, "specular_anisotropy");
    scalar!(&material.surface.transmission_weight, "transmission_weight");
    scalar!(&material.surface.transmission_thickness, "transmission_depth");
    scalar!(&material.surface.coat_weight, "coat_weight");
    scalar!(&material.surface.coat_roughness, "coat_roughness");
    scalar!(&material.surface.fuzz_weight, "fuzz_weight");
    scalar!(&material.surface.fuzz_roughness, "fuzz_roughness");
    scalar!(&material.surface.subsurface_weight, "subsurface_weight");
    scalar!(&material.geometry.height, "geometry_height");
    scalar!(&material.geometry.bump, "geometry_bump");
    let opacity = combined_opacity(material);
    scalar!(opacity.as_ref(), "geometry_opacity");
    scalar!(&material.geometry.ambient_occlusion, "ambient_occlusion");
    if let Some(value) = &material.surface.emission_color {
        author_color3(
            data,
            &material_path,
            &shader_path,
            "emission_color",
            value,
            graph_tier,
            &mut children,
            &mut shader_properties,
        )?;
    }
    if let Some(value) = &material.geometry.normal {
        author_color3(
            data,
            &material_path,
            &shader_path,
            "geometry_normal",
            value,
            graph_tier,
            &mut children,
            &mut shader_properties,
        )?;
    }

    let shader_spec = data.create_spec(shader_path.clone(), SpecType::Prim);
    shader_spec.add(FieldKey::Specifier, UsdValue::Specifier(Specifier::Def));
    shader_spec.add(FieldKey::TypeName, UsdValue::Token("Shader".into()));
    shader_spec.add(ChildrenKey::PropertyChildren, UsdValue::TokenVec(shader_properties));

    let shader_output = shader_path.append_property("outputs:out").map_err(path_error)?;
    create_attribute(
        data,
        &material_path,
        "outputs:surface",
        "token",
        None,
        Some(shader_output),
        None,
    )?;

    author_variants(data, &material_path, material, graph_tier)?;
    let material_spec = data.spec_mut(&material_path).expect("material prim exists");
    material_spec.add(ChildrenKey::PrimChildren, UsdValue::TokenVec(children));
    material_spec.add(
        ChildrenKey::PropertyChildren,
        UsdValue::TokenVec(std::mem::take(&mut properties)),
    );
    Ok(())
}

fn author_variants(
    data: &mut sdf::Data,
    material_path: &Path,
    material: &Material,
    graph_tier: Tier,
) -> Result<(), ExportError> {
    if material.variants.is_empty() {
        return Ok(());
    }
    let set_names: Vec<_> = material
        .variants
        .iter()
        .map(|set| Token::from(usd_identifier(&set.name)))
        .collect();
    let selections: HashMap<_, _> = material
        .variants
        .iter()
        .map(|set| (usd_identifier(&set.name), usd_identifier(&set.default)))
        .collect();
    let material_spec = data.spec_mut(material_path).expect("material prim exists");
    material_spec.add(
        FieldKey::VariantSetNames,
        UsdValue::TokenListOp(ListOp::prepended(set_names.clone())),
    );
    material_spec.add(FieldKey::VariantSelection, UsdValue::VariantSelectionMap(selections));
    material_spec.add(ChildrenKey::VariantSetChildren, UsdValue::TokenVec(set_names));

    for set in &material.variants {
        let set_name = usd_identifier(&set.name);
        let set_path = path(&format!("{}{{{set_name}=}}", material_path.as_str()))?;
        let variants: Vec<_> = set
            .variants
            .iter()
            .map(|variant| Token::from(usd_identifier(&variant.name)))
            .collect();
        data.create_spec(set_path, SpecType::VariantSet)
            .add(ChildrenKey::VariantChildren, UsdValue::TokenVec(variants));
        for variant in &set.variants {
            let variant_name = usd_identifier(&variant.name);
            let variant_path = path(&format!("{}{{{set_name}={variant_name}}}", material_path.as_str()))?;
            data.create_spec(variant_path.clone(), SpecType::Variant).add(
                FieldKey::CustomData,
                UsdValue::Dictionary(HashMap::from([(
                    "olsyn_overrides".into(),
                    UsdValue::String(serde_json::to_string(&variant.overrides).map_err(encode_error)?),
                )])),
            );
            author_variant_shader(data, &variant_path, &variant.overrides, graph_tier)?;
        }
    }
    Ok(())
}

fn author_variant_shader(
    data: &mut sdf::Data,
    variant_path: &Path,
    overrides: &BTreeMap<String, ParameterValue>,
    graph_tier: Tier,
) -> Result<(), ExportError> {
    let shader_path = variant_path.append_path("OpenPBR").map_err(path_error)?;
    let shader_spec = data.create_spec(shader_path.clone(), SpecType::Prim);
    shader_spec.add(FieldKey::Specifier, UsdValue::Specifier(Specifier::Over));

    let mut children = vec![Token::new("OpenPBR")];
    let mut properties = Vec::new();
    for (parameter, value) in overrides {
        match (parameter.as_str(), value) {
            ("base_color", ParameterValue::Color4(value)) => author_color4(
                data,
                variant_path,
                &shader_path,
                "base_color",
                value,
                graph_tier,
                &mut children,
                &mut properties,
            )?,
            ("emission_color", ParameterValue::Color3(value)) => author_color3(
                data,
                variant_path,
                &shader_path,
                "emission_color",
                value,
                graph_tier,
                &mut children,
                &mut properties,
            )?,
            ("geometry_normal", ParameterValue::Color3(value)) => author_color3(
                data,
                variant_path,
                &shader_path,
                "geometry_normal",
                value,
                graph_tier,
                &mut children,
                &mut properties,
            )?,
            (_, ParameterValue::Scalar(value)) => {
                if let Some(shader_parameter) = variant_scalar_name(parameter) {
                    author_scalar(
                        data,
                        variant_path,
                        &shader_path,
                        shader_parameter,
                        value,
                        graph_tier,
                        &mut children,
                        &mut properties,
                    )?;
                }
            }
            (_, ParameterValue::Color3(_) | ParameterValue::Color4(_) | ParameterValue::Tiling(_)) => {}
        }
    }

    data.spec_mut(&shader_path)
        .expect("variant shader override exists")
        .add(ChildrenKey::PropertyChildren, UsdValue::TokenVec(properties));
    data.spec_mut(variant_path)
        .expect("variant exists")
        .add(ChildrenKey::PrimChildren, UsdValue::TokenVec(children));
    Ok(())
}

const fn variant_scalar_name(parameter: &str) -> Option<&'static str> {
    match parameter.as_bytes() {
        b"base_metalness" => Some("base_metalness"),
        b"specular_roughness" => Some("specular_roughness"),
        b"specular_ior" => Some("specular_ior"),
        b"specular_weight" => Some("specular_weight"),
        b"specular_anisotropy" => Some("specular_anisotropy"),
        b"transmission_weight" => Some("transmission_weight"),
        b"transmission_thickness" => Some("transmission_depth"),
        b"coat_weight" => Some("coat_weight"),
        b"coat_roughness" => Some("coat_roughness"),
        b"fuzz_weight" => Some("fuzz_weight"),
        b"fuzz_roughness" => Some("fuzz_roughness"),
        b"subsurface_weight" => Some("subsurface_weight"),
        b"geometry_height" => Some("geometry_height"),
        b"geometry_bump" => Some("geometry_bump"),
        b"geometry_opacity" => Some("geometry_opacity"),
        b"ambient_occlusion" => Some("ambient_occlusion"),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn author_scalar(
    data: &mut sdf::Data,
    material_path: &Path,
    shader_path: &Path,
    name: &'static str,
    value: &Value<f32>,
    graph_tier: Tier,
    children: &mut Vec<Token>,
    properties: &mut Vec<Token>,
) -> Result<(), ExportError> {
    let (default, connection, custom_data) = match value {
        Value::Constant { value } => (Some(UsdValue::Float(*value)), None, None),
        Value::Texture { texture } => (
            None,
            Some(author_texture_node(
                data,
                material_path,
                name,
                "float",
                texture,
                graph_tier,
                children,
            )?),
            None,
        ),
        Value::Modulated { texture, factor } => (
            Some(UsdValue::Float(*factor)),
            Some(author_texture_node(
                data,
                material_path,
                name,
                "float",
                texture,
                graph_tier,
                children,
            )?),
            Some(HashMap::from([("olsyn_factor".into(), UsdValue::Float(*factor))])),
        ),
    };
    let property = format!("inputs:{name}");
    create_attribute(data, shader_path, &property, "float", default, connection, custom_data)?;
    properties.push(Token::from(property));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn author_color3(
    data: &mut sdf::Data,
    material_path: &Path,
    shader_path: &Path,
    name: &'static str,
    value: &Value<Color3>,
    graph_tier: Tier,
    children: &mut Vec<Token>,
    properties: &mut Vec<Token>,
) -> Result<(), ExportError> {
    let (default, connection, custom_data) = match value {
        Value::Constant { value } => (
            Some(UsdValue::Vec3f(gf::vec3f(value[0], value[1], value[2]))),
            None,
            None,
        ),
        Value::Texture { texture } => (
            None,
            Some(author_texture_node(
                data,
                material_path,
                name,
                "color3f",
                texture,
                graph_tier,
                children,
            )?),
            None,
        ),
        Value::Modulated { texture, factor } => (
            Some(UsdValue::Vec3f(gf::vec3f(factor[0], factor[1], factor[2]))),
            Some(author_texture_node(
                data,
                material_path,
                name,
                "color3f",
                texture,
                graph_tier,
                children,
            )?),
            Some(HashMap::from([(
                "olsyn_factor".into(),
                UsdValue::Vec3f(gf::vec3f(factor[0], factor[1], factor[2])),
            )])),
        ),
    };
    let property = format!("inputs:{name}");
    create_attribute(
        data,
        shader_path,
        &property,
        "color3f",
        default,
        connection,
        custom_data,
    )?;
    properties.push(Token::from(property));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn author_color4(
    data: &mut sdf::Data,
    material_path: &Path,
    shader_path: &Path,
    name: &'static str,
    value: &Value<Color4>,
    graph_tier: Tier,
    children: &mut Vec<Token>,
    properties: &mut Vec<Token>,
) -> Result<(), ExportError> {
    let (default, connection, custom_data) = match value {
        Value::Constant { value } => (
            Some(UsdValue::Vec3f(gf::vec3f(value[0], value[1], value[2]))),
            None,
            Some(HashMap::from([("olsyn_alpha".into(), UsdValue::Float(value[3]))])),
        ),
        Value::Texture { texture } => (
            None,
            Some(author_texture_node(
                data,
                material_path,
                name,
                "color3f",
                texture,
                graph_tier,
                children,
            )?),
            None,
        ),
        Value::Modulated { texture, factor } => (
            Some(UsdValue::Vec3f(gf::vec3f(factor[0], factor[1], factor[2]))),
            Some(author_texture_node(
                data,
                material_path,
                name,
                "color3f",
                texture,
                graph_tier,
                children,
            )?),
            Some(HashMap::from([
                (
                    "olsyn_factor".into(),
                    UsdValue::Vec3f(gf::vec3f(factor[0], factor[1], factor[2])),
                ),
                ("olsyn_alpha".into(), UsdValue::Float(factor[3])),
            ])),
        ),
    };
    let property = format!("inputs:{name}");
    create_attribute(
        data,
        shader_path,
        &property,
        "color3f",
        default,
        connection,
        custom_data,
    )?;
    properties.push(Token::from(property));
    Ok(())
}

fn author_texture_node(
    data: &mut sdf::Data,
    material_path: &Path,
    parameter: &str,
    output_type: &str,
    texture: &TextureRef,
    tier: Tier,
    children: &mut Vec<Token>,
) -> Result<Path, ExportError> {
    let source = texture
        .source_for(tier)
        .ok_or_else(|| ExportError::InvalidModel(format!("texture `{parameter}` has no source tiers")))?;
    let node_name = format!("{}_texture", usd_identifier(parameter));
    children.push(Token::from(node_name.clone()));
    let node_path = material_path.append_path(&node_name).map_err(path_error)?;
    let properties = vec![
        Token::new("info:id"),
        Token::new("inputs:file"),
        Token::new("inputs:colorspace"),
        Token::new("inputs:channel"),
        Token::new("outputs:out"),
    ];
    let spec = data.create_spec(node_path.clone(), SpecType::Prim);
    spec.add(FieldKey::Specifier, UsdValue::Specifier(Specifier::Def));
    spec.add(FieldKey::TypeName, UsdValue::Token("Shader".into()));
    spec.add(ChildrenKey::PropertyChildren, UsdValue::TokenVec(properties));
    create_attribute(
        data,
        &node_path,
        "info:id",
        "token",
        Some(UsdValue::Token(if output_type == "float" {
            "ND_image_float".into()
        } else {
            "ND_image_color3".into()
        })),
        None,
        None,
    )?;
    create_attribute(
        data,
        &node_path,
        "inputs:file",
        "asset",
        Some(UsdValue::AssetPath(AssetPath::new(format!(
            "./{}",
            source.package_path()
        )))),
        None,
        Some(HashMap::from([
            ("olsyn_sha256".into(), UsdValue::String(source.hash.0.clone())),
            ("olsyn_role".into(), UsdValue::Token(texture.role.as_str().into())),
            ("olsyn_tier".into(), UsdValue::Token(tier.as_str().into())),
        ])),
    )?;
    create_attribute(
        data,
        &node_path,
        "inputs:colorspace",
        "token",
        Some(UsdValue::Token(
            format!("{:?}", texture.color_space).to_ascii_lowercase().into(),
        )),
        None,
        None,
    )?;
    create_attribute(
        data,
        &node_path,
        "inputs:channel",
        "token",
        Some(UsdValue::Token(
            format!("{:?}", texture.channel).to_ascii_lowercase().into(),
        )),
        None,
        None,
    )?;
    create_attribute(data, &node_path, "outputs:out", output_type, None, None, None)?;
    node_path.append_property("outputs:out").map_err(path_error)
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

fn create_attribute(
    data: &mut sdf::Data,
    prim_path: &Path,
    name: &str,
    type_name: &str,
    default: Option<UsdValue>,
    connection: Option<Path>,
    custom_data: Option<HashMap<String, UsdValue>>,
) -> Result<(), ExportError> {
    let property_path = prim_path.append_property(name).map_err(path_error)?;
    let spec = data.create_spec(property_path, SpecType::Attribute);
    spec.add(FieldKey::TypeName, UsdValue::Token(type_name.into()));
    if let Some(default) = default {
        spec.add(FieldKey::Default, default);
    }
    if let Some(connection) = connection {
        spec.add(
            FieldKey::ConnectionPaths,
            UsdValue::PathListOp(ListOp::explicit([connection])),
        );
    }
    if let Some(custom_data) = custom_data {
        spec.add(FieldKey::AssetInfo, UsdValue::Dictionary(custom_data));
    }
    Ok(())
}

fn write_usdz(
    materials: &[Material],
    data: &sdf::Data,
    include_provenance_mirror: bool,
) -> Result<Vec<u8>, ExportError> {
    let stage = TextWriter::write_to_string(data).map_err(|error| encode_error(error.to_string()))?;
    let mut entries = BTreeMap::<String, Vec<u8>>::new();
    for material in materials {
        let mut unsupported_texture = None;
        material.visit_textures(|parameter, texture| {
            if unsupported_texture.is_none() {
                unsupported_texture = texture
                    .tiers
                    .values()
                    .find(|source| !is_usdz_allowed_extension(&source.extension))
                    .map(|source| (parameter, source.extension.clone()));
            }
        });
        if let Some((parameter, extension)) = unsupported_texture {
            return Err(ExportError::Unsupported(format!(
                "USDZ cannot contain `{extension}` texture data for `{parameter}`"
            )));
        }
        material.visit_textures(|_, texture| {
            for source in texture.tiers.values() {
                entries
                    .entry(source.package_path())
                    .or_insert_with(|| source.bytes.clone());
            }
        });
        for asset in &material.auxiliary {
            if is_usdz_allowed_extension(path_extension(&asset.name)) {
                entries
                    .entry(auxiliary_path(&asset.hash, &asset.name))
                    .or_insert_with(|| asset.bytes.clone());
            }
        }
        for (hash, asset) in &material.provenance.source_assets {
            if is_usdz_allowed_extension(&asset.extension) {
                entries
                    .entry(asset.package_path(hash))
                    .or_insert_with(|| asset.bytes.clone());
            }
        }
    }
    if include_provenance_mirror {
        entries.insert(PROVENANCE_ENTRY.into(), provenance_mirror(materials)?);
    }

    let cursor = Cursor::new(Vec::new());
    let mut archive = ArchiveWriter::new(cursor);
    archive
        .add_layer(ROOT_LAYER_USDA, stage.as_bytes())
        .map_err(|error| encode_error(error.to_string()))?;
    for (name, bytes) in entries {
        archive
            .add_layer(&name, &bytes)
            .map_err(|error| encode_error(error.to_string()))?;
    }
    archive
        .finish()
        .map(Cursor::into_inner)
        .map_err(|error| encode_error(error.to_string()))
}

fn import_usdz(bytes: &[u8], options: &UsdImportOptions) -> Result<Vec<Material>, ImportError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| invalid_usd(error.to_string()))?;
    if archive.len() > options.max_entries {
        return Err(invalid_usd(format!(
            "package has {} entries, exceeding limit {}",
            archive.len(),
            options.max_entries
        )));
    }
    let mut total = 0_u64;
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|error| invalid_usd(error.to_string()))?;
        let name = file.name().to_owned();
        validate_entry_name(&name)?;
        total = total
            .checked_add(file.size())
            .ok_or_else(|| invalid_usd("declared package size overflow"))?;
        if total > options.max_uncompressed_bytes {
            return Err(invalid_usd(format!(
                "package expands to more than {} bytes",
                options.max_uncompressed_bytes
            )));
        }
        if options.verify_usdz_layout {
            if !is_usdz_allowed_extension(path_extension(&name)) {
                return Err(invalid_usd(format!(
                    "entry `{name}` has a file type USDZ does not allow"
                )));
            }
            if index == 0 && !is_usd_layer_extension(path_extension(&name)) {
                return Err(invalid_usd(format!("first entry `{name}` is not a USD layer")));
            }
            if file.compression() != CompressionMethod::Stored {
                return Err(invalid_usd(format!("entry `{name}` is compressed")));
            }
            let data_start = file
                .data_start()
                .ok_or_else(|| invalid_usd(format!("entry `{name}` has no data offset")))?;
            if data_start % 64 != 0 {
                return Err(invalid_usd(format!("entry `{name}` is not aligned to 64 bytes")));
            }
        }
        let mut contents = Vec::with_capacity(usize::try_from(file.size()).unwrap_or(0));
        file.read_to_end(&mut contents)
            .map_err(|error| invalid_usd(format!("could not read `{name}`: {error}")))?;
        if entries.insert(name.clone(), contents).is_some() {
            return Err(invalid_usd(format!("duplicate package entry `{name}`")));
        }
    }

    let mut usd_archive = Archive::from_reader(Cursor::new(bytes)).map_err(|error| invalid_usd(error.to_string()))?;
    let root_layer = usd_archive
        .read_first_layer()
        .map_err(|error| invalid_usd(format!("default layer is invalid: {error}")))?;
    match manifest_from_layer(root_layer.as_ref()) {
        Ok(materials) => rehydrate(materials, &entries, options.verify_content_hashes),
        Err(ImportError::Missing(_)) if options.allow_partial_graph => {
            generic_materials_from_layer(root_layer.as_ref())
        }
        Err(error) => Err(error),
    }
}

fn provenance_mirror(materials: &[Material]) -> Result<Vec<u8>, ExportError> {
    let mirror: Vec<_> = materials
        .iter()
        .map(|material| {
            let mut provenance = material.provenance.clone();
            for asset in provenance.source_assets.values_mut() {
                asset.bytes.clear();
            }
            (&material.id, provenance)
        })
        .collect();
    let json = serde_json::to_string(&mirror).map_err(encode_error)?;
    let mut data = sdf::Data::new();
    data.create_spec(Path::abs_root(), SpecType::PseudoRoot).add(
        FieldKey::CustomLayerData,
        UsdValue::Dictionary(HashMap::from([("olsyn_provenance".into(), UsdValue::String(json))])),
    );
    TextWriter::write_to_string(&data)
        .map(String::into_bytes)
        .map_err(|error| encode_error(error.to_string()))
}

fn import_layer(bytes: &[u8], options: &UsdImportOptions) -> Result<Vec<Material>, ImportError> {
    let result = if bytes.starts_with(USDC_MAGIC) {
        let data = CrateData::open(Cursor::new(bytes), true).map_err(|error| invalid_usd(error.to_string()))?;
        manifest_from_layer(&data).or_else(|error| match error {
            ImportError::Missing(_) if options.allow_partial_graph => generic_materials_from_layer(&data),
            error => Err(error),
        })
    } else {
        let text = std::str::from_utf8(bytes).map_err(|error| invalid_usd(error.to_string()))?;
        let data = openusd::usda::parse(text).map_err(|error| invalid_usd(error.to_string()))?;
        manifest_from_layer(&data).or_else(|error| match error {
            ImportError::Missing(_) if options.allow_partial_graph => generic_materials_from_layer(&data),
            error => Err(error),
        })
    };
    let materials = result?;
    validate_imported_materials(&materials)?;
    Ok(materials)
}

fn manifest_from_layer(data: &dyn AbstractData) -> Result<Vec<Material>, ImportError> {
    let Some(value) = data
        .try_field(&Path::abs_root(), FieldKey::CustomLayerData.as_str())
        .map_err(|error| invalid_usd(error.to_string()))?
    else {
        return Err(ImportError::Missing("customLayerData".into()));
    };
    let UsdValue::Dictionary(dictionary) = value.as_ref() else {
        return Err(ImportError::Missing("customLayerData".into()));
    };
    let Some(UsdValue::String(json)) = dictionary.get("olsyn_manifestJson") else {
        return Err(ImportError::Missing("customLayerData[olsyn_manifestJson]".into()));
    };
    let manifest: Manifest = serde_json::from_str(json).map_err(|error| invalid_usd(error.to_string()))?;
    if manifest.schema_version != MANIFEST_VERSION {
        return Err(ImportError::Unsupported(format!(
            "manifest schema version {}",
            manifest.schema_version
        )));
    }
    Ok(manifest.materials)
}

/// Conservative inspection path for third-party USD Preview Surface and
/// OpenPBR graphs. Only direct authored constants are imported. Connections
/// remain explicit unresolved metadata until their asset bytes and color
/// interpretation can be proven.
fn generic_materials_from_layer(data: &dyn AbstractData) -> Result<Vec<Material>, ImportError> {
    let paths = data.spec_paths();
    let material_paths = paths
        .iter()
        .filter(|path| {
            data.spec_type(path) == Some(SpecType::Prim)
                && token_field(data, path, FieldKey::TypeName.as_str()).as_deref() == Some("Material")
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut materials = Vec::new();
    for material_path in material_paths {
        let mut id = material_path
            .as_str()
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("material")
            .to_owned();
        let mut name = id.clone();
        if let Some(UsdValue::Dictionary(asset_info)) = owned_field(data, &material_path, FieldKey::AssetInfo.as_str())
        {
            if let Some(UsdValue::String(identifier)) = asset_info.get("identifier") {
                id.clone_from(identifier);
            }
            if let Some(UsdValue::String(display_name)) = asset_info.get("name") {
                name.clone_from(display_name);
            }
        }
        let prefix = format!("{}/", material_path.as_str().trim_end_matches('/'));
        let shader = paths.iter().find_map(|candidate| {
            if data.spec_type(candidate) != Some(SpecType::Prim)
                || !candidate.as_str().starts_with(&prefix)
                || token_field(data, candidate, FieldKey::TypeName.as_str()).as_deref() != Some("Shader")
            {
                return None;
            }
            let info_path = candidate.append_property("info:id").ok()?;
            let shader_id = token_field(data, &info_path, FieldKey::Default.as_str())?;
            matches!(
                shader_id.as_str(),
                "UsdPreviewSurface" | "ND_open_pbr_surface_surfaceshader" | "OpenPBRSurface"
            )
            .then(|| (candidate.clone(), shader_id))
        });
        let Some((shader_path, shader_id)) = shader else {
            continue;
        };
        let mut material = Material::new(id, name);
        material.model = if shader_id == "UsdPreviewSurface" {
            ShadingModel::GltfPbr
        } else {
            ShadingModel::OpenPbr
        };
        material.provenance.builder = Some("usd-toolbox USD partial importer".into());
        material
            .provenance
            .metadata
            .insert("partial_graph_import".into(), serde_json::Value::Bool(true));
        let property_prefix = format!("{}.", shader_path.as_str());
        let mut unresolved = Vec::new();
        for property_path in paths.iter().filter(|path| path.as_str().starts_with(&property_prefix)) {
            let property_name = property_path
                .as_str()
                .rsplit('.')
                .next()
                .unwrap_or_default()
                .trim_start_matches("inputs:");
            if data.has_field(property_path, FieldKey::ConnectionPaths.as_str()) {
                unresolved.push(property_name.to_owned());
                continue;
            }
            if let Some(value) = owned_field(data, property_path, FieldKey::Default.as_str()) {
                assign_usd_constant(&mut material, property_name, value);
            }
        }
        if !unresolved.is_empty() {
            unresolved.sort();
            unresolved.dedup();
            material
                .provenance
                .metadata
                .insert("unresolved_connections".into(), serde_json::json!(unresolved));
        }
        materials.push(material);
    }
    if materials.is_empty() {
        return Err(ImportError::Unsupported(
            "no UsdPreviewSurface or OpenPBR material graph was found".into(),
        ));
    }
    validate_imported_materials(&materials)?;
    Ok(materials)
}

fn owned_field(data: &dyn AbstractData, path: &Path, field: &str) -> Option<UsdValue> {
    data.try_field(path, field)
        .ok()
        .flatten()
        .map(|value| value.into_owned())
}

fn token_field(data: &dyn AbstractData, path: &Path, field: &str) -> Option<String> {
    match owned_field(data, path, field)? {
        UsdValue::Token(value) => Some(value.as_str().to_owned()),
        UsdValue::String(value) => Some(value),
        _ => None,
    }
}

fn assign_usd_constant(material: &mut Material, name: &str, value: UsdValue) {
    let scalar = match &value {
        UsdValue::Float(value) => Some(*value),
        UsdValue::Double(value) => Some(*value as f32),
        UsdValue::Int(value) => Some(*value as f32),
        _ => None,
    };
    let color = match value {
        UsdValue::Vec3f(value) => Some([value.x, value.y, value.z]),
        UsdValue::Vec3d(value) => Some([value.x as f32, value.y as f32, value.z as f32]),
        UsdValue::Vec4f(value) => Some([value.x, value.y, value.z]),
        UsdValue::Vec4d(value) => Some([value.x as f32, value.y as f32, value.z as f32]),
        _ => None,
    };
    match name {
        "diffuseColor" | "base_color" => {
            if let Some(color) = color {
                material.surface.base_color = Value::from([color[0], color[1], color[2], 1.0]);
            }
        }
        "metallic" | "base_metalness" => set_scalar(&mut material.surface.base_metalness, scalar),
        "roughness" | "specular_roughness" => set_scalar(&mut material.surface.specular_roughness, scalar),
        "ior" | "specular_ior" => set_optional_scalar(&mut material.surface.specular_ior, scalar),
        "specular_weight" => set_optional_scalar(&mut material.surface.specular_weight, scalar),
        "specular_anisotropy" => set_optional_scalar(&mut material.surface.specular_anisotropy, scalar),
        "transmission_weight" => set_optional_scalar(&mut material.surface.transmission_weight, scalar),
        "transmission_depth" => set_optional_scalar(&mut material.surface.transmission_thickness, scalar),
        "coat_weight" => set_optional_scalar(&mut material.surface.coat_weight, scalar),
        "coat_roughness" => set_optional_scalar(&mut material.surface.coat_roughness, scalar),
        "fuzz_weight" => set_optional_scalar(&mut material.surface.fuzz_weight, scalar),
        "fuzz_roughness" => set_optional_scalar(&mut material.surface.fuzz_roughness, scalar),
        "subsurface_weight" => set_optional_scalar(&mut material.surface.subsurface_weight, scalar),
        "opacity" | "geometry_opacity" => set_optional_scalar(&mut material.geometry.opacity, scalar),
        "ambient_occlusion" => set_optional_scalar(&mut material.geometry.ambient_occlusion, scalar),
        "geometry_height" => set_optional_scalar(&mut material.geometry.height, scalar),
        "geometry_bump" => set_optional_scalar(&mut material.geometry.bump, scalar),
        "emissiveColor" | "emission_color" => {
            if let Some(color) = color {
                material.surface.emission_color = Some(Value::from(color));
            }
        }
        _ => {}
    }
}

fn set_scalar(target: &mut Value<f32>, value: Option<f32>) {
    if let Some(value) = value {
        *target = Value::from(value);
    }
}

fn set_optional_scalar(target: &mut Option<Value<f32>>, value: Option<f32>) {
    if let Some(value) = value {
        *target = Some(Value::from(value));
    }
}

fn rehydrate(
    mut materials: Vec<Material>,
    entries: &BTreeMap<String, Vec<u8>>,
    verify_hashes: bool,
) -> Result<Vec<Material>, ImportError> {
    for material in &mut materials {
        let mut error = None;
        material.visit_textures_mut(|parameter, texture| {
            if error.is_some() {
                return;
            }
            for source in texture.tiers.values_mut() {
                match entries.get(&source.package_path()) {
                    Some(bytes) => {
                        if verify_hashes && digest(bytes) != source.hash {
                            error = Some(invalid_usd(format!("content hash mismatch for `{parameter}`")));
                            return;
                        }
                        source.bytes.clone_from(bytes);
                    }
                    None => {
                        error = Some(ImportError::Missing(source.package_path()));
                        return;
                    }
                }
            }
        });
        if let Some(error) = error {
            return Err(error);
        }
        for asset in &mut material.auxiliary {
            if !asset.bytes.is_empty() {
                verify_hash(&asset.hash, &asset.bytes, &asset.name, verify_hashes)?;
                continue;
            }
            let asset_path = auxiliary_path(&asset.hash, &asset.name);
            let bytes = entries
                .get(&asset_path)
                .ok_or_else(|| ImportError::Missing(asset_path.clone()))?;
            verify_hash(&asset.hash, bytes, &asset_path, verify_hashes)?;
            asset.bytes.clone_from(bytes);
        }
        for (hash, asset) in &mut material.provenance.source_assets {
            if !asset.bytes.is_empty() {
                verify_hash(hash, &asset.bytes, &asset.name, verify_hashes)?;
                continue;
            }
            let asset_path = asset.package_path(hash);
            let bytes = entries
                .get(&asset_path)
                .ok_or_else(|| ImportError::Missing(asset_path.clone()))?;
            verify_hash(hash, bytes, &asset_path, verify_hashes)?;
            asset.bytes.clone_from(bytes);
        }
    }
    let issues = validate_materials(&materials);
    if !issues.is_empty() {
        return Err(invalid_usd(
            issues
                .into_iter()
                .map(|issue| format!("{}: {}", issue.path, issue.detail))
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    Ok(materials)
}

fn verify_hash(hash: &Sha256, bytes: &[u8], path: &str, verify: bool) -> Result<(), ImportError> {
    if verify && digest(bytes) != *hash {
        Err(invalid_usd(format!("content hash mismatch for `{path}`")))
    } else {
        Ok(())
    }
}

fn validate_imported_materials(materials: &[Material]) -> Result<(), ImportError> {
    let issues = validate_materials(materials);
    if !issues.is_empty() {
        return Err(invalid_usd(
            issues
                .into_iter()
                .map(|issue| format!("{}: {}", issue.path, issue.detail))
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    validate_payload_hashes(materials).map_err(|error| invalid_usd(error.to_string()))
}

fn auxiliary_path(hash: &Sha256, name: &str) -> String {
    let extension = path_extension(name);
    format!("auxiliary/{hash}.{}", usd_identifier(&extension.to_ascii_lowercase()))
}

fn path_extension(name: &str) -> &str {
    name.rsplit_once('.').map_or("bin", |(_, extension)| extension)
}

fn is_usdz_allowed_extension(extension: &str) -> bool {
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "usd" | "usda" | "usdc" | "png" | "jpg" | "jpeg" | "jpe" | "exr" | "avif" | "m4a" | "mp3" | "wav"
    )
}

fn is_usd_layer_extension(extension: &str) -> bool {
    matches!(extension.to_ascii_lowercase().as_str(), "usd" | "usda" | "usdc")
}

fn validate_entry_name(name: &str) -> Result<(), ImportError> {
    if name.is_empty()
        || name.starts_with('/')
        || name.ends_with('/')
        || name.contains('\\')
        || name
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        Err(invalid_usd(format!("unsafe package entry name `{name}`")))
    } else {
        Ok(())
    }
}

fn usd_identifier(value: &str) -> String {
    let mut result: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    if result.is_empty() {
        result.push_str("Material");
    }
    if result.as_bytes()[0].is_ascii_digit() {
        result.insert(0, '_');
    }
    result
}

fn digest(bytes: &[u8]) -> Sha256 {
    Sha256(format!("{:x}", Sha256Hasher::digest(bytes)))
}

fn validate_payload_hashes(materials: &[Material]) -> Result<(), ExportError> {
    for material in materials {
        let mut mismatch = None;
        material.visit_textures(|parameter, texture| {
            if mismatch.is_none() {
                mismatch = texture
                    .tiers
                    .iter()
                    .find(|(_, source)| digest(&source.bytes) != source.hash)
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
            if digest(&asset.bytes) != asset.hash {
                return Err(ExportError::InvalidModel(format!(
                    "material `{}` has a payload/hash mismatch for auxiliary `{}`",
                    material.id, asset.name
                )));
            }
        }
        for (hash, asset) in &material.provenance.source_assets {
            if digest(&asset.bytes) != *hash {
                return Err(ExportError::InvalidModel(format!(
                    "material `{}` has a payload/hash mismatch for provenance source `{}`",
                    material.id, asset.name
                )));
            }
        }
    }
    Ok(())
}

fn path(value: &str) -> Result<Path, ExportError> {
    Path::new(value).map_err(path_error)
}

fn path_error(error: impl std::fmt::Display) -> ExportError {
    ExportError::Encode {
        format: "USD",
        detail: error.to_string(),
    }
}

fn encode_error(error: impl std::fmt::Display) -> ExportError {
    ExportError::Encode {
        format: "USD",
        detail: error.to_string(),
    }
}

fn invalid_usd(error: impl std::fmt::Display) -> ImportError {
    ImportError::Invalid {
        format: "USD",
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_is_safe_and_non_empty() {
        assert_eq!(usd_identifier("12 red/blue"), "_12_red_blue");
        assert_eq!(usd_identifier(""), "Material");
    }

    #[test]
    fn generic_preview_surface_constants_can_be_inspected_without_a_manifest() {
        let bytes = br#"#usda 1.0
def Material "External" {
    token outputs:surface.connect = </External/Preview.outputs:surface>
    def Shader "Preview" {
        uniform token info:id = "UsdPreviewSurface"
        color3f inputs:diffuseColor = (0.1, 0.2, 0.3)
        float inputs:metallic = 0.4
        float inputs:roughness = 0.7
        token outputs:surface
    }
}
"#;
        let materials = UsdImporter
            .import(
                Input::Bytes(bytes),
                &UsdImportOptions {
                    allow_partial_graph: true,
                    ..UsdImportOptions::default()
                },
            )
            .unwrap();
        assert_eq!(materials[0].id.0, "External");
        assert_eq!(materials[0].surface.base_metalness, Value::from(0.4));
        assert_eq!(materials[0].surface.specular_roughness, Value::from(0.7));
    }
}
