//! Top-down density maps of the galactic plane (x across, z down the image).
//!
//! * `coverage.png`: how many catalogued systems fall in each pixel. This
//!   shows where players have been, not what the galaxy looks like.
//! * `density.png`: an estimate of how many systems each pixel's column
//!   holds, logged or not. The galaxy glow is drawn from it.
//!
//! Both maps cover the octree root's x/z square, with row 0 at minimum z.
//!
//! # The estimate
//!
//! Every system's id64 names its boxel and its index within that boxel, and
//! a boxel's systems are numbered from 0. So for each boxel with any system
//! logged, the highest index seen and the number logged give an estimate of
//! how many systems it holds (the "German tank" estimate, [`german_tank`]).
//!
//! Summing those estimates only counts boxels someone has logged a system
//! in, so it followed exploration: about 1 ly⁻² around the bubble, falling
//! tenfold within 10,000 ly, with the busy routes standing out as streaks.
//! Instead, for each mass code separately, [`column_estimate`] takes the
//! boxels logged around each pixel as a sample of the boxels there and
//! scales up to every boxel in the pixel's column:
//!
//! 1. Average the logged boxels' counts, over a window that grows (up to
//!    about 800 ly) until it holds enough of them.
//! 2. Logged boxels hold at least one system, but in sparse space many
//!    boxels hold none. Treating the counts as Poisson, the mean of the
//!    non-empty ones gives the mean over all of them ([`poisson_mean`]).
//! 3. Divide by the disc's vertical profile at the logged boxels' heights,
//!    and multiply by the profile summed over the column. The profile is the
//!    glow's own (glow.ts): exponential, 1,600 ly scale height at the core
//!    thinning to 320 ly in the disc. Near Sol the catalogue gives 290 ly.
//!
//! Mass codes b to h fill nearly every boxel near the plane around Sol, so
//! an unlogged boxel among logged ones is usually unvisited, not empty.
//! Mass code a is the exception: around Sol 5% of its boxels hold systems,
//! clustered, so its counts can't be scaled up the same way. It's taken as
//! a fixed share of mass code b, measured within 500 ly of Sol, where the
//! catalogue is complete.
//!
//! The full catalogue touches about 52M boxels, so the per-boxel state is
//! kept small: one `u64` each, the id64 (without its body bits) of the
//! highest-numbered system seen in the boxel, with the number of systems
//! seen there in the top 9 bits where the body number would go.

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
/// Systems logged per boxel are counted in the top bits, up to this many.
const MAX_LOGGED: u64 = (1 << 9) - 1;

/// Boxels touched per catalogued system: 51.7M boxels for 99.4M systems in
/// October 2026. Used to size the table up front so it never has to grow.
pub const BOXELS_PER_SYSTEM: f64 = 0.55;

/// Sagittarius A*, the centre of the disc profile (game coordinates, ly,
/// rounded: the profile changes over hundreds of ly).
const CORE: [f32; 3] = [25.2, -20.9, 25_900.0];
/// Averaging windows, in pixels, from the widest down (see [`column_estimate`]).
const WINDOWS: [usize; 6] = [16, 8, 4, 2, 1, 0];
/// Logged boxels a window needs before its average counts in full.
const ENOUGH_SAMPLES: f32 = 12.0;
/// The least profile weight a logged boxel gets: the disc profile is far too
/// steep to describe the halo of systems thousands of ly above the plane.
const MIN_PROFILE: f32 = 0.05;
/// Where mass code a's share of mass code b is measured: around Sol, where
/// the catalogue is complete.
const CALIBRATION_RADIUS: f32 = 500.0;

pub struct Accumulator {
    coverage: Vec<u32>,
    /// Per boxel, hashed by its boxel key: the system with the highest index,
    /// with the number of systems logged in the bits above [`SYSTEM_BITS`].
    boxels: HashTable<u64>,
}

fn boxel_hash(system: u64) -> u64 {
    FxBuildHasher.hash_one(Id64::boxel_key(system))
}

