# Model boundaries beyond static assets

The neutral core stays application-independent. Existing Revit/Omniverse spokes
export material packages; they do not establish RVT/RFA model ingestion. A future
CAD/BIM adapter should run at an explicit host boundary, produce the same static
Scene/metadata contract, and expose its runtime/licensing/platform requirements.
No proprietary SDK or native CAD dependency belongs in the standard WASM build.

Evaluate adapters against permissioned assemblies from actual authoring tools:
identity across revisions, physical units, placements/shared instances, category
and classification metadata, product/manufacturer IDs, external dependencies,
tessellation tolerances and model/material provenance. Retain original domain
properties in namespaced metadata; project selected catalogue fields separately.
An adapter must distinguish analytic/B-rep source geometry from its tessellated
preview and report loss. Do not treat a filename extension as verified compatibility.

The next decision requires real consumers and sample exports, not a new domain
schema embedded in core. Compare a host-native converter, an existing open static
exchange export and a dedicated portable adapter using measured fidelity/time/
memory/download costs. The current portable static pipeline can consume the
resulting supported USD/GLB snapshots today.

Animation, skeletons/skinning, morph targets, curves, point clouds and volumes are
separate projects. Each needs a time/sample contract, interpolation/skinning or
schema semantics, bounds/selection rules, fidelity fixtures and matching browser
consumers. The current native adapter may select a static time sample; it does not
preserve an animation system or imply animated export support.

Rendering, shader compilation, procedural/AI generation and application UI stay
in consumers. The toolbox supplies portable scene/material data, transformations,
metadata, queries, diagnostics and derived exports. This boundary is deliberate
and prevents a model catalogue from inheriting a full DCC/runtime dependency.
