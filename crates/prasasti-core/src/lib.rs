pub mod file;
pub mod words;
pub mod zoom;

/// UTF-16 code-unit offsets into the text.
pub type Range = std::ops::Range<usize>;
