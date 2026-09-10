//! SVG input support rendering vector images through [`resvg`].

pub mod decoder;

pub(crate) mod fonts;

pub use decoder::{parse_size_limit, SvgDecoder, SvgOptions, SIZE_LIMIT_MARKER};
