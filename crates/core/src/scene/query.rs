//! Reusable documents retain validated payloads and bounded indexes. No mutable
//! scene reference escapes: successful edits advance generation and clear caches.
use super::*;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct QueryLimits {
    pub max_results: usize,
    pub max_response_bytes: usize,
    pub max_predicates: usize,
    pub max_index_bytes: usize,
    pub max_cache_bytes: usize,
}
impl Default for QueryLimits {
    fn default() -> Self {
        Self {
            max_results: 1000,
            max_response_bytes: 2 * 1024 * 1024,
            max_predicates: 64,
            max_index_bytes: 32 * 1024 * 1024,
            max_cache_bytes: 32 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataIndex {
    #[serde(default)]
    pub namespace: MetadataNamespace,
    pub key: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DocumentOptions {
    pub scene_limits: SceneLimits,
    pub metadata_limits: MetadataLimits,
    pub query_limits: QueryLimits,
    pub metadata_indexes: Vec<MetadataIndex>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextMode {
    #[default]
    Exact,
    Prefix,
    Contains,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextFilter {
    pub value: String,
    #[serde(default)]
    pub mode: TextMode,
    #[serde(default)]
    pub ignore_case: bool,
}
impl TextFilter {
    fn matches(&self, value: &str) -> bool {
        fn compare(mode: TextMode, needle: &str, value: &str) -> bool {
            match mode {
                TextMode::Exact => value == needle,
                TextMode::Prefix => value.starts_with(needle),
                TextMode::Contains => value.contains(needle),
            }
        }
        if self.ignore_case {
            compare(self.mode, &self.value.to_lowercase(), &value.to_lowercase())
        } else {
            compare(self.mode, &self.value, value)
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    Group,
    Mesh,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NumericOperator {
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "operator", rename_all = "snake_case", deny_unknown_fields)]
pub enum MetadataPredicate {
    Exists {
        exists: bool,
    },
    Equals {
        value: MetadataValue,
    },
    Numeric {
        comparison: NumericOperator,
        value: MetadataValue,
    },
}
#[derive(Clone, Debug, Serialize)]
pub struct MetadataFilter {
    #[serde(default)]
    pub namespace: MetadataNamespace,
    pub key: String,
    #[serde(flatten)]
    pub predicate: MetadataPredicate,
}
impl<'de> Deserialize<'de> for MetadataFilter {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = serde_json::Value::deserialize(deserializer)?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| serde::de::Error::custom("metadata filter must be an object"))?;
        let namespace = object
            .remove("namespace")
            .map(serde_json::from_value)
            .transpose()
            .map_err(serde::de::Error::custom)?
            .unwrap_or_default();
        let key = serde_json::from_value(
            object
                .remove("key")
                .ok_or_else(|| serde::de::Error::missing_field("key"))?,
        )
        .map_err(serde::de::Error::custom)?;
        let predicate = serde_json::from_value(value).map_err(serde::de::Error::custom)?;
        Ok(Self {
            namespace,
            key,
            predicate,
        })
    }
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct NodeFilter {
    pub ids: Option<Vec<String>>,
    pub name: Option<TextFilter>,
    pub path: Option<TextFilter>,
    pub node_type: Option<NodeType>,
    pub metadata: Vec<MetadataFilter>,
    pub material: Option<MaterialId>,
    pub visible: Option<bool>,
    pub purpose: Option<Purpose>,
    pub subtree: Option<String>,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryField {
    Name,
    SourcePath,
    Parent,
    Children,
    NodeType,
    Mesh,
    Metadata,
    LocalTransform,
    WorldTransform,
    Bounds,
    EffectiveMaterial,
    Visibility,
    Purpose,
    Dependencies,
    SourceHash,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuerySort {
    #[default]
    Id,
    Name,
    Path,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct QueryRequest {
    pub filter: NodeFilter,
    pub fields: Vec<QueryField>,
    pub sort: QuerySort,
    pub descending: bool,
    pub limit: usize,
    pub cursor: Option<String>,
}
impl Default for QueryRequest {
    fn default() -> Self {
        Self {
            filter: NodeFilter::default(),
            fields: vec![QueryField::Name, QueryField::Parent, QueryField::NodeType],
            sort: QuerySort::Id,
            descending: false,
            limit: 100,
            cursor: None,
        }
    }
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct QueryRow {
    pub id: String,
    pub fields: BTreeMap<String, serde_json::Value>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct QueryPage {
    pub schema: String,
    pub generation: u64,
    pub rows: Vec<QueryRow>,
    pub next_cursor: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DocumentSummary {
    pub generation: u64,
    pub nodes: usize,
    pub meshes: usize,
    pub materials: usize,
    pub roots: usize,
    pub dependencies: usize,
    pub indexed_metadata: Vec<MetadataIndex>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EditReport {
    pub generation: u64,
    #[serde(default)]
    pub geometry: Vec<GeometryReport>,
    pub affected: Vec<SceneChange>,
    pub diagnostics: Vec<SceneDiagnostic>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    revision: String,
    generation: u64,
    query: String,
    offset: usize,
}

#[derive(Default)]
struct Indexes {
    estimated_bytes: usize,
    paths: BTreeMap<String, Vec<String>>,
    children: BTreeMap<Option<String>, Vec<String>>,
    ids: Vec<String>,
    by_name: Vec<String>,
    by_path: Vec<String>,
    effective: BTreeMap<String, (bool, Purpose, Option<MaterialId>)>,
    metadata: BTreeMap<(String, String), BTreeSet<String>>,
    materials: BTreeSet<String>,
    subtrees: BTreeMap<String, (usize, usize)>,
}
#[derive(Default)]
struct Caches {
    bytes: usize,
    worlds: BTreeMap<String, Matrix4>,
    bounds: BTreeMap<String, Option<Bounds>>,
}

pub struct SceneDocument {
    scene: Scene,
    options: DocumentOptions,
    generation: u64,
    revision: String,
    indexes: Indexes,
    metadata_usage: metadata::MetadataUsage,
    caches: RefCell<Caches>,
}

impl SceneDocument {
    pub fn new(scene: Scene, limits: SceneLimits) -> Result<Self, SceneError> {
        Self::with_options(
            scene,
            DocumentOptions {
                scene_limits: limits,
                ..Default::default()
            },
        )
    }
    pub fn with_options(scene: Scene, options: DocumentOptions) -> Result<Self, SceneError> {
        require_valid_scene(&scene, &options.scene_limits)?;
        validate_metadata(&scene.metadata, &options.metadata_limits)?;
        let indexes = build_indexes(&scene, &options)?;
        let metadata_usage = metadata::measure_metadata(&scene.metadata, &options.metadata_limits)?;
        let revision = stream_hash(&scene)?;
        Ok(Self {
            scene,
            options,
            generation: 0,
            revision,
            indexes,
            metadata_usage,
            caches: RefCell::default(),
        })
    }
    pub fn scene(&self) -> &Scene {
        &self.scene
    }
    pub fn into_scene(self) -> Scene {
        self.scene
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn inspect(&self) -> Result<SceneInspection, SceneError> {
        inspect_scene(&self.scene, &self.options.scene_limits)
    }
    pub fn summary(&self) -> DocumentSummary {
        DocumentSummary {
            generation: self.generation,
            nodes: self.scene.nodes.len(),
            meshes: self.scene.meshes.len(),
            materials: self.scene.materials.len(),
            roots: self.indexes.children.get(&None).map_or(0, Vec::len),
            dependencies: self.scene.dependencies.len(),
            indexed_metadata: self.options.metadata_indexes.clone(),
        }
    }
    /// Direct stable-ID lookup; borrowed node headers contain no geometry/images.
    pub fn node(&self, id: &str) -> Option<&SceneNode> {
        self.scene.nodes.get(id)
    }
    /// Source paths can be ambiguous after duplication/merge; never choose one silently.
    pub fn ids_for_path(&self, path: &str) -> &[String] {
        self.indexes.paths.get(path).map_or(&[], Vec::as_slice)
    }
    pub fn parent(&self, id: &str) -> Result<Option<&str>, SceneError> {
        Ok(self.require_node(id)?.parent.as_deref())
    }
    pub fn children(&self, parent: Option<&str>) -> Result<Vec<String>, SceneError> {
        if let Some(id) = parent {
            self.require_node(id)?;
        }
        let ids = self
            .indexes
            .children
            .get(&parent.map(str::to_owned))
            .cloned()
            .unwrap_or_default();
        self.check_hierarchy_response(&ids)?;
        Ok(ids)
    }
    pub fn ancestors(&self, id: &str) -> Result<Vec<String>, SceneError> {
        let mut result = Vec::new();
        let mut current = self.parent(id)?;
        while let Some(id) = current {
            result.push(id.to_owned());
            current = self.parent(id)?;
        }
        self.check_hierarchy_response(&result)?;
        Ok(result)
    }
    pub fn descendants(&self, id: &str) -> Result<Vec<String>, SceneError> {
        let ids = self.descendant_ids(id)?;
        self.check_hierarchy_response(&ids)?;
        Ok(ids)
    }
    fn descendant_ids(&self, id: &str) -> Result<Vec<String>, SceneError> {
        self.require_node(id)?;
        let mut result = Vec::new();
        let mut queue = VecDeque::from(self.indexes.children.get(&Some(id.into())).cloned().unwrap_or_default());
        while let Some(id) = queue.pop_front() {
            queue.extend(
                self.indexes
                    .children
                    .get(&Some(id.clone()))
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
            result.push(id);
        }
        Ok(result)
    }
    fn check_hierarchy_response(&self, ids: &[String]) -> Result<(), SceneError> {
        if ids.len() > self.options.query_limits.max_results {
            return Err(SceneError::new(
                "query.limit",
                "/hierarchy",
                "hierarchy response exceeds result budget; use a paginated subtree query",
            ));
        }
        self.check_response(&ids)
    }
    pub fn get_metadata(&self, target: &MetadataTarget) -> Result<Option<Metadata>, SceneError> {
        self.require_target(target)?;
        let value = self.scene.metadata.get(target);
        if let Some(value) = value {
            self.check_response(value)?;
        }
        Ok(value.cloned())
    }
    pub fn get_property(
        &self,
        target: &MetadataTarget,
        namespace: MetadataNamespace,
        key: &str,
    ) -> Result<Option<MetadataProperty>, SceneError> {
        self.require_target(target)?;
        let property = self.scene.metadata.get(target).and_then(|m| match namespace {
            MetadataNamespace::Original => m.original.get(key),
            MetadataNamespace::Normalized => m.normalized.get(key),
        });
        if let Some(property) = property {
            self.check_response(property)?;
        }
        Ok(property.cloned())
    }
    pub fn get_node(&self, id: &str, fields: &[QueryField]) -> Result<Option<QueryRow>, SceneError> {
        if fields.len() > 32 {
            return Err(SceneError::new("query.fields", "/fields", "too many projected fields"));
        }
        if !self.scene.nodes.contains_key(id) {
            return Ok(None);
        }
        let row = self.project(id, fields)?;
        self.check_response(&row)?;
        Ok(Some(row))
    }
    pub fn count(&self, filter: &NodeFilter) -> Result<usize, SceneError> {
        self.check_filter(filter)?;
        let subtree = self.subtree(filter)?;
        let candidates = self.candidates(filter);
        let ids: Box<dyn Iterator<Item = &str> + '_> = match &candidates {
            Some(ids) => Box::new(ids.iter().copied()),
            None => Box::new(self.indexes.ids.iter().map(String::as_str)),
        };
        Ok(ids.filter(|id| self.matches(id, filter, subtree.as_ref())).count())
    }

    pub fn query(&self, request: &QueryRequest) -> Result<QueryPage, SceneError> {
        self.check_filter(&request.filter)?;
        if request.limit == 0 || request.limit > self.options.query_limits.max_results {
            return Err(SceneError::new(
                "query.limit",
                "/limit",
                "limit must be positive and within document query budget",
            ));
        }
        if request.fields.len() > 32 {
            return Err(SceneError::new("query.fields", "/fields", "too many projected fields"));
        }
        let mut canonical = request.clone();
        canonical.cursor = None;
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&canonical).map_err(json_error)?)
        );
        let offset = if let Some(cursor) = &request.cursor {
            if cursor.len() > 1024 {
                return Err(SceneError::new("query.cursor", "/cursor", "cursor exceeds size limit"));
            }
            let cursor: Cursor = serde_json::from_str(cursor)
                .map_err(|_| SceneError::new("query.cursor", "/cursor", "malformed cursor"))?;
            if cursor.revision != self.revision || cursor.generation != self.generation {
                return Err(SceneError::new(
                    "query.stale_cursor",
                    "/cursor",
                    "cursor belongs to another document or generation",
                ));
            }
            if cursor.query != fingerprint {
                return Err(SceneError::new(
                    "query.cursor_mismatch",
                    "/cursor",
                    "cursor belongs to a different query",
                ));
            }
            cursor.offset
        } else {
            0
        };
        let subtree = self.subtree(&request.filter)?;
        let candidates = self.candidates(&request.filter);
        let selected = candidates.map(|ids| {
            let mut ids = ids.into_iter().collect::<Vec<_>>();
            ids.sort_by(|a, b| match request.sort {
                QuerySort::Id => a.cmp(b),
                QuerySort::Name => (&self.scene.nodes[*a].name, a).cmp(&(&self.scene.nodes[*b].name, b)),
                QuerySort::Path => (&self.scene.nodes[*a].source_path, a).cmp(&(&self.scene.nodes[*b].source_path, b)),
            });
            ids
        });
        let indexed = match request.sort {
            QuerySort::Id => &self.indexes.ids,
            QuerySort::Name => &self.indexes.by_name,
            QuerySort::Path => &self.indexes.by_path,
        };
        let length = selected.as_ref().map_or(indexed.len(), Vec::len);
        if offset > length {
            return Err(SceneError::new(
                "query.cursor",
                "/cursor",
                "cursor offset outside candidates",
            ));
        }
        let ids: Box<dyn DoubleEndedIterator<Item = &str> + '_> = match &selected {
            Some(ids) => Box::new(ids.iter().copied()),
            None => Box::new(indexed.iter().map(String::as_str)),
        };
        let ids: Box<dyn Iterator<Item = &str> + '_> = if request.descending { Box::new(ids.rev()) } else { ids };
        let mut rows = Vec::with_capacity(request.limit.min(length));
        let mut response_bytes = 128usize;
        let mut next_offset = None;
        for (position, id) in ids.enumerate().skip(offset) {
            if !self.matches(id, &request.filter, subtree.as_ref()) {
                continue;
            }
            if rows.len() == request.limit {
                next_offset = Some(position);
                break;
            }
            let row = self.project(id, &request.fields)?;
            response_bytes = response_bytes.saturating_add(serialized_size(&row)?);
            if response_bytes > self.options.query_limits.max_response_bytes {
                return Err(SceneError::new(
                    "query.response_limit",
                    id,
                    "project fewer fields or request a smaller page",
                ));
            }
            rows.push(row);
        }
        let next_cursor = next_offset
            .map(|offset| {
                serde_json::to_string(&Cursor {
                    revision: self.revision.clone(),
                    generation: self.generation,
                    query: fingerprint,
                    offset,
                })
                .map_err(json_error)
            })
            .transpose()?;
        let page = QueryPage {
            schema: "usd-toolbox.query-page.v1".into(),
            generation: self.generation,
            rows,
            next_cursor,
        };
        self.check_response(&page)?;
        Ok(page)
    }
    pub fn world_transform(&self, id: &str) -> Result<Matrix4, SceneError> {
        if let Some(v) = self.caches.borrow().worlds.get(id) {
            return Ok(*v);
        }
        let mut chain = Vec::new();
        let mut current = Some(id);
        let mut base = IDENTITY;
        while let Some(id) = current {
            if let Some(v) = self.caches.borrow().worlds.get(id) {
                base = *v;
                break;
            }
            let node = self.require_node(id)?;
            chain.push((id, node.transform));
            current = if node.reset_transform {
                None
            } else {
                node.parent.as_deref()
            };
        }
        let mut caches = self.caches.borrow_mut();
        for (id, local) in chain.into_iter().rev() {
            base = multiply(base, local);
            if base.iter().flatten().any(|v| !v.is_finite()) {
                return Err(SceneError::new(
                    "scene.non_finite_transform",
                    id,
                    "world transform overflow",
                ));
            }
            let bytes = id.len().saturating_add(256);
            if caches.bytes.saturating_add(bytes) <= self.options.query_limits.max_cache_bytes {
                caches.bytes = caches.bytes.saturating_add(bytes);
                caches.worlds.insert(id.into(), base);
            }
        }
        Ok(base)
    }
    /// Own mesh's exact world-axis-aligned bounds, expressed in metres. Children
    /// are separate rows. This scans positions once per node per generation.
    pub fn bounds(&self, id: &str) -> Result<Option<Bounds>, SceneError> {
        if let Some(v) = self.caches.borrow().bounds.get(id) {
            return Ok(*v);
        }
        let node = self.require_node(id)?;
        let matrix = self.world_transform(id)?;
        let mut bounds: Option<Bounds> = None;
        if let Some(mesh) = node.mesh.as_ref().and_then(|id| self.scene.meshes.get(id)) {
            for p in &mesh.positions {
                let p = transform_point(matrix, *p).map(|v| v * self.scene.meters_per_unit);
                if p.iter().any(|v| !v.is_finite()) {
                    return Err(SceneError::new("scene.non_finite_bounds", id, "world bounds overflow"));
                }
                match &mut bounds {
                    Some(bounds) => bounds.include(p),
                    None => bounds = Some(Bounds { min: p, max: p }),
                }
            }
        }
        let mut caches = self.caches.borrow_mut();
        let bytes = id.len().saturating_add(128);
        if caches.bytes.saturating_add(bytes) <= self.options.query_limits.max_cache_bytes {
            caches.bytes = caches.bytes.saturating_add(bytes);
            caches.bounds.insert(id.into(), bounds);
        }
        Ok(bounds)
    }
    pub fn apply(&mut self, edits: &[SceneEdit]) -> Result<EditReport, SceneError> {
        self.apply_checked(edits, |_| Ok(()))
    }
    /// Validate/reserve the outgoing report before committing the transaction.
    /// A boundary can serialize into its result handle here; any error rolls back.
    pub fn apply_checked(
        &mut self,
        edits: &[SceneEdit],
        check: impl FnOnce(&EditReport) -> Result<(), SceneError>,
    ) -> Result<EditReport, SceneError> {
        if edits.is_empty() {
            let report = EditReport {
                generation: self.generation,
                geometry: Vec::new(),
                affected: Vec::new(),
                diagnostics: Vec::new(),
            };
            check(&report)?;
            return Ok(report);
        }
        if self.scene.provenance.operations.len().saturating_add(edits.len()) > self.options.scene_limits.max_operations
        {
            return Err(SceneError::new(
                "scene.operation_limit",
                "/provenance/operations",
                "operation history limit exceeded",
            ));
        }
        if edits.iter().all(|edit| matches!(edit, SceneEdit::SetMetadata { .. })) {
            return self.apply_metadata_only(edits, check);
        }
        if edits.iter().all(|edit| matches!(edit, SceneEdit::Rename { .. })) {
            return self.apply_rename_only(edits, check);
        }
        let next_generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| SceneError::new("scene.generation", "/", "generation exhausted"))?;
        let revision = stream_hash(&(&self.revision, edits))?;
        // Lightweight transactions clone only headers/metadata; payload ownership
        // moves into the candidate and is restored unchanged on any failure.
        let light = edits.iter().all(SceneEdit::is_lightweight);
        let mut candidate = if light {
            Scene {
                schema: self.scene.schema.clone(),
                id: self.scene.id.clone(),
                name: self.scene.name.clone(),
                meters_per_unit: self.scene.meters_per_unit,
                up_axis: self.scene.up_axis,
                nodes: self.scene.nodes.clone(),
                meshes: std::mem::take(&mut self.scene.meshes),
                materials: std::mem::take(&mut self.scene.materials),
                dependencies: self.scene.dependencies.clone(),
                provenance: self.scene.provenance.clone(),
                metadata: self.scene.metadata.clone(),
            }
        } else {
            self.scene.clone()
        };
        let result = (|| {
            apply_scene_edits(&mut candidate, edits, &self.options.scene_limits)?;
            if light {
                validation::require_valid_structure(&candidate, &self.options.scene_limits)?;
            } else {
                require_valid_scene(&candidate, &self.options.scene_limits)?;
            }
            let metadata_usage = metadata::measure_metadata(&candidate.metadata, &self.options.metadata_limits)?;
            let indexes = build_indexes(&candidate, &self.options)?;
            Ok((indexes, metadata_usage))
        })();
        let (indexes, metadata_usage) = match result {
            Ok(v) => v,
            Err(e) => {
                if light {
                    self.scene.meshes = std::mem::take(&mut candidate.meshes);
                    self.scene.materials = std::mem::take(&mut candidate.materials);
                }
                return Err(e);
            }
        };
        if light {
            self.scene.meshes = std::mem::take(&mut candidate.meshes);
            self.scene.materials = std::mem::take(&mut candidate.materials);
        }
        let affected = if light {
            diff_scene_headers(&self.scene, &candidate)
        } else {
            diff_scenes(&self.scene, &candidate)
        };
        let geometry = candidate
            .provenance
            .operations
            .iter()
            .skip(self.scene.provenance.operations.len())
            .filter_map(|operation| serde_json::from_str::<GeometryReport>(&operation.detail).ok())
            .collect::<Vec<_>>();
        let diagnostics = geometry
            .iter()
            .filter(|report| {
                matches!(
                    report.operation.as_str(),
                    "simplify_mesh" | "weld_vertices" | "remove_degenerate_faces" | "generate_normals"
                )
            })
            .map(|report| {
                SceneDiagnostic::warning(
                    "geometry.explicit_conversion",
                    report.mesh.as_deref().unwrap_or("/"),
                    report.notes.join(" "),
                )
            })
            .collect();
        let report = EditReport {
            generation: next_generation,
            geometry,
            affected,
            diagnostics,
        };
        check(&report)?;
        if light {
            candidate.meshes = std::mem::take(&mut self.scene.meshes);
            candidate.materials = std::mem::take(&mut self.scene.materials);
        }
        self.revision = revision;
        self.metadata_usage = metadata_usage;
        self.scene = candidate;
        self.indexes = indexes;
        if edits.iter().any(|e| {
            !matches!(
                e,
                SceneEdit::Rename { .. }
                    | SceneEdit::SetMetadata { .. }
                    | SceneEdit::SetVisibility { .. }
                    | SceneEdit::AssignMaterial { .. }
                    | SceneEdit::ReplaceMaterial { .. }
                    | SceneEdit::RebindMaterial { .. }
            )
        }) {
            self.caches = RefCell::default();
        }
        self.generation = next_generation;
        Ok(report)
    }

    fn apply_rename_only(
        &mut self,
        edits: &[SceneEdit],
        check: impl FnOnce(&EditReport) -> Result<(), SceneError>,
    ) -> Result<EditReport, SceneError> {
        let generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| SceneError::new("scene.generation", "/", "generation exhausted"))?;
        let revision = stream_hash(&(&self.revision, edits))?;
        let mut patches = BTreeMap::new();
        for edit in edits {
            let SceneEdit::Rename { node, name } = edit else {
                unreachable!()
            };
            self.require_node(node)?;
            patches.insert(node.clone(), name.clone());
        }
        let mut affected = patches
            .iter()
            .filter(|(id, name)| self.scene.nodes[*id].name != **name)
            .map(|(id, _)| SceneChange {
                entity: format!("node:{id}"),
                change: "modified".into(),
                fields: vec!["name".into()],
            })
            .collect::<Vec<_>>();
        affected.push(SceneChange {
            entity: "scene".into(),
            change: "modified".into(),
            fields: vec!["provenance".into()],
        });
        let report = EditReport {
            generation,
            geometry: Vec::new(),
            affected,
            diagnostics: Vec::new(),
        };
        check(&report)?;
        for (id, name) in patches {
            self.scene.nodes.get_mut(&id).expect("validated ID").name = name;
        }
        self.indexes
            .by_name
            .sort_by(|a, b| (&self.scene.nodes[a].name, a).cmp(&(&self.scene.nodes[b].name, b)));
        for edit in edits {
            let SceneEdit::Rename { node, .. } = edit else {
                unreachable!()
            };
            self.scene.provenance.operations.push(SceneOperation {
                operation: "edit".into(),
                detail: format!("renamed {node}; stable identity and source path retained"),
            });
        }
        self.generation = generation;
        self.revision = revision;
        Ok(report)
    }
    fn apply_metadata_only(
        &mut self,
        edits: &[SceneEdit],
        check: impl FnOnce(&EditReport) -> Result<(), SceneError>,
    ) -> Result<EditReport, SceneError> {
        let generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| SceneError::new("scene.generation", "/", "generation exhausted"))?;
        let revision = stream_hash(&(&self.revision, edits))?;
        let mut patches: BTreeMap<MetadataTarget, Metadata> = BTreeMap::new();
        for edit in edits {
            let SceneEdit::SetMetadata {
                target,
                namespace,
                key,
                value,
            } = edit
            else {
                unreachable!()
            };
            self.require_target(target)?;
            if key.is_empty() {
                return Err(SceneError::new("metadata.key", key, "metadata key must not be empty"));
            }
            let record = patches
                .entry(target.clone())
                .or_insert_with(|| self.scene.metadata.get(target).cloned().unwrap_or_default());
            let values = match namespace {
                MetadataNamespace::Original => &mut record.original,
                MetadataNamespace::Normalized => &mut record.normalized,
            };
            if let Some(value) = value {
                values.insert(key.clone(), value.clone());
            } else {
                values.remove(key);
            }
        }
        let defaults = MetadataLimits::default();
        let limits = MetadataLimits {
            max_bytes: self.options.metadata_limits.max_bytes.min(defaults.max_bytes),
            max_property_bytes: self
                .options
                .metadata_limits
                .max_property_bytes
                .min(defaults.max_property_bytes),
            max_depth: self.options.metadata_limits.max_depth.min(defaults.max_depth),
            max_properties: self.options.metadata_limits.max_properties.min(defaults.max_properties),
        };
        let mut usage = self.metadata_usage;
        let mut affected = Vec::new();
        let mut index_bytes = self.indexes.estimated_bytes;
        let mut index_updates = Vec::new();
        for (target, record) in &patches {
            let old = self.scene.metadata.get(target);
            let before = old
                .map(|old| metadata::measure_metadata_record(target, old, &limits))
                .transpose()?
                .unwrap_or_default();
            let after = metadata::measure_metadata_record(target, record, &limits)?;
            usage.bytes = usage.bytes.saturating_sub(before.bytes).saturating_add(after.bytes);
            usage.properties = usage
                .properties
                .saturating_sub(before.properties)
                .saturating_add(after.properties);
            let entity = match target {
                MetadataTarget::Scene => "metadata:scene".into(),
                MetadataTarget::Node(id) => format!("metadata:node:{id}"),
                MetadataTarget::Mesh(id) => format!("metadata:mesh:{id}"),
                MetadataTarget::Material(id) => format!("metadata:material:{id}"),
            };
            let fields = [
                ("original", old.is_none_or(|old| old.original != record.original)),
                ("normalized", old.is_none_or(|old| old.normalized != record.normalized)),
            ]
            .into_iter()
            .filter(|(_, changed)| *changed)
            .map(|(name, _)| name.into())
            .collect::<Vec<_>>();
            if old.is_none() || !fields.is_empty() {
                affected.push(SceneChange {
                    entity,
                    change: if old.is_some() { "modified" } else { "added" }.into(),
                    fields,
                });
            }
            if let MetadataTarget::Node(id) = target {
                for index in &self.options.metadata_indexes {
                    let property = |record: &Metadata| {
                        match index.namespace {
                            MetadataNamespace::Normalized => record.normalized.get(&index.key),
                            MetadataNamespace::Original => record.original.get(&index.key),
                        }
                        .and_then(MetadataProperty::resolved)
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(json_error)
                    };
                    let before = old.map(property).transpose()?.flatten();
                    let after = property(record)?;
                    if before == after {
                        continue;
                    }
                    if let Some(value) = &before {
                        index_bytes = index_bytes.saturating_sub(index.key.len() + value.len() + id.len() + 64);
                    }
                    if let Some(value) = &after {
                        index_bytes = index_bytes.saturating_add(index.key.len() + value.len() + id.len() + 64);
                    }
                    index_updates.push((
                        id.clone(),
                        metadata_index_key(index.namespace, &index.key),
                        before,
                        after,
                    ));
                }
            }
        }
        if usage.bytes > limits.max_bytes || usage.properties > limits.max_properties {
            return Err(SceneError::new(
                "metadata.limit",
                "/metadata",
                "metadata byte or property limit exceeded",
            ));
        }
        if index_bytes > self.options.query_limits.max_index_bytes {
            return Err(SceneError::new(
                "query.index_limit",
                "/indexes",
                "document index budget exceeded",
            ));
        }
        affected.push(SceneChange {
            entity: "scene".into(),
            change: "modified".into(),
            fields: vec!["provenance".into()],
        });
        let report = EditReport {
            generation,
            geometry: Vec::new(),
            affected,
            diagnostics: Vec::new(),
        };
        check(&report)?;
        for (target, record) in patches {
            *self.scene.metadata.get_or_insert(&target) = record;
        }
        for (id, key, before, after) in index_updates {
            if let Some(value) = before {
                let map_key = (key.clone(), value);
                if let Some(ids) = self.indexes.metadata.get_mut(&map_key) {
                    ids.remove(&id);
                    if ids.is_empty() {
                        self.indexes.metadata.remove(&map_key);
                    }
                }
            }
            if let Some(value) = after {
                self.indexes.metadata.entry((key, value)).or_default().insert(id);
            }
        }
        for edit in edits {
            let SceneEdit::SetMetadata {
                target, namespace, key, ..
            } = edit
            else {
                unreachable!()
            };
            self.scene.provenance.operations.push(SceneOperation {
                operation: "edit".into(),
                detail: format!("updated {namespace:?} metadata {key} on {target:?}"),
            });
        }
        self.metadata_usage = usage;
        self.indexes.estimated_bytes = index_bytes;
        self.generation = generation;
        self.revision = revision;
        Ok(report)
    }
    fn require_node(&self, id: &str) -> Result<&SceneNode, SceneError> {
        self.scene
            .nodes
            .get(id)
            .ok_or_else(|| SceneError::new("scene.missing_node", id, "node not found"))
    }
    fn require_target(&self, target: &MetadataTarget) -> Result<(), SceneError> {
        let exists = match target {
            MetadataTarget::Scene => true,
            MetadataTarget::Node(id) => self.scene.nodes.contains_key(id),
            MetadataTarget::Mesh(id) => self.scene.meshes.contains_key(id),
            MetadataTarget::Material(id) => self.indexes.materials.contains(id),
        };
        if exists {
            Ok(())
        } else {
            Err(SceneError::new(
                "metadata.target",
                "/metadata",
                "metadata target does not exist",
            ))
        }
    }
    fn check_response<T: Serialize>(&self, value: &T) -> Result<(), SceneError> {
        if serialized_size(value)? > self.options.query_limits.max_response_bytes {
            Err(SceneError::new(
                "query.response_limit",
                "/",
                "response exceeds query byte budget",
            ))
        } else {
            Ok(())
        }
    }
    fn subtree(&self, filter: &NodeFilter) -> Result<Option<(usize, usize)>, SceneError> {
        filter
            .subtree
            .as_ref()
            .map(|id| {
                self.require_node(id)?;
                Ok(self.indexes.subtrees[id])
            })
            .transpose()
    }
    fn check_filter(&self, filter: &NodeFilter) -> Result<(), SceneError> {
        if filter.metadata.len() > self.options.query_limits.max_predicates
            || filter
                .ids
                .as_ref()
                .is_some_and(|ids| ids.len() > self.options.query_limits.max_results)
            || serialized_size(filter)? > 64 * 1024
        {
            return Err(SceneError::new(
                "query.filter_limit",
                "/filter",
                "query filter exceeds budget",
            ));
        }
        for m in &filter.metadata {
            if m.key.is_empty() {
                return Err(SceneError::new(
                    "query.filter",
                    "/filter/metadata",
                    "metadata key is empty",
                ));
            }
            if let MetadataPredicate::Equals { value } | MetadataPredicate::Numeric { value, .. } = &m.predicate {
                let mut metadata = SceneMetadata::default();
                metadata
                    .scene
                    .normalized
                    .insert(m.key.clone(), MetadataProperty::authored(value.clone()));
                validate_metadata(&metadata, &self.options.metadata_limits)
                    .map_err(|error| SceneError::new("query.metadata_value", &m.key, error.detail))?;
            }
            if let MetadataPredicate::Numeric { value, .. } = &m.predicate
                && numeric(value, value).is_none()
            {
                return Err(SceneError::new(
                    "query.numeric",
                    "/filter/metadata",
                    "numeric comparison requires finite f64 or canonical i64/u64",
                ));
            }
        }
        Ok(())
    }
    fn candidates(&self, filter: &NodeFilter) -> Option<BTreeSet<&str>> {
        if let Some(ids) = &filter.ids {
            return Some(
                ids.iter()
                    .filter_map(|id| self.scene.nodes.get_key_value(id).map(|(id, _)| id.as_str()))
                    .collect(),
            );
        }
        if let Some(path) = &filter.path
            && path.mode == TextMode::Exact
            && !path.ignore_case
        {
            return Some(self.ids_for_path(&path.value).iter().map(String::as_str).collect());
        }
        for m in &filter.metadata {
            if let MetadataPredicate::Equals { value } = &m.predicate
                && self
                    .options
                    .metadata_indexes
                    .iter()
                    .any(|i| i.namespace == m.namespace && i.key == m.key)
            {
                let key = metadata_index_key(m.namespace, &m.key);
                let value = serde_json::to_string(value).unwrap_or_default();
                return Some(
                    self.indexes
                        .metadata
                        .get(&(key, value))
                        .map(|ids| ids.iter().map(String::as_str).collect())
                        .unwrap_or_default(),
                );
            }
        }
        None
    }

    fn matches(&self, id: &str, filter: &NodeFilter, subtree: Option<&(usize, usize)>) -> bool {
        let node = &self.scene.nodes[id];
        let effective = &self.indexes.effective[id];
        if subtree.is_some_and(|(start, end)| {
            let position = self.indexes.subtrees[id].0;
            position < *start || position >= *end
        }) || filter.name.as_ref().is_some_and(|f| !f.matches(&node.name))
            || filter
                .path
                .as_ref()
                .is_some_and(|f| node.source_path.as_ref().is_none_or(|p| !f.matches(p)))
            || filter
                .node_type
                .is_some_and(|t| (t == NodeType::Mesh) != node.mesh.is_some())
            || filter.visible.is_some_and(|v| v != effective.0)
            || filter.purpose.is_some_and(|p| p != effective.1)
            || filter.material.as_ref().is_some_and(|m| {
                effective.2.as_ref() != Some(m) && !node.subsets.iter().any(|subset| &subset.material == m)
            })
        {
            return false;
        }
        filter.metadata.iter().all(|filter| {
            let property = self.scene.metadata.nodes.get(id).and_then(|m| match filter.namespace {
                MetadataNamespace::Normalized => m.normalized.get(&filter.key),
                MetadataNamespace::Original => m.original.get(&filter.key),
            });
            match &filter.predicate {
                MetadataPredicate::Exists { exists } => property.is_some() == *exists,
                MetadataPredicate::Equals { value } => property.and_then(MetadataProperty::resolved) == Some(value),
                MetadataPredicate::Numeric { comparison, value } => property
                    .and_then(MetadataProperty::resolved)
                    .and_then(|v| numeric(v, value))
                    .is_some_and(|ordering| match comparison {
                        NumericOperator::Less => ordering.is_lt(),
                        NumericOperator::LessOrEqual => ordering.is_le(),
                        NumericOperator::Greater => ordering.is_gt(),
                        NumericOperator::GreaterOrEqual => ordering.is_ge(),
                    }),
            }
        })
    }
    fn project(&self, id: &str, fields: &[QueryField]) -> Result<QueryRow, SceneError> {
        let node = self.require_node(id)?;
        let effective = &self.indexes.effective[id];
        let mut output = BTreeMap::new();
        for field in fields {
            let (name, value) = match field {
                QueryField::Name => ("name", serde_json::json!(node.name)),
                QueryField::SourcePath => ("source_path", serde_json::json!(node.source_path)),
                QueryField::Parent => ("parent", serde_json::json!(node.parent)),
                QueryField::Children => ("children", serde_json::json!(self.children(Some(id))?)),
                QueryField::NodeType => (
                    "node_type",
                    serde_json::json!(if node.mesh.is_some() { "mesh" } else { "group" }),
                ),
                QueryField::Mesh => ("mesh", serde_json::json!(node.mesh)),
                QueryField::Metadata => ("metadata", serde_json::json!(self.scene.metadata.nodes.get(id))),
                QueryField::LocalTransform => ("local_transform", serde_json::json!(node.transform)),
                QueryField::WorldTransform => ("world_transform", serde_json::json!(self.world_transform(id)?)),
                QueryField::Bounds => (
                    "bounds",
                    serde_json::json!({"value":self.bounds(id)?,"unit":"m","derivation":"own_mesh_world_positions","generation":self.generation}),
                ),
                QueryField::EffectiveMaterial => (
                    "effective_material",
                    serde_json::json!({"material":effective.2,"subsets":node.subsets}),
                ),
                QueryField::Visibility => ("visibility", serde_json::json!(effective.0)),
                QueryField::Purpose => ("purpose", serde_json::json!(effective.1)),
                QueryField::Dependencies => ("dependencies", serde_json::json!(self.scene.dependencies)),
                QueryField::SourceHash => ("source_hash", serde_json::json!(self.scene.provenance.source_sha256)),
            };
            output.insert(name.into(), value);
        }
        Ok(QueryRow {
            id: id.into(),
            fields: output,
        })
    }
}

fn numeric(a: &MetadataValue, b: &MetadataValue) -> Option<std::cmp::Ordering> {
    fn integer(v: &MetadataValue) -> Option<i128> {
        match v {
            MetadataValue::I64(v) => v.parse::<i64>().ok().filter(|i| i.to_string() == *v).map(i128::from),
            MetadataValue::U64(v) => v.parse::<u64>().ok().filter(|i| i.to_string() == *v).map(i128::from),
            _ => None,
        }
    }
    match (a, b) {
        (MetadataValue::F64(a), MetadataValue::F64(b)) if a.is_finite() && b.is_finite() => a.partial_cmp(b),
        _ => Some(integer(a)?.cmp(&integer(b)?)),
    }
}
fn metadata_index_key(namespace: MetadataNamespace, key: &str) -> String {
    format!(
        "{}:{key}",
        if namespace == MetadataNamespace::Original {
            "original"
        } else {
            "normalized"
        }
    )
}
fn build_indexes(scene: &Scene, options: &DocumentOptions) -> Result<Indexes, SceneError> {
    if options.metadata_indexes.len() > 32 {
        return Err(SceneError::new(
            "query.index_limit",
            "/indexes",
            "at most 32 metadata indexes are supported",
        ));
    }
    let mut output = Indexes::default();
    let mut index_bytes = 0usize;
    for (id, node) in &scene.nodes {
        index_bytes = index_bytes.saturating_add(
            id.len() * 6
                + node.parent.as_ref().map_or(0, String::len)
                + node.source_path.as_ref().map_or(0, String::len)
                + 256,
        );
        output.ids.push(id.clone());
        output.children.entry(node.parent.clone()).or_default().push(id.clone());
        if let Some(path) = &node.source_path {
            output.paths.entry(path.clone()).or_default().push(id.clone());
        }
        if let Some(metadata) = scene.metadata.nodes.get(id) {
            for index in &options.metadata_indexes {
                let property = match index.namespace {
                    MetadataNamespace::Original => metadata.original.get(&index.key),
                    MetadataNamespace::Normalized => metadata.normalized.get(&index.key),
                };
                if let Some(value) = property.and_then(MetadataProperty::resolved) {
                    let value = serde_json::to_string(value).map_err(json_error)?;
                    index_bytes = index_bytes.saturating_add(index.key.len() + value.len() + id.len() + 64);
                    output
                        .metadata
                        .entry((metadata_index_key(index.namespace, &index.key), value))
                        .or_default()
                        .insert(id.clone());
                }
            }
        }
        if index_bytes > options.query_limits.max_index_bytes {
            return Err(SceneError::new(
                "query.index_limit",
                id,
                "document index budget exceeded",
            ));
        }
    }
    output.by_name = output.ids.clone();
    output
        .by_name
        .sort_by(|a, b| (&scene.nodes[a].name, a).cmp(&(&scene.nodes[b].name, b)));
    output.by_path = output.ids.clone();
    output
        .by_path
        .sort_by(|a, b| (&scene.nodes[a].source_path, a).cmp(&(&scene.nodes[b].source_path, b)));
    let mut queue = VecDeque::from(output.children.get(&None).cloned().unwrap_or_default());
    while let Some(id) = queue.pop_front() {
        let node = &scene.nodes[&id];
        let parent = node.parent.as_ref().and_then(|id| output.effective.get(id));
        let visible = node.visible && parent.is_none_or(|p| p.0);
        let purpose = if node.purpose == Purpose::Inherited {
            parent.map_or(Purpose::Default, |p| p.1)
        } else {
            node.purpose
        };
        let material = node.material.clone().or_else(|| parent.and_then(|p| p.2.clone()));
        output.effective.insert(id.clone(), (visible, purpose, material));
        queue.extend(output.children.get(&Some(id)).into_iter().flatten().cloned());
    }
    output.materials = scene.materials.iter().map(|material| material.id.0.clone()).collect();
    index_bytes = index_bytes.saturating_add(
        output
            .materials
            .iter()
            .map(|id| id.len().saturating_add(64))
            .sum::<usize>(),
    );
    let mut pending = output
        .children
        .get(&None)
        .into_iter()
        .flatten()
        .rev()
        .map(|id| (id.clone(), false))
        .collect::<Vec<_>>();
    let mut ordinal = 0usize;
    while let Some((id, closing)) = pending.pop() {
        if closing {
            output.subtrees.get_mut(&id).expect("opened subtree").1 = ordinal;
        } else {
            output.subtrees.insert(id.clone(), (ordinal, ordinal));
            ordinal += 1;
            pending.push((id.clone(), true));
            pending.extend(
                output
                    .children
                    .get(&Some(id))
                    .into_iter()
                    .flatten()
                    .rev()
                    .map(|id| (id.clone(), false)),
            );
        }
    }
    index_bytes = index_bytes.saturating_add(scene.nodes.len().saturating_mul(64));
    if index_bytes > options.query_limits.max_index_bytes {
        return Err(SceneError::new(
            "query.index_limit",
            "/indexes",
            "document index budget exceeded",
        ));
    }
    output.estimated_bytes = index_bytes;
    Ok(output)
}
fn json_error(error: serde_json::Error) -> SceneError {
    SceneError::new("query.json", "/", error.to_string())
}
fn serialized_size<T: Serialize>(value: &T) -> Result<usize, SceneError> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(buf.len());
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value).map_err(json_error)?;
    Ok(counter.0)
}

fn stream_hash<T: Serialize>(value: &T) -> Result<String, SceneError> {
    struct Hasher(Sha256);
    impl std::io::Write for Hasher {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut hasher = Hasher(Sha256::new());
    serde_json::to_writer(&mut hasher, value).map_err(json_error)?;
    Ok(format!("{:x}", hasher.0.finalize()))
}
