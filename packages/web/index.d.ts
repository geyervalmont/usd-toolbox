export type Bytes = ArrayBuffer | Uint8Array;
export interface Progress { phase: string }
export interface RequestOptions { signal?: AbortSignal; onProgress?: (progress: Progress) => void }
export interface Asset { name: string; bytes: Bytes }
export interface OpenOptions extends RequestOptions {
  name?: string;
  assets?: Asset[];
  document?: DocumentOptions;
  /** Transfer whole buffers to the worker. Defaults to copying; slices are copied. */
  transfer?: boolean;
}
export interface PreflightOptions extends OpenOptions {
  profile?: { target?: 'usd' | 'usdz' | 'glb' | 'browser_preview'; check_geometry?: boolean; max_specs?: number; max_issues?: number; max_metadata_bytes?: number };
}
export interface ToolboxOptions {
  wasmModuleUrl?: string | URL;
  wasmUrl?: string | URL;
  workerUrl?: string | URL;
  workerFactory?: () => Worker;
}
export type MetadataValue =
  | { type: 'null' }
  | { type: 'bool'; value: boolean }
  | { type: 'string' | 'i64' | 'u64'; value: string }
  | { type: 'f64'; value: number }
  | { type: 'array'; value: MetadataValue[] }
  | { type: 'dictionary'; value: Record<string, MetadataValue> }
  | { type: 'typed'; value: { value_type: string; value: MetadataValue } }
  | { type: 'typed_array'; value: { element_type: string; values: MetadataValue[] } };
export interface MetadataProperty {
  value: MetadataValue;
  state?: 'authored' | 'defaulted' | 'inherited' | 'unresolved';
  source?: { format: string; path: string; property: string | null; value_type: string | null } | null;
  unit?: string | null;
}
export interface DocumentOptions {
  metadata_indexes?: { namespace?: MetadataNamespace; key: string }[];
  metadata_limits?: Partial<{ max_bytes: number; max_property_bytes: number; max_depth: number; max_properties: number }>;
  query_limits?: Partial<{ max_results: number; max_response_bytes: number; max_predicates: number; max_index_bytes: number; max_cache_bytes: number }>;
}
export interface Metadata { original: Record<string, MetadataProperty>; normalized: Record<string, MetadataProperty> }
export type MetadataTarget = { scope: 'scene' } | { scope: 'node' | 'mesh' | 'material'; id: string };
export type MetadataNamespace = 'original' | 'normalized';
export interface TextFilter { value: string; mode?: 'exact' | 'prefix' | 'contains'; ignore_case?: boolean }
export type MetadataFilter = { namespace?: MetadataNamespace; key: string } & (
  | { operator: 'exists'; exists: boolean }
  | { operator: 'equals'; value: MetadataValue }
  | { operator: 'numeric'; comparison: 'less' | 'less_or_equal' | 'greater' | 'greater_or_equal'; value: MetadataValue }
);
export interface NodeFilter {
  ids?: string[]; name?: TextFilter; path?: TextFilter; node_type?: 'group' | 'mesh';
  metadata?: MetadataFilter[]; material?: string; visible?: boolean;
  purpose?: 'inherited' | 'default' | 'render' | 'proxy' | 'guide'; subtree?: string;
}
export type QueryField = 'name' | 'source_path' | 'parent' | 'children' | 'node_type' | 'mesh' | 'metadata' | 'local_transform' | 'world_transform' | 'bounds' | 'effective_material' | 'visibility' | 'purpose' | 'dependencies' | 'source_hash';
export interface QueryRequest {
  filter?: NodeFilter; fields?: QueryField[]; sort?: 'id' | 'name' | 'path';
  descending?: boolean; limit?: number; cursor?: string | null;
}
export type Matrix4 = [[number, number, number, number], [number, number, number, number], [number, number, number, number], [number, number, number, number]];
export interface Bounds { min: [number, number, number]; max: [number, number, number] }
export interface Dependency { authored_path: string; resolved_path: string; sha256: string; bytes: number }
export interface NodeFields {
  name: string; source_path: string | null; parent: string | null; children: string[];
  node_type: 'group' | 'mesh'; mesh: string | null; metadata: Metadata | null;
  local_transform: Matrix4; world_transform: Matrix4;
  bounds: { value: Bounds | null; unit: 'm'; derivation: string; generation: number };
  effective_material: { material: string | null; subsets: { name: string; faces: number[]; material: string }[] }; visibility: boolean; purpose: string;
  dependencies: Dependency[]; source_hash: string | null;
}
export interface QueryRow { id: string; fields: Partial<NodeFields> }
export interface QueryPage { schema: string; generation: number; rows: QueryRow[]; next_cursor: string | null }
export interface Diagnostic { code: string; severity: 'error' | 'warning' | 'info'; path: string; detail: string }
export interface Summary { generation: number; nodes: number; meshes: number; materials: number; roots: number; dependencies: number; indexed_metadata: { namespace: MetadataNamespace; key: string }[] }
export interface GeometryReport { operation: string; mesh: string | null; vertices_before: number; vertices_after: number; faces_before: number; faces_after: number; max_vertex_displacement: number; notes: string[] }
export interface EditReport { generation: number; affected: { entity: string; change: string; fields: string[] }[]; diagnostics: Diagnostic[]; geometry: GeometryReport[] }
export interface SceneNode {
  name: string; source_path: string | null; parent: string | null; transform: Matrix4;
  reset_transform?: boolean; mesh: string | null; visible?: boolean; purpose?: NodeFilter['purpose'];
  material: string | null; subsets?: { name: string; faces: number[]; material: string }[];
}
/** Full material/scene interchange objects come from the Rust neutral schema. */
export type NeutralObject = Record<string, unknown>;
export type SceneEdit =
  | { operation: 'rename'; node: string; name: string }
  | { operation: 'reparent'; node: string; parent: string | null; preserve_world?: boolean; preserve_inherited?: boolean }
  | { operation: 'add_node'; id: string; node: SceneNode }
  | { operation: 'remove_subtree' | 'bake_transform'; node: string }
  | { operation: 'duplicate_subtree'; node: string; prefix: string; parent: string | null; preserve_world?: boolean }
  | { operation: 'merge_scene'; scene: NeutralObject; prefix: string; parent: string | null }
  | { operation: 'set_pivot'; node: string; pivot: [number, number, number] }
  | { operation: 'convert_axis'; up_axis: 'Y' | 'Z' }
  | { operation: 'set_metadata'; target: MetadataTarget; namespace?: MetadataNamespace; key: string; value: MetadataProperty | null }
  | { operation: 'set_transform'; node: string; transform: Matrix4 }
  | { operation: 'convert_units'; meters_per_unit: number }
  | { operation: 'replace_material'; material: NeutralObject }
  | { operation: 'rebind_material'; source: string; material: NeutralObject }
  | { operation: 'assign_material'; node: string; material: string }
  | { operation: 'set_visibility'; node: string; visible: boolean }
  | { operation: 'triangulate_mesh'; mesh: string }
  | { operation: 'generate_normals'; mesh: string; crease_angle_degrees: number; area_weighted?: boolean }
  | { operation: 'simplify_mesh'; mesh: string; target_faces: number; max_vertex_displacement: number }
  | { operation: 'weld_vertices'; mesh: string; tolerance: number }
  | { operation: 'remove_degenerate_faces'; mesh: string; area_tolerance?: number };
