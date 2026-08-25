mod document;
mod layout;

pub use document::{Block, BlockKind, Document, DocumentBuilder, Link, LinkId, Span};
pub use layout::{Layout, LinkRange, SourcePos};
