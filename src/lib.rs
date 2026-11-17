//! Library root for fuzz targets and integration use.
//!
//! The binary (`src/main.rs`) and the library compile the same module
//! files; `crate::` paths resolve identically in both. Fuzz targets
//! depend on this crate (`typewriter = { path = ".." }`) because a
//! binary-only crate cannot be imported.

pub mod geometry;
pub mod overlay;
pub mod pdf;
pub mod signature;
#[cfg(test)]
pub mod test_util;
