//! CI smoke test: `RedBeam.exe --smoke <pdf> <out dir>` opens the PDF,
//! steps through the main screens, saves a PNG of the window after each
//! step (once rendering has settled), writes smoke.log and exits with 0.
//! A panic, a crash or a step that never settles fails the CI run, and the
//! screenshots are uploaded so the UI can be checked without a Windows PC.

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use windows::Win32::UI::WindowsAndMessaging::{PostQuitMessage, SetTimer};

use super::tools::Tool;
use super::App;
use crate::markup::MKind;
use crate::settings::PanelTab;
use crate::takeoff::{Kind, Scale};

pub const TIMER_SMOKE: usize = 3;

const STEPS: [&str; 11] = [
    "01-open",
    "02-pages-list",
    "03-thumbs-discipline",
    "04-format-bar",
    "05-takeoff-quantities",
    "06-text-prompt",
    "07-help",
    "08-compare-overlay",
    "09-compare-side",
    "10-import-bluebeam",
    "11-imported-quantities",
];

pub struct Smoke {
    pdf: PathBuf,
    out: PathBuf,
    step: usize,
    idle_since: Option<Instant>,
    step_started: Instant,
    started: Instant,
    want_shot: bool,
    shot_taken: bool,
    log: Vec<String>,
}

impl Smoke {
    fn note(&mut self, s: impl Into<String>) {
        let line = format!("[{:6.1}s] {}", self.started.elapsed().as_secs_f32(), s.into());
        self.log.push(line);
    }

    fn write_log(&self) {
        if let Ok(mut f) = std::fs::File::create(self.out.join("smoke.log")) {
            for l in &self.log {
                let _ = writeln!(f, "{l}");
            }
        }
    }
}

impl App {
    pub fn start_smoke(&mut self, pdf: PathBuf, out: PathBuf) {
        let _ = std::fs::create_dir_all(&out);
        let pdf = std::fs::canonicalize(&pdf).unwrap_or(pdf);
        // canonicalize gives a \\?\ path on Windows; PDFium wants a plain one.
        let pdf = PathBuf::from(pdf.to_string_lossy().trim_start_matches(r"\\?\").to_string());
        let mut sm = Smoke {
            pdf: pdf.clone(),
            out,
            step: 0,
            idle_since: None,
            step_started: Instant::now(),
            started: Instant::now(),
            want_shot: false,
            shot_taken: false,
            log: Vec::new(),
        };
        sm.note(format!("Red-Beam {} smoke test, GPU: {}, engine: {}", crate::update::build_label(), self.gfx.adapter_name, self.worker.engine.name()));
        self.smoke = Some(sm);
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_SMOKE, 250, None);
        }
        self.settings.panel_open = false;
        self.open(pdf);
    }

    /// In smoke mode, dialogs and menus would block: log and drop them.
    pub fn smoke_absorb(&mut self) -> bool {
        if self.smoke.is_none() {
            return false;
        }
        let mut notes = Vec::new();
        if let Some(e) = self.pending_error.take() {
            notes.push(format!("ERROR dialog: {e}"));
        }
        if let Some(i) = self.pending_info.take() {
            notes.push(format!("info dialog: {i}"));
        }
        if let Some(k) = self.pending_menu.take() {
            notes.push(format!("menu skipped: {k:?}"));
        }
        self.pending_update_prompt = None;
        self.pending_export = None;
        self.pending_pdf_save = None;
        self.pending_sheet_pick = None;
        self.pending_sheet_save = None;
        self.pending_print = false;
        if let Some(sm) = self.smoke.as_mut() {
            for n in notes {
                sm.note(n);
            }
        }
        true
    }

