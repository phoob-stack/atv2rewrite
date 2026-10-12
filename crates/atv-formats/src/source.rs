//! Access to the player's own copy of the game.
//!
//! Accepts either the disc image (`.iso` / `.bin` / `.xiso`) or a folder the
//! player extracted the disc to (the one containing `ATV.bah`).

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::bark::{self, BarkIndex};
use crate::xiso::{self, Xiso};
use crate::{Error, Result};

enum Backing {
    Disc(Mutex<Xiso<BufReader<File>>>),
    Folder(PathBuf),
}

pub struct GameSource {
    backing: Backing,
    pub archive: BarkIndex,
    /// Offset of `ATV.bad` inside the disc image (disc backing only).
    bad_offset: u64,
}

impl GameSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if path.is_dir() {
            let bah = std::fs::read(find_ci(path, "ATV.bah")?)?;
            return Ok(Self {
                backing: Backing::Folder(path.to_path_buf()),
                archive: BarkIndex::parse(&bah)?,
                bad_offset: 0,
            });
        }
        let mut disc = Xiso::open(BufReader::new(File::open(path)?))?;
        let bah = disc.read("/ATV.bah")?;
        let bad = disc.entry("/ATV.bad").ok_or_else(|| Error::NotFound("ATV.bad".into()))?;
        Ok(Self {
            backing: Backing::Disc(Mutex::new(disc)),
            archive: BarkIndex::parse(&bah)?,
            bad_offset: bad.sector as u64 * 2048,
        })
    }

    /// Reads a game file by path, looking in the archive first and then on
    /// the disc itself (music, movies, fonts). Case-insensitive; `\` or `/`.
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        if let Some(e) = self.archive.get(path) {
            let blob = self.read_bad(e.offset as u64, e.stored_size as usize)?;
            return bark::decode_blob(&blob);
        }
        match &self.backing {
            Backing::Disc(d) => d.lock().unwrap().read(path),
            Backing::Folder(root) => {
                let mut p = root.clone();
                for part in xiso::normalize(path).split('/').filter(|s| !s.is_empty()) {
                    p = find_ci(&p, part)?;
                }
                Ok(std::fs::read(p)?)
            }
        }
    }

    pub fn exists(&self, path: &str) -> bool {
        self.archive.get(path).is_some()
            || match &self.backing {
                Backing::Disc(d) => d.lock().unwrap().entry(path).is_some(),
                Backing::Folder(root) => {
                    let mut p = root.clone();
                    xiso::normalize(path).split('/').filter(|s| !s.is_empty()).all(|part| find_ci(&p, part).map(|f| p = f).is_ok()) && p.is_file()
                }
            }
    }

    /// Every file path in the archive, in index order of the original names.
    pub fn archive_paths(&self) -> impl Iterator<Item = &str> {
        self.archive.entries.values().map(|e| e.path.as_str())
    }

    fn read_bad(&self, offset: u64, len: usize) -> Result<Vec<u8>> {
        match &self.backing {
            Backing::Disc(d) => d.lock().unwrap().read_range(self.bad_offset + offset, len),
            Backing::Folder(root) => {
                let mut f = File::open(find_ci(root, "ATV.bad")?)?;
                f.seek(SeekFrom::Start(offset))?;
                let mut buf = vec![0u8; len];
                f.read_exact(&mut buf)?;
                Ok(buf)
            }
        }
    }
}

fn find_ci(dir: &Path, name: &str) -> Result<PathBuf> {
    let direct = dir.join(name);
    if direct.exists() {
        return Ok(direct);
    }
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        if e.file_name().to_string_lossy().eq_ignore_ascii_case(name) {
            return Ok(e.path());
        }
    }
    Err(Error::NotFound(dir.join(name).display().to_string()))
}
