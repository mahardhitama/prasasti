pub mod file;
pub mod markdown;
pub mod search;
pub mod words;
pub mod zoom;

/// UTF-16 code-unit offsets into the text.
pub type Range = std::ops::Range<usize>;
