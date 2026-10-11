//! Decoded sound, and the RIFF WAV reader for the disc's `Music/*.wav`.
//!
//! The music is Xbox IMA ADPCM (format tag 0x69): 36-byte blocks per
//! channel, each a 4-byte header (i16 first sample, u8 step index, u8 0)
//! and 32 bytes of 4-bit codes. Stereo blocks interleave the channels in
//! 4-byte runs (8 codes each), low nibble first. A block decodes to 65
//! samples per channel: the header sample, then the 64 codes.

use crate::bin::Reader;
use crate::{Error, Result};

/// 16-bit PCM, channels interleaved.
#[derive(Debug, Clone, Default)]
pub struct Pcm {
    pub channels: u16,
    pub rate: u32,
    pub samples: Vec<i16>,
}

impl Pcm {
    /// Length in seconds.
    pub fn duration(&self) -> f32 {
        self.samples.len() as f32 / (self.channels.max(1) as u32 * self.rate.max(1)) as f32
    }
}

/// Reads a RIFF WAV holding 16-bit PCM or Xbox ADPCM.
pub fn decode(bytes: &[u8]) -> Result<Pcm> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(Error::Magic { what: "WAV", expected: "RIFF....WAVE" });
    }
    let mut fmt = None;
    let mut data = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &bytes[at + 8..(at + 8 + len).min(bytes.len())];
        match id {
            b"fmt " => fmt = Some(body),
            b"data" => data = Some(body),
            _ => {}
        }
        at += 8 + len + (len & 1);
    }
    let (Some(fmt), Some(data)) = (fmt, data) else {
        return Err(Error::Format("WAV without fmt or data chunk".into()));
    };
    let mut r = Reader::at(fmt, 0, "WAV fmt");
    let tag = r.u16()?;
    let channels = r.u16()?;
    let rate = r.u32()?;
    let _byte_rate = r.u32()?;
    let block_align = r.u16()? as usize;
    let bits = r.u16()?;
    if channels == 0 {
        return Err(Error::Format("WAV with no channels".into()));
    }
    let samples = match (tag, bits) {
        (1, 16) => data.as_chunks::<2>().0.iter().map(|&b| i16::from_le_bytes(b)).collect(),
        (0x69, 4) => xbox_adpcm(data, channels as usize, block_align)?,
        _ => return Err(Error::Format(format!("unsupported WAV format {tag:#x}, {bits} bits"))),
    };
    Ok(Pcm { channels, rate, samples })
}

const STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130,
    143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282,
    1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630,
    9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
const INDEX_STEP: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

struct Ima {
    sample: i32,
    index: i32,
}

impl Ima {
    fn next(&mut self, code: u8) -> i16 {
        let step = STEPS[self.index as usize];
        let diff = ((2 * (code & 7) as i32 + 1) * step) >> 3;
        self.sample = if code & 8 != 0 { self.sample - diff } else { self.sample + diff }.clamp(-32768, 32767);
        self.index = (self.index + INDEX_STEP[(code & 7) as usize]).clamp(0, 88);
        self.sample as i16
    }
}

pub(crate) fn xbox_adpcm(data: &[u8], channels: usize, block_align: usize) -> Result<Vec<i16>> {
    if block_align != 36 * channels {
        return Err(Error::Format(format!("Xbox ADPCM block of {block_align} bytes for {channels} channels")));
    }
    let per_block = 65;
    let mut out = Vec::with_capacity(data.len() / block_align * per_block * channels);
    let mut frame = vec![0i16; per_block * channels];
    for block in data.chunks_exact(block_align) {
        let mut state: Vec<Ima> = (0..channels)
            .map(|c| {
                let h = &block[c * 4..c * 4 + 4];
                Ima { sample: i16::from_le_bytes([h[0], h[1]]) as i32, index: (h[2] as i32).clamp(0, 88) }
            })
            .collect();
        for (c, s) in state.iter().enumerate() {
            frame[c] = s.sample as i16;
        }
        let body = &block[channels * 4..];
        for (run, bytes) in body.as_chunks::<4>().0.iter().enumerate() {
            let (round, c) = (run / channels, run % channels);
            for (k, &b) in bytes.iter().enumerate() {
                let i = 1 + round * 8 + k * 2;
                frame[i * channels + c] = state[c].next(b & 0x0F);
                frame[(i + 1) * channels + c] = state[c].next(b >> 4);
            }
        }
        out.extend_from_slice(&frame);
    }
    Ok(out)
}

/// Wraps PCM in a minimal RIFF WAV (tests and tools).
pub fn encode_pcm16(pcm: &Pcm) -> Vec<u8> {
    let data_len = pcm.samples.len() as u32 * 2;
    let mut v = Vec::with_capacity(44 + data_len as usize);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&pcm.channels.to_le_bytes());
    v.extend_from_slice(&pcm.rate.to_le_bytes());
    v.extend_from_slice(&(pcm.rate * pcm.channels as u32 * 2).to_le_bytes());
    v.extend_from_slice(&(pcm.channels * 2).to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for s in &pcm.samples {
        v.extend_from_slice(&s.to_le_bytes());
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm16_round_trip() {
        let pcm = Pcm { channels: 2, rate: 22050, samples: vec![1, -2, 300, -32768] };
        let back = decode(&encode_pcm16(&pcm)).unwrap();
        assert_eq!((back.channels, back.rate), (2, 22050));
        assert_eq!(back.samples, pcm.samples);
    }

    #[test]
    fn adpcm_block() {
        // Mono header: first sample 1000, step index 0; then codes.
        let mut block = vec![0u8; 36];
        block[0..2].copy_from_slice(&1000i16.to_le_bytes());
        block[4] = 0x07; // codes 7, then 0
        let out = xbox_adpcm(&block, 1, 36).unwrap();
        assert_eq!(out.len(), 65);
        assert_eq!(out[0], 1000);
        // Code 7 at step 7: diff = (2*7 + 1) * 7 / 8 = 13.
        assert_eq!(out[1], 1013);
    }
}
