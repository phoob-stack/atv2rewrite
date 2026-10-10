//! LZSS decompressor used by the Bark archive (classic Okumura variant:
//! 4 KB ring buffer, zero filled, write cursor starting at 0xFEE, flag byte
//! read LSB first with 1 meaning a literal, 12-bit position + 4-bit length).

pub fn decompress(src: &[u8], raw_size: usize) -> Vec<u8> {
    let mut ring = [0u8; 4096];
    let mut r = 0xFEEusize;
    let mut out = Vec::with_capacity(raw_size);
    let mut i = 0;
    'outer: while out.len() < raw_size && i < src.len() {
        let flags = src[i];
        i += 1;
        for bit in 0..8 {
            if out.len() >= raw_size || i >= src.len() {
                break 'outer;
            }
            if flags >> bit & 1 == 1 {
                let c = src[i];
                i += 1;
                out.push(c);
                ring[r] = c;
                r = (r + 1) & 0xFFF;
            } else {
                if i + 1 >= src.len() {
                    break 'outer;
                }
                let (b0, b1) = (src[i] as usize, src[i + 1] as usize);
                i += 2;
                let pos = b0 | (b1 & 0xF0) << 4;
                let len = (b1 & 0x0F) + 3;
                for k in 0..len {
                    let c = ring[(pos + k) & 0xFFF];
                    out.push(c);
                    ring[r] = c;
                    r = (r + 1) & 0xFFF;
                }
            }
        }
    }
    out.truncate(raw_size);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_then_run() {
        // Three literals land at ring slots 0xFEE..=0xFF0; the match then
        // copies 5 bytes starting at 0xFF0, overlapping itself.
        let src = [0b0000_0111, b'a', b'b', b'c', 0xF0, 0xF2];
        assert_eq!(decompress(&src, 8), b"abcccccc");
    }
}
