//! The Audio page as the window runs it: the shared viewer top left, the
//! page's tools top right, the clarity check across the middle, and the
//! bottom (the shared timeline, the mixer, or both) under a bar with the
//! view toggle, the Show filter, the timeline's tools and the lines tool.
//! Every frame it takes the mixer's meters once, counts for the loudness
//! overview, tells playback which track the analyser listens to, and
//! keeps the page's timeline in step with the Edit page's when either is
//! left.

use super::Editor;
use crate::audio_page::{self, Bottom, Live, SplitWay, lines, roles};
use crate::timeline_panel;
use project::{Command, Time};
use std::time::Instant;
use ui_kit::{Action, Page, layout};

/// The tools' panel's width beside the viewer.
pub const TOOLS_WIDTH: f32 = layout::TOOLS_WIDTH;
/// The clarity check's band across the middle.
const CLARITY_HEIGHT: f32 = 112.0;
/// The bar over the bottom views.
const BAR_HEIGHT: f32 = 30.0;
/// Room between the page's parts.
const GAP: f32 = 10.0;
/// How many timeline tracks show above the mixer when Split is stacked.
const STACKED_TRACKS: f32 = 3.0;

impl Editor {
    /// Every frame, on every page: what the Audio page follows.
    pub(super) fn follow_audio_page(&mut self) {
        if self.page != self.shown_page {
            // The page's timeline looks where the Edit page's did, and back.
            let (from, to) = match (self.shown_page, self.page) {
                (_, Page::Audio) => (&self.timeline, &mut self.audio_page.timeline),
                (Page::Audio, _) => (&self.audio_page.timeline, &mut self.timeline),
                _ => return self.follow_audio_learning(),
            };
            to.zoom = from.zoom;
            to.scroll = from.scroll;
            to.tool = from.tool;
            to.snapping = from.snapping;
        }
        self.follow_audio_learning();
    }

    /// A noise learned on a worker becomes one undo step.
    fn follow_audio_learning(&mut self) {
        let Some(document) = &self.document else { return };
        match self.audio_page.take_learned(document.project()) {
            Some(Ok(command)) => match self.with_document(|d| d.apply(command)) {
                Ok(()) => self.status = "The noise is learned and clean-up is on".into(),
                Err(err) => self.errors.push(&err),
            },
            Some(Err(err)) => self.errors.push(&err),
            None => {}
        }
    }

    /// What the mixer measured this frame, taken once (peaks are held
    /// until taken). Also counts for the overview and chooses the track
    /// the analyser listens to.
    fn audio_live(&mut self) -> Live {
        let Some(document) = &self.document else {
            return Live::default();
        };
        let p = document.project();
        let seq = p.active();
        let track = seq
            .and_then(|s| audio_page::chosen(s, self.audio_page.track))
            .map(|t| t.id);
        let mut live = Live::default();
        if let Some(playback) = &mut self.playback {
            live.playing = true;
            let wants = (self.audio_page.tool == audio_page::Tool::Eq)
                .then_some(track)
                .flatten();
            playback.analyse(wants);
            live.tracks = playback
                .track_meters()
                .into_iter()
                .map(|(id, meter)| (id, meter.take()))
                .collect();
            if let Some(readout) = playback.readout() {
                live.master = Some(readout.levels());
                live.master_reduction = Some((readout.compressor_db(), readout.limiter_db()));
            }
            live.spectrum = playback
                .spectrum()
                .filter(|_| wants.is_some())
                .map(|s| s.levels());
        }
        if live.master.is_some() {
            self.audio_held_master = live.master;
        } else {
            live.master = self.audio_held_master;
        }
        let peak = |l: [f32; 2]| l[0].max(l[1]);
        let frame = audio_page::counting::Frame {
            counting: self.settings.loudness_counting,
            playing: live.playing,
            at: self.viewer.playhead,
            region: seq.and_then(|s| s.region),
            track,
            mix: live
                .master
                .filter(|_| live.playing)
                .map(|m| audio_page::counting::Reading {
                    momentary: m.momentary,
                    short_term: m.short_term,
                    peak_db: peak(m.true_peak_db),
                }),
            track_reading: track
                .and_then(|t| live.tracks.get(&t))
                .map(|l| audio_page::counting::Reading {
                    momentary: l.momentary,
                    short_term: l.short_term,
                    peak_db: peak(l.peak_db),
                }),
        };
        self.audio_page.counted.follow(frame);
        live
    }

