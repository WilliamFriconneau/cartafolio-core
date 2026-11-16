//! Helpers shared by the integration tests: temporary directories, modes, spaces.
#![allow(dead_code)]

use std::error::Error;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use memcp_core::text::Language;
use memcp_core::{Brain, Space};

pub type Res = Result<(), Box<dyn Error>>;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A unique temporary directory (0700), removed at the end of the test.
pub struct Temp(PathBuf);

impl Temp {
    pub fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("memcp-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).ok();
        #[cfg(unix)]
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).ok();
        Temp(p)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
pub fn mode(p: &Path) -> Result<u32, Box<dyn Error>> {
    Ok(fs::metadata(p)?.permissions().mode() & 0o777)
}

/// A space name known to be valid in tests.
pub fn sp(name: &str) -> Space {
    match Space::new(name) {
        Ok(s) => s,
        Err(e) => panic!("test space {name}: {e}"),
    }
}

/// The spaces most tests use.
pub const SPACES: [&str; 4] = ["health", "work", "home", "personal"];

pub fn spaces() -> Vec<Space> {
    SPACES.iter().map(|n| sp(n)).collect()
}

/// A fresh data directory with the usual four spaces, English search.
pub fn brain(t: &Temp) -> Result<Brain, Box<dyn Error>> {
    Ok(Brain::init(
        &t.path().join("data"),
        &spaces(),
        Language::English,
    )?)
}
