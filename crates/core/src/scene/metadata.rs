//! Portable, typed metadata. Integers are decimal strings so JavaScript does not
//! round values outside its safe integer range. Absence is distinct from null;
//! unresolved values remain visible but are never treated as resolved answers.
use super::*;

pub const METADATA_SCHEMA: &str = "usd-toolbox.metadata.v1";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case", deny_unknown_fields)]
pub enum MetadataValue {
    Typed {
        value_type: String,
        value: Box<MetadataValue>,
    },
    Null,
    Bool(bool),
    String(String),
    I64(String),
    U64(String),
    F64(f64),
    Array(Vec<MetadataValue>),
    Dictionary(BTreeMap<String, MetadataValue>),
    TypedArray {
        element_type: String,
        values: Vec<MetadataValue>,
    },
}

impl MetadataValue {
    pub fn from_json(value: serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(v) => Self::Bool(v),
            serde_json::Value::String(v) => Self::String(v),
            serde_json::Value::Number(v) => {
                if let Some(v) = v.as_i64() {
                    Self::I64(v.to_string())
                } else if let Some(v) = v.as_u64() {
                    Self::U64(v.to_string())
                } else {
                    Self::F64(v.as_f64().unwrap_or_default())
                }
            }
            serde_json::Value::Array(v) => Self::Array(v.into_iter().map(Self::from_json).collect()),
            serde_json::Value::Object(v) => {
                Self::Dictionary(v.into_iter().map(|(k, v)| (k, Self::from_json(v))).collect())
            }
        }
    }

    /// Untagged JSON for source formats. The typed API must be used by JS clients
    /// that need exact integers; native serde_json preserves all 64-bit integers.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Typed { value, .. } => value.to_json(),
            Self::Null => serde_json::Value::Null,
            Self::Bool(v) => (*v).into(),
            Self::String(v) => v.clone().into(),
            Self::I64(v) => v.parse::<i64>().map(Into::into).unwrap_or(serde_json::Value::Null),
            Self::U64(v) => v.parse::<u64>().map(Into::into).unwrap_or(serde_json::Value::Null),
            Self::F64(v) => serde_json::json!(v),
            Self::Array(v) | Self::TypedArray { values: v, .. } => v.iter().map(Self::to_json).collect(),
            Self::Dictionary(v) => v.iter().map(|(k, v)| (k.clone(), v.to_json())).collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataState {
    #[default]
    Authored,
    Defaulted,
    Inherited,
    Unresolved,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataSource {
    pub format: String,
    pub path: String,
    pub property: Option<String>,
    pub value_type: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataProperty {
    pub value: MetadataValue,
    #[serde(default)]
    pub state: MetadataState,
    #[serde(default)]
    pub source: Option<MetadataSource>,
    #[serde(default)]
    pub unit: Option<String>,
}

impl MetadataProperty {
    pub fn authored(value: MetadataValue) -> Self {
        Self {
            value,
            state: MetadataState::Authored,
            source: None,
            unit: None,
        }
    }
    pub fn resolved(&self) -> Option<&MetadataValue> {
        (self.state != MetadataState::Unresolved).then_some(&self.value)
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Metadata {
    /// Exact source properties, with their source types and locations.
    pub original: BTreeMap<String, MetadataProperty>,
    /// Application-neutral catalogue fields such as label, description, tags,
    /// classification, product_id and manufacturer_id. Source fields stay intact.
    pub normalized: BTreeMap<String, MetadataProperty>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SceneMetadata {
    pub schema: String,
    pub scene: Metadata,
    pub nodes: BTreeMap<String, Metadata>,
    pub meshes: BTreeMap<String, Metadata>,
    pub materials: BTreeMap<String, Metadata>,
}
impl Default for SceneMetadata {
    fn default() -> Self {
        Self {
            schema: METADATA_SCHEMA.into(),
            scene: Metadata::default(),
            nodes: BTreeMap::new(),
            meshes: BTreeMap::new(),
            materials: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "scope", content = "id", rename_all = "snake_case", deny_unknown_fields)]
pub enum MetadataTarget {
    Scene,
    Node(String),
    Mesh(String),
    Material(String),
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataNamespace {
    #[default]
    Normalized,
    Original,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MetadataLimits {
    pub max_bytes: usize,
    pub max_property_bytes: usize,
    pub max_depth: usize,
    pub max_properties: usize,
}
impl Default for MetadataLimits {
    fn default() -> Self {
        Self {
            max_bytes: 8 * 1024 * 1024,
            max_property_bytes: 1024 * 1024,
            max_depth: 32,
            max_properties: 100_000,
        }
    }
}

impl SceneMetadata {
    pub fn get(&self, target: &MetadataTarget) -> Option<&Metadata> {
        match target {
            MetadataTarget::Scene => Some(&self.scene),
            MetadataTarget::Node(id) => self.nodes.get(id),
            MetadataTarget::Mesh(id) => self.meshes.get(id),
            MetadataTarget::Material(id) => self.materials.get(id),
        }
    }
    pub(crate) fn get_or_insert(&mut self, target: &MetadataTarget) -> &mut Metadata {
        match target {
            MetadataTarget::Scene => &mut self.scene,
            MetadataTarget::Node(id) => self.nodes.entry(id.clone()).or_default(),
            MetadataTarget::Mesh(id) => self.meshes.entry(id.clone()).or_default(),
            MetadataTarget::Material(id) => self.materials.entry(id.clone()).or_default(),
        }
    }
}

pub fn validate_metadata(metadata: &SceneMetadata, limits: &MetadataLimits) -> Result<(), SceneError> {
    measure_metadata(metadata, limits).map(|_| ())
}
#[derive(Clone, Copy, Default)]
pub(crate) struct MetadataUsage {
    pub bytes: usize,
    pub properties: usize,
}
pub(crate) fn measure_metadata(metadata: &SceneMetadata, limits: &MetadataLimits) -> Result<MetadataUsage, SceneError> {
    if metadata.schema != METADATA_SCHEMA {
        return Err(SceneError::new(
            "metadata.schema",
            "/metadata",
            "unsupported metadata schema; no implicit migration",
        ));
    }
    let mut bytes = metadata.schema.len();
    let mut properties = 0usize;
    for (scope, data) in std::iter::once(("scene", &metadata.scene))
        .chain(metadata.nodes.iter().map(|(id, m)| (id.as_str(), m)))
        .chain(metadata.meshes.iter().map(|(id, m)| (id.as_str(), m)))
        .chain(metadata.materials.iter().map(|(id, m)| (id.as_str(), m)))
    {
        bytes = bytes.saturating_add(scope.len());
        for (key, property) in data.original.iter().chain(data.normalized.iter()) {
            properties = properties.saturating_add(1);
            if key.is_empty() {
                return Err(SceneError::new("metadata.key", scope, "metadata key must not be empty"));
            }
            let mut size = key.len() + property.unit.as_ref().map_or(0, String::len);
            if let Some(s) = &property.source {
                size = size.saturating_add(
                    s.format.len()
                        + s.path.len()
                        + s.property.as_ref().map_or(0, String::len)
                        + s.value_type.as_ref().map_or(0, String::len),
                );
            }
            let mut stack = vec![(&property.value, 1)];
            while let Some((v, depth)) = stack.pop() {
                if depth > limits.max_depth {
                    return Err(SceneError::new(
                        "metadata.depth",
                        key,
                        "metadata nesting limit exceeded",
                    ));
                }
                size = size.saturating_add(16);
                match v {
                    MetadataValue::Typed { value_type, value } => {
                        if value_type.is_empty() {
                            return Err(SceneError::new("metadata.type", key, "typed value needs a source type"));
                        }
                        size = size.saturating_add(value_type.len());
                        stack.push((value, depth + 1));
                    }
                    MetadataValue::I64(v) => {
                        if v.parse::<i64>().is_err() || v.parse::<i64>().is_ok_and(|i| i.to_string() != *v) {
                            return Err(SceneError::new(
                                "metadata.integer",
                                key,
                                "expected canonical signed 64-bit decimal string",
                            ));
                        }
                        size = size.saturating_add(v.len());
                    }
                    MetadataValue::U64(v) => {
                        if v.parse::<u64>().is_err() || v.parse::<u64>().is_ok_and(|i| i.to_string() != *v) {
                            return Err(SceneError::new(
                                "metadata.integer",
                                key,
                                "expected canonical unsigned 64-bit decimal string",
                            ));
                        }
                        size = size.saturating_add(v.len());
                    }
                    MetadataValue::F64(v) if !v.is_finite() => {
                        return Err(SceneError::new("metadata.number", key, "metadata float must be finite"));
                    }
                    MetadataValue::String(v) => size = size.saturating_add(v.len()),
                    MetadataValue::Array(v) => stack.extend(v.iter().map(|v| (v, depth + 1))),
                    MetadataValue::TypedArray { element_type, values } => {
                        if element_type.is_empty() {
                            return Err(SceneError::new(
                                "metadata.type",
                                key,
                                "typed array needs an element type",
                            ));
                        }
                        size = size.saturating_add(element_type.len());
                        stack.extend(values.iter().map(|v| (v, depth + 1)));
                    }
                    MetadataValue::Dictionary(v) => {
                        for (k, v) in v {
                            size = size.saturating_add(k.len());
                            stack.push((v, depth + 1));
                        }
                    }
                    _ => {}
                }
                if size > limits.max_property_bytes {
                    return Err(SceneError::new(
                        "metadata.limit",
                        key,
                        "metadata property size limit exceeded",
                    ));
                }
            }
            bytes = bytes.saturating_add(size);
            if bytes > limits.max_bytes || properties > limits.max_properties {
                return Err(SceneError::new(
                    "metadata.limit",
                    "/metadata",
                    "metadata byte or property limit exceeded",
                ));
            }
        }
    }
    Ok(MetadataUsage { bytes, properties })
}

pub(crate) fn measure_metadata_record(
    target: &MetadataTarget,
    record: &Metadata,
    limits: &MetadataLimits,
) -> Result<MetadataUsage, SceneError> {
    let mut wrapper = SceneMetadata::default();
    *wrapper.get_or_insert(target) = record.clone();
    let mut usage = measure_metadata(&wrapper, limits)?;
    usage.bytes = usage.bytes.saturating_sub(METADATA_SCHEMA.len() + "scene".len());
    Ok(usage)
}
