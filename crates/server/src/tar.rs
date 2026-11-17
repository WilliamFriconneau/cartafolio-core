//! A minimal tar writer (ustar), enough to hand a directory to a browser as one download
//! without another dependency: regular files only, paths up to 255 characters.

use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

fn put(field: &mut [u8], bytes: &[u8]) {
    let n = bytes.len().min(field.len());
    field[..n].copy_from_slice(&bytes[..n]);
}

/// The name and the prefix of a path, so that a long path fits the two fields of ustar.
fn split(path: &str) -> io::Result<(&str, &str)> {
    if path.len() <= 100 {
        return Ok(("", path));
    }
    for (i, c) in path.char_indices().rev() {
        if c == '/' {
            let (prefix, name) = (&path[..i], &path[i + 1..]);
            if prefix.len() <= 155 && name.len() <= 100 {
                return Ok((prefix, name));
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("path too long for tar: {path}"),
    ))
}

fn header(path: &str, size: u64, mtime: u64) -> io::Result<[u8; 512]> {
    let (prefix, name) = split(path)?;
    let mut h = [0u8; 512];
    put(&mut h[0..100], name.as_bytes());
    put(&mut h[100..108], b"0000600\0");
    put(&mut h[108..116], b"0000000\0");
    put(&mut h[116..124], b"0000000\0");
    put(&mut h[124..136], format!("{size:011o}\0").as_bytes());
    put(&mut h[136..148], format!("{mtime:011o}\0").as_bytes());
    h[148..156].copy_from_slice(b"        ");
    h[156] = b'0';
    put(&mut h[257..263], b"ustar\0");
    put(&mut h[263..265], b"00");
    put(&mut h[345..500], prefix.as_bytes());
    let sum: u32 = h.iter().map(|b| u32::from(*b)).sum();
    put(&mut h[148..156], format!("{sum:06o}\0 ").as_bytes());
    Ok(h)
}

fn collect(dir: &Path, base: &Path, out: &mut Vec<String>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(&path, base, out)?;
        } else if let Ok(rel) = path.strip_prefix(base) {
            // An entry of an archive is named with `/`, on every system.
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// The files of `dir`, sorted, as one tar under the top-level folder `top`.
pub fn pack(dir: &Path, top: &str, mtime: u64) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    pack_to(dir, top, mtime, &mut out)?;
    Ok(out)
}

/// Like [`pack`], written to `out` one file at a time: a big memory is never held whole.
pub fn pack_to(dir: &Path, top: &str, mtime: u64, out: &mut impl Write) -> io::Result<()> {
    let mut rels = vec![];
    collect(dir, dir, &mut rels)?;
    rels.sort();
    for rel in rels {
        let mut file = fs::File::open(dir.join(&rel))?;
        let size = file.metadata()?.len();
        out.write_all(&header(&format!("{top}/{rel}"), size, mtime)?)?;
        let copied = io::copy(&mut file, out)?;
        if copied != size {
            return Err(io::Error::other(format!("{rel} changed while it was read")));
        }
        let pad = (512 - (size % 512) as usize) % 512;
        out.write_all(&vec![0u8; pad])?;
    }
    out.write_all(&[0u8; 1024])
}

/// Reads the number in a ustar field (octal, padded with spaces or NULs).
fn octal(field: &[u8]) -> u64 {
    field
        .iter()
        .take_while(|b| **b != 0 && **b != b' ')
        .fold(0, |n, b| n * 8 + u64::from(b.wrapping_sub(b'0') & 7))
}

/// Writes the files of a tar read from `input` under `dest`, one at a time. Only plain files and folders are made, and a
/// name that is absolute or leaves `dest` is refused. Answers the folders made at the top of `dest`.
pub fn unpack(input: &mut impl Read, dest: &Path) -> io::Result<Vec<String>> {
    let mut tops: Vec<String> = vec![];
    loop {
        let mut h = [0u8; 512];
        if input.read_exact(&mut h).is_err() || h.iter().all(|b| *b == 0) {
            return Ok(tops);
        }
        let end = |f: &[u8]| f.iter().position(|b| *b == 0).unwrap_or(f.len());
        let name = String::from_utf8_lossy(&h[..end(&h[..100])]).to_string();
        let prefix = String::from_utf8_lossy(&h[345..345 + end(&h[345..500])]).to_string();
        let full = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let size = octal(&h[124..136]);
        let kind = h[156];
        let safe = !full.is_empty()
            && !full.starts_with('/')
            && !full
                .split('/')
                .any(|p| p == ".." || p.contains('\\') || p.contains(':'));
        if !safe {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("the archive names a path outside the folder: {full}"),
            ));
        }
        let target = dest.join(&full);
        if let Some(top) = full.split('/').next()
            && !tops.iter().any(|t| t == top)
        {
            tops.push(top.to_string());
        }
        match kind {
            b'5' => fs::create_dir_all(&target)?,
            b'0' | 0 => {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut out = fs::File::create(&target)?;
                let copied = io::copy(&mut input.take(size), &mut out)?;
                if copied != size {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the archive ends in the middle of a file",
                    ));
                }
                let pad = (512 - (size % 512)) % 512;
                io::copy(&mut input.take(pad), &mut io::sink())?;
            }
            _ => {
                let skip = size.div_ceil(512) * 512;
                io::copy(&mut input.take(skip), &mut io::sink())?;
            }
        }
    }
}
