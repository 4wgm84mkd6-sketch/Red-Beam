//! Camera / coordinate math. Page space is in DIPs (1/96 inch) as reported by
//! `PdfPage.Size`; screen space is physical pixels of the canvas.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RectF {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32,
}

impl RectF {
    pub const fn new(l: f32, t: f32, r: f32, b: f32) -> Self {
        Self { l, t, r, b }
    }
    pub fn w(&self) -> f32 {
        self.r - self.l
    }
    pub fn h(&self) -> f32 {
        self.b - self.t
    }
    pub fn is_empty(&self) -> bool {
        self.r <= self.l || self.b <= self.t
    }
    pub fn intersect(&self, o: &RectF) -> RectF {
        RectF::new(self.l.max(o.l), self.t.max(o.t), self.r.min(o.r), self.b.min(o.b))
    }
    pub fn contains_rect(&self, o: &RectF, eps: f32) -> bool {
        o.l >= self.l - eps && o.t >= self.t - eps && o.r <= self.r + eps && o.b <= self.b + eps
    }
    pub fn contains_pt(&self, x: f32, y: f32) -> bool {
        x >= self.l && x < self.r && y >= self.t && y < self.b
    }
    /// Expand outward to whole pixels.
    pub fn snap_out(&self) -> RectF {
        RectF::new(self.l.floor(), self.t.floor(), self.r.ceil(), self.b.ceil())
    }
    pub fn round(&self) -> RectF {
        RectF::new(self.l.round(), self.t.round(), self.r.round(), self.b.round())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitMode {
    None,
    Page,
    Width,
}

/// `screen = page * zoom + origin`. `zoom` is physical pixels per page DIP.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub zoom: f32,
    pub ox: f32,
    pub oy: f32,
}

impl Default for View {
    fn default() -> Self {
        Self { zoom: 1.0, ox: 0.0, oy: 0.0 }
    }
}

impl View {
    pub fn page_to_screen(&self, r: &RectF) -> RectF {
        RectF::new(
            r.l * self.zoom + self.ox,
            r.t * self.zoom + self.oy,
            r.r * self.zoom + self.ox,
            r.b * self.zoom + self.oy,
        )
    }

    pub fn screen_to_page(&self, r: &RectF) -> RectF {
        RectF::new(
            (r.l - self.ox) / self.zoom,
            (r.t - self.oy) / self.zoom,
            (r.r - self.ox) / self.zoom,
            (r.b - self.oy) / self.zoom,
        )
    }

    /// Zoom by `factor`, keeping the page point under (sx, sy) fixed on screen.
    pub fn zoom_at(&mut self, sx: f32, sy: f32, factor: f32, min: f32, max: f32) {
        let nz = (self.zoom * factor).clamp(min, max);
        let px = (sx - self.ox) / self.zoom;
        let py = (sy - self.oy) / self.zoom;
        self.zoom = nz;
        self.ox = sx - px * nz;
        self.oy = sy - py * nz;
    }

    pub fn fit_page(vw: f32, vh: f32, pw: f32, ph: f32, margin: f32) -> View {
        let zoom = ((vw - 2.0 * margin) / pw).min((vh - 2.0 * margin) / ph).max(1e-4);
        View {
            zoom,
            ox: ((vw - pw * zoom) * 0.5).round(),
            oy: ((vh - ph * zoom) * 0.5).round(),
        }
    }

    pub fn fit_width(vw: f32, vh: f32, pw: f32, ph: f32, margin: f32) -> View {
        let zoom = ((vw - 2.0 * margin) / pw).max(1e-4);
        let oy = if ph * zoom < vh { (vh - ph * zoom) * 0.5 } else { margin };
        View { zoom, ox: margin.round(), oy: oy.round() }
    }

    /// Keep at least `keep` pixels of the page inside the viewport.
    pub fn clamp_pan(&mut self, vw: f32, vh: f32, pw: f32, ph: f32, keep: f32) {
        let sw = pw * self.zoom;
        let sh = ph * self.zoom;
        let kx = keep.min(sw);
        let ky = keep.min(sh);
        self.ox = self.ox.clamp(kx - sw, vw - kx);
        self.oy = self.oy.clamp(ky - sh, vh - ky);
    }
}
