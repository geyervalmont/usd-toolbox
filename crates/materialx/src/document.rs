//! Guarded native document reading and dependency resolution.
use quick_xml::events::{BytesEnd, BytesStart, Event};
use quick_xml::{Reader, Writer};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Cursor;
use usd_toolbox_core::{ImportError, Input, Material, MaterialXElement, MaterialXGraph, TextureSource};

fn invalid(detail: impl Into<String>) -> ImportError {
    ImportError::Invalid {
        format: "MaterialX",
        detail: detail.into(),
    }
}

/// Parse a complete document without resolving includes, entities, or URLs.
/// Assets are supplied by the caller, never fetched by the format library.
pub fn parse_document(bytes: &[u8]) -> Result<MaterialXElement, ImportError> {
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(invalid("document exceeds 16 MiB"));
    }
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut stack = Vec::<MaterialXElement>::new();
    let mut root = None;
    let mut count = 0;
    loop {
        match reader.read_event().map_err(|e| invalid(e.to_string()))? {
            Event::Start(event) | Event::Empty(event) => {
                // is_empty() is not exposed by the element; read the lexical close.
                let end = reader.buffer_position() as usize;
                let empty = bytes.get(end.saturating_sub(2)..end) == Some(b"/>");
                count += 1;
                if count > 100_000 || stack.len() >= 64 {
                    return Err(invalid("document complexity limit exceeded"));
                }
                let category = event.name().into_inner().to_owned();
                if category == "xi:include" || category == "include" {
                    return Err(invalid("external includes must be flattened before import"));
                }
                let mut attributes = BTreeMap::new();
                for attr in event.attributes() {
                    let attr = attr.map_err(|e| invalid(e.to_string()))?;
                    let key = attr.key.into_inner().to_owned();
                    let value = attr
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|e| invalid(e.to_string()))?
                        .into_owned();
                    if matches!(key.as_str(), "fileprefix" | "sourceuri") && !value.is_empty() {
                        return Err(invalid("fileprefix/sourceuri must be resolved before import"));
                    }
                    attributes.insert(key, value);
                }
                let element = MaterialXElement {
                    category,
                    attributes,
                    children: Vec::new(),
                };
                if empty {
                    append(&mut stack, &mut root, element)?;
                } else {
                    stack.push(element);
                }
            }
            Event::End(_) => {
                let element = stack.pop().ok_or_else(|| invalid("unexpected closing element"))?;
                append(&mut stack, &mut root, element)?;
            }
            Event::DocType(_) | Event::GeneralRef(_) => {
                return Err(invalid("DTD and entity declarations are not supported"));
            }
            Event::Text(text) if !text.as_ref().trim().is_empty() => return Err(invalid("unexpected text content")),
            Event::CData(_) => return Err(invalid("CDATA is not a MaterialX graph element")),
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err(invalid("unclosed document"));
    }
    let root = root.ok_or_else(|| invalid("missing document"))?;
    if root.category != "materialx" || !matches!(root.attribute("version"), "1.38" | "1.39") {
        return Err(invalid("expected a MaterialX 1.38 or 1.39 document"));
    }
    Ok(root)
}

fn append(
    stack: &mut [MaterialXElement],
    root: &mut Option<MaterialXElement>,
    element: MaterialXElement,
) -> Result<(), ImportError> {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(element);
    } else if root.replace(element).is_some() {
        return Err(invalid("multiple root elements"));
    }
    Ok(())
}

/// Safe relative asset paths required at both filesystem and browser boundaries.
pub fn validate_asset_path(path: &str) -> Result<(), ImportError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains(['\\', ':', '?', '#', '%', '\0'])
        || path.split('/').any(|part| part.is_empty() || part == "..")
    {
        return Err(invalid(format!("unsafe asset path `{path}`")));
    }
    Ok(())
}

