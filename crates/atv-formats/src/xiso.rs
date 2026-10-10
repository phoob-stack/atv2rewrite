//! Xbox disc filesystem (XISO / XDVDFS).

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};

use crate::{Error, Result};

const SECTOR: u64 = 2048;
const MAGIC: &[u8; 20] = b"MICROSOFT*XBOX*MEDIA";

#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub sector: u32,
    pub size: u32,
    pub is_dir: bool,
}

/// An opened disc image with its full file table loaded.
pub struct Xiso<R> {
    inner: R,
    base: u64,
    /// Lower-cased path ("/folder/file.ext") -> entry.
    pub files: BTreeMap<String, Entry>,
}

impl<R: Read + Seek> Xiso<R> {
    pub fn open(mut inner: R) -> Result<Self> {
        // Redump-style images start at the game partition; full DVD dumps put
        // it at 0x18300000 behind a video partition.
        for base in [0u64, 0x1830_0000, 0x0FD9_0000, 0x0208_0000] {
            let mut hdr = [0u8; 28];
            if inner.seek(SeekFrom::Start(base + 32 * SECTOR)).is_err() {
                continue;
            }
            if inner.read_exact(&mut hdr).is_err() || &hdr[..20] != MAGIC {
                continue;
            }
            let root_sector = u32::from_le_bytes(hdr[20..24].try_into().unwrap());
            let root_size = u32::from_le_bytes(hdr[24..28].try_into().unwrap());
            let mut me = Self { inner, base, files: BTreeMap::new() };
            me.walk(root_sector, root_size, String::new())?;
            return Ok(me);
        }
        Err(Error::Magic { what: "Xbox disc image", expected: "MICROSOFT*XBOX*MEDIA" })
    }

    fn walk(&mut self, sector: u32, size: u32, path: String) -> Result<()> {
        let mut table = vec![0u8; size as usize];
        self.inner.seek(SeekFrom::Start(self.base + sector as u64 * SECTOR))?;
        self.inner.read_exact(&mut table)?;
        let mut stack = vec![0usize];
        let mut seen = std::collections::HashSet::new();
        let mut subdirs = Vec::new();
        while let Some(off) = stack.pop() {
            let p = off * 4;
            if !seen.insert(off) || p + 14 > table.len() {
                continue;
            }
            let left = u16::from_le_bytes([table[p], table[p + 1]]);
            if left == 0xFFFF {
                continue; // sector padding
            }
            let right = u16::from_le_bytes([table[p + 2], table[p + 3]]);
            let sec = u32::from_le_bytes(table[p + 4..p + 8].try_into().unwrap());
            let sz = u32::from_le_bytes(table[p + 8..p + 12].try_into().unwrap());
            let attr = table[p + 12];
            let nl = table[p + 13] as usize;
            let name = String::from_utf8_lossy(table.get(p + 14..p + 14 + nl).unwrap_or(&[]));
            let full = format!("{path}/{}", name.to_ascii_lowercase());
            let is_dir = attr & 0x10 != 0;
            self.files.insert(full.clone(), Entry { sector: sec, size: sz, is_dir });
            if is_dir && sz > 0 {
                subdirs.push((sec, sz, full));
            }
            if left != 0 {
                stack.push(left as usize);
            }
            if right != 0 {
                stack.push(right as usize);
            }
        }
        for (sec, sz, full) in subdirs {
            self.walk(sec, sz, full)?;
        }
        Ok(())
    }

    pub fn read(&mut self, path: &str) -> Result<Vec<u8>> {
        let key = normalize(path);
        let e = *self.files.get(&key).ok_or_else(|| Error::NotFound(path.to_string()))?;
        self.read_range(e.sector as u64 * SECTOR, e.size as usize)
    }

    pub fn entry(&self, path: &str) -> Option<Entry> {
        self.files.get(&normalize(path)).copied()
    }

    /// Reads `len` bytes at `offset` within the game partition.
    pub fn read_range(&mut self, offset: u64, len: usize) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        self.inner.seek(SeekFrom::Start(self.base + offset))?;
        self.inner.read_exact(&mut buf)?;
        Ok(buf)
    }
}

pub fn normalize(path: &str) -> String {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    if p.starts_with('/') { p } else { format!("/{p}") }
}
