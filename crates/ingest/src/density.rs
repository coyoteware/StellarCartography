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

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use anyhow::Result;
use galaxy_core::id64::Id64;
use galaxy_core::octree::{ROOT_MIN, ROOT_SIZE};
use rustc_hash::FxHashMap;

pub const MAP_SIZE: usize = 2048;

pub struct Accumulator {
    coverage: Vec<u32>,
    max_index: FxHashMap<u64, u32>,
}

fn pixel(v: f32, min: f32) -> Option<usize> {
    let p = ((v - min) / ROOT_SIZE * MAP_SIZE as f32).floor();
    (p >= 0.0 && p < MAP_SIZE as f32).then_some(p as usize)
}

impl Accumulator {
    pub fn new() -> Self {
        Self { coverage: vec![0; MAP_SIZE * MAP_SIZE], max_index: FxHashMap::default() }
    }

    pub fn add_system(&mut self, id64: u64, pos: [f32; 3]) {
        if let (Some(col), Some(row)) = (pixel(pos[0], ROOT_MIN[0]), pixel(pos[2], ROOT_MIN[2])) {
            self.coverage[row * MAP_SIZE + col] += 1;
        }
        let index = Id64::decode(id64).system_index;
        let slot = self.max_index.entry(Id64::boxel_key(id64)).or_insert(0);
        *slot = (*slot).max(index);
    }

    pub fn boxels(&self) -> usize {
        self.max_index.len()
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
        for (&key, &max_index) in &self.max_index {
            let id = Id64::decode(key);
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