    /// Draws the Audio page and does what was asked on it.
    pub(super) fn audio_page_ui(&mut self, ui: &mut egui::Ui) {
        let live = self.audio_live();
        let page = ui.available_rect_before_wrap();
        // A copy (cheap: the project's parts are shared), so the page can be
        // drawn while the window's other parts change.
        let Some(p) = self.document.as_ref().map(|d| d.project().clone()) else {
            return;
        };
        let p = &p;
        let Some(seq) = p.active() else { return };
        // The project as it would be with what is being dragged.
        let shown = self.audio_page.shown(p);
        let shown_p = shown.as_ref().unwrap_or(p);
        let Some(shown_seq) = shown_p.active() else { return };
        let top_h = (page.height() * 0.36).clamp(230.0, 420.0);
        let top = egui::Rect::from_min_size(page.min, egui::vec2(page.width(), top_h));
        let tools = egui::Rect::from_min_max(egui::pos2(top.right() - TOOLS_WIDTH, top.top()), top.max);
        let viewer = egui::Rect::from_min_max(top.min, egui::pos2(tools.left() - GAP, top.bottom()));

        // Top left: the viewer every page shares, with its transport.
        let needle = (!self.settings.edit_at_playhead).then_some(seq.edit_needle);
        let display_state = self.display_state(false);
        let away = self.display.away();
        let mut left = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(viewer)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        let viewer_command = self.viewer.ui(
            &mut left,
            &self.theme,
            p,
            &mut self.settings.preview_quality,
            needle,
            top_h,
            &self.shortcuts,
            display_state,
            crate::viewer::Stage { source: None, away },
            |_, _, _| {},
        );

        // Top right: the track's tools.
        let learn_from = learn_from(seq, &self.selected, self.audio_page.track);
        let mut panel = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(tools)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        let mut asks = egui::Frame::new()
            .fill(self.theme.panel)
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(&mut panel, |ui| {
                ui.set_min_size(tools.size() - egui::vec2(24.0, 20.0));
                self.audio_page.header(ui, &self.theme, shown_seq);
                ui.add_space(8.0);
                self.audio_page.tool_ui(
                    ui,
                    &self.theme,
                    shown_seq,
                    &live,
                    learn_from,
                    self.settings.loudness_counting,
                    self.settings.loudness_target,
                )
            })
            .inner;

        // The clarity check: followed every frame, its check on a worker.
        if let Some(err) = self.audio_page.clarity.follow(
            &self.jobs,
            p,
            self.playback.is_some(),
            self.viewer.playhead,
            Instant::now(),
        ) {
            self.errors.push(&err);
        }
        let clarity = egui::Rect::from_min_size(
            egui::pos2(page.left(), top.bottom() + GAP),
            egui::vec2(page.width(), CLARITY_HEIGHT),
        );
        let clarity_asks =
            audio_page::clarity_draw::band(ui, &self.theme, clarity, &mut self.audio_page.clarity, seq);

        // The bar over the bottom views.
        let bar = egui::Rect::from_min_size(
            egui::pos2(page.left(), clarity.bottom() + GAP),
            egui::vec2(page.width(), BAR_HEIGHT),
        );
        self.audio_bar(ui, bar, shown_seq, &live);
        let bottom = egui::Rect::from_min_max(egui::pos2(page.left(), bar.bottom() + 6.0), page.max);
        let (timeline_rect, mixer_rect) = match self.settings.audio_bottom {
            Bottom::Timeline => (Some(bottom), None),
            Bottom::Mixer => (None, Some(bottom)),
            Bottom::Split => match self.settings.audio_split {
                SplitWay::SideBySide => {
                    let strips = ((bottom.width() * 0.42).max(ui_kit::mixer::STRIP_WIDTH * 3.0))
                        .min(bottom.width() - 400.0);
                    let mixer = egui::Rect::from_min_size(
                        bottom.min,
                        egui::vec2(
                            strips + layout::MASTER_WIDTH + audio_page::MASTER_SETTINGS + 8.0,
                            bottom.height(),
                        ),
                    );
                    let timeline =
                        egui::Rect::from_min_max(egui::pos2(mixer.right() + GAP, bottom.top()), bottom.max);
                    (Some(timeline), Some(mixer))
                }
                SplitWay::Stacked => {
                    let tracks_h =
                        26.0 + 18.0 + 30.0 + STACKED_TRACKS * ui_kit::timeline::AUDIO_TRACK * 0.7 + 14.0;
                    let timeline = egui::Rect::from_min_size(
                        bottom.min,
                        egui::vec2(bottom.width(), tracks_h.min(bottom.height() * 0.55)),
                    );
                    let mixer = egui::Rect::from_min_max(
                        egui::pos2(bottom.left(), timeline.bottom() + GAP),
                        bottom.max,
                    );
                    (Some(timeline), Some(mixer))
                }
            },
        };
        let mut events = Vec::new();
        if let Some(r) = timeline_rect {
            events = self.audio_timeline(ui, r, p, shown_seq);
        }
        let mut mixer_asks = audio_page::MixerAsks::default();
        if let Some(r) = mixer_rect {
            let tip = "The whole mix: drag to change its volume, double-click for 0 dB. Right-click for the loudness to aim for.";
            mixer_asks = self.audio_page.mixer_ui(
                ui,
                &self.theme,
                r,
                shown_seq,
                &live,
                self.settings.loudness_target,
                tip,
            );
        }
        ui.allocate_rect(page, egui::Sense::hover());

        // What was asked, now that drawing is done.
        let mut commands = asks.commands;
        commands.extend(mixer_asks.commands);
        for command in commands {
            if let Err(err) = self.with_document(|d| d.apply(command)) {
                self.errors.push(&err);
            }
        }
        if let Some((track, span)) = asks.learn.take()
            && let Some(document) = &self.document
        {
            self.audio_page.learn(&self.jobs, document.project(), track, span);
            self.status = "Learning the noise…".into();
        }
        // Settings changed here are written by the settings keeper.
        if let Some(counting) = asks.counting {
            self.settings.loudness_counting = counting;
        }
        if let Some(target) = asks.target.or(mixer_asks.master.target) {
            self.settings.loudness_target = target;
        }
        if asks.reset {
            self.audio_page.counted.reset();
            if let Some(playback) = &self.playback {
                playback.reset_loudness();
            }
        }
        if mixer_asks.master.reset
            && let Some(playback) = &self.playback
        {
            playback.reset_loudness();
        }
        if let Some(gain_db) = mixer_asks.master.gain_db {
            let result = self.with_document(|d| {
                let sequence = d.project().active_sequence;
                d.apply(Command::SetMasterGain { sequence, gain_db })
            });
            if let Err(err) = result {
                self.errors.push(&err);
            }
        }
        for event in events {
            self.timeline_event(event);
        }
        self.clarity_asks(clarity_asks);
        if let Some(command) = viewer_command {
            self.run_action(command);
        }
    }

