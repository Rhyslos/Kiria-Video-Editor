//! The Edit page as the window lays it out: the toolbar, the viewer (with
//! the split view and the inspector beside it), the timeline and its
//! details, the media drawer, and the full-screen viewer with its scrub bar.

use super::*;

/// Room between the viewer and the inspector beside it.
const INSPECTOR_GAP: f32 = 12.0;

/// The height of the full-screen viewer's scrub bar row.
const FULL_SCREEN_SCRUB: f32 = 30.0;

/// Where the timeline's picture cuts from one clip to another, in seconds:
/// every start and end of a clip on a video track, once each, in order.
fn picture_cuts(seq: &project::Sequence) -> Vec<f64> {
    let spans = seq
        .video_tracks
        .iter()
        .flat_map(|track| track.clips.iter())
        .map(|clip| (clip.start, clip.duration));
    cut_times(spans)
}

/// The starts and ends of `spans` (start, length), once each, in order.
fn cut_times(spans: impl Iterator<Item = (Time, Time)>) -> Vec<f64> {
    let mut cuts: Vec<Time> = spans
        .flat_map(|(start, length)| [start, start + length])
        .collect();
    cuts.sort();
    cuts.dedup();
    cuts.into_iter().map(|t| t.as_secs_f64()).collect()
}

impl Editor {
    /// The viewer and its transport, filling the window, with a scrub bar
    /// for the whole timeline along the bottom.
    pub(super) fn full_screen_viewer(&mut self, ui: &mut egui::Ui) {
        let Some(document) = &self.document else { return };
        let p = document.project();
        let height = ui.available_height() - 28.0 - FULL_SCREEN_SCRUB;
        // Full screen is for watching: no handles on the picture. A change
        // still being made when it was entered stays on show.
        let being_changed = self.effect_previewed(p, self.inspector.shown(p));
        let display_state = self.display_state(true);
        // The quality is the setting, as on every page: the settings keeper
        // writes a pick made here.
        let command = self.viewer.ui(
            ui,
            &self.theme,
            being_changed.as_ref().unwrap_or(p),
            &mut self.settings.preview_quality,
            None,
            height,
            &self.shortcuts,
            display_state,
            crate::viewer::Stage::default(),
            |_, _, _| {},
        );
        let seek = p.active().and_then(|seq| self.full_screen_scrub(ui, seq));
        if let Some(command) = command {
            self.run_action(command);
        }
        if let Some(seconds) = seek {
            self.seek(seconds);
        }
    }

    /// Puts `kind` on `clip` (one undo step) and keeps it among the effects
    /// used last; says why not when the clip can't have it.
    pub(super) fn add_effect(&mut self, kind: project::EffectKind, clip: project::ClipId) {
        let result = self.with_document(|d| {
            let sequence = d.project().active_sequence;
            d.apply(Command::AddEffect { sequence, clip, kind })
        });
        match result {
            Ok(()) => {
                self.settings.recent_effects =
                    crate::effects_drawer::used(&self.settings.recent_effects, &kind);
                self.selected = vec![clip];
                self.status = format!("{} added: tune it in the inspector's Effects", kind.name());
            }
            Err(err) => self.errors.push(&err),
        }
    }

    /// Puts a transition of `kind` into `clip` from the clip before it, at
    /// the usual length (one undo step), and selects the clip so the
    /// inspector shows it.
    pub(super) fn add_transition(&mut self, kind: project::TransitionKind, clip: project::ClipId) {
        let result = self.with_document(|d| {
            let sequence = d.project().active_sequence;
            d.apply(Command::SetTransition {
                sequence,
                clip,
                transition: Some(project::Transition::new(kind)),
            })
        });
        match result {
            Ok(()) => {
                self.selected = vec![clip];
                self.status = format!("{} added: change its length in the inspector", kind.name());
            }
            Err(err) => self.errors.push(&err),
        }
    }

    /// `being_changed` (or the project as it is) with the effect the
    /// pointer rests on in the drawer added to the selected picture, to be
    /// seen before it is added. No undo step.
    fn effect_previewed(
        &self,
        p: &project::Project,
        being_changed: Option<project::Project>,
    ) -> Option<project::Project> {
        let kind = self.drawer.effects.previewing?;
        let clip = crate::inspector::effects_target(p, &self.selected)?;
        let base = being_changed.as_ref().unwrap_or(p);
        let sequence = base.active_sequence;
        Command::AddEffect { sequence, clip, kind }
            .apply(base)
            .ok()
            .or(being_changed)
    }