fn system_index(system: u64) -> u32 {
    Id64::decode(system & SYSTEM_BITS).system_index
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
                let logged = ((*kept >> 55) + 1).min(MAX_LOGGED);
                let best = if system_index(system) > system_index(*kept) { system } else { *kept & SYSTEM_BITS };
                *kept = best | (logged << 55);
            }
            Entry::Vacant(e) => {
                e.insert(system | (1 << 55));
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

    /// Each logged boxel: mass code, centre, and estimated systems.
    fn samples(&self) -> impl Iterator<Item = (usize, [f32; 3], f64)> + '_ {
        self.boxels.iter().map(|&v| {
            let id = Id64::decode(v & SYSTEM_BITS);
            (usize::from(id.mass_code), id.boxel_centre(), german_tank(id.system_index, v >> 55))
        })
    }

    /// Systems per pixel column, logged or not (see the module notes).
    fn estimate(&self) -> Vec<f32> {
        let n = MAP_SIZE * MAP_SIZE;
        let mut total = vec![0f32; n];
        let mut code_b = Vec::new();
        for code in 1..8 {
            let size = 10.0 * (1u32 << code) as f32;
            let mut grids = Grids::new();
            let mut any = false;
            for (_, centre, count) in self.samples().filter(|s| s.0 == code) {
                let profile = mean_profile(centre[1], size, scale_height(core_distance(centre[0], centre[2])));
                grids.add(centre, size, count as f32, profile.max(MIN_PROFILE));
                any = true;
            }
            if !any {
                continue;
            }
            let column = column_estimate(&grids, size);
            for (t, c) in total.iter_mut().zip(&column) {
                *t += c;
            }
            if code == 1 {
                code_b = column;
            }
        }
        // Mass code a, as a share of b measured around Sol.
        let (mut a, mut b) = (0.0, 0.0);
        for (code, centre, count) in self.samples().filter(|s| s.0 <= 1) {
            if centre.iter().map(|c| c * c).sum::<f32>() < CALIBRATION_RADIUS * CALIBRATION_RADIUS {
                if code == 0 {
                    a += count;
                } else {
                    b += count;
                }
            }
        }
        if b > 0.0 {
            let share = (a / b) as f32;
            for (t, c) in total.iter_mut().zip(&code_b) {
                *t += share * c;
            }
        }
        total
    }
}

/// How many systems a boxel holds, from the highest index seen in it and how
/// many of its systems were seen: the minimum-variance unbiased estimate for
/// numbers drawn without replacement from 0..n, never fewer than were seen.
fn german_tank(max_index: u32, logged: u64) -> f64 {
    let m = f64::from(max_index) + 1.0;
    let k = logged.max(1) as f64;
    (m * (1.0 + 1.0 / k) - 1.0).max(k)
}

/// The mean count over all boxels, empty ones included, given the mean over
/// non-empty ones: inverts `occupied = mean / (1 - exp(-mean))`.
fn poisson_mean(occupied: f32) -> f32 {
    if occupied <= 1.0 {
        return 0.0;
    }
    // Newton's method on mean - occupied * (1 - exp(-mean)), which is convex,
    // so from `occupied` (above the root) it closes in without overshooting.
    let mut mean = occupied;
    for _ in 0..30 {
        let e = (-mean).exp();
        let step = (mean - occupied * (1.0 - e)) / (1.0 - occupied * e);
        mean -= step;
        if step.abs() <= 1e-5 * mean.max(1e-3) {
            break;
        }
    }
    mean.max(0.0)
}

fn core_distance(x: f32, z: f32) -> f32 {
    (x - CORE[0]).hypot(z - CORE[2])
}

/// The disc's scale height at `r` ly from the core, as glow.ts draws it.
fn scale_height(r: f32) -> f32 {
    let t = (r / 9000.0).clamp(0.0, 1.0);
    1600.0 + (320.0 - 1600.0) * t * t * (3.0 - 2.0 * t)
}

/// The mean of the disc profile `exp(-|y - core y| / h)` over a boxel of
/// `size` ly whose centre is at height `y`.
fn mean_profile(y: f32, size: f32, h: f32) -> f32 {
    let lo = y - size / 2.0 - CORE[1];
    let hi = y + size / 2.0 - CORE[1];
    let integral = if lo >= 0.0 || hi <= 0.0 {
        let (near, far) = (lo.abs().min(hi.abs()), lo.abs().max(hi.abs()));
        h * ((-near / h).exp() - (-far / h).exp())
    } else {
        h * (2.0 - (lo / h).exp() - (-hi / h).exp())
    };
    integral / size
}

/// Per pixel, sums over the logged boxels of one mass code covering it.
struct Grids {
    /// Estimated systems.
    count: Vec<f32>,
    /// Profile weight at the boxel's height.
    profile: Vec<f32>,
    /// Boxels.
    boxels: Vec<f32>,
}

impl Grids {
    fn new() -> Self {
        let n = MAP_SIZE * MAP_SIZE;
        Self { count: vec![0.0; n], profile: vec![0.0; n], boxels: vec![0.0; n] }
    }

