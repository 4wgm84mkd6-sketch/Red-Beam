//! Importing markups and measurements that other apps (Bluebeam Revu,
//! Acrobat, consultants) left in the PDF. When a drawing is opened for the
//! first time, its annotations are read in the background; if there are
//! any, Red-Beam asks once whether to import them. Imported items become
//! normal Red-Beam markups and measurements (editable, totalled, exported),
//! and the PDF's own copies are hidden so nothing shows twice. The PDF
//! itself is never changed.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use super::tools::Undo;
use super::{App, WM_APP_BG};
use crate::import::{convert, snap_ratio, Imported, RawAnnot};
use crate::takeoff::{Scale, Viewport};

/// Layer imported measurements go on (so they can be hidden or locked).
pub const IMPORT_LAYER: &str = "Imported";

pub type ImportResult = (PathBuf, bool, Result<Vec<RawAnnot>, String>);

/// The import channel: (drawing, asked automatically?, annotations).
pub struct ImportChannel {
    pub tx: Sender<ImportResult>,
    pub rx: Receiver<ImportResult>,
}

impl ImportChannel {
    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        ImportChannel { tx, rx }
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl App {
    /// Source keys of everything already imported into this drawing.
    fn known_sources(&self) -> Vec<String> {
        self.takeoff
            .markups
            .iter()
            .filter_map(|m| m.source.clone())
            .chain(self.takeoff.items.iter().filter_map(|m| m.source.clone()))
            .collect()
    }

    /// Read the open drawing's annotations in the background. `auto`: on
    /// opening (ask first); otherwise from the menu (import right away).
    pub(super) fn start_import_scan(&mut self, auto: bool) {
        let Some(doc) = &self.doc else { return };
        if !crate::pdfium::available() {
            if !auto {
                self.pending_error = Some("Importing markups needs the PDFium engine.".into());
            }
            return;
        }
        let path = doc.path.clone();
        let tx = self.import.tx.clone();
        let hwnd = self.hwnd.0 as isize;
        if !auto {
            self.status = Some("Reading the PDF's markups\u{2026}".into());
        }
        self.import_pending += 1;
        std::thread::spawn(move || {
            let r = crate::pdfium::read_annots(&path);
            let _ = tx.send((path, auto, r));
            unsafe {
                let _ = PostMessageW(Some(windows::Win32::Foundation::HWND(hwnd as *mut _)), WM_APP_BG, WPARAM(0), LPARAM(0));
            }
        });
    }

    /// Results of a scan arrived (called from the background-message pump).
    pub(super) fn on_import_msgs(&mut self) {
        while let Ok((path, auto, result)) = self.import.rx.try_recv() {
            self.import_pending = self.import_pending.saturating_sub(1);
            if !self.doc.as_ref().is_some_and(|d| crate::settings::same_path(&d.path, &path)) {
                continue; // drawing closed or switched meanwhile
            }
            let raws = match result {
                Ok(r) => r,
                Err(e) => {
                    if !auto {
                        self.pending_error = Some(format!("Could not read the PDF's markups:\n{e}"));
                    }
                    continue;
                }
            };
            let preview = convert(&raws, &mut 0, &self.known_sources(), 0);
            let (nm, nt) = (preview.markups.len(), preview.measures.len() + preview.own_measures.len());
            if nm + nt == 0 {
                if !auto {
                    self.status = None;
                    self.pending_info = Some("This PDF has no markups that haven't been imported already.".into());
                }
                continue;
            }
            if auto && self.smoke.is_none() {
                self.pending_import = Some(raws);
                self.pending_import_prompt = Some(format!(
                    "This drawing has {} made in another app (such as Bluebeam or Acrobat).\n\n\
                     Import them so you can edit, restyle and total them in Red-Beam?\n\n\
                     The PDF itself is not changed. You can also do this later from Save PDF \u{25BE}.",
                    describe(nm, nt)
                ));
            } else {
                self.apply_import(raws);
            }
        }
    }

    /// The user answered "No" to importing: don't ask again for this drawing.
    pub fn decline_import(&mut self) {
        self.pending_import = None;
        self.takeoff.annots_checked = true;
        self.save_takeoff();
    }

    /// The user answered "Yes".
    pub fn accept_import(&mut self) {
        if let Some(raws) = self.pending_import.take() {
            self.apply_import(raws);
        }
    }

    /// Turn the annotations into markups and measurements.
    pub(super) fn apply_import(&mut self, raws: Vec<RawAnnot>) {
        let before = self.takeoff.clone();
        let mut next = self.takeoff.next_id;
        let Imported { markups, measures, own_measures, skipped } = convert(&raws, &mut next, &self.known_sources(), now());
        self.takeoff.next_id = next;
        let (n_markups, mut n_measures) = (markups.len(), 0usize);
        self.takeoff.markups.extend(markups);
        for mut m in own_measures {
            m.group = self.takeoff.ensure_group(&m.group);
            m.layer = self.takeoff.ensure_layer(&m.layer);
            self.takeoff.next_id += 1;
            m.id = self.takeoff.next_id;
            self.takeoff.items.push(m);
            n_measures += 1;
        }
        // Scales: each sheet takes the most common scale of its imported
        // measurements (if it has none); measurements at another scale
        // get a detail-scale box around them.
        let mut per_page: std::collections::BTreeMap<u32, Vec<(u64, f64)>> = Default::default();
        let mut groups_seen: HashSet<String> = self.takeoff.groups.iter().map(|g| g.name.clone()).collect();
        for spec in measures {
            let new_group = !groups_seen.contains(&spec.group);
            let id = self.takeoff.add(spec.page, spec.kind, spec.points, &spec.group, IMPORT_LAYER);
            if new_group {
                groups_seen.insert(spec.group.clone());
                if let (Some(c), Some(g)) = (spec.color, self.takeoff.groups.iter_mut().find(|g| g.name == spec.group)) {
                    g.color = c;
                }
            }
            if let Some(m) = self.takeoff.items.iter_mut().find(|m| m.id == id) {
                m.source = Some(spec.source);
            }
            if let Some(r) = spec.ratio {
                per_page.entry(spec.page).or_default().push((id, r));
            }
            n_measures += 1;
        }
        let mut scale_notes = Vec::new();
        let mut detail_boxes = 0usize;
        for (page, list) in per_page {
            // Most common ratio (to 0.5%).
            let mut best: Option<(f64, usize)> = None;
            for (_, r) in &list {
                let n = list.iter().filter(|(_, q)| (q / r - 1.0).abs() < 0.005).count();
                if best.is_none_or(|(_, bn)| n > bn) {
                    best = Some((*r, n));
                }
            }
            let Some((common, _)) = best else { continue };
            if self.takeoff.scale(page).is_none() {
                self.takeoff.scales.insert(page, scale_for(common));
                scale_notes.push(format!("sheet {}: {}", page + 1, scale_for(common).label));
            }
            let sheet_ratio = self.takeoff.scale(page).map(|s| s.real_in_per_dip * 96.0).unwrap_or(common);
            for (id, r) in list {
                if (r / sheet_ratio - 1.0).abs() < 0.015 {
                    continue;
                }
                let Some(m) = self.takeoff.items.iter().find(|m| m.id == id) else { continue };
                let b = m.points.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |a, p| [a[0].min(p[0]), a[1].min(p[1]), a[2].max(p[0]), a[3].max(p[1])]);
                let pad = 24.0;
                self.takeoff.viewports.push(Viewport { sheet: page, rect: [b[0] - pad, b[1] - pad, b[2] + pad, b[3] + pad], scale: scale_for(r) });
                detail_boxes += 1;
            }
        }
        self.takeoff.annots_checked = true;
        if n_markups + n_measures > 0 {
            self.takeoff.hide_pdf_annots = true;
            self.undo.push(Undo::Whole(Box::new(before)));
        }
        self.save_takeoff();
        self.refresh_annot_visibility();
        let mut msg = format!("Imported {}.", describe(n_markups, n_measures));
        if n_measures > 0 {
            msg.push_str(&format!(" Measurements are on the \u{201C}{IMPORT_LAYER}\u{201D} layer, grouped by their subject."));
        }
        if !scale_notes.is_empty() {
            msg.push_str(&format!("\n\nScale set from the measurements: {}.", scale_notes.join(", ")));
        }
        if detail_boxes > 0 {
            msg.push_str(&format!("\n\n{detail_boxes} measurement(s) were made at a different scale; each got a detail-scale box (Scale \u{25BE}) so its quantity is right."));
        }
        if skipped > 0 {
            msg.push_str(&format!("\n\n{skipped} item(s) of a kind Red-Beam doesn't support were left out."));
        }
        msg.push_str("\n\nThe PDF's own copies are hidden so nothing shows twice (Save PDF \u{25BE} \u{2192} Show the PDF's original markups). The PDF itself was not changed. Ctrl+Z undoes the import.");
        self.status = None;
        self.pending_info = Some(msg);
        self.invalidate();
    }

