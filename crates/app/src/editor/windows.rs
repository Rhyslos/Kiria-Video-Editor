//! The windows over the pages, as the window runs them: each hands its
//! dialog what it shows and does what was asked.

use super::*;

impl Editor {
    /// The window for one timeline label, and what it asks for.
    pub(super) fn label_window(&mut self, ctx: &egui::Context) {
        use crate::label_editor::Asked;
        let Some(editor) = self.label_editor.as_mut() else {
            return;
        };
        let asked = editor.ui(ctx, &self.theme);
        let result = match asked {
            Asked::Nothing => return,
            Asked::Close => Ok(()),
            Asked::Save { id, name, color } => self.with_document(|d| {
                let sequence = d.project().active_sequence;
                d.apply(Command::EditLabel {
                    sequence,
                    label: id,
                    name,
                    color,
                })
            }),
            Asked::Remove(label) => self.with_document(|d| {
                let sequence = d.project().active_sequence;
                d.apply(Command::RemoveLabel { sequence, label })
            }),
        };
        match result {
            Ok(()) => self.label_editor = None,
            Err(err) => self.errors.push(&err),
        }
    }

    /// The keyboard shortcuts page, in its own window. Shortcuts changed
    /// there are in use at once and kept in settings.
    pub(super) fn shortcuts_window(&mut self, ctx: &egui::Context) {
        if let Some(page) = self.shortcuts_page.as_mut() {
            let mut open = true;
            let mut asked = ui_kit::keyboard::PageResponse::default();
            egui::Window::new("Keyboard shortcuts")
                .open(&mut open)
                .collapsible(false)
                .default_pos([48.0, 64.0])
                .default_size([1040.0, 800.0])
                .show(ctx, |ui| {
                    asked = page.show(
                        ui,
                        &self.theme,
                        &mut self.shortcuts,
                        &mut self.settings.keyboard,
                        self.system_keyboard.now(),
                    );
                });
            if !open {
                self.shortcuts_page = None;
            }
            if asked.changed {
                self.settings.shortcuts = self.shortcuts.saved();
            }
            if asked.cheat_sheet {
                self.cheat_sheet
                    .start(ctx, &self.shortcuts, self.shortcuts.layout());
            }
        }
    }

    /// The Saved versions window, and what it asks for.
    pub(super) fn versions_window(&mut self, ctx: &egui::Context) {
        use crate::versions_window::Asked;
        if !self.versions.open {
            return;
        }
        let Some(document) = &self.document else {
            self.versions.open = false;
            return;
        };
        let file = document.path().to_path_buf();
        let undo = self.shortcuts.keys_or_name(Action::Undo);
        match self
            .versions
            .ui(ctx, &self.theme, &self.jobs, document.project(), &file, &undo)
        {
            Asked::Nothing => {}
            Asked::Saved(name) => self.status = format!("Saved the version \"{name}\"."),
            Asked::Restore(name, project) => {
                match self.with_document(|d| d.apply(Command::Restore { project })) {
                    Ok(()) => {
                        self.selected.clear();
                        self.refit();
                        self.status = format!(
                            "Restored \"{name}\". {} returns to before.",
                            self.shortcuts.keys_or_name(Action::Undo)
                        );
                    }
                    Err(err) => self.errors.push(&err),
                }
            }
            Asked::Failed(err) => self.errors.push(&err),
        }
    }

    /// The Prune dialog, and the files it takes out of the project.
    pub(super) fn prune_window(&mut self, ctx: &egui::Context) {
        if !self.prune.is_open() {
            return;
        }
        let Some(document) = &self.document else {
            return;
        };
        let undo = self.shortcuts.keys_or_name(Action::Undo);
        let asked = self
            .prune
            .ui(ctx, &self.theme, document.project(), &self.relink.missing, &undo);
        if let crate::prune_window::Asked::Prune { media, says } = asked {
            let gone = media.clone();
            match self.with_document(|d| d.apply(Command::RemoveMedia { media })) {
                Ok(()) => {
                    for media in gone {
                        self.waveforms.forget(media);
                        self.pictures.forget(media);
                    }
                    self.status = says;
                }
                Err(err) => self.errors.push(&err),
            }
        }
    }

    /// Package project: the dialog, and what it says when done.
    pub(super) fn package_window(&mut self, ctx: &egui::Context) {
        let Some(document) = &self.document else {
            return;
        };
        if std::mem::take(&mut self.package_requested) {
            self.package.open(ctx);
        }
        if self.package.busy() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        use crate::package_window::Asked;
        match self
            .package
            .ui(ctx, &self.theme, &self.jobs, document.project(), document.path())
        {
            Asked::Nothing => {}
            Asked::Said(says) => self.status = says,
            Asked::Failed(err) => self.errors.push(&err),
        }
    }

