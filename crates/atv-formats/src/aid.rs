//! "AID 1.02" AI racing lines (`Levels/<Track>/World/<Track>.aid`).
//!
//! A track's AI data is a graph of waypoints laid along the racing line.
//! Each node carries the drivable corridor either side of it and up to four
//! links back and forward; nodes with two forward links are where the line
//! splits around a shortcut. Read by the retail game's loader at
//! 0xf24b0 (base class) and 0x640e0 (the AI's per-node data).
//!
//! ```text
//! 0x00 "AID 1.02\0\0\0\0", u32 node_count, u32 record_size (152)
//! 0x14 node[node_count], record_size bytes each:
//!      +0x00 name[32]        (empty in the shipped data; the rest of the
//!                             field is uninitialised editor memory)
//!      +0x20 pos[3]          game frame (left-handed, +Z forward)
//!      +0x2C forward[3]      unit direction of travel
//!      +0x38 up[3]
//!      +0x44 f32 left        corridor edge along the node's right vector
//!                            (negative = left of the node)
//!      +0x48 f32 right       other edge (positive = right of the node)
//!      +0x4C f32             unused by the game (stale copy of `left`)
//!      +0x50 i32 prev[4]     -1 = no link
//!      +0x60 i32 next[4]
//!      +0x70 f32 speed       speed limit at this node, m/s (0 = none)
//!      +0x74 u32 flags       bit 0: a respot may use this node (clear
//!                            on jumps and in water); bit 1: unknown
//!      +0x78 u8  boost       % of the approach speed kept as a run-up for
//!                            a jump (the AI's hop timing)
//!      +0x79 ...             zero
//! ```
//! "AID 1.01" files (none ship on the disc) have no record size field and
//! 0x70-byte records.

use crate::bin::Reader;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct AiNode {
    pub name: String,
    pub pos: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
    /// Corridor edges along the node's right vector, metres (`left` < 0).
    pub left: f32,
    pub right: f32,
    pub prev: Vec<usize>,
    pub next: Vec<usize>,
    /// Speed limit, metres per second.
    pub speed_limit: Option<f32>,
    pub flags: u32,
    pub boost: u8,
}

impl AiNode {
    /// Whether a quad may be respotted at this node.
    pub fn respot(&self) -> bool {
        self.flags & 1 != 0
    }
}

#[derive(Debug, Clone)]
pub struct AiPath {
    pub nodes: Vec<AiNode>,
}

impl AiPath {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data, "aid");
        let magic = r.bytes(8)?;
        let (record, start) = match magic {
            b"AID 1.02" => {
                r.seek(0x10);
                (r.u32()? as usize, 0x14)
            }
            b"AID 1.01" => (0x70, 0x10),
            _ => {
                return Err(Error::Magic {
                    what: "aid",
                    expected: "AID 1.02",
                });
            }
        };
        r.seek(0x0C);
        let count = r.u32()? as usize;
        if record < 0x70 || count > 100_000 {
            return Err(Error::Format(format!(
                "aid: bad header ({count} nodes of {record} bytes)"
            )));
        }
        let mut nodes = Vec::with_capacity(count);
        for i in 0..count {
            let base = start + i * record;
            let mut r = Reader::at(data, base, "aid node");
            let name = r.bytes(32)?;
            let name =
                String::from_utf8_lossy(&name[..name.iter().position(|&b| b == 0).unwrap_or(32)])
                    .into_owned();
            let pos = r.vec3()?;
            let forward = r.vec3()?;
            let up = r.vec3()?;
            let left = r.f32()?;
            let right = r.f32()?;
            let _stale = r.f32()?;
            let links = |r: &mut Reader| -> Result<Vec<usize>> {
                let mut v = Vec::new();
                for _ in 0..4 {
                    let l = r.i32()?;
                    if l >= 0 && (l as usize) < count {
                        v.push(l as usize);
                    }
                }
                Ok(v)
            };
            let prev = links(&mut r)?;
            let next = links(&mut r)?;
            // The 1.01 records end at the links.
            let (speed, flags, boost) = if record >= 0x79 {
                (r.f32()?, r.u32()?, r.u8()?)
            } else {
                (0.0, 1, 0)
            };
            nodes.push(AiNode {
                name,
                pos,
                forward,
                up,
                left,
                right,
                prev,
                next,
                speed_limit: (speed > 0.0).then_some(speed),
                flags,
                boost,
            });
        }
        Ok(Self { nodes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(buf: &mut Vec<u8>, pos: [f32; 3], prev: i32, next: i32, speed: f32) {
        let start = buf.len();
        buf.resize(start + 152, 0);
        let mut put =
            |off: usize, b: [u8; 4]| buf[start + off..start + off + 4].copy_from_slice(&b);
        for (k, v) in pos.iter().enumerate() {
            put(0x20 + k * 4, v.to_le_bytes());
        }
        put(0x2C + 8, 1f32.to_le_bytes());
        put(0x44, (-4f32).to_le_bytes());
        put(0x48, 5f32.to_le_bytes());
        for k in 0..4 {
            put(0x50 + k * 4, (if k == 0 { prev } else { -1 }).to_le_bytes());
            put(0x60 + k * 4, (if k == 0 { next } else { -1 }).to_le_bytes());
        }
        put(0x70, speed.to_le_bytes());
        put(0x74, 1u32.to_le_bytes());
    }

    #[test]
    fn loop_of_three() {
        let mut b = b"AID 1.02\0\0\0\0".to_vec();
        b.extend(3u32.to_le_bytes());
        b.extend(152u32.to_le_bytes());
        node(&mut b, [0.0, 0.0, 0.0], 2, 1, 0.0);
        node(&mut b, [0.0, 0.0, 10.0], 0, 2, 20.0);
        node(&mut b, [10.0, 0.0, 5.0], 1, 0, 0.0);
        let p = AiPath::parse(&b).unwrap();
        assert_eq!(p.nodes.len(), 3);
        assert_eq!(p.nodes[1].pos, [0.0, 0.0, 10.0]);
        assert_eq!(p.nodes[1].next, vec![2]);
        assert_eq!(p.nodes[0].prev, vec![2]);
        assert_eq!(p.nodes[1].speed_limit, Some(20.0));
        assert_eq!(p.nodes[0].speed_limit, None);
        assert_eq!((p.nodes[2].left, p.nodes[2].right), (-4.0, 5.0));
        assert!(p.nodes[0].respot());
    }
}