    /// What the clarity check's band asked: a fix (one undo step, then a
    /// check again at once), the region or the playhead, going to a finding.
    fn clarity_asks(&mut self, asks: audio_page::clarity_draw::BandAsks) {
        if let Some(scope) = asks.scope {
            self.audio_page.clarity.scope = scope;
        }
        if let Some(at) = asks.seek {
            self.timeline_event(ui_kit::TimelineEvent::Seek(at.as_secs_f64()));
        }
        if let Some(fix) = asks.fix {
            let result = self.with_document(|d| {
                let command = fix.apply(d.project())?;
                d.apply(command)
            });
            match result {
                Ok(()) => {
                    self.status = format!("{}: done. Undo takes it back.", fix.label);
                    self.audio_page.clarity.again();
                }
                Err(err) => self.errors.push(&err),
            }
        }
    }

    /// The bar over the bottom views. The settings it changes are written
    /// by the settings keeper.
    fn audio_bar(&mut self, ui: &mut egui::Ui, bar: egui::Rect, seq: &project::Sequence, live: &Live) {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(bar)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let ui = &mut child;
        for view in Bottom::ALL {
            if ui
                .selectable_label(self.settings.audio_bottom == view, view.label())
                .clicked()
            {
                self.settings.audio_bottom = view;
            }
        }
        if self.settings.audio_bottom == Bottom::Split {
            ui.add_space(4.0);
            for (way, text, tip) in [
                (
                    SplitWay::SideBySide,
                    "◧",
                    "Side by side: the mixer on the left, the timeline on the right",
                ),
                (
                    SplitWay::Stacked,
                    "⬒",
                    "Stacked: a few timeline tracks above the mixer",
                ),
            ] {
                if ui
                    .selectable_label(self.settings.audio_split == way, text)
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.settings.audio_split = way;
                }
            }
        }
        ui.add_space(10.0);
        ui.label(egui::RichText::new("Show").color(self.theme.text_secondary));
        egui::ComboBox::from_id_salt("audio-show")
            .selected_text(shown_count(seq, self.audio_page.show))
            .width(130.0)
            .show_ui(ui, |ui| {
                for show in roles::Show::all() {
                    if ui
                        .selectable_label(self.audio_page.show == show, shown_count(seq, show))
                        .clicked()
                    {
                        self.audio_page.show = show;
                    }
                }
            });
        ui.add_space(10.0);
        // The lines tool, the same on every page (its key in the shortcut table).
        let lines_tip = self.shortcuts.tip(Action::VolumeLines);
        if ui
            .selectable_label(self.lines_on, "Lines")
            .on_hover_text(lines_tip)
            .clicked()
        {
            self.run_action(Action::VolumeLines);
        }
        let which = |l: ui_kit::LinesShown| match (l.kind, l.on_clips) {
            (ui_kit::LineKind::Volume, false) => "Volume · tracks",
            (ui_kit::LineKind::Pan, false) => "Pan · tracks",
            (ui_kit::LineKind::Volume, true) => "Volume · clips",
            (ui_kit::LineKind::Pan, true) => "Pan · clips",
        };
        egui::ComboBox::from_id_salt("audio-lines")
            .selected_text(which(self.lines))
            .width(110.0)
            .show_ui(ui, |ui| {
                for kind in [ui_kit::LineKind::Volume, ui_kit::LineKind::Pan] {
                    for on_clips in [false, true] {
                        let choice = ui_kit::LinesShown { kind, on_clips };
                        if ui.selectable_label(self.lines == choice, which(choice)).clicked() {
                            self.lines = choice;
                            self.lines_on = true;
                        }
                    }
                }
            })
            .response
            .on_hover_text("A track's line runs across the whole track; a clip's own line stays with the clip when it moves.");

