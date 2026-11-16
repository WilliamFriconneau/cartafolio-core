//! A minimal tar writer (ustar), enough to hand a directory to a browser as one download
//! without another dependency: regular files only, paths up to 255 characters.

use std::fs;
use std::io;
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
    let mut rels = vec![];
    collect(dir, dir, &mut rels)?;
    rels.sort();
    let mut out = Vec::new();
    for rel in rels {
        let bytes = fs::read(dir.join(&rel))?;
        out.extend_from_slice(&header(&format!("{top}/{rel}"), bytes.len() as u64, mtime)?);
        out.extend_from_slice(&bytes);
        out.resize(out.len().div_ceil(512) * 512, 0);
    }
    out.extend_from_slice(&[0u8; 1024]);
    Ok(out)
}
