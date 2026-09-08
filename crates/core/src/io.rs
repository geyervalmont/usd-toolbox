//! Bytes-in, bytes-out contracts shared by every format spoke.

use crate::{Capabilities, ExportError, ImportError, Loss, Material};

/// A named member of an in-memory input bundle.
#[derive(Clone, Copy, Debug)]
pub struct InputFile<'a> {
    /// Bundle-relative name, always using `/` separators.
    pub name: &'a str,
    /// Complete file contents.
    pub bytes: &'a [u8],
}

/// Input supplied to an importer without imposing filesystem or async I/O.
#[derive(Clone, Copy, Debug)]
pub enum Input<'a> {
    /// A single unnamed byte buffer.
    Bytes(&'a [u8]),
    /// A single named byte buffer.
    Named {
        /// Source name used for diagnostics and format detection.
        name: &'a str,
        /// Complete source contents.
        bytes: &'a [u8],
    },
    /// An ordered collection of named buffers.
    Bundle(&'a [InputFile<'a>]),
}

impl<'a> Input<'a> {
    /// Returns the only byte buffer, or `None` for a bundle.
    #[must_use]
    pub const fn single_bytes(self) -> Option<&'a [u8]> {
        match self {
            Self::Bytes(bytes) | Self::Named { bytes, .. } => Some(bytes),
            Self::Bundle(_) => None,
        }
    }

    /// Returns the optional source name for a single input.
    #[must_use]
    pub const fn name(self) -> Option<&'a str> {
        match self {
            Self::Named { name, .. } => Some(name),
            Self::Bytes(_) | Self::Bundle(_) => None,
        }
    }
}

/// The result of a format export.
#[derive(Clone, Debug, PartialEq)]
pub struct Export {
    /// Encoded output bytes.
    pub bytes: Vec<u8>,
    /// Ordered, deterministic list of fidelity losses.
    pub losses: Vec<Loss>,
}

/// A format spoke which converts bytes into neutral materials.
pub trait Importer {
    /// Import-specific configuration.
    type Options: Default;

    /// Decode the supplied in-memory input.
    fn import(&self, input: Input<'_>, options: &Self::Options) -> Result<Vec<Material>, ImportError>;
}

/// A format spoke which converts neutral materials into bytes.
pub trait Exporter {
    /// Export-specific configuration.
    type Options: Default;

    /// Describes what this exporter can preserve.
    fn capabilities(&self) -> Capabilities;

    /// Encode materials and return every fidelity loss alongside the bytes.
    fn export(&self, materials: &[Material], options: &Self::Options) -> Result<Export, ExportError>;

    /// Compute the deterministic loss report without writing output.
    fn dry_run(&self, materials: &[Material]) -> Vec<Loss> {
        self.capabilities().assess(materials)
    }
}
