//! Private files and folders, on every system (the same helpers as the core's `fsx`: the capture program does not depend on
//! the core, so it keeps its own copy).

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
