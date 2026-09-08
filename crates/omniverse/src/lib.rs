//! NVIDIA Omniverse-compatible material export.
//!
//! Omniverse's native interchange boundary is OpenUSD. This adapter keeps a
//! named, independently evolvable target while delegating byte production to
//! the tested USDZ writer. A future connector can add MDL render contexts
//! without changing callers or the neutral material model.

use serde::{Deserialize, Serialize};
use usd_toolbox_core::{Capabilities, Export, ExportError, Exporter, Material, Target, Tier, target_capabilities};
use usd_toolbox_usd::{UsdExportOptions, UsdExporter, UsdFormat};

/// Omniverse export settings.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct OmniverseExportOptions {
    /// Preferred texture tier connected into the USD shader graph.
    pub texture_tier: Tier,
    /// Include a separate provenance layer inside the USDZ package.
    pub include_provenance_mirror: bool,
}

impl Default for OmniverseExportOptions {
    fn default() -> Self {
        Self {
            texture_tier: Tier::K2,
            include_provenance_mirror: true,
        }
    }
}

/// Omniverse/OpenUSD exporter.
#[derive(Clone, Copy, Debug, Default)]
pub struct OmniverseExporter;

impl Exporter for OmniverseExporter {
    type Options = OmniverseExportOptions;

    fn capabilities(&self) -> Capabilities {
        target_capabilities(Target::Omniverse)
    }

    fn export(&self, materials: &[Material], options: &Self::Options) -> Result<Export, ExportError> {
        UsdExporter.export(
            materials,
            &UsdExportOptions {
                format: UsdFormat::Usdz,
                graph_tier: options.texture_tier,
                include_provenance_mirror: options.include_provenance_mirror,
            },
        )
    }
}
