//! Format-neutral material data and conversion contracts.
//!
//! This crate performs no filesystem or network I/O. Format spokes accept bytes,
//! produce bytes, and share these types on native and WebAssembly targets.

mod capabilities;
mod error;
mod io;
mod model;
mod validation;

pub use capabilities::{Capabilities, Parameter, Support, Target, dry_run, target_capabilities};
pub use error::{ExportError, ImportError};
pub use io::{Export, Exporter, Importer, Input, InputFile};
pub use model::*;
pub use validation::{ValidationIssue, ValidationIssueKind, validate_material, validate_materials};
