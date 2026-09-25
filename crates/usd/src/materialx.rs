//! Native MaterialX graphs authored as USD Shader / NodeGraph prims.
use super::*;
use usd_toolbox_core::{MaterialXElement, MaterialXGraph};

pub(super) fn author(data: &mut sdf::Data, path: &Path, graph: &MaterialXGraph) -> Result<(), ExportError> {
    let mut children = Vec::new();
    for element in &graph.document.children {
        if matches!(element.category.as_str(), "surfacematerial" | "propertyset" | "look") {
            continue;
        }
        if matches!(element.category.as_str(), "nodedef" | "implementation") {
            return Err(ExportError::Unsupported(
                "custom MaterialX definitions require a renderer plug-in; inline USD export is not available".into(),
            ));
        }
        author_node(data, path, element)?;
        children.push(Token::from(element.attribute("name")));
    }
    let binding = graph
        .document
        .children
        .iter()
        .find(|element| element.category == "surfacematerial" && element.attribute("name") == graph.material_name)
        .ok_or_else(|| ExportError::InvalidModel("MaterialX material binding is missing".into()))?;
    let mut outputs = Vec::new();
    for port in &binding.children {
        let output = match port.attribute("name") {
            "surfaceshader" => "surface",
            "displacementshader" => "displacement",
            "volumeshader" => "volume",
            _ => continue,
        };
        let source = connection(path, port)?
            .ok_or_else(|| ExportError::InvalidModel(format!("MaterialX {output} is not connected")))?;
        let name = format!("outputs:mtlx:{output}");
        create_attribute(data, path, &name, "token", None, Some(source), None)?;
        outputs.push(Token::from(name));
    }
    let prim = data.spec_mut(path).expect("material exists");
    prim.add(ChildrenKey::PrimChildren, UsdValue::TokenVec(children));
    prim.add(ChildrenKey::PropertyChildren, UsdValue::TokenVec(outputs));
    Ok(())
}

fn author_node(data: &mut sdf::Data, parent: &Path, element: &MaterialXElement) -> Result<(), ExportError> {
    let name = element.attribute("name");
    if name.is_empty() || usd_identifier(name) != name {
        return Err(ExportError::Unsupported(format!(
            "MaterialX node name `{name}` is not a USD identifier"
        )));
    }
    let node = parent.append_path(name).map_err(path_error)?;
    let graph = element.category == "nodegraph";
    let prim = data.create_spec(node.clone(), SpecType::Prim);
    prim.add(FieldKey::Specifier, UsdValue::Specifier(Specifier::Def));
    prim.add(
        FieldKey::TypeName,
        UsdValue::Token(if graph { "NodeGraph" } else { "Shader" }.into()),
    );
    let mut properties = Vec::new();
    let mut children = Vec::new();
    if !graph {
        let id = if !element.attribute("nodedef").is_empty() {
            element.attribute("nodedef").to_owned()
        } else if element.category == "normalmap" {
            "ND_normalmap_float".into()
        } else if element.category == "extract" || element.category == "convert" || element.category == "displacement" {
            let input_type = element
                .children
                .iter()
                .find(|child| matches!(child.attribute("name"), "in" | "displacement"))
                .map(|child| child.attribute("type"))
                .unwrap_or("float");
            if element.category == "convert" {
                format!("ND_convert_{}_{}", input_type, element.attribute("type"))
            } else {
                format!("ND_{}_{}", element.category, input_type)
            }
        } else {
            format!("ND_{}_{}", element.category, element.attribute("type"))
        };
        create_attribute(
            data,
            &node,
            "info:id",
            "token",
            Some(UsdValue::Token(id.into())),
            None,
            None,
        )?;
        properties.push("info:id".into());
        if !element.children.iter().any(|child| child.category == "output") {
            create_attribute(
                data,
                &node,
                "outputs:out",
                usd_type(element.attribute("type"))?,
                None,
                None,
                None,
            )?;
            properties.push("outputs:out".into());
        }
    }
    for input in &element.children {
        if !matches!(input.category.as_str(), "input" | "output") {
            if graph {
                author_node(data, &node, input)?;
                children.push(Token::from(input.attribute("name")));
            } else {
                return Err(ExportError::Unsupported(format!(
                    "unsupported MaterialX shader child `{}`",
                    input.category
                )));
            }
            continue;
        }
        let property = format!("{}s:{}", input.category, input.attribute("name"));
        let scope = if graph { &node } else { parent };
        let connection = connection(scope, input)?;
        let default = if input.attributes.contains_key("value") {
            Some(value(input.attribute("type"), input.attribute("value"))?)
        } else {
            None
        };
        create_attribute(
            data,
            &node,
            &property,
            usd_type(input.attribute("type"))?,
            default,
            connection,
            None,
        )?;
        let color_space = if input.attribute("colorspace").is_empty() {
            element.attribute("colorspace")
        } else {
            input.attribute("colorspace")
        };
        if input.attribute("type") == "filename" && !color_space.is_empty() {
            data.spec_mut(&node.append_property(&property).map_err(path_error)?)
                .expect("input exists")
                .add("colorSpace", UsdValue::Token(color_space.into()));
        }
        properties.push(Token::from(property));
    }
    let prim = data.spec_mut(&node).expect("node exists");
    prim.add(ChildrenKey::PropertyChildren, UsdValue::TokenVec(properties));
    if !children.is_empty() {
        prim.add(ChildrenKey::PrimChildren, UsdValue::TokenVec(children));
    }
    Ok(())
}

