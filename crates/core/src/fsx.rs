//! Private files and folders, on every system.
//!
//! On Unix a data folder is `0700` and a file `0600`. Windows has no such bits: a folder and its files take the
//! access list of the folder they are made in (the person's own profile), so these calls are then no-ops and
//! the data directory belongs under the person's profile (see docs/SPACES.md, *Running on Windows*).

use std::fs;
use std::io;
use std::path::Path;

/// A folder made private to its owner.
pub trait PrivateDir {
    fn private_dir(&mut self) -> &mut Self;
}

impl PrivateDir for fs::DirBuilder {
    #[cfg(unix)]
    fn private_dir(&mut self) -> &mut Self {
        use std::os::unix::fs::DirBuilderExt;
        self.mode(0o700)
    }

    #[cfg(not(unix))]
    fn private_dir(&mut self) -> &mut Self {
        self
    }
}

/// A file made private to its owner.
pub trait PrivateFile {
    fn private_file(&mut self) -> &mut Self;
}

impl PrivateFile for fs::OpenOptions {
    #[cfg(unix)]
    fn private_file(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;
        self.mode(0o600)
    }

    #[cfg(not(unix))]
    fn private_file(&mut self) -> &mut Self {
        self
    }
}

/// Makes an existing folder private to its owner.
pub fn set_private_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Makes an existing file private to its owner.
pub fn set_private_file(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// The permission bits of a file or folder, where the system has them.
pub fn mode_bits(meta: &fs::Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(meta.permissions().mode())
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

/// Whether the group or others can reach it (always false where the system has no such bits).
pub fn loose(meta: &fs::Metadata) -> bool {
    mode_bits(meta).is_some_and(|m| m & 0o077 != 0)
}