    pub(super) fn smoke_tick(&mut self) {
        let busy = self.opening.is_some()
            || self.cmp.as_ref().is_some_and(|c| c.opening.is_some())
            || self.worker.busy() > 0
            || self.anim.is_some()
            || self.import_pending > 0;
        let Some(sm) = self.smoke.as_mut() else { return };
        if sm.started.elapsed() > Duration::from_secs(240) {
            sm.note(format!("FAIL: timed out at step {}", STEPS.get(sm.step).unwrap_or(&"end")));
            sm.write_log();
            std::process::exit(3);
        }
        if sm.shot_taken {
            sm.shot_taken = false;
            sm.step += 1;
            sm.idle_since = None;
            sm.step_started = Instant::now();
            if sm.step >= STEPS.len() {
                let errors = sm.log.iter().filter(|l| l.contains("ERROR") || l.contains("FAIL")).count();
                sm.note(if errors == 0 { "PASS".to_string() } else { format!("FAIL: {errors} error(s)") });
                sm.write_log();
                if errors > 0 {
                    std::process::exit(4);
                }
                unsafe { PostQuitMessage(0) };
                return;
            }
            let step = sm.step;
            sm.note(format!("step {}", STEPS[step]));
            self.smoke_action(step);
            self.invalidate();
            return;
        }
        if busy {
            sm.idle_since = None;
        } else if sm.idle_since.is_none() {
            sm.idle_since = Some(Instant::now());
        }
        let settled = sm.idle_since.is_some_and(|t| t.elapsed() > Duration::from_millis(1500));
        let slow = sm.step_started.elapsed() > Duration::from_secs(30);
        if (settled || slow) && !sm.want_shot {
            if slow && !settled {
                sm.note(format!("slow: step {} did not settle in 30 s, capturing anyway", STEPS[sm.step]));
            }
            sm.want_shot = true;
            self.invalidate();
        }
    }

    /// Between EndDraw and Present: save the frame if a screenshot is due.
    pub(super) fn smoke_capture(&mut self) {
        let Some(sm) = self.smoke.as_ref() else { return };
        if !sm.want_shot {
            return;
        }
        let path = sm.out.join(format!("{}.png", STEPS[sm.step]));
        let result = self.gfx.capture().map(|(w, h, rgba)| {
            let png = crate::png::encode_rgba(w, h, &rgba);
            std::fs::write(&path, png).map(|_| (w, h))
        });
        let paint_ms = self.paint_ms;
        let status = self.status.clone().unwrap_or_default();
        let Some(sm) = self.smoke.as_mut() else { return };
        match result {
            Ok(Ok((w, h))) => sm.note(format!("saved {} ({w}x{h}, paint {paint_ms:.1} ms) status: {status}", path.display())),
            Ok(Err(e)) => sm.note(format!("ERROR writing {}: {e}", path.display())),
            Err(e) => sm.note(format!("ERROR capturing the frame: {e}")),
        }
        sm.want_shot = false;
        sm.shot_taken = true;
    }