    /// The Templates window, and what it asks for.
    pub(super) fn templates_window(&mut self, ctx: &egui::Context) {
        use crate::templates_window::Asked;
        if !self.templates.open {
            return;
        }
        let Some(document) = &self.document else {
            self.templates.open = false;
            return;
        };
        let dir = project::templates::folder(&self.data_dir);
        match self
            .templates
            .ui(ctx, &self.theme, &self.jobs, document.project(), &dir)
        {
            Asked::Nothing => {}
            Asked::Said(text) => self.status = text,
            Asked::Insert { name, template, spot } => {
                let edit_point = self.edit_point();
                let result = self.with_document(|d| {
                    let Some(seq) = d.project().active() else {
                        return Ok(());
                    };
                    let (sequence, at) = (seq.id, spot.time(edit_point, seq.length()));
                    d.apply(Command::InsertTemplate {
                        sequence,
                        template,
                        at,
                        name_tracks: true,
                    })
                });
                match result {
                    Ok(()) => {
                        self.refit();
                        self.status = format!(
                            "Inserted the template \"{name}\" {}. {} takes it out again.",
                            spot.said(),
                            self.shortcuts.keys_or_name(Action::Undo)
                        );
                    }
                    Err(err) => {
                        self.templates.failed(&err);
                        self.errors.push(&err);
                    }
                }
            }
            Asked::Failed(err) => self.errors.push(&err),
        }
    }

    /// The Remove silence window, and the cut it asks for.
    pub(super) fn silence_window(&mut self, ctx: &egui::Context) {
        if !self.silence.open {
            return;
        }
        let Some(document) = &self.document else {
            self.silence.open = false;
            return;
        };
        let (jobs, waveforms) = (&self.jobs, &mut self.waveforms);
        let ready = self.silence.refresh(document.project(), |m, stream| {
            waveforms.whole(jobs, ctx, m, stream)
        });
        let undo = self.shortcuts.keys_or_name(Action::Undo);
        let asked = self
            .silence
            .ui(ctx, &self.theme, document.project(), ready, &undo);
        if let crate::silence_panel::Asked::Remove(ranges) = asked {
            let count = ranges.len();
            let result = self.with_document(|d| {
                let sequence = d.project().active_sequence;
                d.apply(Command::RemoveRanges { sequence, ranges })
            });
            match result {
                Ok(()) => {
                    self.selected.clear();
                    self.refit();
                    self.status = format!(
                        "Removed {count} quiet stretch{}.",
                        if count == 1 { "" } else { "es" }
                    );
                }
                Err(err) => self.errors.push(&err),
            }
        }
    }

    /// Starts Sync by sound on the selected clips, or says why it can't.
    pub(super) fn start_sync(&mut self) {
        let Some(document) = &self.document else { return };
        if let Some(says) = self.sync.start(document.project(), &self.selected, &self.jobs) {
            self.status = says;
        }
    }

    /// The Sync by sound window, and the move it asks for.
    pub(super) fn sync_window(&mut self, ctx: &egui::Context) {
        if !self.sync.open {
            return;
        }
        let Some(document) = &self.document else {
            self.sync.open = false;
            return;
        };
        self.sync.poll(ctx);
        let undo = self.shortcuts.keys_or_name(Action::Undo);
        let asked = self
            .sync
            .ui(ctx, &self.theme, document.project(), &self.jobs, &undo);
        if let crate::sync_panel::Asked::Move(moves) = asked {
            let count = moves.len();
            let cut: Vec<String> = document
                .project()
                .active()
                .map(|seq| crate::sync_panel::cut_offs(document.project(), seq, &moves))
                .unwrap_or_default()
                .iter()
                .map(|(file, cut)| format!(" {}", crate::sync_panel::cut_off_text(file, *cut, true)))
                .collect();
            let result = self.with_document(|d| {
                let sequence = d.project().active_sequence;
                d.apply(Command::LineUp { sequence, moves })
            });
            match result {
                Ok(()) => {
                    self.status = format!(
                        "Lined up {count} clip{} by sound.{} {undo} undoes it.",
                        if count == 1 { "" } else { "s" },
                        cut.concat()
                    );
                }
                Err(err) => self.errors.push(&err),
            }
        }
    }

