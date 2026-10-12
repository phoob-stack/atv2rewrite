//! Track surfaces (`Setup/Materials.xml`): how much grip a tyre has on
//! each and how hard it drags at the quad.
//!
//! Every `.kdt` collision triangle names one of these by index. The
//! original (`0x78380`) copies each entry's `Friction`, `Hardness` and
//! `Damping` into a table; a wheel touching a triangle takes its surface's
//! `Friction` as grip and `Damping` as drag (`0x5d030`, a tenth of the
//! grip on faces over 45 degrees from the strut, `0xbc800`), and the quad
//! averages both over the wheels on the ground (`0xb53e0`). Grip blends
//! the handling pairs between their grippy and slippery values; drag pulls
//! against the ground speed squared. A body touching a triangle takes its
//! `Friction` and `Hardness` (times the body's own) as the contact's
//! friction and bounce (0x6fef0).

use atv_formats::setup;

/// Surface names in index order (the id table at `0x1d0af8`).
pub const NAMES: [&str; 28] = [
    "None",
    "Invisible",
    "Tarmac",
    "Wet_Tarmac",
    "Icy_Tarmac",
    "Dirt",
    "Loose_Dirt",
    "Dirt_Bank",
    "Gravel",
    "Grass",
    "Sand",
    "Loose_Sand",
    "Sand_Bank",
    "Mud",
    "Loose_Mud",
    "Mud_Bank",
    "Snow",
    "Loose_Snow",
    "Snow_Bank",
    "Ice",
    "Water",
    "Wood",
    "Wall_Stone",
    "Wall_Metal",
    "Wall_Wood",
    "No_Friction",
    "Deep_Water",
    "Shallow_Water",
];

/// Surface id of collision that doesn't come from the `.kdt` (props, or a
/// track without one): full grip and no drag.
pub const UNKNOWN: u8 = u8::MAX;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface {
    /// Grip, 1 = full.
    pub friction: f32,
    /// Drag on the quad.
    pub damping: f32,
    /// How much a body bounces off it (times the body's own `HARDNESS`).
    pub hardness: f32,
}

impl Default for Surface {
    fn default() -> Self {
        Self {
            friction: 1.0,
            damping: 0.0,
            hardness: 1.0,
        }
    }
}

/// The surface table, indexed by `.kdt` surface id.
#[derive(Debug, Clone, Default)]
pub struct Surfaces(pub Vec<Surface>);

impl Surfaces {
    /// Reads `Setup/Materials.xml`. Surfaces it doesn't list keep full
    /// grip and no drag.
    pub fn parse(xml: &str) -> Self {
        let entries = setup::entries(xml);
        Self(
            NAMES
                .iter()
                .map(|name| {
                    entries.iter().find(|e| e.name.eq_ignore_ascii_case(name)).map_or_else(Surface::default, |e| {
                        Surface {
                            friction: e.num("Friction").unwrap_or(1.0),
                            damping: e.num("Damping").unwrap_or(0.0),
                            hardness: e.num("Hardness").unwrap_or(1.0),
                        }
                    })
                })
                .collect(),
        )
    }

    pub fn get(&self, id: u8) -> Surface {
        self.0.get(id as usize).copied().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_materials() {
        let xml = r#"<Root FileVersion="1.0">
  <None Friction="0.0" Hardness="1.00" Damping="10.0"><WheelDustColour A="1" R="0" G="0" B="0"/></None>
  <Mud_Bank Friction="0.75" Hardness="0.5" Damping="15.0"/>
</Root>"#;
        let s = Surfaces::parse(xml);
        assert_eq!(s.0.len(), NAMES.len());
        assert_eq!(s.get(0), Surface { friction: 0.0, damping: 10.0, hardness: 1.0 });
        assert_eq!(s.get(15), Surface { friction: 0.75, damping: 15.0, hardness: 0.5 });
        assert_eq!(s.get(2), Surface::default());
        assert_eq!(s.get(UNKNOWN), Surface::default());
    }
}
