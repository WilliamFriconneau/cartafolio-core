//! A big file between nodes, in pieces: a file over [`files::MAX_INLINE`](crate::files::MAX_INLINE) does not travel
//! inside its `file_put` event. The event carries its digest, its size and the digests of its chunks of
//! [`CHUNK_BYTES`] (the last one shorter), and the chunks travel on their own (see `docs/FEDERATION.md`, « Files in
//! chunks »).
//!
//! A node gives a chunk from the whole file it holds (`files/<space>/<sha256>`, read at the chunk's offset) or from the
//! chunks it has received of a file it does not hold whole yet. It takes a chunk only when the bytes match the digest
//! asked for, and keeps it aside, by file and digest, in `sync-chunks/<space>/<file sha256>/<chunk sha256>` (0600):
//! a transfer cut halfway keeps what came, and the next pass, with this peer or another, asks only for the rest. The
//! file is put together once every chunk is here, and counts as held only when the whole matches its digest; the
//! chunks aside are then removed. A file erased or pruned takes its chunks aside with it.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::brain::Brain;
use crate::error::{Error, Result};
use crate::fsx::{PrivateDir as _, PrivateFile as _};
use crate::space::Space;
use crate::text::hex;

/// The size of a chunk: 4 MiB. Small enough for one request of any node (it reads at most 34 MiB) and for a phone's
/// memory, big enough that a file of 2 GiB is 512 chunks and its list of digests about 34 kB in its event.
pub const CHUNK_BYTES: usize = 4 * 1024 * 1024;