    /// Show or hide the PDF's own annotations (menu).
    pub(super) fn toggle_pdf_annots(&mut self) {
        self.takeoff.hide_pdf_annots = !self.takeoff.hide_pdf_annots;
        self.save_takeoff();
        self.refresh_annot_visibility();
    }

    /// Tell the renderer which documents hide their own annotations, and
    /// redraw the open one.
    pub(super) fn refresh_annot_visibility(&mut self) {
        self.sync_hidden_annots();
        let id = self.doc.as_ref().map(|d| d.id);
        self.drop_doc_images(id);
        self.update_jobs(true);
        self.invalidate();
    }

    pub(super) fn sync_hidden_annots(&self) {
        let mut ids = HashSet::new();
        if let (Some(d), true) = (&self.doc, self.takeoff.hide_pdf_annots) {
            ids.insert(d.id);
        }
        for t in self.tabs.iter().flatten() {
            if let (Some(d), true) = (&t.doc, t.takeoff.hide_pdf_annots) {
                ids.insert(d.id);
            }
        }
        self.worker.set_hidden_annots(ids);
    }
}

fn describe(markups: usize, measures: usize) -> String {
    let p = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    match (markups, measures) {
        (m, 0) => p(m, "markup", "markups"),
        (0, t) => p(t, "measurement", "measurements"),
        (m, t) => format!("{} and {}", p(m, "markup", "markups"), p(t, "measurement", "measurements")),
    }
}

/// A scale for a ratio: the standard one if it matches, else "Imported".
fn scale_for(ratio: f64) -> Scale {
    match snap_ratio(ratio) {
        (r, Some(label)) => Scale::from_ratio(label, r),
        (r, None) => Scale::from_ratio(&format!("1\" = {:.2}' (imported)", r / 12.0), r),
    }
}
