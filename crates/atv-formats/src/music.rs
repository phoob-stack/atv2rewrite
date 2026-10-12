//! `Setup/Music.xml`, the retail disc's licensed songs, and the disc's
//! region, which decides which of them the retail game plays.
//!
//! The file holds `<Track1>`, `<Track2>`... under `<Root>`, each with
//! `ArtistName`, `SongName`, `LevelSpecific` and `FileName`
//! (`Licensed\Track1` -> `/Licensed/Track1.wav` on the disc). The retail
//! executable reads them in that order until a number is missing (XBE
//! 0xae300) and never reads `LevelSpecific`.

use crate::setup;

#[derive(Debug, Clone, PartialEq)]
pub struct Song {
    pub artist: String,
    pub title: String,
    /// Disc path without the extension (`Licensed\Track1`).
    pub file: String,
    /// `LevelSpecific`: 0 on every retail song, and unused.
    pub level_specific: bool,
}

impl Song {
    /// Where the song is on the disc.
    pub fn path(&self) -> String {
        format!("/{}.wav", self.file.replace('\\', "/"))
    }
}

/// The songs, `Track1` first.
pub fn parse(xml: &str) -> Vec<Song> {
    let entries = setup::entries(xml);
    let mut songs = Vec::new();
    for n in 1.. {
        let Some(e) = entries.iter().find(|e| e.name == format!("Track{n}")) else { break };
        songs.push(Song {
            artist: e.get("ArtistName").unwrap_or_default().to_string(),
            title: e.get("SongName").unwrap_or_default().to_string(),
            file: e.get("FileName").unwrap_or_default().to_string(),
            level_specific: e.get("LevelSpecific").is_some_and(|v| v.trim() != "0"),
        });
    }
    songs
}

/// The Xbox regions an executable may run in (its certificate's game
/// region flags: 1 North America, 2 Japan, 4 the rest of the world).
pub fn xbe_regions(xbe: &[u8]) -> Option<u32> {
    let u32_at = |at: usize| Some(u32::from_le_bytes(xbe.get(at..at + 4)?.try_into().ok()?));
    if xbe.get(0..4)? != b"XBEH" {
        return None;
    }
    let cert = u32_at(0x118)?.checked_sub(u32_at(0x104)?)? as usize;
    u32_at(cert + 0xa0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn songs_in_order_until_a_gap() {
        let xml = r#"<Root FileVersion="1.0000000">
	<Track2 ArtistName="Rollins Band" SongName="Your Number Is 1" LevelSpecific="0" FileName="Licensed\Track2"/>
	<Track1 ArtistName="Godsmack" SongName="Bad Religion" LevelSpecific="0" FileName="Licensed\Track1"/>
	<Track4 ArtistName="Midtown" SongName="Become What You Hate" LevelSpecific="1" FileName="Licensed\Track4"/>
</Root>"#;
        let songs = parse(xml);
        assert_eq!(songs.len(), 2);
        assert_eq!(songs[0].artist, "Godsmack");
        assert_eq!(songs[1].title, "Your Number Is 1");
        assert_eq!(songs[0].path(), "/Licensed/Track1.wav");
        assert!(!songs[0].level_specific);
    }

    #[test]
    fn region_from_certificate() {
        let mut xbe = vec![0u8; 0x300];
        xbe[0..4].copy_from_slice(b"XBEH");
        xbe[0x104..0x108].copy_from_slice(&0x10000u32.to_le_bytes());
        xbe[0x118..0x11c].copy_from_slice(&0x10180u32.to_le_bytes());
        xbe[0x220..0x224].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(xbe_regions(&xbe), Some(1));
        assert_eq!(xbe_regions(b"nope"), None);
    }
}
