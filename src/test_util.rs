//! RAII temporary files for tests.
//!
//! Tests hand-rolling `temp_dir().join(format!("{pid}-{nanos}"))`
//! and removing the file only at the end leak the file whenever an
//! assertion panics in between. `TempFile` removes its file on
//! drop, panic or not.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique path in the system temp dir, deleted on drop.
#[derive(Debug)]
pub struct TempFile {
    path: PathBuf,
}

impl TempFile {
    /// New temp `.pdf` path tagged `tag` (not yet created).
    #[must_use]
    pub fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pdf-editor-test-{}-{nanos}-{seq}-{tag}.pdf",
            std::process::id()
        ));
        Self { path }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
