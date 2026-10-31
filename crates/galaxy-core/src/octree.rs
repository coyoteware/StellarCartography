//! Point-cloud octree tiles.
//!
//! The galaxy is split into a Potree-style octree. Each node stores a spatial
//! subsample of the points beneath it, and the remaining points are pushed to
//! its children. Rendering a node and all of its ancestors therefore draws
//! every point in that region, while drawing only the shallow nodes gives a
//! cheap overview.
//!
//! On disk there is one `tiles/<key>.bin` per node plus `tiles/index.json`.
//! Keys are `r` followed by one octant digit per level. Octant bit 0 is +x,
//! bit 1 is +y, bit 2 is +z. Tile layout (little endian):
//!
//! ```text
//! 0   magic  "SCT1"
//! 4   u32    point count n
//! 8   f32x3  node min corner (ly)
//! 20  f32    node edge length (ly)
//! 24  u32 x n          system rowid in galaxy.sqlite
//! ..  u16 x 3n         x,y,z quantised to [0, 65535] across the node
//! ..  u8  x n          id64 mass code (0 = a .. 7 = h)
//! ```

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::Path;
use std::sync::Mutex;

use anyhow::{bail, Context, Result};
use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};

use crate::Vec3;

pub const MAGIC: &[u8; 4] = b"SCT1";
pub const HEADER_LEN: usize = 24;
pub const INDEX_VERSION: u32 = 1;

/// Fixed root cube. It covers the whole sector grid in x and z, and far more
/// than the galaxy's thickness in y.
pub const ROOT_MIN: Vec3 = [-50000.0, -50000.0, -25000.0];
pub const ROOT_SIZE: f32 = 100000.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub pos: Vec3,
    pub rowid: u32,
    pub mass_code: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct BuildParams {
    /// Sampling grid resolution per node axis: at most one point per cell is
    /// kept in a node, the rest go to its children.
    pub sample_grid: u32,
    /// Upper bound on the points stored in one inner node.
    pub max_node_points: usize,
    /// Nodes with this many points or fewer become leaves.
    pub leaf_points: usize,
    pub max_depth: usize,
}