    /// Picture size, frame rate and sound rate of the open timeline: Automatic,
    /// a template, or the same as one of the project's videos.
    pub(super) fn timeline_settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_timeline_settings {
            return;
        }
        let Some(document) = &self.document else {
            self.show_timeline_settings = false;
            return;
        };
        let p = document.project();
        let Some(seq) = p.active() else { return };
        let current = if seq.automatic {
            "Automatic: the first video you import decides.".to_owned()
        } else {
            format!(
                "{} · {} kHz",
                import::settings_text(&seq.settings),
                seq.settings.sample_rate as f32 / 1000.0
            )
        };
        let mut choices: Vec<(String, project::SequenceSettings)> = project::SequenceSettings::TEMPLATES
            .iter()
            .map(|(name, s)| (format!("{name} · {}", import::settings_text(s)), *s))
            .collect();
        for media in p.media.values() {
            if let (project::MediaKind::Video, Some(v)) = (media.kind, &media.info.video) {
                let s = project::SequenceSettings::matching(v, &media.info.audio);
                let name = media
                    .path
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                choices.push((format!("Same as {name} · {}", import::settings_text(&s)), s));
            }
        }
        let sequence = seq.id;
        let mut chosen = None;
        let mut open = true;
        egui::Window::new("Timeline settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(current).family(ui_kit::fonts::semibold()));
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Change to").color(self.theme.text_secondary));
                for (label, settings) in &choices {
                    if ui.button(label).clicked() {
                        chosen = Some(*settings);
                    }
                }
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Clips of another size or frame rate are fitted, never changed.")
                        .size(11.5)
                        .color(self.theme.muted),
                );
            });
        if let Some(settings) = chosen {
            if let Err(err) = self.with_document(|d| d.apply(Command::SetSettings { sequence, settings })) {
                self.errors.push(&err);
            }
            self.show_timeline_settings = false;
        } else {
            self.show_timeline_settings = open;
        }
    }

    pub(super) fn search_window(&mut self, ctx: &egui::Context) {
        use crate::search::Hit;
        if !self.show_search {
            return;
        }
        let project = self.document.as_ref().map(Document::project);
        let seq = project.and_then(|p| p.active());
        let fps = seq.map_or(60.0, |s| s.settings.frame_rate.as_f64());
        // The files the index found for these words, once it has answered them.
        let files: Option<Vec<(project::MediaId, String)>> = self
            .library
            .answer(crate::search::ASKER)
            .filter(|a| a.query.text == self.search_text.trim())
            .zip(project)
            .map(|(answer, p)| {
                answer
                    .media
                    .iter()
                    .filter_map(|id| p.media.get(id))
                    .map(|m| (m.id, library::file_name(&m.path)))
                    .collect()
            });
        let open_timeline = seq.map(|s| s.id);
        let timelines: Vec<(project::SequenceId, String)> = project
            .map(|p| {
                p.sequences
                    .iter()
                    .filter(|s| Some(s.id) != open_timeline)
                    .map(|s| (s.id, s.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let hits = crate::search::results(
            &self.search_text,
            &self.shortcuts,
            seq,
            &timelines,
            |clip| {
                project
                    .and_then(|p| p.media.get(&clip.media))
                    .and_then(|m| m.path.file_name())
                    .map_or_else(|| "missing".into(), |n| n.to_string_lossy().into_owned())
            },
            files.as_deref(),
        );
        let answer = crate::search::palette(ctx, &self.theme, &mut self.search_text, &hits, fps);
        let chosen = match answer {
            None => return,
            Some(crate::search::Answer::Closed) => None,
            Some(crate::search::Answer::Chosen(hit)) => Some(hit),
        };
        self.show_search = false;
        match chosen {
            Some(Hit::Command { action, .. }) => self.run_action(action),
            Some(Hit::Label { at, .. }) => self.seek(at),
            Some(Hit::Clip { id, at, .. }) => {
                self.selected = vec![id];
                self.seek(at);
            }
            Some(Hit::File { name, .. }) => {
                self.page = Page::Edit;
                self.drawer.open = true;
                self.drawer.tab = ui_kit::drawer::DrawerTab::Media;
                self.drawer.filter = name;
            }
            Some(Hit::Settings) => self.show_settings = true,
            Some(Hit::Setting(section)) => {
                self.open_settings_at(section.id());
            }
            Some(Hit::Timeline { id, .. }) => {
                self.page = Page::Edit;
                self.open_timeline(id);
            }
            Some(Hit::TimelineSettings) => self.show_timeline_settings = true,
            Some(Hit::Shortcuts) => self.run_action(Action::KeyboardShortcuts),
            None => {}
        }
    }

    /// The viewer's own window, while it is open: declared every frame (or
    /// it closes), handed the picture, and what was done there run here.
    pub(super) fn viewer_window(&mut self, ctx: &egui::Context) {
        let news = self
            .display
            .show_window(ctx, &mut self.viewer, self.theme, &self.shortcuts);
        for action in news.actions {
            self.run_action(action);
        }
        if news.closed && self.display.window_open() {
            self.run_action(Action::ViewerWindow);
        }
    }
}