fn connection(parent: &Path, input: &MaterialXElement) -> Result<Option<Path>, ExportError> {
    if !input.attribute("interfacename").is_empty() {
        return parent
            .append_property(format!("inputs:{}", input.attribute("interfacename")))
            .map(Some)
            .map_err(path_error);
    }
    let name = if !input.attribute("nodename").is_empty() {
        input.attribute("nodename")
    } else {
        input.attribute("nodegraph")
    };
    if name.is_empty() {
        return Ok(None);
    }
    let output = if input.attribute("output").is_empty() {
        "out"
    } else {
        input.attribute("output")
    };
    parent
        .append_path(name)
        .map_err(path_error)?
        .append_property(format!("outputs:{output}"))
        .map(Some)
        .map_err(path_error)
}

fn usd_type(kind: &str) -> Result<&'static str, ExportError> {
    Ok(match kind {
        "float" => "float",
        "integer" => "int",
        "boolean" => "bool",
        "string" => "string",
        "filename" => "asset",
        "vector2" => "float2",
        "vector3" => "vector3f",
        "vector4" => "float4",
        "color3" => "color3f",
        "color4" => "color4f",
        "surfaceshader" | "displacementshader" | "volumeshader" => "token",
        _ => {
            return Err(ExportError::Unsupported(format!(
                "MaterialX `{kind}` values are not yet supported by the USD graph adapter"
            )));
        }
    })
}

fn value(kind: &str, text: &str) -> Result<UsdValue, ExportError> {
    let invalid = || ExportError::InvalidModel(format!("invalid MaterialX {kind} value `{text}`"));
    if kind == "string" {
        return Ok(UsdValue::String(text.into()));
    }
    if kind == "filename" {
        return Ok(UsdValue::AssetPath(AssetPath::new(text)));
    }
    if kind == "boolean" {
        return match text {
            "true" => Ok(UsdValue::Bool(true)),
            "false" => Ok(UsdValue::Bool(false)),
            _ => Err(invalid()),
        };
    }
    if kind == "integer" {
        return text.parse().map(UsdValue::Int).map_err(|_| invalid());
    }
    let numbers: Vec<f32> = text
        .split(',')
        .map(|n| n.trim().parse().map_err(|_| invalid()))
        .collect::<Result<_, _>>()?;
    if numbers.iter().any(|n| !n.is_finite()) {
        return Err(invalid());
    }
    match (kind, numbers.as_slice()) {
        ("float", [a]) => Ok(UsdValue::Float(*a)),
        ("vector2", [a, b]) => Ok(UsdValue::Vec2f(gf::Vec2f { x: *a, y: *b })),
        ("vector3" | "color3", [a, b, c]) => Ok(UsdValue::Vec3f(gf::Vec3f { x: *a, y: *b, z: *c })),
        ("vector4" | "color4", [a, b, c, d]) => Ok(UsdValue::Vec4f(gf::Vec4f {
            x: *a,
            y: *b,
            z: *c,
            w: *d,
        })),
        _ => Err(invalid()),
    }
}
