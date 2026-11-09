//! Top-down density maps of the galactic plane (x across, z down the image).
//!
//! * `coverage.png`: how many catalogued systems fall in each pixel. This
//!   shows where players have been, not what the galaxy looks like.
//! * `density.png`: an estimate of the real stellar density. Every system's
//!   id64 carries its index within its boxel, so the highest index seen in a
//!   boxel is a lower bound on how many systems that boxel holds. Summing
//!   those bounds recovers the galaxy's arms and core even where only a few
//!   systems have been logged.
//!
//! Both maps cover the octree root's x/z square, with row 0 at minimum z.
//!
//! The full catalogue touches about 52M boxels, so the per-boxel state is
//! kept small: one `u64` each, the id64 (without its body bits) of the
//! highest-numbered system seen in the boxel. That one value holds both the
//! boxel and its system index, half the size of a key/value map entry.

use std::fs::File;
use std::hash::BuildHasher;
use std::io::BufWriter;
use std::path::Path;

use anyhow::Result;
use galaxy_core::id64::Id64;
use galaxy_core::octree::{ROOT_MIN, ROOT_SIZE};
use hashbrown::hash_table::{Entry, HashTable};
use rustc_hash::FxBuildHasher;

pub const MAP_SIZE: usize = 2048;

/// The id64 bits below the body number: mass code, boxel and system index.
const SYSTEM_BITS: u64 = (1 << 55) - 1;

/// Boxels touched per catalogued system: 51.7M boxels for 99.4M systems in
/// October 2026. Used to size the table up front so it never has to grow.
pub const BOXELS_PER_SYSTEM: f64 = 0.55;

pub struct Accumulator {
    coverage: Vec<u32>,
    /// Per boxel, hashed by its boxel key: the system with the highest index.
    boxels: HashTable<u64>,
}

fn boxel_hash(system: u64) -> u64 {
    FxBuildHasher.hash_one(Id64::boxel_key(system))
}

fn pixel(v: f32, min: f32) -> Option<usize> {
    let p = ((v - min) / ROOT_SIZE * MAP_SIZE as f32).floor();
    (p >= 0.0 && p < MAP_SIZE as f32).then_some(p as usize)
}

impl Accumulator {
    /// `boxels` is how many distinct boxels to expect; more is fine, the table grows.
    pub fn with_capacity(boxels: usize) -> Self {
        Self { coverage: vec![0; MAP_SIZE * MAP_SIZE], boxels: HashTable::with_capacity(boxels) }
    }

    pub fn add_system(&mut self, id64: u64, pos: [f32; 3]) {
        if let (Some(col), Some(row)) = (pixel(pos[0], ROOT_MIN[0]), pixel(pos[2], ROOT_MIN[2])) {
            self.coverage[row * MAP_SIZE + col] += 1;
        }
        let system = id64 & SYSTEM_BITS;
        let key = Id64::boxel_key(system);
        match self.boxels.entry(boxel_hash(system), |&s| Id64::boxel_key(s) == key, |&s| boxel_hash(s)) {
            Entry::Occupied(mut e) => {
                let kept = e.get_mut();
                if Id64::decode(system).system_index > Id64::decode(*kept).system_index {
                    *kept = system;
                }
            }
            Entry::Vacant(e) => {
                e.insert(system);
            }
        }
    }

    pub fn boxels(&self) -> usize {
        self.boxels.len()
    }

    pub fn write_pngs(&self, out: &Path) -> Result<()> {
        let coverage: Vec<f32> = self.coverage.iter().map(|&c| c as f32).collect();
        write_log_png(&out.join("coverage.png"), &coverage)?;
        write_log_png(&out.join("density.png"), &self.estimate())?;
        Ok(())
    }

    /// Spreads each boxel's population bound over the pixels it covers.
    fn estimate(&self) -> Vec<f32> {
        let mut grid = vec![0f32; MAP_SIZE * MAP_SIZE];
        let px = ROOT_SIZE / MAP_SIZE as f32;
        for &system in &self.boxels {
            let id = Id64::decode(system);
            let max_index = id.system_index;
            let centre = id.boxel_centre();
            let half = id.boxel_size() as f32 / 2.0;
            let span = |c: f32, min: f32| {
                let lo = ((c - half - min) / px).floor().max(0.0) as usize;
                let hi = (((c + half - min) / px).ceil() as usize).min(MAP_SIZE);
                lo..hi.max(lo + 1).min(MAP_SIZE)
            };
            let cols = span(centre[0], ROOT_MIN[0]);
            let rows = span(centre[2], ROOT_MIN[2]);
            if cols.is_empty() || rows.is_empty() {
                continue;
            }
            let share = (max_index + 1) as f32 / (cols.len() * rows.len()) as f32;
            for row in rows {
                for col in cols.clone() {
                    grid[row * MAP_SIZE + col] += share;
                }
            }
        }
        grid
    }
}

/// Log-scales `values` to 8-bit grey, clipping at the 99.9th percentile of
/// non-empty pixels so the Sol bubble doesn't wash everything else out.
fn write_log_png(path: &Path, values: &[f32]) -> Result<()> {
    let mut nonzero: Vec<f32> = values.iter().copied().filter(|&v| v > 0.0).collect();
    let ceiling = if nonzero.is_empty() {
        1.0
    } else {
        let k = ((nonzero.len() as f64 * 0.999) as usize).min(nonzero.len() - 1);
        *nonzero.select_nth_unstable_by(k, |a, b| a.total_cmp(b)).1
    };
    let denom = ceiling.ln_1p().max(f32::EPSILON);
    let bytes: Vec<u8> = values.iter().map(|&v| ((v.ln_1p() / denom).min(1.0) * 255.0).round() as u8).collect();

    let mut encoder = png::Encoder::new(BufWriter::new(File::create(path)?), MAP_SIZE as u32, MAP_SIZE as u32);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `id64` with its system index replaced by `index`.
    fn with_index(id64: u64, index: u64) -> u64 {
        let mc = id64 & 7;
        Id64::boxel_key(id64) | (index << (44 - 3 * mc))
    }

    #[test]
    fn keeps_the_highest_index_per_boxel() {
        const SOL: u64 = 10477373803;
        const COLONIA: u64 = 3238296097059;
        let mut acc = Accumulator::with_capacity(4);
        for index in [5, 2, 9] {
            acc.add_system(with_index(SOL, index), [0.0; 3]);
        }
        // A body's id64 (body number in the top bits) is the same boxel.
        acc.add_system(with_index(SOL, 1) | (3 << 55), [0.0; 3]);
        acc.add_system(COLONIA, [-9530.5, -910.3, 19808.1]);
        assert_eq!(acc.boxels(), 2);

        // Each boxel spreads (max index + 1) over its pixels.
        let colonia_index = u64::from(Id64::decode(COLONIA).system_index);
        let total: f64 = acc.estimate().iter().map(|&v| f64::from(v)).sum();
        assert!((total - (10 + colonia_index + 1) as f64).abs() < 1e-3, "{total}");
    }

    #[test]
    fn grows_past_its_initial_capacity() {
        let mut acc = Accumulator::with_capacity(1);
        for i in 0..10_000u64 {
            // Mass code a (0), a different boxel each time.
            acc.add_system(i << 3, [0.0; 3]);
        }
        assert_eq!(acc.boxels(), 10_000);
    }
}
