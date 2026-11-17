//! The Color page as the editor lays it out: the shared viewer large on
//! the left, the bar of what is being graded on the right, the tools under
//! them, and the timeline at the bottom with the sound folded into one bar
//! and a dot on graded clips.

use super::Editor;
use crate::color_page;
use crate::timeline_panel;
use project::Time;

/// The right column's width.
const BAR_WIDTH: f32 = 300.0;
/// Space between the parts.
const GAP: f32 = 8.0;

impl Editor {
    pub(super) fn color_page_ui(&mut self, ui: &mut egui::Ui) {
        let page = ui.available_rect_before_wrap();
        let Some(p) = self.document.as_ref().map(|d| d.project().clone()) else {
            return;
        };
        let p = &p;
        let Some(seq) = p.active() else { return };
        let shown = self.color_page.shown(p);
        let shown_p = shown.as_ref().unwrap_or(p);
        let tools_h = if self.color_page.advanced && self.color_page.tool == color_page::Tool::Wheels {
            290.0
        } else {
            230.0
        };
        let timeline_h = (page.height() * 0.26).clamp(150.0, 320.0);
        let top_h = (page.height() - tools_h - timeline_h - 2.0 * GAP).max(200.0);
        let top = egui::Rect::from_min_size(page.min, egui::vec2(page.width(), top_h));
        let bar = egui::Rect::from_min_max(egui::pos2(top.right() - BAR_WIDTH, top.top()), top.max);
        let viewer = egui::Rect::from_min_max(top.min, egui::pos2(bar.left() - GAP, top.bottom()));
        let tools = egui::Rect::from_min_size(
            egui::pos2(page.left(), top.bottom() + GAP),
            egui::vec2(page.width(), tools_h),
        );
        let timeline = egui::Rect::from_min_max(egui::pos2(page.left(), tools.bottom() + GAP), page.max);

        // The picture, as it is graded (with what is being dragged).
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
            shown_p,
            &mut self.settings.preview_quality,
            needle,
            top_h,
            &self.shortcuts,
            display_state,
            crate::viewer::Stage { source: None, away },
            |_, _, _| {},
        );

        let graded = color_page::graded_clip(seq, &self.selected, self.viewer.playhead).and_then(|id| {
            seq.video_tracks
                .iter()
                .flat_map(|t| t.clips.iter())
                .find(|c| c.id == id)
        });
        let mut asked = Vec::new();
        let panel = |ui: &mut egui::Ui, rect: egui::Rect| {
            ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            )
        };
        let frame = egui::Frame::new()
            .fill(self.theme.panel)
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(12, 10));
        let mut right = panel(ui, bar);
        frame.show(&mut right, |ui| {
            ui.set_min_size(bar.size() - egui::vec2(24.0, 20.0));
            match graded {
                Some(clip) => asked.push(self.color_page.bar_ui(ui, &self.theme, p, clip, &self.selected)),
                None => {
                    ui.label(egui::RichText::new("Nothing to grade here").strong());
                    ui.label(
                        egui::RichText::new("Select a clip on the timeline, or put the playhead over one.")
                            .color(self.theme.muted),
                    );
                }
            }
            ui.add_space(10.0);
            ui.separator();
            self.color_page
                .scope_ui(ui, &self.theme, self.viewer.sampled.as_ref());
        });
        let mut under = panel(ui, tools);
        frame.show(&mut under, |ui| {
            ui.set_min_size(tools.size() - egui::vec2(24.0, 20.0));
            if let Some(clip) = graded {
                let media = p.media.get(&clip.media);
                asked.push(self.color_page.tools_ui(ui, &self.theme, seq.id, clip, media));
            }
        });

        // The timeline: pictures as clips, the sound folded into one bar.
        timeline_panel::keep_existing(&mut self.selected, seq);
        let (jobs, waveforms, pictures, ctx) = (
            &self.jobs,
            &mut self.waveforms,
            &mut self.pictures,
            ui.ctx().clone(),
        );
        let mut drawn = timeline_panel::drawn(
            shown_p,
            seq,
            &self.selected,
            &self.relink.missing,
            |m, stream| waveforms.get(jobs, &ctx, m, stream),
            |m, stream| pictures.get(jobs, &ctx, m, stream),
        );
        timeline_panel::fold_sound(&mut drawn);
        let length = seq.length().as_secs_f64();
        if !self.fitted {
            self.timeline
                .fit(length, timeline.width() - ui_kit::timeline::HEADER_WIDTH);
            self.fitted = true;
        }
        let roles = crate::audio_page::roles::offered();
        let data = ui_kit::TimelineData {
            video: &drawn.video,
            audio: &drawn.audio,
            length,
            fps: seq.settings.frame_rate.as_f64(),
            playhead: self.viewer.playhead.as_secs_f64(),
            needle: needle.map(Time::as_secs_f64),
            region: seq.region.map(|r| (r.start.as_secs_f64(), r.end.as_secs_f64())),
            markers: &drawn.markers,
            highlights: &[],
            spans: &[],
            follow: self.playback.is_some(),
            cache: ui_kit::timeline::CacheBar {
                ranges: &drawn.cache,
                about: timeline_panel::CACHE_BAR_ABOUT,
            },
            keyframe_eases: &timeline_panel::EASES,
            roles: &roles,
        };
        let mut bottom = panel(ui, timeline);
        let events = self
            .timeline
            .show(&mut bottom, &self.theme, &data, timeline.size());
        ui.allocate_rect(page, egui::Sense::hover());

        for asked in asked {
            if let Some(status) = asked.status {
                self.status = status;
            }
            for command in asked.commands {
                if let Err(err) = self.with_document(|d| d.apply(command)) {
                    self.errors.push(&err);
                }
            }
        }
        for event in events {
            self.timeline_event(event);
        }
        if let Some(command) = viewer_command {
            self.run_action(command);
        }
    }
}
