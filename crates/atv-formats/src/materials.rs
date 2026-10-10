//! `Setup/Materials.xml`: what each kind of ground does to a wheel rolling
//! on it. One entry per surface (`Sand`, `Loose_Mud`, `Tarmac`...) with
//! its grip and the particles its wheels throw up, each colour an
//! attribute-only child (`<WheelDustColour A= R= G= B=/>`).
//!
//! The original reads it at 0x782e0 into a 0x58-byte record per surface
//! (field parser 0x785c0); the wheel effects at 0x8b900 and 0x8bf40 use
//! the dust and particle fields below.

use crate::setup;

/// The kind of chunks a wheel throws: the original's `WheelParticleType`,
/// each a run of frames in `Sprites/Particles` (set up at 0x8e440).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chunk {
    /// `Dirt0`..`Dirt2`.
    Dirt,
    /// `Gravel0`..`Gravel3`.
    Gravel,
    /// `Dust0` (sand and snow use it, as fine spray).
    Snow,
}

impl Chunk {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "Dirt" => Some(Self::Dirt),
            "Gravel" => Some(Self::Gravel),
            "Snow" => Some(Self::Snow),
            _ => None,
        }
    }

    /// The sprite names this kind picks from at random.
    pub fn frames(self) -> &'static [&'static str] {
        match self {
            Self::Dirt => &["Dirt0", "Dirt1", "Dirt2"],
            Self::Gravel => &["Gravel0", "Gravel1", "Gravel2", "Gravel3"],
            Self::Snow => &["Dust0"],
        }
    }
}

/// An RGBA colour as the file writes it (0..1 each).
pub type Colour = [f32; 4];

#[derive(Debug, Clone, PartialEq)]
pub struct Surface {
    pub name: String,
    /// Whether wheels raise dust here at all (`WheelDustType` not None).
    pub dust: bool,
    /// Chance of a dust puff off a spinning, moving wheel.
    pub dust_prob: f32,
    pub dust_colour: Colour,
    pub chunk: Option<Chunk>,
    /// Chance of a chunk off a wheel throwing up spray.
    pub chunk_prob: f32,
    /// Chance of a dust puff off a rear wheel throwing up spray.
    pub chunk_dust_prob: f32,
    pub chunk_colour: Colour,
    /// Scale on each chunk's size.
    pub chunk_size: f32,
}

impl Surface {
    /// Every surface in the file, in file order.
    pub fn parse_all(xml: &str) -> Vec<Self> {
        setup::entries(xml)
            .into_iter()
            .map(|e| {
                let colour = |child: &str| {
                    let c = setup::child(xml, &e.name, child).unwrap_or_default();
                    let n = |k: &str| c.get(k).and_then(|v| v.trim().parse().ok()).unwrap_or(1.0);
                    [n("R"), n("G"), n("B"), n("A")]
                };
                let num = |k: &str| e.num(k).unwrap_or(0.0);
                Surface {
                    dust: e.get("WheelDustType").is_some_and(|t| t != "None"),
                    dust_prob: num("WheelDustProb"),
                    dust_colour: colour("WheelDustColour"),
                    chunk: e.get("WheelParticleType").and_then(Chunk::parse),
                    chunk_prob: num("WheelParticleProb"),
                    chunk_dust_prob: num("WheelParticleDustProb"),
                    chunk_colour: colour("WheelParticleColour"),
                    chunk_size: e.num("WheelParticleSize").unwrap_or(1.0),
                    name: e.name,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surfaces_with_colours() {
        let xml = r#"<Root FileVersion="1.0000000">
	<Tarmac Friction="1.0" WheelDustType="None" WheelDustProb="0.0" WheelParticleType="None" WheelParticleProb="0.0" WheelParticleDustProb="0.0" WheelParticleSize="1.0">
		<WheelDustColour A="0.4980392" R="0.3098039" G="0.2392157" B="0.2000000"/>
		<WheelParticleColour A="1.0000000" R="0.5" G="0.5" B="0.5"/>
	</Tarmac>
	<Gravel WheelDustType="Dirt" WheelDustProb="0.5000000" WheelParticleType="Gravel" WheelParticleProb="0.7500000" WheelParticleDustProb="0.2000000" WheelParticleSize="0.5000000">
		<WheelDustColour A="0.4980392" R="0.25" G="0.25" B="0.25"/>
		<WheelParticleColour A="1.0" R="0.5" G="0.4" B="0.3"/>
	</Gravel>
</Root>"#;
        let s = Surface::parse_all(xml);
        assert_eq!(s.len(), 2);
        assert!(!s[0].dust && s[0].chunk.is_none());
        let g = &s[1];
        assert_eq!(g.name, "Gravel");
        assert!(g.dust && g.chunk == Some(Chunk::Gravel));
        assert_eq!((g.dust_prob, g.chunk_prob, g.chunk_dust_prob, g.chunk_size), (0.5, 0.75, 0.2, 0.5));
        assert_eq!(g.dust_colour, [0.25, 0.25, 0.25, 0.4980392]);
        assert_eq!(g.chunk_colour, [0.5, 0.4, 0.3, 1.0]);
    }
}
