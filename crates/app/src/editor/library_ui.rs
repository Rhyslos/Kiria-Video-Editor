//! The Library page, as the window runs it: handing the page what it shows
//! (the project, the library index's answer, proxies, the readers of
//! thumbnails and waveforms) and doing what was asked there (edits to
//! labels, lists and notes, adding a file to the timeline, proxies, import).
//! Its player follows its clock on every page, stops when the page is left,
//! and the timeline's playback stops when the player starts.

use super::Editor;
use crate::library_page::{About, Picked, PictureLook, Readers, placing};
use crate::settings::LibraryMarks;
use project::{Document, MediaRef};
use ui_kit::{Action, Page};

impl Editor {
    /// Draws the Library page and does what was asked on it.
    pub(super) fn library_page_ui(&mut self, ui: &mut egui::Ui) {
        let Some(document) = &self.document else { return };
        self.library_page.player.ensure_viewer(&self.viewer);
        let proxies = &self.proxies;
        let status = |media| proxies.status(media);
        let new_files = self.media_folders.new_files(document.project());
        let importer = &self.importer;
        let checking = |path: &std::path::Path| importer.is_reading(path);
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
            new_files: crate::new_files::NewFiles {
                files: &new_files,
                checking: &checking,
            },
            waves: self.settings.wave_settings(),
        };
        let (jobs, waveforms, pictures, ctx) = (
            &self.jobs,
            &mut self.waveforms,
            &mut self.pictures,
            ui.ctx().clone(),
        );
        let mut read_pictures = |m: &MediaRef, stream| {
            // The card's one picture first, then the strip it skims.
            let cover = (m.kind == project::MediaKind::Video)
                .then(|| pictures.cover(jobs, &ctx, m, stream))
                .flatten();
            let (strip, state) = pictures.get(jobs, &ctx, m, stream);
            PictureLook { strip, state, cover }
        };
        let mut read_waveform = |m: &MediaRef, stream| waveforms.get(jobs, &ctx, m, stream);
        let mut readers = Readers {
            pictures: &mut read_pictures,
            waveform: &mut read_waveform,
        };
        let picked = self.library_page.ui(ui, &self.theme, &about, &mut readers);
        for asked in picked {
            self.library_picked(&ctx, asked);
        }
    }

    fn library_picked(&mut self, ctx: &egui::Context, asked: Picked) {
        match asked {
            Picked::Edit(command) => {
                if let Err(err) = self.with_document(|d| d.apply(*command)) {
                    self.errors.push(&err);
                }
            }
            Picked::Everywhere(change) => match self.global_labels.change(&self.data_dir, change) {
                Ok(line) => self.status = line,
                Err(err) => self.errors.push(&err),
            },
            Picked::Add(adding) => {
                let at = self.edit_point();
                let targets = self.targets();
                let mut says = String::new();
                let result = self.with_document(|d: &mut Document| {
                    let command = placing::command(d.project(), adding.clone(), at, targets)?;
                    if let Some(media) = d.project().media.get(&adding.media) {
                        says = placing::says(media, &adding);
                    }
                    d.apply(command)
                });
                match result {
                    Ok(()) => {
                        self.status = says;
                        self.refit();
                    }
                    Err(err) => self.errors.push(&err),
                }
            }
            Picked::AddSeveral(addings, at) => {
                let edit_point = self.edit_point();
                let targets = self.targets();
                let count = addings.len();
                let result = self.with_document(|d: &mut Document| {
                    // An automatic timeline takes what most of them share.
                    if let Some(settle) = placing::settle_first(d.project(), &addings) {
                        d.apply(settle)?;
                    }
                    let command = placing::command_many(d.project(), addings, at, edit_point, targets)?;
                    d.apply(command)
                });
                match result {
                    Ok(()) => {
                        let undo = self.shortcuts.keys_or_name(Action::Undo);
                        self.status = format!(
                            "Added {count} files {}, one after another ({undo} takes them all out)",
                            match at {
                                placing::AddAt::End => "at the end of the timeline",
                                placing::AddAt::EditPoint => "at the edit point",
                            }
                        );
                        self.refit();
                    }
                    Err(err) => self.errors.push(&err),
                }
            }
            Picked::BuildProxy(media) => {
                self.proxies.build_now(media);
                let name = self
                    .document
                    .as_ref()
                    .and_then(|d| d.project().media.get(&media))
                    .map(|m| library::file_name(&m.path))
                    .unwrap_or_default();
                self.status = format!("Building the proxy of {name} next: its card shows how far it is");
            }
            Picked::BuildProxies(media) => {
                for id in &media {
                    self.proxies.build_now(*id);
                }
                self.status = format!(
                    "Building {} proxies: their cards show how far each is",
                    media.len()
                );
            }
            Picked::DeleteProxy(media) => self.proxies.delete(&self.jobs, media),
            Picked::Remove(media) => self.remove_from_project(media),
            Picked::Import => self.run_action(Action::Import),
            Picked::AddFound(paths) => self.importer.add(&self.jobs, ctx, paths),
            Picked::Dismiss(path) => self.media_folders.dismiss(path),
            Picked::Prune => self.run_action(Action::Prune),
        }
    }

    /// Settings › Library changed where marks are kept. To the project: the
    /// marks made this session are written into it at once, as one step.
    /// To the session: the page keeps the project's marks for the session,
    /// and the project keeps them too (nothing is removed).
    pub(super) fn library_marks_moved(&mut self) {
        let saved = self.settings.library_marks == LibraryMarks::Project;
        let Some(document) = &self.document else { return };
        let write = saved
            .then(|| self.library_page.uses.write_all(document.project()))
            .flatten();
        if let Some(command) = write {
            match self.with_document(|d| d.apply(command)) {
                Ok(()) => self.status = "This session's marks are saved in the project".into(),
                Err(err) => self.errors.push(&err),
            }
        }
        if let Some(document) = &self.document {
            self.library_page.uses.follow(saved, document.project());
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
        // Marks and levels changed on the page, when they are kept in the
        // project; then the page follows the project (undo shows at once).
        for command in self.library_page.uses.take_outbox() {
            if let Err(err) = self.with_document(|d| d.apply(command)) {
                self.errors.push(&err);
            }
        }
        if let Some(document) = &self.document {
            let saved = self.settings.library_marks == LibraryMarks::Project;
            self.library_page.uses.follow(saved, document.project());
        }
        if let Some(err) = self.library_page.poll(ctx, self.settings.preview_quality) {
            self.errors.push(&err);
        }
    }
}
