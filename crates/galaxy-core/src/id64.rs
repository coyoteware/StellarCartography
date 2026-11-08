//! Decoding of 64-bit system addresses ("id64").
//!
//! Every system address encodes the boxel the system lives in. Reading from
//! the least significant bit:
//!
//! | field      | bits          |
//! |------------|---------------|
//! | mass code  | 3             |
//! | boxel z    | 14 - mc       |
//! | boxel y    | 13 - mc       |
//! | boxel x    | 14 - mc       |
//! | system idx | 11 + 3 * mc   |
//! | body id    | 9             |
//!
//! A boxel of mass code `mc` (a = 0 .. h = 7) is a cube `10 * 2^mc` ly on a
//! side, on a grid anchored at [`GRID_ORIGIN`]. The exact position inside the
//! boxel is not encoded, so [`Id64::boxel_centre`] is accurate to half a boxel.

use crate::Vec3;

/// Corner of the sector grid, in light years.
pub const GRID_ORIGIN: [f64; 3] = [-49985.0, -40985.0, -24105.0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Id64 {
    pub mass_code: u8,
    pub boxel: [u32; 3],
    pub system_index: u32,
    pub body: u16,
}

impl Id64 {
    pub fn decode(id64: u64) -> Self {
        let mc = (id64 & 7) as u32;
        let field = |shift: u32, bits: u32| ((id64 >> shift) & ((1u64 << bits) - 1)) as u32;
        let z = field(3, 14 - mc);
        let y = field(17 - mc, 13 - mc);
        let x = field(30 - 2 * mc, 14 - mc);
        let system_index = field(44 - 3 * mc, 11 + 3 * mc);
        Self {
            mass_code: mc as u8,
            boxel: [x, y, z],
            system_index,
            body: (id64 >> 55) as u16,
        }
    }

    /// Edge length of this system's boxel, in light years.
    pub fn boxel_size(&self) -> f64 {
        10.0 * f64::from(1u32 << self.mass_code)
    }

    /// Centre of the boxel containing the system.
    pub fn boxel_centre(&self) -> Vec3 {
        let size = self.boxel_size();
        let axis = |i: usize| (GRID_ORIGIN[i] + (f64::from(self.boxel[i]) + 0.5) * size) as f32;
        [axis(0), axis(1), axis(2)]
    }

    /// The address with the system index and body id cleared, i.e. a key
    /// identifying the boxel itself.
    pub fn boxel_key(id64: u64) -> u64 {
        let mc = (id64 & 7) as u32;
        id64 & ((1u64 << (44 - 3 * mc)) - 1)
    }

    /// Mass-code letter as used in procedural names ("d" in "Eol Prou RS-T d3-94").
    pub fn mass_code_letter(&self) -> char {
        (b'a' + self.mass_code) as char
    }
}

#[cfg(test)]
// Coordinates are exact multiples of 1/32 ly, which f32 holds exactly;
// clippy only sees more digits than f32 prints.
#[allow(clippy::excessive_precision)]
mod tests {
    use super::*;

    /// (name, id64, coordinates) taken from the EDSM dump.
    const FIXTURES: &[(&str, u64, Vec3)] = &[
        ("Sol", 10477373803, [0.0, 0.0, 0.0]),
        ("Sagittarius A*", 20578934, [25.21875, -20.90625, 25899.96875]),
        ("4 Sextantis", 663329196387, [87.25, 96.84375, -65.0]),
        ("Synuefue QA-D a12-1", 22776748329056, [-1701.375, -25.8125, -944.0]),
        ("Col 359 Sector QV-Q b49-1", 2864073025409, [-441.59375, 258.0625, 1175.3125]),
        ("Oochorrs XO-Q a73-0", 5406826841712, [371.5, -33.3125, -1561.78125]),
    ];

    #[test]
    fn known_systems_lie_inside_their_boxel() {
        for &(name, id64, pos) in FIXTURES {
            let id = Id64::decode(id64);
            let centre = id.boxel_centre();
            let half = id.boxel_size() / 2.0;
            for axis in 0..3 {
                let dev = f64::from((centre[axis] - pos[axis]).abs());
                assert!(dev <= half, "{name}: axis {axis} off by {dev} (boxel {})", id.boxel_size());
            }
            assert_eq!(id.body, 0, "{name}");
        }
    }

    #[test]
    fn procedural_name_fields_match() {
        // "Synuefue QA-D a12-1": mass code a, system index 1.
        let id = Id64::decode(22776748329056);
        assert_eq!(id.mass_code_letter(), 'a');
        assert_eq!(id.system_index, 1);
        // "Col 359 Sector QV-Q b49-1": mass code b, system index 1.
        let id = Id64::decode(2864073025409);
        assert_eq!(id.mass_code_letter(), 'b');
        assert_eq!(id.system_index, 1);
    }

    #[test]
    fn boxel_key_drops_system_index() {
        let a = Id64::boxel_key(5406826841712); // Oochorrs XO-Q a73-0
        let decoded = Id64::decode(a);
        assert_eq!(decoded.system_index, 0);
        assert_eq!(decoded.boxel, Id64::decode(5406826841712).boxel);
    }
}