impl Default for BuildParams {
    fn default() -> Self {
        Self { sample_grid: 128, max_node_points: 40_000, leaf_points: 40_000, max_depth: 20 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OctreeIndex {
    pub version: u32,
    pub root_min: Vec3,
    pub root_size: f32,
    pub sample_grid: u32,
    pub total_points: u64,
    pub nodes: Vec<NodeEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeEntry {
    pub key: String,
    pub count: u32,
    /// Bit `i` is set when child octant `i` exists.
    pub children: u8,
}

pub fn contains_root(p: Vec3) -> bool {
    (0..3).all(|i| p[i] >= ROOT_MIN[i] && p[i] < ROOT_MIN[i] + ROOT_SIZE)
}

/// Min corner and edge length of the node with the given key.
pub fn node_bounds(key: &str) -> (Vec3, f32) {
    let mut min = ROOT_MIN;
    let mut size = ROOT_SIZE;
    for digit in key.bytes().skip(1) {
        size /= 2.0;
        let octant = digit - b'0';
        for (axis, m) in min.iter_mut().enumerate() {
            if octant & (1 << axis) != 0 {
                *m += size;
            }
        }
    }
    (min, size)
}

/// Shuffles the points, then builds the octree into `out_dir`
/// (`out_dir/index.json`, `out_dir/<key>.bin`). Points outside the root cube
/// must already have been filtered out.
pub fn build(points: &mut [Point], out_dir: &Path, params: BuildParams) -> Result<OctreeIndex> {
    std::fs::create_dir_all(out_dir)?;
    // Random order makes first-come grid sampling an unbiased subsample.
    shuffle(points, 0x5eed_cafe_f00d_d00d);
    let nodes = Mutex::new(Vec::new());
    build_node(points, "r".to_string(), ROOT_MIN, ROOT_SIZE, 0, out_dir, &params, &nodes)?;
    let mut nodes = nodes.into_inner().unwrap();
    nodes.sort_by(|a: &NodeEntry, b| a.key.len().cmp(&b.key.len()).then_with(|| a.key.cmp(&b.key)));
    let index = OctreeIndex {
        version: INDEX_VERSION,
        root_min: ROOT_MIN,
        root_size: ROOT_SIZE,
        sample_grid: params.sample_grid,
        total_points: points.len() as u64,
        nodes,
    };
    let file = BufWriter::new(File::create(out_dir.join("index.json"))?);
    serde_json::to_writer(file, &index)?;
    Ok(index)
}

#[allow(clippy::too_many_arguments)]
fn build_node(
    points: &mut [Point],
    key: String,
    min: Vec3,
    size: f32,
    depth: usize,
    out_dir: &Path,
    params: &BuildParams,
    nodes: &Mutex<Vec<NodeEntry>>,
) -> Result<()> {
    if points.is_empty() {
        return Ok(());
    }
    if points.len() <= params.leaf_points || depth >= params.max_depth {
        write_tile(&out_dir.join(format!("{key}.bin")), min, size, points)?;
        nodes.lock().unwrap().push(NodeEntry { key, count: points.len() as u32, children: 0 });
        return Ok(());
    }

    let selected = sample_to_front(points, min, size, params);
    let (own, rest) = points.split_at_mut(selected);
    write_tile(&out_dir.join(format!("{key}.bin")), min, size, own)?;

    let half = size / 2.0;
    let mid = [min[0] + half, min[1] + half, min[2] + half];
    let counts = partition_octants(rest, mid);

    let mut children = 0u8;
    let mut slices = Vec::with_capacity(8);
    let mut remaining = rest;
    for (octant, &count) in counts.iter().enumerate() {
        let (child, tail) = remaining.split_at_mut(count);
        remaining = tail;
        if count > 0 {
            children |= 1 << octant;
            slices.push((octant, child));
        }
    }
    nodes.lock().unwrap().push(NodeEntry { key: key.clone(), count: selected as u32, children });

    let child_min = |octant: usize| {
        let mut m = min;
        for (axis, v) in m.iter_mut().enumerate() {
            if octant & (1 << axis) != 0 {
                *v += half;
            }
        }
        m
    };
    // Fan out across threads near the root only; deeper levels run inline.
    if depth < 2 {
        std::thread::scope(|scope| {
            let handles: Vec<_> = slices
                .into_iter()
                .map(|(octant, child)| {
                    let (child_key, cmin) = (format!("{key}{octant}"), child_min(octant));
                    scope.spawn(move || build_node(child, child_key, cmin, half, depth + 1, out_dir, params, nodes))
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect::<Result<()>>()
        })
    } else {
        for (octant, child) in slices {
            build_node(child, format!("{key}{octant}"), child_min(octant), half, depth + 1, out_dir, params, nodes)?;
        }
        Ok(())
    }
}

/// Moves up to `max_node_points` grid-spread points to the front of the
/// slice and returns how many were moved.
fn sample_to_front(points: &mut [Point], min: Vec3, size: f32, params: &BuildParams) -> usize {
    let grid = params.sample_grid;
    let scale = grid as f32 / size;
    let mut taken = FxHashSet::default();
    taken.reserve(params.max_node_points.min(points.len()));
    let mut selected = 0;
    for i in 0..points.len() {
        if selected >= params.max_node_points {
            break;
        }
        let p = points[i].pos;
        let cell = |axis: usize| (((p[axis] - min[axis]) * scale) as u32).min(grid - 1);
        let id = (cell(2) * grid + cell(1)) * grid + cell(0);
        if taken.insert(id) {
            points.swap(i, selected);
            selected += 1;
        }
    }
    selected
}

/// In-place 8-way partition by octant around `mid`. Returns the per-octant counts.
fn partition_octants(points: &mut [Point], mid: Vec3) -> [usize; 8] {
    let octant = |p: &Point| {
        (usize::from(p.pos[0] >= mid[0]))
            | (usize::from(p.pos[1] >= mid[1]) << 1)
            | (usize::from(p.pos[2] >= mid[2]) << 2)
    };
    let mut counts = [0usize; 8];
    for p in points.iter() {
        counts[octant(p)] += 1;
    }
    let mut next = [0usize; 8];
    let mut end = [0usize; 8];
    let mut offset = 0;
    for b in 0..8 {
        next[b] = offset;
        offset += counts[b];
        end[b] = offset;
    }
    for b in 0..8 {
        while next[b] < end[b] {
            let o = octant(&points[next[b]]);
            if o == b {
                next[b] += 1;
            } else {
                points.swap(next[b], next[o]);
                next[o] += 1;
            }
        }
    }
    counts
}

fn shuffle(points: &mut [Point], seed: u64) {
    let mut state = seed;
    for i in (1..points.len()).rev() {
        // xorshift64*
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let r = state.wrapping_mul(0x2545_f491_4f6c_dd1d);
        points.swap(i, (r % (i as u64 + 1)) as usize);
    }
}

pub fn quantise(v: f32, min: f32, size: f32) -> u16 {
    (((v - min) / size) * 65535.0).round().clamp(0.0, 65535.0) as u16
}

pub fn dequantise(q: u16, min: f32, size: f32) -> f32 {
    min + f32::from(q) / 65535.0 * size
}

pub fn write_tile(path: &Path, min: Vec3, size: f32, points: &[Point]) -> Result<()> {
    let mut w = BufWriter::with_capacity(1 << 20, File::create(path)?);
    w.write_all(MAGIC)?;
    w.write_all(&(points.len() as u32).to_le_bytes())?;
    for m in min {
        w.write_all(&m.to_le_bytes())?;
    }
    w.write_all(&size.to_le_bytes())?;
    for p in points {
        w.write_all(&p.rowid.to_le_bytes())?;
    }
    for p in points {
        for axis in 0..3 {
            w.write_all(&quantise(p.pos[axis], min[axis], size).to_le_bytes())?;
        }
    }
    for p in points {
        w.write_all(&[p.mass_code])?;
    }
    w.flush()?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Tile {
    pub min: Vec3,
    pub size: f32,
    pub points: Vec<Point>,
}

pub fn read_tile(path: &Path) -> Result<Tile> {
    let mut buf = Vec::new();
    File::open(path).with_context(|| format!("open {}", path.display()))?.read_to_end(&mut buf)?;
    if buf.len() < HEADER_LEN || &buf[0..4] != MAGIC {
        bail!("{} is not a tile", path.display());
    }
    let u32_at = |o: usize| u32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
    let f32_at = |o: usize| f32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
    let n = u32_at(4) as usize;
    let min = [f32_at(8), f32_at(12), f32_at(16)];
    let size = f32_at(20);
    let expected = HEADER_LEN + n * 11;
    if buf.len() != expected {
        bail!("{}: expected {expected} bytes, found {}", path.display(), buf.len());
    }
    let pos_off = HEADER_LEN + 4 * n;
    let mc_off = pos_off + 6 * n;
    let points = (0..n)
        .map(|i| {
            let q = |axis: usize| {
                let o = pos_off + (i * 3 + axis) * 2;
                u16::from_le_bytes([buf[o], buf[o + 1]])
            };
            Point {
                rowid: u32_at(HEADER_LEN + 4 * i),
                pos: [
                    dequantise(q(0), min[0], size),
                    dequantise(q(1), min[1], size),
                    dequantise(q(2), min[2], size),
                ],
                mass_code: buf[mc_off + i],
            }
        })
        .collect();
    Ok(Tile { min, size, points })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn random_points(n: usize) -> Vec<Point> {
        let mut state = 42u64;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((state >> 33) as f32) / (1u64 << 31) as f32
        };
        (0..n)
            .map(|i| {
                // A thin, clustered "disc" like the real data, plus a dense bubble at Sol.
                let pos = if i % 3 == 0 {
                    [next() * 400.0 - 200.0, next() * 400.0 - 200.0, next() * 400.0 - 200.0]
                } else {
                    [next() * 90000.0 - 45000.0, next() * 2000.0 - 1000.0, next() * 90000.0 - 20000.0]
                };
                Point { pos, rowid: i as u32, mass_code: (i % 8) as u8 }
            })
            .collect()
    }

    #[test]
    fn round_trip_keeps_every_point_within_quantisation_error() {
        let dir = tempfile::tempdir().unwrap();
        let original = random_points(250_000);
        let mut points = original.clone();
        let params = BuildParams { max_node_points: 5_000, leaf_points: 5_000, ..Default::default() };
        let index = build(&mut points, dir.path(), params).unwrap();

        assert_eq!(index.total_points, original.len() as u64);
        let stored: u64 = index.nodes.iter().map(|n| u64::from(n.count)).sum();
        assert_eq!(stored, original.len() as u64);
        assert!(index.nodes.len() > 8, "expected a real tree, got {} nodes", index.nodes.len());

        let mut seen = vec![false; original.len()];
        for node in &index.nodes {
            let tile = read_tile(&dir.path().join(format!("{}.bin", node.key))).unwrap();
            let (min, size) = node_bounds(&node.key);
            assert_eq!(tile.min, min);
            assert_eq!(tile.size, size);
            assert_eq!(tile.points.len(), node.count as usize);
            let tolerance = size / 65535.0;
            for p in &tile.points {
                let o = &original[p.rowid as usize];
                assert!(!seen[p.rowid as usize], "rowid {} stored twice", p.rowid);
                seen[p.rowid as usize] = true;
                assert_eq!(p.mass_code, o.mass_code);
                for axis in 0..3 {
                    let err = (p.pos[axis] - o.pos[axis]).abs();
                    assert!(err <= tolerance, "node {} axis {axis}: err {err} > {tolerance}", node.key);
                }
            }
        }
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn children_mask_matches_written_nodes() {
        let dir = tempfile::tempdir().unwrap();
        let mut points = random_points(60_000);
        let params = BuildParams { max_node_points: 2_000, leaf_points: 2_000, ..Default::default() };
        let index = build(&mut points, dir.path(), params).unwrap();
        let keys: std::collections::HashSet<_> = index.nodes.iter().map(|n| n.key.clone()).collect();
        for node in &index.nodes {
            for octant in 0..8u8 {
                let child = format!("{}{}", node.key, octant);
                assert_eq!(node.children & (1 << octant) != 0, keys.contains(&child), "{child}");
            }
        }
    }

    #[test]
    fn node_bounds_follow_octant_bits() {
        let (min, size) = node_bounds("r5"); // +x, +z
        assert_eq!(size, ROOT_SIZE / 2.0);
        assert_eq!(min, [ROOT_MIN[0] + size, ROOT_MIN[1], ROOT_MIN[2] + size]);
    }
}