    /// The whole timeline as a scrub bar: the playhead, the region, a tick
    /// at each cut of the picture; a click or drag moves the playhead.
    fn full_screen_scrub(&self, ui: &mut egui::Ui, seq: &project::Sequence) -> Option<f64> {
        let length = seq.length().as_secs_f64();
        let (row, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), FULL_SCREEN_SCRUB),
            egui::Sense::hover(),
        );
        let bar = egui::Rect::from_center_size(
            row.center(),
            egui::vec2((row.width() - 48.0).max(1.0), ui_kit::scrub::BAR_HEIGHT),
        );
        let marks = ui_kit::scrub::Marks {
            mark_in: seq.region.map(|r| r.start.as_secs_f64()),
            mark_out: seq.region.map(|r| r.end.as_secs_f64()),
        };
        let cuts = picture_cuts(seq);
        ui_kit::scrub::cuts(ui, &self.theme, bar, length, &cuts);
        let at = self.viewer.playhead.as_secs_f64();
        let look = ui_kit::scrub::Look::Bar;
        ui_kit::scrub::scrub(ui, &self.theme, "full screen", bar, length, at, marks, look)
    }

    /// The tab on the left edge, beside the toolbar and the viewer (`tab`),
    /// and the media drawer it opens, floating over the page (`page`) so
    /// nothing underneath moves. A click on the page away from it closes it;
    /// it steps aside while a file dragged out of it is over the timeline,
    /// so the whole timeline can take the drop.
    pub(super) fn media_drawer(&mut self, ui: &mut egui::Ui, page: egui::Rect, tab: egui::Rect) {
        let mut edge = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(tab)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        if ui_kit::drawer::edge_tab(&mut edge, &self.theme, "Media", self.drawer.open).clicked() {
            self.drawer.open = !self.drawer.open;
        }
        let ctx = ui.ctx().clone();
        if self.drawer.open && ui_kit::drawer::clicked_away(&ctx, tab) {
            self.drawer.open = false;
        }
        let rect = ui_kit::drawer::drawer_rect(page);
        let dragging_out = egui::DragAndDrop::has_payload_of_type::<ui_kit::DraggedMedia>(&ctx)
            || egui::DragAndDrop::has_payload_of_type::<ui_kit::DraggedEffect>(&ctx)
            || egui::DragAndDrop::has_payload_of_type::<ui_kit::DraggedTransition>(&ctx)
            || egui::DragAndDrop::has_payload_of_type::<ui_kit::DraggedTitle>(&ctx);
        let keep = ui_kit::drawer::keep_while_dragging(rect, tab);
        let named = self.document.as_ref().and_then(|d| {
            self.drawer
                .dialog(&ctx, &self.theme, d.project(), &self.global_labels.labels)
        });
        let shown = ui_kit::drawer::shown(self.drawer.open, dragging_out, ctx.pointer_latest_pos(), keep);
        if !shown && named.is_none() {
            return;
        }
        let mut picked = named;
        let answer = self.library.answer(crate::media_drawer::ASKER);
        let found = answer.as_ref().map(|a| a.media.as_slice());
        let new_files = match &self.document {
            Some(document) => self.media_folders.new_files(document.project()),
            None => Default::default(),
        };
        let importer = &self.importer;
        let new_files = crate::new_files::NewFiles {
            files: &new_files,
            checking: &|path| importer.is_reading(path),
        };
        let picture_selected = self
            .document
            .as_ref()
            .is_some_and(|d| crate::inspector::effects_target(d.project(), &self.selected).is_some());
        let recent_effects = &self.settings.recent_effects;
        self.drawer.effects.tiles = self.settings.effect_tiles;
        let (drawer, theme, missing) = (&mut self.drawer, &self.theme, &self.relink.missing);
        let (proxies, global_labels) = (&self.proxies, &self.global_labels.labels);
        let proxy = crate::media_drawer::ProxyInfo {
            status: &|media| proxies.status(media),
            mode: self.settings.proxies,
        };
        if let Some(document) = &self.document
            && shown
        {
            picked = ui_kit::drawer::overlay(&ctx, theme, egui::Id::unique("media-drawer"), rect, |ui| {
                let about = crate::media_drawer::About {
                    found,
                    missing,
                    proxy,
                    global: global_labels,
                    new_files,
                    picture_selected,
                    recent_effects,
                };
                drawer.ui(ui, theme, document.project(), &about)
            })
            .inner
            .or(picked);
        }
        use crate::media_drawer::Picked;
        let picked = match picked {
            Some(Picked::Place(media, placement)) => Some((media, placement)),
            Some(Picked::AddEffect(kind)) => {
                let target = self
                    .document
                    .as_ref()
                    .and_then(|d| crate::inspector::effects_target(d.project(), &self.selected));
                match target {
                    Some(clip) => self.add_effect(kind, clip),
                    None => {
                        self.status =
                            "Select a picture to add the effect to, or drag it onto a clip".to_owned()
                    }
                }
                None
            }
            Some(Picked::EffectTiles(tiles)) => {
                self.settings.effect_tiles = tiles;
                None
            }
            Some(Picked::AddTitle(style)) => {
                let at = self.viewer.playhead;
                if let Err(err) = self.add_title(style, at, None) {
                    self.errors.push(&err);
                }
                None
            }
            Some(Picked::AddTransition(kind)) => {
                let target = self
                    .document
                    .as_ref()
                    .and_then(|d| crate::inspector::effects_target(d.project(), &self.selected));
                match target {
                    Some(clip) => self.add_transition(kind, clip),
                    None => {
                        self.status =
                            "Select the clip the transition goes into, or drag it onto a cut".to_owned()
                    }
                }
                None
            }
            Some(Picked::BuildProxy(media)) => {
                self.proxies.build_now(media);
                None
            }
            Some(Picked::DeleteProxy(media)) => {
                self.proxies.delete(&self.jobs, media);
                None
            }
            Some(Picked::AddFound(paths)) => {
                self.importer.add(&self.jobs, &ctx, paths);
                None
            }
            Some(Picked::Run(action)) => {
                self.run_action(action);
                None
            }
            Some(Picked::Dismiss(path)) => {
                self.media_folders.dismiss(path);
                None
            }
            Some(Picked::Edit(command)) => {
                if let Err(err) = self.with_document(|d| d.apply(*command)) {
                    self.errors.push(&err);
                }
                None
            }
            Some(Picked::Everywhere(change)) => {
                match self.global_labels.change(&self.data_dir, change) {
                    Ok(line) => self.status = line,
                    Err(err) => self.errors.push(&err),
                }
                None
            }
            None => None,
        };
        if let Some((media, placement)) = picked {
            use crate::media_drawer::Placement;
            let at = self.edit_point();
            let placed = match placement {
                // As a drop on the first video track does it.
                Placement::Overwrite => self.lay_media(media, at, None),
                Placement::End | Placement::Insert => {
                    let result = self.with_document(|d| {
                        let sequence = d.project().active_sequence;
                        d.apply(match placement {
                            Placement::End => Command::PlaceMedia { sequence, media },
                            _ => Command::InsertMedia { sequence, media, at },
                        })
                    });
                    if let Err(err) = &result {
                        self.errors.push(err);
                    }
                    result.is_ok()
                }
            };
            if placed {
                self.refit();
            }
        }
    }

    /// Lays a file of the project over the timeline at `at`, its picture on
    /// the video track `track` (the first when `None`): what a drop on a
    /// track and the drawer's "Overwrite at the edit point" both do. Its
    /// sound finds free audio tracks, and the status line says where it
    /// went when that is not the usual place. Returns whether it was done.
    pub(super) fn lay_media(
        &mut self,
        media: project::MediaId,
        at: Time,
        track: Option<project::TrackId>,
    ) -> bool {
        let mut says = None;
        let result = self.with_document(|d| {
            let Some(before) = d.project().active().cloned() else {
                return Ok(());
            };
            d.apply(Command::LayMedia {
                sequence: before.id,
                media,
                at,
                track,
            })?;
            says = d
                .project()
                .active()
                .map(|after| timeline_panel::laid_says(&before, after, media, at));
            Ok(())
        });
        match result {
            Ok(()) => {
                self.status = says.unwrap_or_default();
                true
            }
            Err(err) => {
                self.errors.push(&err);
                false
            }
        }
    }

    /// The right-hand panel of the Edit page: the selected clip's settings.
    /// A finished change is applied as one undo step.
    fn inspector_panel(&mut self, ui: &mut egui::Ui) {
        if !crate::inspector::shows_a_clip(self.document.as_ref().map(|d| d.project()), &self.selected) {
            // Scrolls when the panel is short (a tall timeline under it).
            egui::ScrollArea::vertical()
                .id_salt("timeline details")
                .auto_shrink([false, true])
                .show(ui, |ui| self.timeline_details(ui));
        }
        self.inspector.on_picture = self.settings.picture_handles;
        // A clip's level line being dragged on the timeline shows here as it moves.
        self.inspector.volume_from_timeline = self
            .timeline
            .level_being_set()
            .map(|(clip, db)| (project::ClipId(clip), timeline_panel::gain_mdb(db)));
        self.inspector.on_picture_tip = format!(
            "Show the clip's outline in the viewer, to move it, resize it by its corners, crop it by its sides and turn it by the round handle.{}",
            match self.shortcuts.keys_for(Action::PictureHandles) {
                keys if keys.is_empty() => String::new(),
                keys => format!(" ({keys})"),
            }
        );
        let Some(document) = &self.document else { return };
        let playhead = self.viewer.playhead;
        // A fixed id: its place in the panel is kept whatever comes before
        // it, so a click in it (an arrow beside a diamond) never scrolls it
        // back to the top.
        // A scroll bar with room of its own, beside the rows instead of over
        // their right end, where the keyframe arrows are (a click there went
        // to the bar and scrolled the panel to the top).
        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
        let commands = egui::ScrollArea::vertical()
            .id_salt("inspector")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.inspector
                    .ui(ui, &self.theme, document.project(), &self.selected, playhead)
            })
            .inner;
        // An arrow beside a keyframe diamond moves the playhead.
        if let Some(at) = self.inspector.seek.take() {
            self.go_to(at);
        }
        if let Some((clip, effect)) = self.inspector.follow.take()
            && let Err(err) = self.start_follow(clip, effect)
        {
            self.errors.push(&err);
        }
        if let Some((clip, effect)) = self.inspector.steady.take()
            && let Err(err) = self.start_steady(clip, effect)
        {
            self.errors.push(&err);
        }
        if self.inspector.on_picture != self.settings.picture_handles {
            self.run_action(Action::PictureHandles);
        }
        for command in commands {
            let speed = matches!(
                command,
                Command::SetSpeed { .. } | Command::SetRamp { .. } | Command::SetSound { .. }
            );
            if let Err(err) = self.with_document(|d| d.apply(command)) {
                self.errors.push(&err);
            }
            if speed {
                // The sound follows the clip's new length or sound.
                self.play_on_with_new_mix();
            }
        }
    }

    /// The inspector with no clip selected: what the timeline is, and its
    /// settings a click away.
    fn timeline_details(&mut self, ui: &mut egui::Ui) {
        let Some(seq) = self.document.as_ref().and_then(|d| d.project().active()) else {
            return;
        };
        let rows = crate::inspector::timeline_rows(seq);
        let summary = rows.first().map(|(_, value)| value.clone()).unwrap_or_default();
        ui.add(
            egui::Label::new(
                egui::RichText::new(&seq.name)
                    .family(ui_kit::fonts::semibold())
                    .size(15.0),
            )
            .truncate(),
        );
        ui.label(
            egui::RichText::new("The timeline")
                .color(self.theme.muted)
                .size(12.0),
        );
        ui.add_space(6.0);
        let theme = self.theme;
        let mut open_settings = false;
        ui_kit::inspector::section(ui, &theme, "Timeline", &summary, false, |ui| {
            for (label, value) in &rows {
                ui.horizontal(|ui| {
                    ui_kit::inspector::label_cell(ui, &theme, label, 86.0);
                    ui.label(egui::RichText::new(value).color(theme.text));
                });
            }
            ui.add_space(4.0);
            open_settings = ui.button("Timeline settings…").clicked();
        });
        if open_settings {
            self.show_timeline_settings = true;
        }
        ui_kit::inspector::note(
            ui,
            &theme,
            "Select a clip to place, crop and colour its picture, or change its speed.",
        );
    }

    /// The Edit page: the toolbar and the viewer (with the inspector beside
    /// it) next to the media drawer's tab, and the timeline under them.
    pub(super) fn edit_page(&mut self, ui: &mut egui::Ui) {
        // The toolbar and the viewer sit beside the media drawer's tab; the
        // timeline below runs from the page's left edge to its right.
        let page = ui.available_rect_before_wrap();
        let mut viewer_command = None;
        let mut picture_change = None;
        let mut master = ui_kit::master::MasterResponse::default();
        let mut timeline_events = Vec::new();
        let mut toolbar_click = None;
        let mut lines_off = false;
        let mut tabs_events = Vec::new();
        let shortcut = self.shortcuts.keys_for(Action::Import);
        let beside = egui::UiBuilder::new()
            .max_rect(ui_kit::drawer::beside_tab(page))
            .layout(egui::Layout::top_down(egui::Align::Min));
        ui.scope_builder(beside, |ui| ui.horizontal(|ui| {
            let button = ui_kit::accent_button(ui, &self.theme, "Import media…").on_hover_text(&shortcut);
            if button.clicked() {
                self.run_action(Action::Import);
            }
            if ui.button("Timeline settings…").clicked() {
                self.show_timeline_settings = true;
            }
            if ui
                .button("Templates…")
                .on_hover_text(self.shortcuts.tip(Action::Templates))
                .clicked()
            {
                self.run_action(Action::Templates);
            }
            if let Some(document) = &self.document
                && !self.relink.missing.is_empty()
            {
                ui.add_space(16.0);
                ui_kit::status_dot(ui, ui_kit::status::WARNING);
                let text = crate::relink::summary(document.project(), &self.relink.missing);
                ui.label(egui::RichText::new(text).color(self.theme.text_secondary));
                match self.relink.searching() {
                    Some(words) => {
                        ui.spinner();
                        ui.label(egui::RichText::new(words).color(self.theme.muted));
                    }
                    None => {
                        if ui
                            .button("Find them…")
                            .on_hover_text("Pick a folder: the files are found in it or its subfolders, even if renamed.")
                            .clicked()
                        {
                            self.relink.choose_folder(ui.ctx());
                        }
                    }
                }
            }
            let busy = self.importer.busy();
            if busy > 0 {
                ui.spinner();
                ui.label(
                    egui::RichText::new(format!(
                        "Checking {busy} file{}…",
                        if busy == 1 { "" } else { "s" }
                    ))
                    .color(self.theme.text_secondary),
                );
            }
        }));
        ui.add_space(12.0);
        // The inspector sits beside the viewer, as high as it is; the
        // timeline below runs the whole width of the page.
        let shared_height = ui.available_height();
        let (least_viewer, least_timeline) = (ui_kit::splitter::MIN_VIEWER, ui_kit::splitter::MIN_TIMELINE);
        let viewer_height =
            self.settings
                .viewer_split
                .top_height(shared_height, least_viewer, least_timeline);
        let top = ui_kit::drawer::beside_tab(ui.available_rect_before_wrap());
        let top = egui::Rect::from_min_size(top.min, egui::vec2(top.width(), viewer_height));
        let inspector_rect = egui::Rect::from_min_max(
            egui::pos2(top.right() - layout::RIGHT_PANEL_WIDTH, top.top()),
            top.right_bottom(),
        );
        let viewer_rect = egui::Rect::from_min_max(
            top.min,
            egui::pos2(inspector_rect.left() - INSPECTOR_GAP, top.bottom()),
        );
        if self.document.is_some() {
            let mut panel = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(inspector_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            // Nothing in it is drawn past its edge, over the timeline below,
            // however tall the timeline is made.
            panel.set_clip_rect(inspector_rect.intersect(ui.clip_rect()));
            egui::Frame::new()
                .fill(self.theme.panel)
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(&mut panel, |ui| {
                    // Everything stays inside the panel: what doesn't fit
                    // scrolls, and nothing is drawn past its edge.
                    let inner = inspector_rect.shrink2(egui::vec2(12.0, 10.0));
                    ui.set_min_size(inner.size());
                    ui.set_max_size(inner.size());
                    ui.set_clip_rect(ui.clip_rect().intersect(inner));
                    self.inspector_panel(ui);
                });
        }
        self.media_drawer(
            ui,
            page,
            ui_kit::drawer::tab_rect(page, top.bottom() - page.top()),
        );
        let Some(document) = &self.document else { return };
        let p = document.project();
        let Some(seq) = p.active() else { return };
        // A removed track's height is forgotten.
        self.timeline.heights.keep_only(&timeline_panel::track_ids(seq));
        if seq.length() == project::Time::ZERO {
            let mut left = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(viewer_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            left.add_space(40.0);
            left.vertical_centered(|ui| {
                ui.label(egui::RichText::new("Nothing here yet").family(ui_kit::fonts::semibold()).size(18.0));
                ui.label(
                    egui::RichText::new(format!(
                        "Drop recordings on the window, or {} to import them. Each file goes at the end of the timeline, with every sound stream on its own track.",
                        if shortcut.is_empty() {
                            "click Import media".to_owned()
                        } else {
                            format!("press {shortcut}")
                        }
                    ))
                    .color(self.theme.muted),
                );
            });
            // The page's top part is taken, so the tabs (and the status
            // line) come under it, clear of the drawer's tab: an empty
            // timeline still shows the others, to go back to.
            ui.allocate_rect(top, egui::Sense::hover());
            ui.add_space(8.0);
            let new_tip = self.shortcuts.tip(Action::NewTimeline);
            let shown_tabs = self.tabs_cache.tabs(p);
            ui.horizontal(|ui| {
                tabs_events = self.tabs.show(ui, &self.theme, &shown_tabs, &new_tip);
            });
        } else {
            let quality_before = self.settings.preview_quality;
            let needle = (!self.settings.edit_at_playhead).then_some(seq.edit_needle);
            // A value being dragged in the inspector, the picture being
            // dragged in the viewer, or keyframe marks being dragged on the
            // timeline, are seen at once.
            let being_changed = self.effect_previewed(p, self.inspector.shown(p));
            let mut being_changed =
                timeline_panel::keyframes_moved(&self.timeline, being_changed.as_ref().unwrap_or(p))
                    .or(being_changed);
            // While a side of the picture is dragged, the whole picture is
            // shown, the handles veiling what is cut off.
            if let Some(clip) = self.picture_handles.cropping() {
                let whole = crate::picture_handles::uncropped(being_changed.as_ref().unwrap_or(p), clip);
                being_changed = whole.or(being_changed);
            }
            // A clip being slipped: the viewer shows its first frame as it
            // would be (and the Source its file there), live; the playhead
            // goes back when the drag ends.
            match timeline_panel::slipped(&self.timeline, being_changed.as_ref().unwrap_or(p)) {
                Some((moved, clip)) => {
                    let start = moved
                        .active()
                        .and_then(|s| s.tracks().flat_map(|t| t.clips.iter()).find(|c| c.id == clip))
                        .map(|c| c.start);
                    if let Some(start) = start {
                        self.slip_playhead.get_or_insert(self.viewer.playhead);
                        self.viewer.playhead = start;
                    }
                    being_changed = Some(moved);
                }
                None => {
                    if let Some(back) = self.slip_playhead.take() {
                        self.viewer.playhead = back;
                    }
                }
            }
            let playhead = self.viewer.playhead;
            let snapping = self.timeline.snapping;
            let on_picture = self.settings.picture_handles;
            let display_state = self.display_state(false);
            let (handles, inspector) = (&mut self.picture_handles, &mut self.inspector);
            let (theme, selected) = (&self.theme, &self.selected);
            let mut left = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(viewer_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            // Split view: the selected clip raw, beside the timeline's picture.
            let (split, away) = (self.display.split(), self.display.away());
            let display = &mut self.display;
            let shown = being_changed.as_ref().unwrap_or(p);
            let mut source = |ui: &mut egui::Ui, size: egui::Vec2| {
                display.source_ui(ui, theme, size, shown, selected, playhead, quality_before);
            };
            let stage = crate::viewer::Stage {
                source: split.then_some(&mut source as &mut crate::viewer::DrawSource),
                away,
            };
            viewer_command = self.viewer.ui(
                &mut left,
                theme,
                being_changed.as_ref().unwrap_or(p),
                &mut self.settings.preview_quality,
                needle,
                viewer_height,
                &self.shortcuts,
                display_state,
                stage,
                // The selected clip's picture can be taken hold of.
                // Only when handles on the picture are switched on.
                |ui, frame, response| {
                    if on_picture {
                        picture_change = handles.ui(
                            ui, theme, frame, response, p, selected, playhead, inspector, snapping,
                        );
                    }
                },
            );
            // The viewer and the inspector take their height of the page;
            // the handle and the timeline follow, across its whole width.
            ui.allocate_rect(top, egui::Sense::hover());
            // The quality and the handle's place are settings: the
            // settings keeper writes them once the pointer is let go.
            ui_kit::splitter::splitter(
                ui,
                &self.theme,
                &mut self.settings.viewer_split,
                shared_height,
                least_viewer,
                least_timeline,
            );
            let new_tip = self.shortcuts.tip(Action::NewTimeline);
            let shown_tabs = self.tabs_cache.tabs(p);
            ui.horizontal(|ui| {
                toolbar_click = ui_kit::timeline::toolbar(ui, &self.theme, &self.timeline, |click| {
                    self.shortcuts.tip(toolbar_action(click))
                });
                if self.lines_on {
                    ui.add_space(8.0);
                    if ui_kit::timeline::lines_chip(
                        ui,
                        &self.theme,
                        &self.shortcuts.keys_or_name(Action::VolumeLines),
                    ) {
                        lines_off = true;
                    }
                }
                ui.add_space(16.0);
                tabs_events = self.tabs.show(ui, &self.theme, &shown_tabs, &new_tip);
            });
            timeline_panel::keep_existing(&mut self.selected, seq);
            let (jobs, waveforms, pictures, ctx) = (
                &self.jobs,
                &mut self.waveforms,
                &mut self.pictures,
                ui.ctx().clone(),
            );
            let missing = &self.relink.missing;
            let mut drawn = timeline_panel::drawn(
                p,
                seq,
                &self.selected,
                missing,
                |m, stream| waveforms.get(jobs, &ctx, m, stream),
                |m, stream| pictures.get(jobs, &ctx, m, stream),
            );
            // Sound being set in the inspector shows on its clips at once.
            timeline_panel::show_sounds(&mut drawn, &self.inspector.sounds_being_set());
            timeline_panel::show_chosen_label(&mut drawn, self.selected_label);
            crate::audio_page::lines::fill(&mut drawn.audio, seq, self.lines.kind);
            self.timeline.lines = self.lines_on.then_some(self.lines);
            self.timeline.rough_cut = self.settings.rough_cut;
            self.timeline.compact = self.settings.compact_tracks;
            self.timeline.divider = !self.settings.one_track_list;
            self.timeline.click_moves = self.settings.click_moves;
            self.timeline.zoom_to_playhead = self.settings.zoom_to_playhead;
            self.timeline.volume_control = self.settings.volume_control();
            self.timeline.detailed_on_selected = self.settings.detailed_gain_selected;
            if self.settings.rough_cut {
                timeline_panel::fold_sound(&mut drawn);
            }
            let now = Instant::now();
            self.track_paces.time(&mut drawn.video, now);
            self.track_paces.time(&mut drawn.audio, now);
            let length = seq.length().as_secs_f64();
            let width = ui.available_width();
            if !self.fitted {
                self.timeline.fit(length, width - self.timeline.header_width());
                self.fitted = true;
            }
            let roles = crate::audio_page::roles::offered();
            let data = ui_kit::TimelineData {
                highlights: &self.silence.found,
                ..timeline_panel::data(
                    seq,
                    &drawn,
                    self.viewer.playhead,
                    needle,
                    self.playback.is_some(),
                    &roles,
                )
            };
            let status_height = if self.status.is_empty() { 0.0 } else { 24.0 };
            let height = (ui.available_height() - status_height).max(160.0);
            // The selected audio track's column: the track of the selected
            // sound clip, else the one it was last for, else the first.
            let picked = seq
                .audio_tracks
                .iter()
                .find(|t| t.clips.iter().any(|c| self.selected.contains(&c.id)))
                .or_else(|| {
                    seq.audio_tracks
                        .iter()
                        .find(|t| Some(t.id) == self.track_column_for)
                })
                .or_else(|| seq.audio_tracks.front());
            self.track_column_for = picked.map(|t| t.id);
            let column = picked.map(|t| {
                let index = seq.audio_tracks.iter().position(|o| o.id == t.id).unwrap_or(0);
                let levels = self.playback.as_ref().and_then(|p| {
                    p.track_meters()
                        .into_iter()
                        .find(|(id, _)| *id == t.id)
                        .map(|(_, meter)| {
                            let l = meter.take();
                            (l.peak_db, l.short_term)
                        })
                });
                (t.id, t.name.clone(), index, t.gain_db, t.muted, t.solo, levels)
            });
            let columns = if column.is_some() { 2.0 } else { 1.0 };
            let size = egui::vec2(width - (layout::MASTER_WIDTH + 8.0) * columns, height);
            let levels = self.playback.as_ref().map(|p| {
                let l = p.levels();
                ui_kit::master::MeterLevels {
                    peak_db: l.peak_db,
                    true_peak_db: l.true_peak_db,
                    true_peak_max: l.true_peak_max,
                    momentary: l.momentary,
                    short_term: l.short_term,
                    integrated: l.integrated,
                }
            });
            let gain = seq.master_gain_db;
            ui.horizontal_top(|ui| {
                timeline_events = self.timeline.show(ui, &self.theme, &data, size);
                ui.add_space(8.0);
                if let Some((id, name, index, gain_db, muted, solo, levels)) = column {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(layout::MASTER_WIDTH, height), egui::Sense::hover());
                    let strip = ui_kit::track_column::TrackStrip {
                        id: id.0,
                        name: &name,
                        colour: ui_kit::timeline::audio_colour(index),
                        gain_db,
                        muted,
                        solo,
                        levels,
                    };
                    let asked = self.track_column.show(ui, &self.theme, rect, strip);
                    let (muted, solo) = asked.mute_solo.unwrap_or((muted, solo));
                    if asked.gain_db.is_some() || asked.mute_solo.is_some() {
                        timeline_events.push(ui_kit::TimelineEvent::TrackMix {
                            track: id.0,
                            muted,
                            solo,
                            gain_db: asked.gain_db.unwrap_or(gain_db),
                        });
                    }
                    ui.add_space(8.0);
                }
                let (rect, _) = ui.allocate_exact_size(egui::vec2(layout::MASTER_WIDTH, height), egui::Sense::hover());
                let mix = ui_kit::master::Mix {
                    levels,
                    unheard: self.playback.as_ref().is_some_and(|p| p.unheard()),
                    gain_db: gain,
                    target: self.settings.loudness_target,
                    tip: "The whole mix: drag to change its volume, double-click for 0 dB. The lines on the channels are their true peaks; the third lane is the loudness over 3 s, with the loudness aimed for across it (right-click to choose). M and S are the loudness over 0.4 s and 3 s, I since playback started, TP the highest true peak.",
                };
                master = self.master.show(ui, &self.theme, rect, mix);
            });
        }
        self.master_asks(master);
        for event in timeline_events {
            self.timeline_event(event);
        }
        for event in tabs_events {
            self.tabs_event(event);
        }
        if lines_off {
            self.run_action(Action::VolumeLines);
        }
        if let Some(click) = toolbar_click {
            self.run_action(toolbar_action(click));
        }
        if let Some(command) = viewer_command {
            self.run_action(command);
        }
        // The picture was let go of in the viewer: one undo step.
        if let Some(command) = picture_change
            && let Err(err) = self.with_document(|d| d.apply(command))
        {
            self.errors.push(&err);
        }
        if !self.status.is_empty() {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(&self.status)
                    .color(self.theme.text_secondary.gamma_multiply(self.status_strength)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_full_screen_scrub_bar_marks_each_cut_once() {
        let s = Time::seconds;
        // Two clips end to end on one track, a third over the join on another.
        let spans = [(s(0), s(4)), (s(4), s(6)), (s(3), s(2))];
        assert_eq!(cut_times(spans.into_iter()), [0.0, 3.0, 4.0, 5.0, 10.0]);
    }
}
