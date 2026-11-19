//! Every shortcut action the window runs (`run_action`), and the command a
//! button of the timeline's toolbar stands for.

use super::*;

/// The command a button of the timeline's toolbar runs (and whose
/// shortcut its tooltip names).
pub(super) fn toolbar_action(click: ui_kit::timeline::ToolbarClick) -> Action {
    use ui_kit::timeline::ToolbarClick as C;
    match click {
        C::Tool(ui_kit::Tool::Select) => Action::SelectTool,
        C::Tool(ui_kit::Tool::Blade) => Action::BladeTool,
        C::Tool(ui_kit::Tool::Ripple) => Action::RippleTool,
        C::Tool(ui_kit::Tool::Roll) => Action::RollTool,
        C::Tool(ui_kit::Tool::Slip) => Action::SlipTool,
        C::Tool(ui_kit::Tool::Slide) => Action::SlideTool,
        C::Tool(ui_kit::Tool::Text) => Action::TextTool,
        C::Snapping => Action::ToggleSnapping,
        C::Fit => Action::ZoomToFit,
        C::RoughCut => Action::RoughCut,
    }
}

impl Editor {
    pub(super) fn run_action(&mut self, action: Action) {
        // The Library page's player takes the transport's keys and marking.
        if self.page == Page::Library && self.document.is_some() && self.library_page.command(action) {
            return;
        }
        let result = match action {
            Action::Undo => self.step_history(false),
            Action::Redo => self.step_history(true),
            Action::Save => self.save_now(),
            Action::AddLabel => {
                let at = self.edit_point();
                self.add_label(at)
            }
            Action::LabelRegion => self.label_region(),
            Action::ToggleLoop => {
                self.looping = !self.looping;
                let has_region = self.region().is_some();
                self.status = match (self.looping, has_region) {
                    (true, true) => "Looping inside the region",
                    (true, false) => "Looping on: Shift-drag in the ruler to mark a region",
                    (false, _) => "Looping off",
                }
                .into();
                Ok(())
            }
            Action::ClearRegion => self.set_region(None),
            Action::RemoveRegion => self.cut_region(ui_kit::RegionCut::Remove),
            Action::LiftRegion => self.cut_region(ui_kit::RegionCut::Lift),
            Action::KeepRegion => self.cut_region(ui_kit::RegionCut::Keep),
            Action::EditAtPlayhead => {
                self.settings.edit_at_playhead = !self.settings.edit_at_playhead;
                self.status = if self.settings.edit_at_playhead {
                    "Edit at the playhead: cuts, splits and pastes happen at the playhead."
                } else {
                    "The edit needle is back: cuts, splits and pastes happen at the red needle."
                }
                .into();
                // The settings keeper writes it once it has settled.
                Ok(())
            }
            Action::MarkIn | Action::MarkOut => self.mark(action == Action::MarkIn),
            Action::RemoveSilence => self.with_document(|_| Ok(())).map(|()| {
                if let Some(d) = &self.document {
                    self.silence.open_on(d.project(), self.audio_page.track);
                }
            }),
            Action::SaveFrame => self
                .with_document(|_| Ok(()))
                .map(|()| self.save_frame_requested = true),
            Action::Split => {
                let at = self.edit_point();
                let result = self.split_at(at);
                // Nothing at the edit needle (often off screen): offer the
                // playhead, and say where the needle is and how to merge them.
                match result {
                    Err(err) if !self.settings.edit_at_playhead && self.playhead_on_frame() != at => Err(err
                        .fix(format!(
                            "Split at the playhead instead, or move the edit needle (the red line, at {}) over a clip. To always cut at the playhead, turn on Display \u{203a} Edit at the playhead{}.",
                            ui_kit::time_text::duration(at.as_secs_f64() as u64),
                            match self.shortcuts.keys_for(Action::EditAtPlayhead) {
                                keys if keys.is_empty() => String::new(),
                                keys => format!(" ({keys})"),
                            }
                        ))
                        .action(diag::FixAction::new("Split at the playhead instead", SPLIT_AT_PLAYHEAD))),
                    other => other,
                }
            }
            Action::Delete | Action::RippleDelete => {
                if self.selected.is_empty()
                    && let Some(label) = self.label_chosen()
                {
                    // A chosen timeline label: removed, whichever key.
                    self.remove_label(label);
                    Ok(())
                } else if self.selected.is_empty()
                    && let Some(gap) = self.shown_view().selected_gap()
                {
                    // A selected gap: closed on every track, whichever key.
                    self.close_gap(gap)
                } else if self.selected.is_empty() {
                    self.status = "Click a clip, or the gap between two clips, on the timeline first.".into();
                    Ok(())
                } else {
                    self.remove_selected(action == Action::RippleDelete)
                }
            }
            Action::SelectAll => {
                if let Some(seq) = self.document.as_ref().and_then(|d| d.project().active()) {
                    self.selected_label = None;
                    self.selected = timeline_panel::all_clips(seq);
                    let count = self.selected.len();
                    let delete = self.shortcuts.keys_or_name(Action::Delete);
                    self.status = match count {
                        0 => "Nothing on this timeline to select".into(),
                        n => format!("{n} clips selected: {delete} deletes them all as one step"),
                    };
                }
                Ok(())
            }
            Action::BuildProxies => self.with_document(|_| Ok(())).map(|()| {
                let count = self.proxies.build_all();
                self.status = match count {
                    0 => "Every video file of the project has its proxy, or one being built".into(),
                    1 => "Building 1 proxy in the background".into(),
                    n => format!("Building {n} proxies in the background"),
                };
            }),
            Action::BuildProxiesFullSpeed => self.with_document(|_| Ok(())).map(|()| {
                let count = self.proxies.build_all();
                self.proxies.set_full_speed(true);
                self.status = match count {
                    0 => "Every video file has its proxy; any being built now carries on, the next ones at full speed".into(),
                    n => format!(
                        "Building {n} {} at full speed, also while playing: the editor may feel slower until they are done",
                        if n == 1 { "proxy" } else { "proxies" }
                    ),
                };
            }),
            Action::ClearSelection => {
                let task = self.following.is_some() || self.steadying.is_some();
                let lines = self.lines_on && matches!(self.page, Page::Edit | Page::Audio);
                match escape_does(task, self.full_screen, lines) {
                    EscapeDoes::StopTask => self.stop_tasks(),
                    EscapeDoes::LeaveFullScreen => self.set_full_screen(false),
                    // Escape leaves the lines first, as it leaves a menu.
                    EscapeDoes::LeaveLines => self.run_action(Action::VolumeLines),
                    EscapeDoes::SelectNothing => self.selected.clear(),
                }
                Ok(())
            }
            Action::Versions => self.with_document(|_| Ok(())).map(|()| {
                self.versions.open = true;
                self.versions.show_again();
            }),
            Action::Prune => self.with_document(|_| Ok(())).map(|()| self.prune.open()),
            Action::PackageProject => self
                .with_document(|_| Ok(()))
                .map(|()| self.package_requested = true),
            Action::Templates | Action::SaveTemplate => self
                .with_document(|_| Ok(()))
                .map(|()| self.templates.show(action == Action::SaveTemplate)),
            Action::KeyboardShortcuts => {
                self.shortcuts_page.get_or_insert_with(Default::default);
                Ok(())
            }
            Action::UserGuide => {
                self.user_guide_requested = true;
                Ok(())
            }
            Action::VolumeLines => {
                self.lines_on = !self.lines_on;
                // About the timeline: a page without one drops it.
                let says = if self.lines_on { LINES_ON } else { LINES_OFF };
                self.status_about_timeline.say(&mut self.status, says);
                Ok(())
            }
            Action::RoughCut => {
                self.settings.rough_cut = !self.settings.rough_cut;
                self.timeline.rough_cut = self.settings.rough_cut;
                self.status = if self.settings.rough_cut {
                    "Rough cut view: each clip's sound is drawn inside it"
                } else {
                    "Full timeline view"
                }
                .into();
                Ok(())
            }
            Action::CompactTracks => {
                self.settings.compact_tracks = !self.settings.compact_tracks;
                self.timeline.compact = self.settings.compact_tracks;
                self.status = if self.settings.compact_tracks {
                    "Compact tracks: every track a thin strip. The heights you gave them come back when you turn it off"
                } else {
                    "Tracks at their own heights again"
                }
                .into();
                Ok(())
            }
            Action::TrackDivider => {
                self.settings.one_track_list = !self.settings.one_track_list;
                self.timeline.divider = !self.settings.one_track_list;
                self.status = if self.settings.one_track_list {
                    "No divider: video and audio tracks are one list that scrolls together"
                } else {
                    "The divider between video and audio tracks is back where it was"
                }
                .into();
                Ok(())
            }
            Action::ZoomToPlayhead => {
                self.settings.zoom_to_playhead = !self.settings.zoom_to_playhead;
                self.timeline.zoom_to_playhead = self.settings.zoom_to_playhead;
                self.status = if self.settings.zoom_to_playhead {
                    "Zoom to playhead: zooming keeps the playhead where it is, and brings it into view"
                } else {
                    "Zooming with the wheel zooms at the pointer again"
                }
                .into();
                Ok(())
            }
            Action::ClickMovesNeedle => {
                self.settings.click_moves = match self.settings.click_moves {
                    ui_kit::ClickMoves::Playhead => ui_kit::ClickMoves::EditNeedle,
                    ui_kit::ClickMoves::EditNeedle => ui_kit::ClickMoves::Playhead,
                };
                self.timeline.click_moves = self.settings.click_moves;
                self.status =
                    click_moves_says(self.settings.click_moves, self.settings.edit_at_playhead).into();
                Ok(())
            }
            // With the viewer in its own window, that window fills its screen.
            Action::FullScreenViewer if self.display.window_open() => {
                self.display.switch_window_full_screen();
                Ok(())
            }
            Action::FullScreenViewer => {
                self.set_full_screen(!self.full_screen);
                Ok(())
            }
            Action::SplitView => {
                let on = !self.display.split();
                self.display.set_split(on, &self.viewer);
                self.settings.split_view = on;
                self.status = if on {
                    "Split view: the selected clip's own picture (Source) beside the timeline's (Program)"
                } else {
                    "Split view off"
                }
                .into();
                Ok(())
            }
            Action::ViewerWindow => self.switch_viewer_window(),
            Action::ViewerZoomFit | Action::ViewerZoom100 | Action::ViewerZoomIn | Action::ViewerZoomOut => {
                self.viewer.zoom_to(ui_kit::viewer::ZoomPick::Command(action));
                Ok(())
            }
            Action::NewTimeline => self.new_timeline(false),
            Action::DuplicateTimeline => self.new_timeline(true),
            Action::NestSelection => self.nest_selection(),
            Action::SyncBySound => {
                self.start_sync();
                Ok(())
            }
            Action::AddAdjustmentLayer => {
                let at = self.viewer.playhead;
                self.add_adjustment(at, None)
            }
            Action::AddVideoTrack => self.add_track(project::TrackKind::Video),
            Action::AddAudioTrack => self.add_track(project::TrackKind::Audio),
            Action::ZoomToFit => {
                self.refit();
                Ok(())
            }
            Action::ZoomIn
            | Action::ZoomOut
            | Action::TallerTracks
            | Action::ShorterTracks
            | Action::UsualTrackHeights => {
                if self.settings.compact_tracks && action != Action::ZoomIn && action != Action::ZoomOut {
                    self.status = format!(
                        "Compact tracks are all one height: turn them off ({}) to change track heights",
                        self.shortcuts.keys_or_name(Action::CompactTracks)
                    );
                }
                self.shown_view().step(match action {
                    Action::ZoomIn => ui_kit::ViewStep::ZoomIn,
                    Action::ZoomOut => ui_kit::ViewStep::ZoomOut,
                    Action::TallerTracks => ui_kit::ViewStep::TallerTracks,
                    Action::ShorterTracks => ui_kit::ViewStep::ShorterTracks,
                    _ => ui_kit::ViewStep::UsualTrackHeights,
                });
                Ok(())
            }
            Action::SelectTool => {
                self.shown_view().tool = ui_kit::Tool::Select;
                Ok(())
            }
            Action::BladeTool
            | Action::RippleTool
            | Action::RollTool
            | Action::SlipTool
            | Action::SlideTool
            | Action::TextTool => {
                self.shown_view().tool = match action {
                    Action::BladeTool => ui_kit::Tool::Blade,
                    Action::RippleTool => ui_kit::Tool::Ripple,
                    Action::RollTool => ui_kit::Tool::Roll,
                    Action::SlipTool => ui_kit::Tool::Slip,
                    Action::TextTool => ui_kit::Tool::Text,
                    _ => ui_kit::Tool::Slide,
                };
                Ok(())
            }
            Action::CleanPicture => {
                self.settings.clean_picture = !self.settings.clean_picture;
                self.status = if self.settings.clean_picture {
                    format!(
                        "Nothing over the picture: the play buttons are under it. {} brings everything back",
                        self.shortcuts.keys_or_name(Action::CleanPicture)
                    )
                } else {
                    "The play buttons are back over the picture".to_owned()
                };
                Ok(())
            }
            Action::PictureHandles => {
                self.settings.picture_handles = !self.settings.picture_handles;
                self.status = if self.settings.picture_handles {
                    "Handles on the picture: drag the selected clip to move it, a corner to resize it, a side to crop it, the round handle to turn it"
                } else {
                    "Handles on the picture off"
                }
                .into();
                Ok(())
            }
            Action::ToggleSnapping => {
                let view = self.shown_view();
                view.snapping = !view.snapping;
                self.status = if view.snapping {
                    "Snapping on"
                } else {
                    "Snapping off"
                }
                .into();
                Ok(())
            }
            Action::PlayPause => {
                match self.playback.take() {
                    // Pause: the playhead stays where playing got to.
                    Some(playback) => self.viewer.playhead = self.snapped(playback.position()),
                    None => self.start_playback(),
                }
                self.after_play_change();
                Ok(())
            }
            Action::Stop => {
                // Stop: back to where playback started, to watch it again.
                if let Some(playback) = self.playback.take() {
                    self.viewer.playhead = playback.started_at();
                }
                self.after_play_change();
                Ok(())
            }
            Action::Search => {
                // A fresh search each time it opens.
                if !self.show_search {
                    self.search_text.clear();
                }
                self.show_search = true;
                Ok(())
            }
            Action::OpenSettings => {
                self.show_settings = true;
                Ok(())
            }
            Action::Import => self.with_document(|_| Ok(())).map(|()| {
                self.import_requested = true;
            }),
            Action::PreviousFrame | Action::NextFrame | Action::GoToStart | Action::GoToEnd => {
                if let Some(seq) = self.document.as_ref().and_then(|d| d.project().active()) {
                    let (rate, length) = (seq.settings.frame_rate, seq.length());
                    match action {
                        Action::PreviousFrame => self.viewer.step(-1, rate, length),
                        Action::NextFrame => self.viewer.step(1, rate, length),
                        Action::GoToStart => self.viewer.go_to(Time::ZERO, rate, length),
                        _ => self.viewer.go_to(length, rate, length),
                    }
                }
                Ok(())
            }
            Action::PreviousKeyframe | Action::NextKeyframe => {
                self.jump_to_keyframe(action == Action::NextKeyframe);
                Ok(())
            }
            Action::PreviousCut | Action::NextCut => {
                self.jump_to_cut(action == Action::NextCut);
                Ok(())
            }
            Action::Copy => self.copy(),
            Action::Cut => self.cut(),
            Action::Paste => self.paste(),
        };
        if let Err(err) = result {
            self.errors.push(&err);
        }
    }
}