        // What is left: the timeline's tools, then the right-hand items, by
        // their real widths (see `audio_page::bar`).
        const GAP: f32 = 10.0;
        let spacing = ui.spacing().item_spacing.x;
        let padding = ui.spacing().button_padding.x;
        let text_w = |ui: &egui::Ui, text: egui::RichText| {
            egui::WidgetText::from(text)
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Extend),
                    f32::INFINITY,
                    egui::TextStyle::Body,
                )
                .size()
                .x
        };
        let rate = seq.settings.frame_rate;
        let integrated = live.master.map_or(f32::NEG_INFINITY, |m| m.integrated);
        let master = egui::RichText::new(format!("{} LUFS", ui_kit::master::db_text(integrated)))
            .monospace()
            .color(crate::audio_page::overview_colour(
                &self.theme,
                integrated,
                self.settings.loudness_target,
            ));
        let master_name = egui::RichText::new("Master").color(self.theme.muted);
        let playhead = egui::RichText::new(self.viewer.playhead.timecode(rate)).monospace();
        let needle = (!self.settings.edit_at_playhead).then(|| {
            egui::RichText::new(format!("Edit {}", seq.edit_needle.timecode(rate)))
                .monospace()
                .color(ui_kit::status::EDIT_NEEDLE)
        });
        // Most important first: the mix, the playhead, the needle.
        let mut right = vec![
            text_w(ui, master.clone()) + text_w(ui, master_name.clone()) + 2.0 * spacing + GAP,
            text_w(ui, playhead.clone()) + spacing,
        ];
        if let Some(needle) = &needle {
            right.push(text_w(ui, needle.clone()) + spacing);
        }
        let row = ui_kit::timeline::TOOLBAR_WIDTH + spacing;
        let tool = self.audio_page.timeline.tool;
        let folded_text = format!("{} ▾", tool_name(tool));
        let folded = text_w(ui, egui::RichText::new(folded_text.as_str())) + 2.0 * padding + spacing;
        let plan = audio_page::bar::plan(ui.available_width() - GAP, row, folded, &right, GAP);
        ui.add_space(GAP);
        let click = if plan.tools_in_a_row {
            // The timeline's tools, the same as on the Edit page.
            let tip = |a: Action| self.shortcuts.tip(a);
            let tips = [
                tip(Action::SelectTool),
                tip(Action::BladeTool),
                tip(Action::RippleTool),
                tip(Action::RollTool),
                tip(Action::SlipTool),
                tip(Action::SlideTool),
                tip(Action::ToggleSnapping),
                tip(Action::ZoomToFit),
                tip(Action::RoughCut),
            ];
            ui_kit::timeline::toolbar(
                ui,
                &self.theme,
                &self.audio_page.timeline,
                [
                    &tips[0], &tips[1], &tips[2], &tips[3], &tips[4], &tips[5], &tips[6], &tips[7], &tips[8],
                ],
            )
        } else {
            folded_tools(ui, &folded_text, &self.audio_page.timeline)
        };
        if let Some(click) = click {
            use ui_kit::timeline::ToolbarClick as C;
            self.run_action(match click {
                C::Tool(ui_kit::Tool::Select) => Action::SelectTool,
                C::Tool(ui_kit::Tool::Blade) => Action::BladeTool,
                C::Tool(ui_kit::Tool::Ripple) => Action::RippleTool,
                C::Tool(ui_kit::Tool::Roll) => Action::RollTool,
                C::Tool(ui_kit::Tool::Slip) => Action::SlipTool,
                C::Tool(ui_kit::Tool::Slide) => Action::SlideTool,
                C::Snapping => Action::ToggleSnapping,
                C::Fit => Action::ZoomToFit,
                C::RoughCut => Action::RoughCut,
            });
        }
        // Right: as many as fit whole (the viewer shows the times too).
        if plan.right > 0 {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(master);
                ui.label(master_name);
                if plan.right > 1 {
                    ui.add_space(GAP);
                    if let (Some(needle), true) = (needle, plan.right > 2) {
                        ui.label(needle);
                    }
                    ui.label(playhead);
                }
            });
        }
    }

    /// The page's timeline: one video reference row, the audio tracks the
    /// Show filter lets through, their lines; the shared widget and tools.
    fn audio_timeline(
        &mut self,
        ui: &mut egui::Ui,
        r: egui::Rect,
        p: &project::Project,
        seq: &project::Sequence,
    ) -> Vec<ui_kit::TimelineEvent> {
        let (jobs, waveforms, pictures, ctx) = (
            &self.jobs,
            &mut self.waveforms,
            &mut self.pictures,
            ui.ctx().clone(),
        );
        let mut drawn = timeline_panel::drawn(
            p,
            seq,
            &self.selected,
            &self.relink.missing,
            |m, stream| waveforms.get(jobs, &ctx, m, stream),
            |m, stream| pictures.get(jobs, &ctx, m, stream),
        );
        timeline_panel::show_sounds(&mut drawn, self.inspector.sounds_being_set());
        lines::fill(&mut drawn.audio, seq, self.lines.kind);
        let show = self.audio_page.show;
        let shown_audio: Vec<ui_kit::TimelineTrack> = drawn
            .audio
            .into_iter()
            .zip(&seq.audio_tracks)
            .filter(|(_, t)| show.shows(t.sound.role))
            .map(|(d, _)| d)
            .collect();
        let reference = video_reference(drawn.video);
        let now = Instant::now();
        let mut audio = shown_audio;
        self.track_paces.time(&mut audio, now);
        let video = reference.into_iter().collect::<Vec<_>>();
        let view = &mut self.audio_page.timeline;
        view.compact = self.settings.compact_tracks;
        view.divider = true;
        view.click_moves = self.settings.click_moves;
        view.lines = self.lines_on.then_some(self.lines);
        view.heights.keep_only(&timeline_panel::track_ids(seq));
        // The reference row keeps to one track's height.
        view.split = 0.0;
        if !self.audio_page.fitted {
            view.fit(
                seq.length().as_secs_f64(),
                r.width() - ui_kit::timeline::HEADER_WIDTH,
            );
            self.audio_page.fitted = true;
        }
        let needle = (!self.settings.edit_at_playhead).then_some(seq.edit_needle);
        let offered = roles::offered();
        let spans = self.audio_page.clarity.spans(seq.id);
        let data = ui_kit::TimelineData {
            video: &video,
            audio: &audio,
            length: seq.length().as_secs_f64(),
            fps: seq.settings.frame_rate.as_f64(),
            playhead: self.viewer.playhead.as_secs_f64(),
            needle: needle.map(Time::as_secs_f64),
            region: seq.region.map(|r| (r.start.as_secs_f64(), r.end.as_secs_f64())),
            markers: &drawn.markers,
            highlights: &[],
            spans: &spans,
            follow: self.playback.is_some(),
            cache: ui_kit::timeline::CacheBar {
                ranges: &drawn.cache,
                about: timeline_panel::CACHE_BAR_ABOUT,
            },
            keyframe_eases: &timeline_panel::EASES,
            roles: &offered,
        };
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(r)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        let events = view.show(&mut child, &self.theme, &data, r.size());
        // The reference row is one row for every video track: a clip let go
        // on it stays on its own track.
        let reference_id = video.first().map(|t| t.id);
        events
            .into_iter()
            .map(|e| match e {
                ui_kit::TimelineEvent::Move {
                    clip,
                    by,
                    track: Some(t),
                    new_track: _,
                } if Some(t) == reference_id => ui_kit::TimelineEvent::Move {
                    clip,
                    by,
                    track: None,
                    new_track: false,
                },
                other => other,
            })
            .collect()
    }

    /// The command for a line let go on a timeline, and what follows it.
    pub(super) fn line_event(
        &mut self,
        owner: ui_kit::LineOwner,
        kind: ui_kit::LineKind,
        points: &[(f64, f32)],
    ) {
        let Some(seq) = self.document.as_ref().and_then(|d| d.project().active()) else {
            return;
        };
        let Some(command) = lines::command(seq, owner, kind, points) else {
            return;
        };
        let clip_line = matches!(command, Command::SetClipLines { .. });
        match self.with_document(|d| d.apply(command)) {
            // A clip's sound is mixed ahead: heard from here at once.
            Ok(()) if clip_line => self.play_on_with_new_mix(),
            Ok(()) => {}
            Err(err) => self.errors.push(&err),
        }
    }

    /// A track's role chosen in the timeline's names menu.
    pub(super) fn role_event(&mut self, track: u64, role: usize) {
        let Some(role) = roles::at(role) else { return };
        let result = self.with_document(|d| {
            let Some(seq) = d.project().active() else {
                return Ok(());
            };
            let Some(t) = seq.audio_tracks.iter().find(|t| t.id.0 == track) else {
                return Ok(());
            };
            let mut sound = t.sound.clone();
            sound.role = role;
            let command = Command::SetTrackSound {
                sequence: seq.id,
                track: t.id,
                sound: Box::new(sound),
            };
            d.apply(command)
        });
        if let Err(err) = result {
            self.errors.push(&err);
        }
    }
}

