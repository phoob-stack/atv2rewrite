//! XACT wave banks, version 1 (`Sound/*/SampleBank.xwb`).
//!
//! Layout (little-endian):
//! ```text
//! 0x00 "WBND", u32 version (1)
//! 0x08 u32 flags (0), u32 entry_count
//! 0x10 name[16] ("SampleBank"), zeros up to 0x50
//! 0x50 entries[entry_count], 20 bytes each:
//!      u32 format, u32 offset, u32 length, u32 loop_start, u32 loop_length
//!      data, starting right after the table; offsets are relative to it
//! ```
//! Format: bit 0 set for Xbox ADPCM (the retail disc) and clear for PCM (the
//! prototype), bits 1..3 the channel count (always 1), sample rate in bits
//! 4..30, and for PCM bit 31 set for 16-bit signed samples and clear for
//! 8-bit unsigned. ADPCM samples are 36-byte mono blocks, decoded as the
//! music is (see `wav`). No entry has a loop region; the game loops whole
//! samples.

use crate::bin::Reader;
use crate::wav::{xbox_adpcm, Pcm};
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct WaveBank {
    pub name: String,
    pub waves: Vec<Pcm>,
}

impl WaveBank {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if !bytes.starts_with(b"WBND") {
            return Err(Error::Magic { what: "wave bank", expected: "WBND" });
        }
        let mut r = Reader::at(bytes, 4, "wave bank");
        let version = r.u32()?;
        if version != 1 {
            return Err(Error::Format(format!("wave bank version {version}")));
        }
        let _flags = r.u32()?;
        let count = r.u32()? as usize;
        let name = r.name(16)?;
        let data = 0x50 + count * 20;
        if data > bytes.len() {
            return Err(Error::Eof("wave bank entries"));
        }
        r.seek(0x50);
        let mut waves = Vec::with_capacity(count);
        for _ in 0..count {
            let format = r.u32()?;
            let offset = r.u32()? as usize;
            let length = r.u32()? as usize;
            let _loop = (r.u32()?, r.u32()?);
            let raw = bytes.get(data + offset..data + offset + length).ok_or(Error::Eof("wave bank sample"))?;
            let rate = (format >> 4) & 0x3FF_FFFF;
            let samples = if format & 1 != 0 {
                xbox_adpcm(raw, 1, 36)?
            } else if format & 0x8000_0000 != 0 {
                raw.as_chunks::<2>().0.iter().map(|&b| i16::from_le_bytes(b)).collect()
            } else {
                raw.iter().map(|&b| ((b as i16) - 128) << 8).collect()
            };
            waves.push(Pcm { channels: 1, rate, samples });
        }
        Ok(WaveBank { name, waves })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_waves() {
        let mut b = vec![0u8; 0x50];
        b[0..4].copy_from_slice(b"WBND");
        b[4..8].copy_from_slice(&1u32.to_le_bytes());
        b[12..16].copy_from_slice(&2u32.to_le_bytes());
        b[16..26].copy_from_slice(b"SampleBank");
        let entry = |f: u32, o: u32, l: u32| [f, o, l, 0, 0].iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        b.extend(entry(0x800a_c442, 0, 4));
        b.extend(entry(0x000a_c442, 4, 2));
        b.extend([0x10, 0x00, 0xF0, 0xFF, 128, 255]);
        let bank = WaveBank::parse(&b).unwrap();
        assert_eq!(bank.name, "SampleBank");
        assert_eq!(bank.waves[0].rate, 44100);
        assert_eq!(bank.waves[0].samples, vec![16, -16]);
        assert_eq!(bank.waves[1].samples, vec![0, 127 << 8]);
    }

    #[test]
    fn adpcm_wave() {
        let mut b = vec![0u8; 0x50];
        b[0..4].copy_from_slice(b"WBND");
        b[4..8].copy_from_slice(&1u32.to_le_bytes());
        b[12..16].copy_from_slice(&1u32.to_le_bytes());
        // Retail format word: ADPCM, one channel, 22050 Hz.
        b.extend([0x0005_6223u32, 0, 36, 0, 0].iter().flat_map(|x| x.to_le_bytes()));
        let mut block = vec![0u8; 36];
        block[0..2].copy_from_slice(&100i16.to_le_bytes());
        b.extend(block);
        let bank = WaveBank::parse(&b).unwrap();
        assert_eq!(bank.waves[0].rate, 22050);
        assert_eq!(bank.waves[0].samples.len(), 64);
        assert_eq!(bank.waves[0].samples[0], 100);
    }
}
