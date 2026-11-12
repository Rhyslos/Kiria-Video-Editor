//! The Library page, as the window runs it: handing the page what it shows
//! (the project, the library index's answer, proxies, the readers of
//! thumbnails and waveforms) and doing what was asked there (edits to
//! labels, lists and notes, adding a file to the timeline, proxies, import).
//! Its player follows its clock on every page, stops when the page is left,
//! and the timeline's playback stops when the player starts.

use super::Editor;
use crate::library_page::{About, Picked, Readers, placing};
use project::{Document, MediaRef};
use ui_kit::{Action, Page};

impl Editor {
    /// Draws the Library page and does what was asked on it.
    pub(super) fn library_page_ui(&mut self, ui: &mut egui::Ui) {
        let Some(document) = &self.document else { return };
        self.library_page.player.ensure_viewer(&self.viewer);
        let proxies = &self.proxies;
        let status = |media| proxies.status(media);
        let about = About {
            project: document.project(),
            answer: self.library.answer(crate::library_page::ASKER),
            missing: &self.relink.missing,
            proxy: &status,
            proxy_mode: self.settings.proxies,
            global: &self.global_labels.labels,
            target: self.settings.loudness_target.lufs(),
            shortcuts: &self.shortcuts,
            quality: self.settings.preview_quality,
            jobs: &self.jobs,
            importing: self.importer.busy(),
            status: &self.status,
        };
        let (jobs, waveforms, pictures, ctx) = (
            &self.jobs,
            &mut self.waveforms,
            &mut self.pictures,
            ui.ctx().clone(),
        );
        let mut read_pictures = |m: &MediaRef, stream| pictures.get(jobs, &ctx, m, stream);
        let mut read_waveform = |m: &MediaRef, stream| waveforms.get(jobs, &ctx, m, stream);
        let mut readers = Readers {
            pictures: &mut read_pictures,
            waveform: &mut read_waveform,
        };
        let picked = self.library_page.ui(ui, &self.theme, &about, &mut readers);
        for asked in picked {
            self.library_picked(asked);
        }
    }

    fn library_picked(&mut self, asked: Picked) {
        match asked {
            Picked::Edit(command) => {
                if let Err(err) = self.with_document(|d| d.apply(*command)) {
                    self.errors.push(&err);
                }
            }
            Picked::Everywhere(change) => match self.global_labels.change(&self.jobs, &self.data_dir, change)
            {
                Ok(line) => self.status = line,
                Err(err) => self.errors.push(&err),
            },
            Picked::Add(adding) => {
                let at = self.edit_point();
                let mut says = String::new();
                let result = self.with_document(|d: &mut Document| {
                    let command = placing::command(d.project(), adding, at)?;
                    if let Some(media) = d.project().media.get(&adding.media) {
                        says = placing::says(&library::file_name(&media.path), &adding);
                    }
                    d.apply(command)
                });
                match result {
                    Ok(()) => {
                        self.status = says;
                        self.fitted = false;
                    }
                    Err(err) => self.errors.push(&err),
                }
            }
            Picked::BuildProxy(media) => self.proxies.build_now(media),
            Picked::DeleteProxy(media) => self.proxies.delete(&self.jobs, media),
            Picked::Import => self.run_action(Action::Import),
        }
    }

    /// Every frame: the player follows its clock (on any page), stops when
    /// the page is left, and the timeline's playback gives way to it; a
    /// `--library-tab` waiting for the project opens.
    pub(super) fn follow_library_page(&mut self, ctx: &egui::Context) {
        if self.page != self.shown_page {
            if self.shown_page == Page::Library {
                self.library_page.leave();
            }
            self.shown_page = self.page;
        }
        if self.library_page.player.playing() && self.playback.is_some() {
            self.playback = None;
            self.after_play_change();
        }
        if let Some(err) = self.library_page.poll(ctx, self.settings.preview_quality) {
            self.errors.push(&err);
        }
        if let (Some(name), Some(document)) = (&self.library_tab, &self.document) {
            if !self.library_page.open_tab_named(name, document.project()) {
                tracing::warn!("--library-tab {name}: no such tab");
            }
            self.library_tab = None;
        }
    }
}
