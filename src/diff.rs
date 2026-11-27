//! Revision change detection: compares two renders of a sheet (old and
//! new, same scale and origin) and returns the regions that changed.
//! Pure logic, testable anywhere.

/// A changed region in image pixels, with how much ink was added/removed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
    pub added: u32,
    pub removed: u32,
}

/// Grid cell size (px) used to group changed pixels.
const CELL: u32 = 8;
/// Changed pixels needed for a cell to count (filters render noise).
const MIN_CELL_PX: u32 = 3;
/// Cells this close are merged into one region.
const MERGE_CELLS: i32 = 5;

/// Ink mask from a BGRA image: dark-ish pixels (lines, text, fills).
pub fn ink_mask(bgra: &[u8], w: u32, h: u32) -> Vec<bool> {
    (0..(w * h) as usize)
        .map(|i| {
            let p = &bgra[i * 4..i * 4 + 3];
            // Luma of B, G, R.
            let l = 0.114 * p[0] as f32 + 0.587 * p[1] as f32 + 0.299 * p[2] as f32;
            l < 200.0
        })
        .collect()
}

/// Dilate a mask by one pixel (3x3), so tiny shifts don't count as changes.
fn dilate(m: &[bool], w: u32, h: u32) -> Vec<bool> {
    let (w, h) = (w as usize, h as usize);
    // Separable: horizontal then vertical.
    let mut hz = vec![false; w * h];
    for y in 0..h {
        let row = &m[y * w..(y + 1) * w];
        for x in 0..w {
            hz[y * w + x] = row[x] || (x > 0 && row[x - 1]) || (x + 1 < w && row[x + 1]);
        }
    }
    let mut out = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            out[i] = hz[i] || (y > 0 && hz[i - w]) || (y + 1 < h && hz[i + w]);
        }
    }
    out
}

/// Compare two same-size renders and return changed regions, sorted in
/// reading order (top to bottom, then left to right).
pub fn find_changes(old: &[u8], new: &[u8], w: u32, h: u32) -> Vec<Region> {
    let a = ink_mask(old, w, h);
    let b = ink_mask(new, w, h);
    let (da, db) = (dilate(&a, w, h), dilate(&b, w, h));
    let (gw, gh) = (w.div_ceil(CELL), h.div_ceil(CELL));
    // Per cell: (added px, removed px).
    let mut cells = vec![(0u32, 0u32); (gw * gh) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let c = ((y / CELL) * gw + x / CELL) as usize;
            if b[i] && !da[i] {
                cells[c].0 += 1;
            } else if a[i] && !db[i] {
                cells[c].1 += 1;
            }
        }
    }
    let changed: Vec<bool> = cells.iter().map(|(ad, rm)| ad + rm >= MIN_CELL_PX).collect();

    // Group changed cells: flood fill with a merge radius.
    let mut label = vec![usize::MAX; changed.len()];
    let mut regions = Vec::new();
    for start in 0..changed.len() {
        if !changed[start] || label[start] != usize::MAX {
            continue;
        }
        let id = regions.len();
        let mut r = Region { x0: u32::MAX, y0: u32::MAX, x1: 0, y1: 0, added: 0, removed: 0 };
        let mut stack = vec![start];
        label[start] = id;
        while let Some(c) = stack.pop() {
            let (cx, cy) = ((c as u32 % gw) as i32, (c as u32 / gw) as i32);
            r.x0 = r.x0.min(cx as u32);
            r.y0 = r.y0.min(cy as u32);
            r.x1 = r.x1.max(cx as u32 + 1);
            r.y1 = r.y1.max(cy as u32 + 1);
            r.added += cells[c].0;
            r.removed += cells[c].1;
            for dy in -MERGE_CELLS..=MERGE_CELLS {
                for dx in -MERGE_CELLS..=MERGE_CELLS {
                    let (nx, ny) = (cx + dx, cy + dy);
                    if nx < 0 || ny < 0 || nx >= gw as i32 || ny >= gh as i32 {
                        continue;
                    }
                    let n = (ny as u32 * gw + nx as u32) as usize;
                    if changed[n] && label[n] == usize::MAX {
                        label[n] = id;
                        stack.push(n);
                    }
                }
            }
        }
        regions.push(r);
    }
    // Cells -> pixels.
    let mut out: Vec<Region> = regions
        .into_iter()
        .map(|r| Region {
            x0: r.x0 * CELL,
            y0: r.y0 * CELL,
            x1: (r.x1 * CELL).min(w),
            y1: (r.y1 * CELL).min(h),
            ..r
        })
        .collect();
    // Merge regions whose boxes overlap (after grouping some can).
    loop {
        let mut merged = false;
        'outer: for i in 0..out.len() {
            for j in i + 1..out.len() {
                let (a, b) = (out[i], out[j]);
                if a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1 {
                    out[i] = Region {
                        x0: a.x0.min(b.x0),
                        y0: a.y0.min(b.y0),
                        x1: a.x1.max(b.x1),
                        y1: a.y1.max(b.y1),
                        added: a.added + b.added,
                        removed: a.removed + b.removed,
                    };
                    out.remove(j);
                    merged = true;
                    break 'outer;
                }
            }
        }
        if !merged {
            break;
        }
    }
    // Reading order on a coarse row grid.
    let band = (h / 12).max(1);
    out.sort_by_key(|r| (r.y0 / band, r.x0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(w: u32, h: u32) -> Vec<u8> {
        vec![255u8; (w * h * 4) as usize]
    }
    fn rect(img: &mut [u8], w: u32, x0: u32, y0: u32, x1: u32, y1: u32) {
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y * w + x) * 4) as usize;
                img[i..i + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
    }

    #[test]
    fn finds_added_removed_and_ignores_noise() {
        let (w, h) = (400u32, 300u32);
        let mut old = canvas(w, h);
        let mut new = canvas(w, h);
        // Unchanged wall in both.
        rect(&mut old, w, 10, 10, 390, 14);
        rect(&mut new, w, 10, 10, 390, 14);
        // A 1 px shift of a line must not count.
        rect(&mut old, w, 50, 100, 51, 200);
        rect(&mut new, w, 51, 100, 52, 200);
        // A door removed at the left, a new wall added at the right.
        rect(&mut old, w, 40, 220, 70, 250);
        rect(&mut new, w, 300, 150, 360, 156);
        let r = find_changes(&old, &new, w, h);
        assert_eq!(r.len(), 2, "{r:?}");
        // Reading order: the added wall (y=150) comes before the door (y=220).
        assert!(r[0].added > 0 && r[0].removed == 0 && r[0].x0 <= 300 && r[0].x1 >= 360);
        assert!(r[1].removed > 0 && r[1].added == 0 && r[1].x0 <= 40 && r[1].x1 >= 70);
        // Identical images: nothing.
        assert!(find_changes(&old, &old, w, h).is_empty());
    }

    #[test]
    fn nearby_changes_merge() {
        let (w, h) = (200u32, 200u32);
        let old = canvas(w, h);
        let mut new = canvas(w, h);
        // Three words of a note, a few px apart: one region.
        rect(&mut new, w, 20, 20, 40, 28);
        rect(&mut new, w, 46, 20, 66, 28);
        rect(&mut new, w, 72, 20, 92, 28);
        let r = find_changes(&old, &new, w, h);
        assert_eq!(r.len(), 1, "{r:?}");
    }
}
