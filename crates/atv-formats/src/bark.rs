//! "Bark 1.06" archive: `ATV.bah` (index) + `ATV.bad` (data).

use std::collections::BTreeMap;

use crate::bin::Reader;
use crate::{lzss, Error, Result};

#[derive(Debug, Clone)]
pub struct BarkEntry {
    /// Path as stored in the index, e.g. `/Cars/Bandit/Body.rom`.
    pub path: String,
    pub offset: u32,
    pub stored_size: u32,
    pub raw_size: u32,
    /// Content hash from the index (not CRC32; unused for loading).
    pub hash: u32,
}

#[derive(Debug, Default)]
pub struct BarkIndex {
    /// Lower-cased path -> entry.
    pub entries: BTreeMap<String, BarkEntry>,
}

impl BarkIndex {
    pub fn parse(bah: &[u8]) -> Result<Self> {
        if !bah.starts_with(b"Bark 1.06") {
            return Err(Error::Magic { what: "ATV.bah", expected: "Bark 1.06" });
        }
        let mut r = Reader::at(bah, 0x14, "ATV.bah");
        let root_dirs = r.u32()?;
        r.seek(0x20);
        let mut idx = BarkIndex::default();
        idx.read_dir(&mut r, "", 0, root_dirs)?;
        Ok(idx)
    }

    fn read_dir(&mut self, r: &mut Reader, path: &str, nfiles: u32, ndirs: u32) -> Result<()> {
        for _ in 0..nfiles {
            let nl = r.u32()? as usize;
            let offset = r.u32()?;
            let stored_size = r.u32()?;
            let raw_size = r.u32()?;
            let hash = r.u32()?;
            let full = format!("{path}/{}", r.name(nl)?);
            self.entries.insert(
                full.to_ascii_lowercase(),
                BarkEntry { path: full, offset, stored_size, raw_size, hash },
            );
        }
        for _ in 0..ndirs {
            let nl = r.u32()? as usize;
            let f = r.u32()?;
            let d = r.u32()?;
            r.u32()?;
            let full = format!("{path}/{}", r.name(nl)?);
            self.read_dir(r, &full, f, d)?;
        }
        Ok(())
    }

    pub fn get(&self, path: &str) -> Option<&BarkEntry> {
        self.entries.get(&crate::xiso::normalize(path))
    }
}

/// Decodes one stored blob (`u32 method, u32 raw_size, data`).
pub fn decode_blob(blob: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(blob, "archive blob");
    let method = r.u32()?;
    let raw = r.u32()? as usize;
    let data = &blob[8..];
    match method {
        0 => Ok(data.get(..raw).ok_or(Error::Eof("stored blob"))?.to_vec()),
        1 => {
            let out = lzss::decompress(data, raw);
            if out.len() != raw {
                return Err(Error::Format(format!("LZSS blob short: {} of {raw}", out.len())));
            }
            Ok(out)
        }
        m => Err(Error::Format(format!("unknown compression method {m}"))),
    }
}