/// Whether `s` is a SHA-256 digest in lowercase hex, and so names a file and never a path.
pub fn is_digest(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The SHA-256 digest of `bytes`, in lowercase hex: what names a chunk.
pub fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// How many chunks a file of `size` bytes has.
pub fn count(size: u64) -> usize {
    usize::try_from(size.div_ceil(CHUNK_BYTES as u64)).unwrap_or(usize::MAX)
}

/// The digests of the chunks of the file at `path`, read once, a chunk at a time.
pub fn hashes_of(path: &Path) -> Result<Vec<String>> {
    let mut f = File::open(path)?;
    let mut buf = vec![0u8; CHUNK_BYTES];
    let mut out = vec![];
    loop {
        let n = read_full(&mut f, &mut buf)?;
        if n == 0 {
            break;
        }
        out.push(hex(&Sha256::digest(&buf[..n])));
        if n < CHUNK_BYTES {
            break;
        }
    }
    Ok(out)
}

/// Reads until `buf` is full or the end: how many bytes came.
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

fn staging(brain: &Brain, space: &Space, file: &str) -> PathBuf {
    brain
        .dir()
        .join("sync-chunks")
        .join(space.as_str())
        .join(file)
}

/// Chunk `n` of the file `file` (its digest) of `space`, as this node can give it: read from the whole file it holds,
/// else the chunk of digest `hash` it keeps aside. `None` when it has neither. The bytes are not checked here: the
/// node that takes them does.
pub fn read(
    brain: &Brain,
    space: &Space,
    file: &str,
    n: usize,
    hash: Option<&str>,
) -> Result<Option<Vec<u8>>> {
    if !is_digest(file) {
        return Ok(None);
    }
    let whole = crate::files::blob_path(brain, space, file);
    if let Ok(mut f) = File::open(&whole) {
        let len = f.metadata()?.len();
        let start = (n as u64).saturating_mul(CHUNK_BYTES as u64);
        if start >= len {
            return Ok(None);
        }
        f.seek(SeekFrom::Start(start))?;
        let mut buf =
            vec![0u8; usize::try_from((len - start).min(CHUNK_BYTES as u64)).unwrap_or(0)];
        f.read_exact(&mut buf)?;
        return Ok(Some(buf));
    }
    let Some(h) = hash.filter(|h| is_digest(h)) else {
        return Ok(None);
    };
    match fs::read(staging(brain, space, file).join(h)) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Keeps aside a chunk of the file `file` of `space`, received from a peer. Refused when the bytes do not match `hash`
/// or are more than a chunk, or when no event of the space asked for that file: nothing is written then.
pub fn store(brain: &Brain, space: &Space, file: &str, hash: &str, bytes: &[u8]) -> Result<()> {
    if !is_digest(file) || !is_digest(hash) {
        return Err(Error::Invalid("a chunk is named by digests".into()));
    }
    if bytes.is_empty() || bytes.len() > CHUNK_BYTES {
        return Err(Error::Invalid("a chunk is at most 4 MiB".into()));
    }
    if hex(&Sha256::digest(bytes)) != hash {
        return Err(Error::Invalid("the chunk does not match its digest".into()));
    }
    let dir = staging(brain, space, file);
    // Only for a file an event of this space asked for: a peer cannot fill this node with chunks of nothing.
    if !dir.is_dir() {
        return Err(Error::Invalid(
            "no file of this space waits for this chunk".into(),
        ));
    }
    let path = dir.join(hash);
    if path.exists() {
        return Ok(());
    }
    let _writing = crate::recover::writing(brain)?;
    let tmp = dir.join(format!(".{hash}.tmp"));
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .private_file()
        .open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    fs::rename(&tmp, &path)?;
    crate::fsx::sync_dir(&dir)?;
    Ok(())
}

/// Marks the file `sha` of `space` as waited for: its chunks are taken from then on (see [`store`]).
pub(crate) fn want(brain: &Brain, space: &Space, sha: &str) -> Result<()> {
    if is_digest(sha) {
        DirBuilder::new()
            .recursive(true)
            .private_dir()
            .create(staging(brain, space, sha))?;
    }
    Ok(())
}

/// The chunks of `hashes` this node lacks for the file `file` of `space`, by their position: none when it holds the
/// whole file.
pub fn missing(brain: &Brain, space: &Space, file: &str, hashes: &[String]) -> Vec<usize> {
    if crate::files::blob_path(brain, space, file).exists() {
        return vec![];
    }
    let dir = staging(brain, space, file);
    hashes
        .iter()
        .enumerate()
        .filter(|(_, h)| !is_digest(h) || !dir.join(h.as_str()).exists())
        .map(|(i, _)| i)
        .collect()
}

/// Puts the file together from its chunks aside, once they are all here: written beside the files of the space,
/// checked against `sha` and `size`, then kept and its chunks aside removed. `Ok(false)` while some are missing; an
/// error when the whole does not match (the chunks aside are then removed: they cannot make this file).
pub(crate) fn assemble(
    brain: &Brain,
    space: &Space,
    sha: &str,
    size: u64,
    hashes: &[String],
) -> Result<bool> {
    let target = crate::files::blob_path(brain, space, sha);
    if crate::files::whole(&target, size) {
        drop_staged(brain, space, sha);
        return Ok(true);
    }
    // Bytes of another size under that name (torn by a crash) are not the file: put together again.
    if target.exists() {
        fs::remove_file(&target)?;
    }
    if !missing(brain, space, sha, hashes).is_empty() {
        return Ok(false);
    }
    let dir = staging(brain, space, sha);
    let blobs = target
        .parent()
        .ok_or_else(|| Error::Schema("no folder for the files".into()))?
        .to_path_buf();
    DirBuilder::new()
        .recursive(true)
        .private_dir()
        .create(&blobs)?;
    let tmp = blobs.join(format!(".{sha}.assembling"));
    let mut out = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .private_file()
        .open(&tmp)?;
    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    for h in hashes {
        let bytes = fs::read(dir.join(h))?;
        hasher.update(&bytes);
        out.write_all(&bytes)?;
        total += bytes.len() as u64;
    }
    out.sync_all()?;
    drop(out);
    if total != size || hex(&hasher.finalize()) != sha {
        let _ = fs::remove_file(&tmp);
        drop_staged(brain, space, sha);
        return Err(Error::Invalid(
            "the chunks put together do not match the file's digest".into(),
        ));
    }
    fs::rename(&tmp, &target)?;
    crate::fsx::sync_dir(&blobs)?;
    drop_staged(brain, space, sha);
    Ok(true)
}

/// Removes the chunks kept aside for the file `sha` of `space` (put together, erased, pruned or refused).
pub(crate) fn drop_staged(brain: &Brain, space: &Space, sha: &str) {
    if is_digest(sha) {
        let _ = fs::remove_dir_all(staging(brain, space, sha));
    }
}

/// How many chunks of the file `sha` of `space` this node keeps aside (for tests and the status).
pub fn staged(brain: &Brain, space: &Space, sha: &str) -> usize {
    if !is_digest(sha) {
        return 0;
    }
    fs::read_dir(staging(brain, space, sha)).map_or(0, |d| {
        d.flatten()
            .filter(|e| is_digest(&e.file_name().to_string_lossy()))
            .count()
    })
}