export interface ExportResult { bytes: Uint8Array; diagnostics: Diagnostic[] }
export interface LodOptions {
  levels: { name: string; face_ratio: number; max_vertex_displacement: number }[];
  max_levels?: number; max_output_bytes?: number; max_output_faces?: number; max_source_corners?: number;
}
export interface LodManifest {
  schema: 'usd-toolbox.geometry-lods.v1'; source_scene_id: string; source_scene_sha256: string;
  source_sha256: string | null; meters_per_unit: number; error_policy: string;
  target: 'glb' | 'usdz'; import_diagnostics: Diagnostic[];
  files: { name: string; level: string; bytes: number; sha256: string; diagnostics: Diagnostic[] }[];
  levels: { name: string; face_ratio: number; displacement_limit: number; target_reached: boolean;
    meshes: { mesh: string; target_faces: number; target_reached: boolean; geometry: GeometryReport }[] }[];
}
export interface LodExport { manifest: LodManifest; files: (ExportResult & { name: string; level: string })[] }
export interface ExportOptions extends RequestOptions {
  /** glTF/GLB only: maximum texture edge; never enlarges smaller inputs. Default 2k. */
  textureTier?: 'preview' | '1k' | '2k' | '4k' | '8k';
  /** glTF/GLB only. Default false. */
  includeHidden?: boolean;
  /** glTF/GLB only. Opaque-primitive cache optimization; default false. */
  optimizeVertexCache?: boolean;
}
export class ToolboxError extends Error { readonly code: string; readonly details?: unknown }
export class Toolbox {
  constructor(options?: ToolboxOptions);
  readonly closed: boolean;
  ready(): Promise<{ ready: boolean }>;
  open(input: Bytes, options?: OpenOptions): Promise<Model>;
  preflight(input: Bytes, options?: PreflightOptions): Promise<Record<string, unknown>>;
  capabilities(options?: RequestOptions): Promise<Record<string, unknown>>;
  memory(options?: RequestOptions): Promise<{ wasm_bytes: number | null; models: number }>;
  /** Terminates the worker and invalidates every model and pending operation. */
  dispose(): void;
}
export class Model {
  inspect(options?: RequestOptions): Promise<Record<string, unknown>>;
  diagnostics(options?: RequestOptions): Promise<Diagnostic[]>;
  query(request?: QueryRequest, options?: RequestOptions): Promise<QueryPage>;
  getNode(id: string, options?: RequestOptions): Promise<QueryRow | null>;
  getMetadata(target?: MetadataTarget, options?: RequestOptions): Promise<Metadata | null>;
  getProperty(target: MetadataTarget, key: string, namespace?: MetadataNamespace, options?: RequestOptions): Promise<MetadataProperty | null>;
  children(id?: string | null, options?: RequestOptions): Promise<string[]>;
  ancestors(id: string, options?: RequestOptions): Promise<string[]>;
  descendants(id: string, options?: RequestOptions): Promise<string[]>;
  idsForPath(path: string, options?: RequestOptions): Promise<string[]>;
  count(filter?: NodeFilter, options?: RequestOptions): Promise<number>;
  summary(options?: RequestOptions): Promise<Summary>;
  apply(edits: SceneEdit[], options?: RequestOptions): Promise<EditReport>;
  export(target?: 'usdz' | 'usda' | 'usdc' | 'glb' | 'gltf', options?: ExportOptions): Promise<ExportResult>;
  exportLods(lods: LodOptions, target?: 'glb' | 'usdz', options?: ExportOptions): Promise<LodExport>;
  assets(options?: RequestOptions): Promise<Map<string, Uint8Array>>;
  extract(id: string, options?: RequestOptions): Promise<Model>;
  dispose(): Promise<void>;
}