    /// Spreads one boxel over the pixels it covers.
    fn add(&mut self, centre: [f32; 3], size: f32, count: f32, profile: f32) {
        let px = ROOT_SIZE / MAP_SIZE as f32;
        let half = size / 2.0;
        let span = |c: f32, min: f32| {
            let lo = ((c - half - min) / px).floor().max(0.0) as usize;
            let hi = (((c + half - min) / px).ceil().max(0.0) as usize).min(MAP_SIZE);
            lo..hi.max(lo + 1).min(MAP_SIZE)
        };
        let cols = span(centre[0], ROOT_MIN[0]);
        let rows = span(centre[2], ROOT_MIN[2]);
        if cols.is_empty() || rows.is_empty() {
            return;
        }
        let share = 1.0 / (cols.len() * rows.len()) as f32;
        for row in rows {
            for col in cols.clone() {
                let i = row * MAP_SIZE + col;
                self.count[i] += count * share;
                self.profile[i] += profile * share;
                self.boxels[i] += share;
            }
        }
    }
}

/// Systems per pixel column of one mass code with boxels `size` ly across,
/// from the boxels logged around each pixel (see the module notes).
///
/// The windows go from the widest to the narrowest, and each narrower one
/// takes over as far as it holds enough logged boxels, so the estimate is
/// as local as the data allows and changes smoothly where that changes.
/// Where even the widest window holds few, the estimate fades out.
fn column_estimate(grids: &Grids, size: f32) -> Vec<f32> {
    let n = MAP_SIZE * MAP_SIZE;
    let mut plane = vec![0f32; n];
    let mut taper = vec![0f32; n];
    for (level, &r) in WINDOWS.iter().enumerate() {
        let count = blur(&grids.count, r);
        let profile = blur(&grids.profile, r);
        let boxels = blur(&grids.boxels, r);
        let area = ((2 * r + 1) * (2 * r + 1)) as f32;
        for i in 0..n {
            let samples = boxels[i] * area;
            if samples <= 1e-3 {
                continue;
            }
            // Expected systems per boxel at the plane, empty boxels included.
            let here = poisson_mean(count[i] / boxels[i]) / (profile[i] / boxels[i]);
            let weight = (samples / ENOUGH_SAMPLES).min(1.0);
            if level == 0 {
                plane[i] = here;
                taper[i] = weight * weight;
            } else {
                plane[i] += (here - plane[i]) * weight;
            }
        }
    }
    let px = ROOT_SIZE / MAP_SIZE as f32;
    let per_layer = px * px / (size * size);
    let mut out = vec![0f32; n];
    for row in 0..MAP_SIZE {
        let z = ROOT_MIN[2] + (row as f32 + 0.5) * px;
        for col in 0..MAP_SIZE {
            let x = ROOT_MIN[0] + (col as f32 + 0.5) * px;
            let i = row * MAP_SIZE + col;
            // The profile summed over the column's layers of boxels.
            let layers = 2.0 * scale_height(core_distance(x, z)) / size;
            out[i] = plane[i] * layers * per_layer * taper[i];
        }
    }
    out
}

/// Mean over a near-Gaussian window: three passes of a box `2r + 1` pixels
/// across, counting outside the map as zero.
fn blur(grid: &[f32], r: usize) -> Vec<f32> {
    let mut out = grid.to_vec();
    if r == 0 {
        return out;
    }
    let mut turned = vec![0f32; out.len()];
    let mut line = vec![0f32; MAP_SIZE];
    let rows = |g: &mut [f32], line: &mut [f32]| {
        for row in g.as_chunks_mut::<MAP_SIZE>().0 {
            box_line(row, r, line);
        }
    };
    // Columns are blurred as rows of the transposed map: far faster than
    // striding down them.
    for _ in 0..3 {
        rows(&mut out, &mut line);
        transpose(&out, &mut turned);
        rows(&mut turned, &mut line);
        transpose(&turned, &mut out);
    }
    out
}

fn transpose(src: &[f32], dst: &mut [f32]) {
    const BLOCK: usize = 32;
    for r0 in (0..MAP_SIZE).step_by(BLOCK) {
        for c0 in (0..MAP_SIZE).step_by(BLOCK) {
            for r in r0..r0 + BLOCK {
                for c in c0..c0 + BLOCK {
                    dst[c * MAP_SIZE + r] = src[r * MAP_SIZE + c];
                }
            }
        }
    }
}

