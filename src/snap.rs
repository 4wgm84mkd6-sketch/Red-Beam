//! Snap index over a sheet's line work (page DIPs): finds the nearest line
//! end / corner, or else the nearest point on a line, within a tolerance.
//! Pure logic so it can be unit-tested anywhere.

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    /// End of a line or corner of a shape.
    Endpoint,
    /// Somewhere along a line.
    OnLine,
}

/// Grid buckets of vertices and segments.
pub struct SnapIndex {
    cell: f32,
    verts: HashMap<(i32, i32), Vec<[f32; 2]>>,
    segs: HashMap<(i32, i32), Vec<u32>>,
    seg_list: Vec<[[f32; 2]; 2]>,
    pub vertex_count: usize,
    /// Segments were skipped because the sheet was too dense.
    pub truncated: bool,
}

/// Segment budget: beyond this only endpoints are indexed (keeps memory
/// and build time bounded on extremely dense sheets).
const MAX_SEGMENTS: usize = 3_000_000;
/// A segment spanning more cells than this is indexed by endpoints only.
const MAX_CELLS_PER_SEG: usize = 4096;

impl SnapIndex {
    /// `segments` in page DIPs. `cell` ~ a few times the snap tolerance.
    pub fn build(segments: &[[[f32; 2]; 2]], cell: f32) -> SnapIndex {
        let mut idx = SnapIndex {
            cell,
            verts: HashMap::new(),
            segs: HashMap::new(),
            seg_list: Vec::new(),
            vertex_count: 0,
            truncated: segments.len() > MAX_SEGMENTS,
        };
        for s in segments {
            for p in s {
                idx.add_vertex(*p);
            }
        }
        if !idx.truncated {
            for s in segments {
                idx.add_segment(*s);
            }
        }
        idx
    }

    fn key(&self, p: [f32; 2]) -> (i32, i32) {
        ((p[0] / self.cell).floor() as i32, (p[1] / self.cell).floor() as i32)
    }

    fn add_vertex(&mut self, p: [f32; 2]) {
        if !(p[0].is_finite() && p[1].is_finite()) {
            return;
        }
        let k = self.key(p);
        let v = self.verts.entry(k).or_default();
        // Shared corners appear many times; keep one.
        if !v.iter().any(|q| (q[0] - p[0]).abs() < 0.01 && (q[1] - p[1]).abs() < 0.01) {
            v.push(p);
            self.vertex_count += 1;
        }
    }

    fn add_segment(&mut self, s: [[f32; 2]; 2]) {
        let (a, b) = (s[0], s[1]);
        if !(a[0].is_finite() && a[1].is_finite() && b[0].is_finite() && b[1].is_finite()) {
            return;
        }
        if (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3 {
            return;
        }
        let id = self.seg_list.len() as u32;
        self.seg_list.push(s);
        // Walk the cells along the segment (sampled at half-cell steps).
        let len = ((b[0] - a[0]).hypot(b[1] - a[1]) / (self.cell * 0.5)).ceil() as usize;
        if len > MAX_CELLS_PER_SEG {
            return;
        }
        let mut last = None;
        for i in 0..=len.max(1) {
            let t = i as f32 / len.max(1) as f32;
            let k = self.key([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
            if last != Some(k) {
                let v = self.segs.entry(k).or_default();
                if v.last() != Some(&id) {
                    v.push(id);
                }
                last = Some(k);
            }
        }
    }

    /// Nearest endpoint within `tol`, else nearest point on a line within
    /// `tol` (page DIPs).
    pub fn snap(&self, p: [f32; 2], tol: f32) -> Option<([f32; 2], SnapKind)> {
        let r = (tol / self.cell).ceil() as i32;
        let (cx, cy) = self.key(p);
        let tol2 = tol * tol;
        let mut best: Option<([f32; 2], f32)> = None;
        for dx in -r..=r {
            for dy in -r..=r {
                for q in self.verts.get(&(cx + dx, cy + dy)).into_iter().flatten() {
                    let d2 = (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2);
                    if d2 <= tol2 && best.is_none_or(|(_, b)| d2 < b) {
                        best = Some((*q, d2));
                    }
                }
            }
        }
        if let Some((q, _)) = best {
            return Some((q, SnapKind::Endpoint));
        }
        let mut best: Option<([f32; 2], f32)> = None;
        for dx in -r..=r {
            for dy in -r..=r {
                for &id in self.segs.get(&(cx + dx, cy + dy)).into_iter().flatten() {
                    let [a, b] = self.seg_list[id as usize];
                    let (vx, vy) = (b[0] - a[0], b[1] - a[1]);
                    let l2 = vx * vx + vy * vy;
                    let t = (((p[0] - a[0]) * vx + (p[1] - a[1]) * vy) / l2).clamp(0.0, 1.0);
                    let q = [a[0] + t * vx, a[1] + t * vy];
                    let d2 = (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2);
                    if d2 <= tol2 && best.is_none_or(|(_, b)| d2 < b) {
                        best = Some((q, d2));
                    }
                }
            }
        }
        best.map(|(q, _)| (q, SnapKind::OnLine))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_corners_then_lines() {
        // A 100 x 50 rectangle plus a long diagonal.
        let segs = [
            [[0.0, 0.0], [100.0, 0.0]],
            [[100.0, 0.0], [100.0, 50.0]],
            [[100.0, 50.0], [0.0, 50.0]],
            [[0.0, 50.0], [0.0, 0.0]],
            [[0.0, 0.0], [1000.0, 1000.0]],
        ];
        let idx = SnapIndex::build(&segs, 8.0);
        assert_eq!(idx.vertex_count, 5); // shared corners de-duplicated
        assert_eq!(idx.snap([98.0, 2.5], 4.0), Some(([100.0, 0.0], SnapKind::Endpoint)));
        assert_eq!(idx.snap([50.0, 2.0], 4.0), Some(([50.0, 0.0], SnapKind::OnLine)));
        let (q, k) = idx.snap([502.0, 498.0], 4.0).unwrap();
        assert_eq!(k, SnapKind::OnLine);
        assert!((q[0] - 500.0).abs() < 1e-3 && (q[1] - 500.0).abs() < 1e-3);
        assert_eq!(idx.snap([50.0, 25.0], 4.0), None);
        // Corner beats a nearer point on a line.
        assert_eq!(idx.snap([3.0, 0.5], 4.0).unwrap().1, SnapKind::Endpoint);
    }
}