/// A timeline tool's name, for the folded tools.
fn tool_name(tool: ui_kit::Tool) -> &'static str {
    match tool {
        ui_kit::Tool::Select => "Select",
        ui_kit::Tool::Blade => "Blade",
        ui_kit::Tool::Ripple => "Ripple",
        ui_kit::Tool::Roll => "Roll",
        ui_kit::Tool::Slip => "Slip",
        ui_kit::Tool::Slide => "Slide",
    }
}

/// The timeline's tools folded into one dropdown, for a narrow bar.
fn folded_tools(
    ui: &mut egui::Ui,
    text: &str,
    view: &ui_kit::TimelineView,
) -> Option<ui_kit::timeline::ToolbarClick> {
    use ui_kit::timeline::ToolbarClick as C;
    let mut clicked = None;
    ui.menu_button(text, |ui| {
        for tool in [
            ui_kit::Tool::Select,
            ui_kit::Tool::Blade,
            ui_kit::Tool::Ripple,
            ui_kit::Tool::Roll,
            ui_kit::Tool::Slip,
            ui_kit::Tool::Slide,
        ] {
            if ui.selectable_label(view.tool == tool, tool_name(tool)).clicked() {
                clicked = Some(C::Tool(tool));
                ui.close();
            }
        }
        ui.separator();
        if ui.selectable_label(view.snapping, "Snapping").clicked() {
            clicked = Some(C::Snapping);
            ui.close();
        }
        if ui.button("Show the whole timeline").clicked() {
            clicked = Some(C::Fit);
            ui.close();
        }
    })
    .response
    .on_hover_text("The timeline's tools (the bar is too narrow for them in a row)");
    clicked
}

