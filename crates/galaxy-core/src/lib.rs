//! Shared data formats for StellarCartography.
//!
//! * [`id64`]: decoding Elite: Dangerous system addresses into boxel positions.
//! * [`octree`]: the on-disk point-cloud tile format the viewer streams.
//! * [`journal`]: parsing the jump events out of the player journal.
//! * [`visited`]: reading the game's `VisitedStarsCache.dat`.

pub mod id64;
pub mod journal;
pub mod octree;
pub mod visited;

/// A galactic position in light years, in the game's own coordinate frame
/// (Sol at the origin, +z towards Sagittarius A*).
pub type Vec3 = [f32; 3];

/// Serialises an id64 as a string: addresses can exceed 2^53, which a
/// JavaScript number can't hold exactly.
pub fn serialize_id64<S: serde::Serializer>(id64: &Option<u64>, s: S) -> Result<S::Ok, S::Error> {
    match id64 {
        Some(v) => s.serialize_str(&v.to_string()),
        None => s.serialize_none(),
    }
}