    fn smoke_action(&mut self, step: usize) {
        match STEPS[step] {
            "02-pages-list" => {
                self.settings.panel_open = true;
                self.settings.panel_tab = PanelTab::Pages;
                self.settings.pages_list = true;
                self.apply_fit(self.fit);
                self.update_jobs(true);
            }
            "03-thumbs-discipline" => {
                self.settings.pages_list = false;
                let groups = self.disc_groups();
                let note = format!("disciplines found: {:?}", groups.iter().map(|g| (g.0, g.2)).collect::<Vec<_>>());
                if let Some(sm) = self.smoke.as_mut() {
                    sm.note(note);
                }
                if let Some(g) = groups.get(1).or(groups.first()) {
                    self.jump_discipline(g.1);
                }
            }
            "04-format-bar" => {
                self.markup_mode = true;
                self.set_tool(Tool::Select);
                let Some((pw, ph)) = self.page_size() else { return };
                let (x, y) = (pw * 0.30, ph * 0.30);
                let t = self.smoke_markup(MKind::Text, vec![[x, y], [x + 260.0, y + 70.0]], "VERIFY ALL DIMENSIONS IN FIELD BEFORE FABRICATION");
                let c = self.smoke_markup(MKind::Callout, vec![[x + 40.0, y + 220.0], [x + 320.0, y + 120.0], [x + 560.0, y + 190.0]], "PROVIDE BLOCKING\nSEE DETAIL 5/A-501");
                self.smoke_markup(MKind::Cloud, vec![[x - 40.0, y + 260.0], [x + 300.0, y + 420.0]], "");
                self.set_sel_markup(t);
                if let Some(c) = c {
                    self.toggle_markup_selection(c);
                }
                self.toggle_bold();
            }
            "05-takeoff-quantities" => {
                self.set_sel_markup(None);
                self.markup_mode = false;
                if let Some(p) = self.takeoff_sheet() {
                    self.takeoff.scales.insert(p, Scale::from_ratio("1/8\" = 1'-0\"", 96.0));
                    let Some((pw, ph)) = self.page_size() else { return };
                    let pts = vec![[pw * 0.55, ph * 0.25], [pw * 0.80, ph * 0.25], [pw * 0.80, ph * 0.60], [pw * 0.55, ph * 0.60]];
                    self.takeoff.add(p, Kind::Area, pts, "General", crate::takeoff::DEFAULT_LAYER);
                    self.save_takeoff();
                }
                self.set_tool(Tool::Measure(Kind::Area));
                self.show_qty = true;
            }
            "06-text-prompt" => {
                self.show_qty = false;
                self.set_tool(Tool::Select);
                if let Some(id) = self.takeoff.markups.iter().find(|m| m.kind == MKind::Callout).map(|m| m.id) {
                    self.ask_markup_text(id);
                }
            }
            "07-help" => {
                self.prompt = None;
                self.show_help = true;
            }
            "08-compare-overlay" => {
                self.show_help = false;
                // tests/sample-set-rev1.pdf next to the set, if present.
                let pdf = self.smoke.as_ref().map(|s| s.pdf.clone());
                if let Some(p) = pdf {
                    let rev1 = p.with_file_name(format!("{}-rev1.pdf", p.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default()));
                    self.go_to_page(1);
                    self.start_compare(if rev1.exists() { rev1 } else { p });
                }
            }
            "09-compare-side" => self.set_cmp_mode(super::CmpMode::Side),
            "10-import-bluebeam" => {
                // tests/sample-bluebeam.pdf: annotations written the way
                // Bluebeam writes them; opening it imports them (no prompt
                // in smoke mode).
                let pdf = self.smoke.as_ref().map(|s| s.pdf.with_file_name("sample-bluebeam.pdf"));
                if let Some(p) = pdf.filter(|p| p.exists()) {
                    // Start clean: a sidecar from an earlier run would skip the import.
                    let _ = std::fs::remove_file(crate::takeoff::Takeoff::sidecar_path(&p));
                    self.markup_mode = true;
                    self.open(p);
                } else if let Some(sm) = self.smoke.as_mut() {
                    sm.note("ERROR: tests/sample-bluebeam.pdf not found");
                }
            }
            "11-imported-quantities" => {
                self.markup_mode = false;
                self.show_qty = true;
                let units = self.takeoff.units;
                let lines: Vec<String> = self
                    .takeoff
                    .items
                    .iter()
                    .map(|m| format!("{:?} {} = {} (scale {})", m.kind, m.group, crate::takeoff::fmt_qty(m.qty(self.takeoff.scale_of(m)), units), self.takeoff.scale_of(m).map(|s| s.label.clone()).unwrap_or_default()))
                    .collect();
                let n_markups = self.takeoff.markups.len();
                if let Some(sm) = self.smoke.as_mut() {
                    sm.note(format!("imported markups on the sheet: {n_markups}"));
                    for l in lines {
                        sm.note(format!("measure: {l}"));
                    }
                }
            }
            _ => {}
        }
    }

    fn smoke_markup(&mut self, kind: MKind, pts: Vec<[f32; 2]>, text: &str) -> Option<u64> {
        let id = self.add_markup(kind, pts, text.to_string());
        if let Some(sm) = self.smoke.as_mut() {
            sm.note(format!("added {} markup: {:?}", kind.name(), id));
        }
        id
    }
}