/// A Show choice with how many tracks it shows: "All tracks (30)".
fn shown_count(seq: &project::Sequence, show: roles::Show) -> String {
    let n = seq
        .audio_tracks
        .iter()
        .filter(|t| show.shows(t.sound.role))
        .count();
    format!("{} ({n})", show.label())
}

/// What the clean-up would learn from: the region, else the selected clip
/// of the track the tools are on.
fn learn_from(
    seq: &project::Sequence,
    selected: &[project::ClipId],
    track: Option<project::TrackId>,
) -> audio_page::LearnFrom {
    if let Some(region) = seq.region {
        return audio_page::LearnFrom::Region(region.start, region.end);
    }
    let track = audio_page::chosen(seq, track);
    track
        .and_then(|t| t.clips.iter().find(|c| selected.contains(&c.id)))
        .map_or(audio_page::LearnFrom::Nothing, |c| {
            audio_page::LearnFrom::Clip(c.start, c.end())
        })
}

/// Every video track's clips as one reference row, named "Video".
fn video_reference(video: Vec<ui_kit::TimelineTrack>) -> Option<ui_kit::TimelineTrack> {
    let mut tracks = video.into_iter();
    let mut first = tracks.next()?;
    for t in tracks {
        first.clips.extend(t.clips);
    }
    first.clips.sort_by(|a, b| a.start.total_cmp(&b.start));
    first.name = "Video".into();
    first.hidden = false;
    Some(first)
}

