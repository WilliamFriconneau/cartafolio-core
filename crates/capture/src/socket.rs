//! The gateway on a Unix socket, for an assistant that must never know the memory's address nor its key.
//!
//! The gateway runs as a user of its own (a service), which alone reads the settings file. It opens a socket that only
//! the person's account may open: mode 600 (only the gateway's user), or 660 with a group of two (the gateway's user and
//! the person). The kernel checks who connects; there is no secret to copy. The assistant starts `memcp-capture connect
//! PATH`, which only carries its messages to the socket and back: it holds a path, nothing else.
//!
//! Each connection speaks the same as standard input and output: one JSON-RPC message a line.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::{Error, Gateway};

/// Opens the socket at `path`: refused in a folder others could write to (someone could put their own socket there);
/// an old socket left by a crash is replaced, anything else at that path is refused.
pub fn bind(path: &Path, group: Option<u32>) -> Result<UnixListener, Error> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| Error::Invalid("the socket needs a folder".into()))?;
    let mode = std::fs::metadata(parent)
        .map_err(|e| Error::Io(format!("{}: {e}", parent.display())))?
        .permissions()
        .mode();
    if mode & 0o022 != 0 {
        return Err(Error::Invalid(format!(
            "{} can be written by others (mode {:o}): the socket goes in a folder only its owner writes",
            parent.display(),
            mode & 0o777
        )));
    }
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_socket() => {
            std::fs::remove_file(path).map_err(|e| Error::Io(e.to_string()))?;
        }
        Ok(_) => {
            return Err(Error::Invalid(format!(
                "{} exists and is not a socket",
                path.display()
            )));
        }
        Err(_) => {}
    }
    let listener = UnixListener::bind(path).map_err(|e| Error::Io(e.to_string()))?;
    // Closed first, then opened to the one group asked: never wider for a moment than it ends.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| Error::Io(e.to_string()))?;
    if let Some(gid) = group {
        std::os::unix::fs::chown(path, None, Some(gid)).map_err(|e| Error::Io(e.to_string()))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))
            .map_err(|e| Error::Io(e.to_string()))?;
    }
    Ok(listener)
}

/// Serves `gateway` on `listener` until the process stops. Each connection has its own thread; the messages of all
/// go through the gateway one at a time.
pub fn serve_socket(gateway: Gateway, listener: &UnixListener) -> Result<(), Error> {
    let gateway = Arc::new(Mutex::new(gateway));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let g = Arc::clone(&gateway);
        std::thread::spawn(move || {
            let _ = serve_one(&g, stream);
        });
    }
    Ok(())
}

fn serve_one(gateway: &Mutex<Gateway>, stream: UnixStream) -> std::io::Result<()> {
    let mut out = stream.try_clone()?;
    for line in BufReader::new(stream).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = {
            let mut g = gateway.lock().unwrap_or_else(|e| e.into_inner());
            let reply = match serde_json::from_str::<Value>(&line) {
                Ok(message) => g.handle(&message),
                Err(_) => Some(serde_json::json!({
                    "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "Parse error" }
                })),
            };
            g.after_reply();
            reply
        };
        if let Some(reply) = reply {
            writeln!(out, "{reply}")?;
            out.flush()?;
        }
    }
    Ok(())
}

/// What `memcp-capture connect PATH` does: carries standard input to the socket and the socket's answers to standard
/// output, until either side closes. It knows nothing of the memory.
pub fn connect(path: &Path) -> Result<(), Error> {
    let stream = UnixStream::connect(path).map_err(|e| {
        Error::Io(format!(
            "the capture program's socket cannot be opened ({e}): is the capture program running, and is this account allowed?"
        ))
    })?;
    let mut to_gateway = stream.try_clone().map_err(|e| Error::Io(e.to_string()))?;
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut to_gateway);
        let _ = to_gateway.shutdown(std::net::Shutdown::Write);
    });
    let mut from_gateway = stream;
    let mut out = std::io::stdout().lock();
    std::io::copy(&mut from_gateway, &mut out).map_err(|e| Error::Io(e.to_string()))?;
    Ok(())
}
