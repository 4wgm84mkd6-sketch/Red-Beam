//! Session safety: remembers which drawings are open (and on which sheet)
//! so they can be reopened after a crash, a forced close or an update, and
//! tells the user about a new crash report on the next start.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::App;

/// What to ask the user at startup after an unclean exit.
pub struct Recovery {
    pub message: String,
    /// Drawings that can be reopened (empty: just inform).
    pub files: usize,
}

impl App {
    /// Paths open in tabs, the active tab first.
    fn open_paths(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.doc.iter().map(|d| d.path.clone()).collect();
        for t in self.tabs.iter().flatten() {
            if let Some(d) = &t.doc {
                if !v.iter().any(|p| crate::settings::same_path(p, &d.path)) {
                    v.push(d.path.clone());
                }
            }
        }
        v
    }

    /// Record the open tabs and the current sheet (call after opening,
    /// closing or switching tabs).
    pub(super) fn save_session(&mut self) {
        self.settings.session = self.open_paths();
        if let Some(d) = &self.doc {
            let (p, c) = (d.path.clone(), d.current);
            self.settings.set_recent_page(&p, c);
            crate::crash::set_context(format!("{} (sheet {})", p.display(), c + 1));
        } else {
            crate::crash::set_context(String::new());
        }
        self.settings.save();
        self.settings_saved = Instant::now();
    }

    /// True the first time only: should we offer to install?
    pub fn first_install_offer(&mut self) -> bool {
        if self.settings.install_asked {
            return false;
        }
        self.settings.install_asked = true;
        self.settings.save();
        true
    }

    /// Save the open tabs now (before handing over to another copy).
    pub fn save_session_now(&mut self) {
        self.save_session();
    }

    /// The sheet changed: remember it (saved at most every few seconds).
    pub(super) fn note_page(&mut self) {
        if let Some(d) = &self.doc {
            let (p, c) = (d.path.clone(), d.current);
            self.settings.set_recent_page(&p, c);
            crate::crash::set_context(format!("{} (sheet {})", p.display(), c + 1));
        }
        if self.settings_saved.elapsed() > Duration::from_secs(4) {
            self.settings.save();
            self.settings_saved = Instant::now();
        }
    }

    /// Mark this run as in progress, and say whether the last one ended
    /// badly (call once, after the window is shown).
    pub fn begin_session(&mut self) -> Option<Recovery> {
        let crashed = !self.settings.clean_exit;
        let report = crate::crash::last_report_time();
        let new_report = report > self.settings.crash_seen;
        self.settings.clean_exit = false;
        self.settings.crash_seen = report;
        self.settings.save();
        if !crashed && !new_report {
            return None;
        }
        let files = self.settings.session.iter().filter(|p| p.exists()).count();
        let mut message = String::from("Red-Beam didn't close normally last time.\n\nTakeoffs and markups are saved after every change, so your work is safe.");
        if new_report {
            message.push_str(&format!(
                "\n\nA crash report was saved here:\n{}\nPlease send it so the cause can be fixed.",
                crate::crash::log_path().display()
            ));
        }
        if files > 0 {
            message.push_str(&format!("\n\nReopen the {} drawing{} you had open?", files, if files == 1 { "" } else { "s" }));
        }
        Some(Recovery { message, files })
    }

    /// Keep marking the run as in progress (after a GPU rebuild).
    pub fn mark_running(&mut self) {
        self.settings.clean_exit = false;
        self.settings.save();
    }

    /// Reopen every drawing of the last session, each in its own tab and
    /// on its last sheet. The first one opens now, the rest one by one.
    pub fn restore_session(&mut self) {
        let mut files: Vec<PathBuf> = self.settings.session.iter().filter(|p| p.exists()).cloned().collect();
        if files.is_empty() {
            return;
        }
        let first = files.remove(0);
        self.restoring = !files.is_empty();
        self.open_queue = files.into();
        self.open(first);
    }

    /// An open finished (or failed): start the next queued one.
    pub(super) fn open_next_queued(&mut self) {
        if self.opening.is_some() {
            return;
        }
        if let Some(next) = self.open_queue.pop_front() {
            self.open(next);
        } else if self.restoring {
            // Back to the tab that was active, which was opened first.
            self.restoring = false;
            self.switch_tab(0);
        }
    }
}