impl Editor {
    /// What the command line asked of the Audio page once the project and
    /// its imports are in: many audio tracks (`--audio-tracks`), and the
    /// track the tools start on (`--audio-track`).
    pub(super) fn audio_demo(&mut self) {
        if self.importer.busy() > 0 {
            return;
        }
        if let Some(n) = self.audio_start.tracks {
            let Some(p) = self.document.as_ref().map(|d| d.project()) else {
                return;
            };
            let has_sound = p
                .active()
                .is_some_and(|s| s.audio_tracks.iter().any(|t| !t.clips.is_empty()));
            if !has_sound {
                return; // Its imports are not in yet.
            }
            self.audio_start.tracks = None;
            self.audio_demo_made = Some(self.frames);
            if let Some(project) = audio_page::start::many_tracks(p, n) {
                if let Err(err) = self.with_document(|d| {
                    d.apply(Command::Restore {
                        project: Box::new(project),
                    })
                }) {
                    self.errors.push(&err);
                }
                // The first track's noise, learned from its first two seconds,
                // so the Clean-up tool has a profile to show.
                if let Some((track, document)) = self
                    .document
                    .as_ref()
                    .and_then(|d| Some((d.project().active()?.audio_tracks.front()?.id, d)))
                {
                    self.audio_page.learn(
                        &self.jobs,
                        document.project(),
                        track,
                        (Time::ZERO, Time::seconds(2)),
                    );
                }
                // Looked at from the top, fitted, as a project opened so.
                let mut fresh = ui_kit::TimelineView::default();
                fresh.zoom = self.audio_page.timeline.zoom;
                self.audio_page.timeline = fresh;
                self.audio_page.fitted = false;
            }
        }
        if self.audio_start.clarity_region && self.audio_start.tracks.is_none() {
            self.audio_start.clarity_region = false;
            let whole = self.document.as_ref().and_then(|d| {
                let seq = d.project().active()?;
                (seq.region.is_none() && seq.length() > Time::ZERO).then(|| Command::SetRegion {
                    sequence: seq.id,
                    region: Some(project::Region {
                        start: Time::ZERO,
                        end: seq.length(),
                    }),
                })
            });
            if let Some(command) = whole
                && let Err(err) = self.with_document(|d| d.apply(command))
            {
                self.errors.push(&err);
            }
            self.audio_page.clarity.scope = audio_page::clarity::Scope::Region;
        }
        if let Some(n) = self.audio_start.track {
            let track = self
                .document
                .as_ref()
                .and_then(|d| d.project().active())
                .and_then(|s| s.audio_tracks.get(n.saturating_sub(1)))
                .map(|t| t.id);
            if track.is_some() {
                self.audio_page.track = track;
                self.audio_start.track = None;
            }
        }
    }
}