/// Lists all image dependencies so filesystem adapters can supply an input bundle.
pub fn referenced_files(bytes: &[u8]) -> Result<Vec<String>, ImportError> {
    let root = parse_document(bytes)?;
    let mut paths = Vec::new();
    root.visit(&mut |element| {
        if element.attribute("type") == "filename" && !element.attribute("value").is_empty() {
            paths.push(element.attribute("value").trim_start_matches("./").to_owned());
        }
    });
    for path in &paths {
        validate_asset_path(path)?;
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

pub(crate) fn import_native(input: Input<'_>, bytes: &[u8]) -> Result<Vec<Material>, ImportError> {
    let mut document = parse_document(bytes)?;
    let mut assets = BTreeMap::new();
    for path in referenced_files(bytes)? {
        let payload = match input {
            Input::Bundle(files) => files
                .iter()
                .find(|file| file.name.trim_start_matches("./") == path)
                .map(|file| file.bytes),
            _ => None,
        }
        .ok_or_else(|| {
            ImportError::Missing(format!(
                "MaterialX texture `{path}`; supply document and assets as Input::Bundle"
            ))
        })?;
        let extension = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        let media_type = match extension.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "exr" => "image/x-exr",
            _ => return Err(invalid(format!("unsupported image extension `{extension}`"))),
        };
        let source = TextureSource {
            hash: usd_toolbox_core::Sha256(format!("{:x}", Sha256::digest(payload))),
            extension,
            media_type: media_type.into(),
            width: None,
            height: None,
            bytes: payload.to_vec(),
        };
        assets.insert(source.package_path(), source.clone());
        rewrite_asset(&mut document, &path, &source.package_path());
    }
    let projected = super::read_generic_graph(bytes)?;
    let bindings: Vec<_> = document
        .children
        .iter()
        .filter(|node| node.category == "surfacematerial")
        .collect();
    if bindings.is_empty() {
        return Err(invalid("a native document requires a surfacematerial binding"));
    }
    let mut materials = Vec::new();
    for binding in bindings {
        let surface_name = binding
            .children
            .iter()
            .find(|port| port.attribute("name") == "surfaceshader")
            .map(|port| port.attribute("nodename"))
            .unwrap_or("");
        let mut material = projected
            .iter()
            .find(|material| material.name == surface_name)
            .cloned()
            .ok_or_else(|| {
                invalid(format!(
                    "material `{}` must bind a top-level Standard Surface or OpenPBR shader",
                    binding.attribute("name")
                ))
            })?;
        material.id = usd_toolbox_core::MaterialId(binding.attribute("name").into());
        material.name = binding.attribute("name").into();
        material.materialx = Some(MaterialXGraph {
            document: document.clone(),
            material_name: material.name.clone(),
            assets: assets.clone(),
        });
        materials.push(material);
    }
    let issues = usd_toolbox_core::validate_materials(&materials);
    if !issues.is_empty() {
        return Err(invalid(
            issues
                .iter()
                .map(|issue| issue.detail.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    Ok(materials)
}

fn rewrite_asset(element: &mut MaterialXElement, from: &str, to: &str) {
    if element.attribute("type") == "filename" && element.attribute("value").trim_start_matches("./") == from {
        element.attributes.insert("value".into(), to.into());
    }
    for child in &mut element.children {
        rewrite_asset(child, from, to);
    }
}

/// Serializes the graph without changing connection structure or node attributes.
pub fn write_element(writer: &mut Writer<Cursor<Vec<u8>>>, element: &MaterialXElement) -> Result<(), std::io::Error> {
    let mut start = BytesStart::new(&element.category);
    for (key, value) in &element.attributes {
        start.push_attribute((key.as_str(), value.as_str()));
    }
    if element.children.is_empty() {
        writer.write_event(Event::Empty(start))?;
    } else {
        writer.write_event(Event::Start(start))?;
        for child in &element.children {
            write_element(writer, child)?;
        }
        writer.write_event(Event::End(BytesEnd::new(&element.category)))?;
    }
    Ok(())
}