/// Replaces `values` with their mean over a sliding window `2r + 1` long.
fn box_line(values: &mut [f32], r: usize, scratch: &mut [f32]) {
    let len = values.len();
    let width = (2 * r + 1) as f64;
    let mut sum: f64 = values[..r.min(len)].iter().map(|&v| f64::from(v)).sum();
    for i in 0..len {
        if i + r < len {
            sum += f64::from(values[i + r]);
        }
        if i > r {
            sum -= f64::from(values[i - r - 1]);
        }
        scratch[i] = (sum / width) as f32;
    }
    values.copy_from_slice(&scratch[..len]);
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
    fn keeps_the_highest_index_and_counts_per_boxel() {
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

        let mut seen: Vec<(u32, u64)> = acc.boxels.iter().map(|&v| (system_index(v), v >> 55)).collect();
        seen.sort();
        let colonia_index = Id64::decode(COLONIA).system_index;
        let mut expected = vec![(9, 4), (colonia_index, 1)];
        expected.sort();
        assert_eq!(seen, expected);
    }

    #[test]
    fn counts_stop_at_the_top_bits() {
        const SOL: u64 = 10477373803;
        let mut acc = Accumulator::with_capacity(1);
        for _ in 0..600 {
            acc.add_system(SOL, [0.0; 3]);
        }
        let v = *acc.boxels.iter().next().unwrap();
        assert_eq!(v >> 55, MAX_LOGGED);
        assert_eq!(v & SYSTEM_BITS, SOL & SYSTEM_BITS);
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

    #[test]
    fn german_tank_estimates() {
        // All of a boxel's systems seen: exactly that many.
        assert_eq!(german_tank(9, 10), 10.0);
        // One seen, index 4: 2 * 5 - 1.
        assert_eq!(german_tank(4, 1), 9.0);
        // Never fewer than were seen.
        assert_eq!(german_tank(0, 3), 3.0);
    }

    #[test]
    fn poisson_mean_inverts_the_occupied_mean() {
        assert_eq!(poisson_mean(1.0), 0.0);
        for mean in [0.3f32, 1.0, 2.5, 20.0] {
            let occupied = mean / (1.0 - (-mean).exp());
            assert!((poisson_mean(occupied) - mean).abs() < 0.01 * mean.max(1.0), "{mean}");
        }
    }

    #[test]
    fn profile_averages_over_the_boxel() {
        // A thin boxel at the plane: the profile's peak.
        assert!((mean_profile(CORE[1], 1.0, 300.0) - 1.0).abs() < 0.01);
        // One scale height up: 1/e.
        assert!((mean_profile(CORE[1] + 300.0, 1.0, 300.0) - (-1f32).exp()).abs() < 0.01);
        // A boxel far taller than the disc averages it out: 2h / size.
        assert!((mean_profile(CORE[1], 6000.0, 300.0) - 0.1).abs() < 0.001);
    }

    /// Mass code d boxels (80 ly) near Sol, at the plane, `per_boxel` systems
    /// each; `logged(i, j)` says which of a 60 x 60 grid of them were logged.
    fn sampled_disc(per_boxel: f32, logged: impl Fn(usize, usize) -> bool) -> Vec<f32> {
        let mut grids = Grids::new();
        let profile = mean_profile(CORE[1], 80.0, scale_height(core_distance(0.0, 0.0)));
        for i in 0..60 {
            for j in 0..60 {
                if logged(i, j) {
                    let centre = [-2400.0 + 80.0 * i as f32, CORE[1], -2400.0 + 80.0 * j as f32];
                    grids.add(centre, 80.0, per_boxel, profile);
                }
            }
        }
        column_estimate(&grids, 80.0)
    }

    fn at(map: &[f32], x: f32, z: f32) -> f32 {
        let (col, row) = (pixel(x, ROOT_MIN[0]).unwrap(), pixel(z, ROOT_MIN[2]).unwrap());
        map[row * MAP_SIZE + col]
    }

    #[test]
    fn estimate_does_not_follow_exploration() {
        let all = sampled_disc(50.0, |_, _| true);
        // A quarter of the boxels logged, in lanes.
        let lanes = sampled_disc(50.0, |i, _| i % 4 == 0);
        for (x, z) in [(0.0, 0.0), (500.0, -700.0), (-1000.0, 300.0)] {
            let (full, sparse) = (at(&all, x, z), at(&lanes, x, z));
            assert!(full > 0.0);
            assert!((sparse / full - 1.0).abs() < 0.1, "at ({x}, {z}): {sparse} vs {full}");
        }
        // Between two lanes as much as on one.
        assert!((at(&lanes, 40.0, 0.0) / at(&lanes, 200.0, 0.0) - 1.0).abs() < 0.1);
    }

    #[test]
    fn estimate_fades_out_beyond_the_logged_boxels() {
        let all = sampled_disc(50.0, |_, _| true);
        assert!(at(&all, 0.0, 0.0) > 0.0);
        assert_eq!(at(&all, 10_000.0, 0.0), 0.0);
    }

    #[test]
    fn mostly_empty_boxels_count_for_little() {
        // Logged boxels holding one system each: most of their neighbours
        // hold none, so the estimate is far below one system per boxel.
        let sparse = sampled_disc(1.05, |i, j| (i + j) % 2 == 0);
        let full = sampled_disc(50.0, |i, j| (i + j) % 2 == 0);
        assert!(at(&sparse, 0.0, 0.0) < 0.01 * at(&full, 0.0, 0.0));
    }
}
