//! The Audio page, as in the mockup: the video preview with its transport
//! top left (the viewer every page shares), the effects of one track top
//! right (a track dropdown and a one-at-a-time switcher: EQ, Compressor,
//! Clean-up, Ducking, Loudness, and the full loudness overview, Meters),
//! the clarity check across the middle (`clarity`, `clarity_draw`), and at the bottom
//! the timeline, the mixer or both (Split, side by side or stacked).
//!
//! The window lays the page out (`editor/audio_ui.rs`); this module keeps
//! the page's state and draws its own parts. Every change is an undoable
//! `SetTrackSound`, `SetMasterSound` or `SetTrackMix`; while a value is
//! being dragged it is kept here ([`AudioPage::shown`] is the project as it
//! would be, which playback follows, so it is heard as it moves) and
//! becomes one undo step when the hand is off it, as the inspector does.

pub mod bar;
pub mod clarity;
pub mod clarity_draw;
mod cleanup;
pub mod counting;
mod ducking;
mod dynamics;
mod eq;
mod leveller;
pub mod lines;
mod overview;
pub mod roles;
pub mod start;
mod words;

pub use counting::Counting;
pub use roles::Show;
pub use start::Start;

use audio::TrackLevels;
use audio::dsp::loudness::Measured;
use egui::{Rect, Ui, pos2, vec2};
use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use project::track_sound::NoiseProfile;
use project::{
    Command, MasterSound, Project, Sequence, SequenceId, Time, Track, TrackId, TrackRole, TrackSound,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ui_kit::Theme;
use ui_kit::inspector::Touched;
use ui_kit::master::LoudnessTarget;

/// What the bottom of the page shows. A setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Bottom {
    Timeline,
    Mixer,
    /// Both at once.
    #[default]
    Split,
}

impl Bottom {
    pub const ALL: [Bottom; 3] = [Bottom::Timeline, Bottom::Mixer, Bottom::Split];

    pub fn label(self) -> &'static str {
        match self {
            Bottom::Timeline => "Timeline",
            Bottom::Mixer => "Mixer",
            Bottom::Split => "Split",
        }
    }
}

/// How Split lays the two out. A setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SplitWay {
    /// The mixer's strips on the left, the timeline on the right, both at full height.
    #[default]
    SideBySide,
    /// A few timeline tracks above the mixer's row.
    Stacked,
}

/// The tool shown top right, one at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Eq,
    Compressor,
    Cleanup,
    Ducking,
    Loudness,
    /// The full loudness overview.
    Meters,
}

impl Tool {
    pub const ALL: [Tool; 6] = [
        Tool::Eq,
        Tool::Compressor,
        Tool::Cleanup,
        Tool::Ducking,
        Tool::Loudness,
        Tool::Meters,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Eq => "EQ",
            Tool::Compressor => "Compressor",
            Tool::Cleanup => "Clean-up",
            Tool::Ducking => "Ducking",
            Tool::Loudness => "Loudness",
            Tool::Meters => "Meters",
        }
    }

    /// The tool called `name` on the command line (`--audio-tool eq`).
    pub fn from_name(name: &str) -> Option<Tool> {
        let name = name.to_lowercase().replace(['-', ' '], "");
        Tool::ALL
            .into_iter()
            .find(|t| t.label().to_lowercase().replace(['-', ' '], "") == name)
    }
}

/// What the mixer measured this frame, taken once and shared by every part.
#[derive(Debug, Clone, Default)]
pub struct Live {
    pub playing: bool,
    pub tracks: HashMap<TrackId, TrackLevels>,
    pub master: Option<Measured>,
    /// How far the master's compressor and limiter turn the mix down (dB).
    pub master_reduction: Option<(f32, f32)>,
    /// The analyser's levels for the track the EQ shows.
    pub spectrum: Option<[f32; audio::dsp::bands::COUNT]>,
}

/// A value being dragged and not yet an undo step.
#[derive(Debug, Clone, PartialEq)]
enum Pending {
    Track(TrackId, Box<TrackSound>),
    Master(MasterSound),
}

/// What the page asks of the window after a frame of its tools.
#[derive(Debug, Default)]
pub struct Asks {
    pub commands: Vec<Command>,
    pub counting: Option<Counting>,
    pub target: Option<LoudnessTarget>,
    /// Start the overview's counting again.
    pub reset: bool,
    /// Learn this track's noise over this stretch.
    pub learn: Option<(TrackId, (Time, Time))>,
}

/// The page's state, kept by the window.
#[derive(Default)]
pub struct AudioPage {
    pub tool: Tool,
    /// The track the tools work on (the first audio track until one is chosen).
    pub track: Option<TrackId>,
    pub show: Show,
    pending: Option<Pending>,
    eq: eq::EqTool,
    pub mixer: ui_kit::mixer::Mixer,
    pub master: ui_kit::master::MasterColumn,
    /// The page's own view of the timeline (its zoom follows the Edit page's).
    pub timeline: ui_kit::TimelineView,
    pub counted: counting::Counted,
    /// The page's timeline was fitted to the timeline's length once.
    pub fitted: bool,
    /// The track the mixer last showed as chosen: another one is scrolled into view.
    strip_shown: Option<TrackId>,
    /// The clarity check across the middle.
    pub clarity: clarity::ClarityBand,
    /// A noise being learned on a worker, for a track.
    learning: Option<(TrackId, SequenceId, JobHandle<NoiseProfile>)>,
}

impl std::fmt::Debug for AudioPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioPage")
            .field("tool", &self.tool)
            .field("track", &self.track)
            .finish_non_exhaustive()
    }
}

/// The track the tools work on in `seq`: the one chosen, or the first.
pub fn chosen(seq: &Sequence, track: Option<TrackId>) -> Option<&Track> {
    track
        .and_then(|id| seq.audio_tracks.iter().find(|t| t.id == id))
        .or_else(|| seq.audio_tracks.front())
}

/// "A3": an audio track's short name.
pub fn short_name(seq: &Sequence, id: TrackId) -> String {
    let n = seq
        .audio_tracks
        .iter()
        .position(|t| t.id == id)
        .map_or(0, |i| i + 1);
    format!("A{n}")
}

/// The effects that are on, as tags: their short name, their tool and
/// what they do now.
pub fn tags(track: &Track, levels: Option<&TrackLevels>) -> Vec<(&'static str, Tool, String)> {
    let s = &track.sound;
    let name = &track.name;
    let mut out = Vec::new();
    if s.high_pass.on || s.eq.on {
        let tag = if s.eq.on { "EQ" } else { "Low cut" };
        out.push((tag, Tool::Eq, words::eq(name, &s.eq, &s.high_pass)));
    }
    if s.compressor.on {
        let gr = levels.map(|l| l.compressor_db);
        out.push((
            "Comp",
            Tool::Compressor,
            words::compressor(name, &s.compressor, gr),
        ));
    }
    if s.cleanup.on {
        let r = levels.map(|l| l.cleanup_db);
        out.push(("Clean", Tool::Cleanup, words::cleanup(name, &s.cleanup, r)));
    }
    if s.ducking.on {
        let d = levels.map(|l| l.ducked_db);
        out.push((
            "Duck",
            Tool::Ducking,
            words::ducking(name, &s.ducking, &["dialogue"], d),
        ));
    }
    if s.loudness.on {
        let g = levels.map(|l| l.level_db);
        out.push(("Level", Tool::Loudness, words::leveller(name, &s.loudness, g)));
    }
    out
}

impl AudioPage {
    /// The project with what is being dragged on the page, for playback
    /// to follow and the page to draw. `None` when nothing is.
    pub fn shown(&self, project: &Project) -> Option<Project> {
        let sequence = project.active_sequence;
        let mut shown: Option<Project> = None;
        if let Some(pending) = &self.pending {
            let command = match pending {
                Pending::Track(track, sound) => Command::SetTrackSound {
                    sequence,
                    track: *track,
                    sound: sound.clone(),
                },
                Pending::Master(sound) => Command::SetMasterSound {
                    sequence,
                    sound: *sound,
                },
            };
            shown = command.apply(project).ok();
        }
        if let Some((track, db)) = self.mixer.gain_being_set()
            && let Some(t) = project
                .active()
                .and_then(|s| s.audio_tracks.iter().find(|t| t.id.0 == track))
        {
            let command = Command::SetTrackMix {
                sequence,
                track: t.id,
                muted: t.muted,
                solo: t.solo,
                gain_db: db,
            };
            shown = command.apply(shown.as_ref().unwrap_or(project)).ok().or(shown);
        }
        shown
    }

    /// A tool's change to `track`'s sound this frame: kept while the hand
    /// is on it, one undo step once it is off.
    fn track_edit(
        &mut self,
        seq: &Sequence,
        track: &Track,
        sound: TrackSound,
        touched: Touched,
        asks: &mut Asks,
    ) {
        if touched.changed {
            self.pending = Some(Pending::Track(track.id, Box::new(sound)));
        }
        if !touched.busy {
            self.finish(seq, asks);
        }
    }

    fn master_edit(&mut self, seq: &Sequence, sound: MasterSound, touched: Touched, asks: &mut Asks) {
        if touched.changed {
            self.pending = Some(Pending::Master(sound));
        }
        if !touched.busy {
            self.finish(seq, asks);
        }
    }

    /// The change in waiting becomes an undo step, unless it changes nothing.
    fn finish(&mut self, seq: &Sequence, asks: &mut Asks) {
        match self.pending.take() {
            Some(Pending::Track(id, sound)) => {
                if seq.audio_tracks.iter().any(|t| t.id == id && t.sound != *sound) {
                    asks.commands.push(Command::SetTrackSound {
                        sequence: seq.id,
                        track: id,
                        sound,
                    });
                }
            }
            Some(Pending::Master(sound)) if seq.master_sound != sound => {
                asks.commands.push(Command::SetMasterSound {
                    sequence: seq.id,
                    sound,
                });
            }
            Some(Pending::Master(_)) | None => {}
        }
    }

    /// Starts learning `track`'s noise over `span` on a worker.
    pub fn learn(&mut self, jobs: &JobSystem, project: &Project, track: TrackId, span: (Time, Time)) {
        let project = project.clone();
        let sequence = project.active_sequence;
        let handle = jobs.submit(
            PoolKind::Analysis,
            Priority::Interactive,
            "learn noise",
            move |_| engine::noise::learn(&project, sequence, track, span),
        );
        self.learning = Some((track, sequence, handle));
    }

    /// A learned noise, as one undo step that also turns clean-up on.
    pub fn take_learned(&mut self, project: &Project) -> Option<diag::Result<Command>> {
        let (track, sequence, handle) = self.learning.as_ref()?;
        let result = handle.try_result()?;
        let (track, sequence) = (*track, *sequence);
        self.learning = None;
        Some(result.and_then(|profile| {
            let seq = project.sequence(sequence).ok_or_else(|| {
                diag::AppError::new("The learned noise can't be kept")
                    .why("The timeline is not in the project any more.")
            })?;
            let t = seq.audio_tracks.iter().find(|t| t.id == track).ok_or_else(|| {
                diag::AppError::new("The learned noise can't be kept")
                    .why("The track is not on the timeline any more.")
            })?;
            let mut sound = t.sound.clone();
            sound.cleanup.profile = Some(profile);
            sound.cleanup.on = true;
            Ok(Command::SetTrackSound {
                sequence,
                track,
                sound: Box::new(sound),
            })
        }))
    }

    /// The header of the tools: the track dropdown and the switcher.
    pub fn header(&mut self, ui: &mut Ui, theme: &Theme, seq: &Sequence) {
        let current = chosen(seq, self.track);
        ui.horizontal(|ui| {
            let text = current.map_or("No audio track".to_owned(), |t| {
                format!("{} {}", short_name(seq, t.id), t.name)
            });
            let colour = current.map_or(theme.muted, |t| roles::colour(t.sound.role));
            ui.label(egui::RichText::new("●").color(colour));
            egui::ComboBox::from_id_salt("audio-track")
                .selected_text(text)
                .width(130.0)
                .height(420.0)
                .show_ui(ui, |ui| {
                    for t in &seq.audio_tracks {
                        let label = egui::RichText::new(format!("{} {}", short_name(seq, t.id), t.name));
                        let row = ui.selectable_label(Some(t.id) == current.map(|c| c.id), label);
                        if row.clicked() {
                            self.track = Some(t.id);
                        }
                    }
                });
            ui.add_space(6.0);
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.spacing_mut().button_padding.x = 5.0;
            for tool in Tool::ALL {
                if tool == Tool::Meters {
                    ui.add_space(4.0);
                    ui.separator();
                }
                let on = tool == self.tool;
                let text = egui::RichText::new(tool.label());
                let text = if on {
                    text.strong().color(theme.text)
                } else {
                    text.color(theme.text_secondary)
                };
                if ui.selectable_label(on, text).clicked() {
                    self.tool = tool;
                }
            }
        });
    }

    /// The tool shown, for the chosen track of `seq` (the project as shown).
    #[allow(clippy::too_many_arguments)]
    pub fn tool_ui(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        seq: &Sequence,
        live: &Live,
        learn_from: cleanup::LearnFrom,
        counting: Counting,
        target: LoudnessTarget,
    ) -> Asks {
        let mut asks = Asks::default();
        let Some(track) = chosen(seq, self.track) else {
            ui_kit::inspector::note(
                ui,
                theme,
                "No audio track yet: import a recording with sound, or add an audio track.",
            );
            return asks;
        };
        let levels = live.tracks.get(&track.id).filter(|_| live.playing);
        let mut sound = track.sound.clone();
        let name = track.name.as_str();
        let room = ui.available_rect_before_wrap();
        let body = Rect::from_min_max(room.min, pos2(room.right(), room.bottom() - 22.0));
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(body)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        let said = match self.tool {
            Tool::Eq => {
                let (touched, said) = self.eq.ui(&mut child, theme, &mut sound, live, name);
                self.track_edit(seq, track, sound, touched, &mut asks);
                said
            }
            Tool::Compressor => {
                let gr = levels.map(|l| l.compressor_db);
                let touched = dynamics::ui(&mut child, theme, &mut sound.compressor, gr);
                let said = words::compressor(name, &sound.compressor, gr);
                self.track_edit(seq, track, sound, touched, &mut asks);
                said
            }
            Tool::Cleanup => {
                let r = levels.map(|l| l.cleanup_db);
                let learning = self.learning.as_ref().map(|(t, ..)| *t);
                let got = cleanup::ui(&mut child, theme, &mut sound.cleanup, r, learn_from, learning);
                asks.learn = got.learn.map(|span| (track.id, span));
                let said = match learning {
                    Some(_) => "Learning the noise…".to_owned(),
                    None => words::cleanup(name, &sound.cleanup, r),
                };
                self.track_edit(seq, track, sound, got.touched, &mut asks);
                said
            }
            Tool::Ducking => {
                let keys: Vec<&str> = seq
                    .audio_tracks
                    .iter()
                    .filter(|t| t.sound.role == TrackRole::Dialogue && t.id != track.id)
                    .map(|t| t.name.as_str())
                    .collect();
                let d = levels.map(|l| l.ducked_db);
                let touched = ducking::ui(&mut child, theme, &mut sound.ducking, track.sound.role, &keys, d);
                let said = words::ducking(name, &sound.ducking, &keys, d);
                self.track_edit(seq, track, sound, touched, &mut asks);
                said
            }
            Tool::Loudness => {
                let g = levels.map(|l| l.level_db);
                let st = levels.map(|l| l.short_term);
                let touched = leveller::ui(&mut child, theme, &mut sound.loudness, g, st);
                let said = words::leveller(name, &sound.loudness, g);
                self.track_edit(seq, track, sound, touched, &mut asks);
                said
            }
            Tool::Meters => {
                let every_play = counting == Counting::EveryPlay;
                let mix_now = live.master.map_or(overview::Now::SILENT, |m| overview::Now {
                    momentary: if live.playing {
                        m.momentary
                    } else {
                        f32::NEG_INFINITY
                    },
                    short_term: if live.playing {
                        m.short_term
                    } else {
                        f32::NEG_INFINITY
                    },
                    // Every play: the master's own count, the same as the master column's.
                    integrated: if every_play {
                        m.integrated
                    } else {
                        self.counted.mix.integrated()
                    },
                    range: self.counted.mix.range(),
                    peak: if every_play {
                        m.true_peak_max
                    } else {
                        self.counted.mix.peak()
                    },
                });
                let track_now = overview::Now {
                    momentary: levels.map_or(f32::NEG_INFINITY, |l| l.momentary),
                    short_term: levels.map_or(f32::NEG_INFINITY, |l| l.short_term),
                    integrated: self.counted.track.integrated(),
                    range: self.counted.track.range(),
                    peak: self.counted.track.peak(),
                };
                let got = overview::ui(
                    &mut child,
                    theme,
                    mix_now,
                    Some((name, track_now)),
                    &self.counted,
                    counting,
                    target,
                );
                asks.counting = got.counting;
                asks.target = got.target;
                asks.reset = got.reset;
                String::new()
            }
        };
        if !said.is_empty() {
            ui.painter().text(
                pos2(room.left(), room.bottom() - 4.0),
                egui::Align2::LEFT_BOTTOM,
                said,
                egui::FontId::proportional(12.5),
                theme.text,
            );
        }
        ui.allocate_rect(room, egui::Sense::hover());
        asks
    }

    /// The mixer: the strips of the tracks shown, scrolling sideways, and
    /// the master pinned on the right with its compressor and limiter.
    #[allow(clippy::too_many_arguments)]
    pub fn mixer_ui(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        rect: Rect,
        seq: &Sequence,
        live: &Live,
        target: LoudnessTarget,
        master_tip: &str,
    ) -> MixerAsks {
        let mut out = MixerAsks::default();
        let master_w = ui_kit::layout::MASTER_WIDTH + MASTER_SETTINGS;
        let strips_rect = Rect::from_min_max(rect.min, pos2(rect.right() - master_w - 8.0, rect.bottom()));
        let master_rect = Rect::from_min_max(pos2(rect.right() - master_w, rect.top()), rect.max);
        let current = chosen(seq, self.track).map(|t| t.id);
        let shown: Vec<&Track> = seq
            .audio_tracks
            .iter()
            .filter(|t| self.show.shows(t.sound.role))
            .collect();
        let shorts: Vec<String> = shown.iter().map(|t| short_name(seq, t.id)).collect();
        let all_tags: Vec<Vec<(&'static str, Tool, String)>> = shown
            .iter()
            .map(|t| tags(t, live.tracks.get(&t.id).filter(|_| live.playing)))
            .collect();
        let tag_lists: Vec<Vec<ui_kit::mixer::Tag>> = all_tags
            .iter()
            .map(|tags| {
                tags.iter()
                    .map(|(name, _, tip)| ui_kit::mixer::Tag { name, tip })
                    .collect()
            })
            .collect();
        let strips: Vec<ui_kit::mixer::Strip> =
            shown
                .iter()
                .zip(&shorts)
                .zip(&tag_lists)
                .map(|((t, short), tags)| ui_kit::mixer::Strip {
                    id: t.id.0,
                    short,
                    name: &t.name,
                    role: Some(roles::index(t.sound.role)),
                    gain_db: t.gain_db,
                    muted: t.muted,
                    solo: t.solo,
                    selected: Some(t.id) == current,
                    levels: live.tracks.get(&t.id).filter(|_| live.playing).map(|l| {
                        ui_kit::mixer::StripLevels {
                            peak_db: l.peak_db,
                            short_term: l.short_term,
                        }
                    }),
                    tags,
                })
                .collect();
        let offered = roles::offered();
        if current != self.strip_shown {
            self.strip_shown = current;
            if let Some(i) = shown.iter().position(|t| Some(t.id) == current) {
                self.mixer.show_strip(i, strips_rect.width());
            }
        }
        let events = self.mixer.show(ui, theme, strips_rect, &strips, &offered);
        for (id, event) in events {
            let Some((i, t)) = shown.iter().enumerate().find(|(_, t)| t.id.0 == id) else {
                continue;
            };
            let mix = |muted: bool, solo: bool, gain_db: f32| Command::SetTrackMix {
                sequence: seq.id,
                track: t.id,
                muted,
                solo,
                gain_db,
            };
            use ui_kit::mixer::StripEvent as E;
            match event {
                E::Gain { db, done: true } => out.commands.push(mix(t.muted, t.solo, db)),
                E::Gain { .. } => {}
                E::Mute(on) => out.commands.push(mix(on, t.solo, t.gain_db)),
                E::Solo(on) => out.commands.push(mix(t.muted, on, t.gain_db)),
                E::Select => self.track = Some(t.id),
                E::Tag(k) => {
                    self.track = Some(t.id);
                    if let Some((_, tool, _)) = all_tags.get(i).and_then(|tags| tags.get(k)) {
                        self.tool = *tool;
                    }
                }
                E::Role(r) => {
                    if let Some(role) = roles::at(r) {
                        let mut sound = t.sound.clone();
                        sound.role = role;
                        out.commands.push(Command::SetTrackSound {
                            sequence: seq.id,
                            track: t.id,
                            sound: Box::new(sound),
                        });
                    }
                }
            }
        }
        // The master: its meter and fader, then its compressor and limiter.
        let column = Rect::from_min_size(
            master_rect.min,
            vec2(ui_kit::layout::MASTER_WIDTH, master_rect.height()),
        );
        let levels = live
            .master
            .filter(|_| live.playing)
            .map(|l| ui_kit::master::MeterLevels {
                peak_db: l.peak_db,
                true_peak_db: l.true_peak_db,
                true_peak_max: l.true_peak_max,
                momentary: l.momentary,
                short_term: l.short_term,
                integrated: l.integrated,
            });
        let mix = ui_kit::master::Mix {
            levels,
            gain_db: seq.master_gain_db,
            target,
            tip: master_tip,
        };
        out.master = self.master.show(ui, theme, column, mix);
        let settings_rect =
            Rect::from_min_max(pos2(column.right() + 6.0, master_rect.top()), master_rect.max);
        let mut asks = Asks::default();
        self.master_settings(ui, theme, settings_rect, seq, live, &mut asks);
        out.commands.extend(asks.commands);
        out
    }

    /// The master's compressor and limiter, beside its column.
    fn master_settings(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        r: Rect,
        seq: &Sequence,
        live: &Live,
        asks: &mut Asks,
    ) {
        let mut sound = match &self.pending {
            Some(Pending::Master(s)) => *s,
            _ => seq.master_sound,
        };
        ui.painter().rect_filled(r, 6.0, theme.panel);
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(r.shrink(8.0))
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        let ui = &mut child;
        let mut touched = Touched::default();
        ui.label(egui::RichText::new("Master").strong());
        let (comp_gr, lim_gr) = live
            .master_reduction
            .filter(|_| live.playing)
            .unwrap_or((0.0, 0.0));
        let mut on = sound.compressor.on;
        if ui.checkbox(&mut on, "Compressor").changed() {
            sound.compressor.on = on;
            touched.changed = true;
        }
        if dynamics::preset_menu(
            ui,
            "master-compressor-preset",
            &mut sound.compressor,
            r.width() - 24.0,
        ) {
            touched.changed = true;
        }
        let mut threshold = f64::from(sound.compressor.threshold_mdb) / 1_000.0;
        let t = ui.add(
            egui::DragValue::new(&mut threshold)
                .range(-60.0..=0.0)
                .speed(0.2)
                .fixed_decimals(1)
                .prefix("Threshold ")
                .suffix(" dB"),
        );
        touched = touched.and(Touched {
            changed: t.changed(),
            busy: t.dragged() || t.has_focus(),
        });
        sound.compressor.threshold_mdb = (threshold * 1_000.0).round() as i32;
        ui.label(
            egui::RichText::new(format!("GR {}", reduction(comp_gr)))
                .color(theme.text_secondary)
                .monospace(),
        );
        ui.add_space(6.0);
        let mut lim = sound.limiter.on;
        if ui.checkbox(&mut lim, "Limiter").changed() {
            sound.limiter.on = lim;
            touched.changed = true;
        }
        let mut ceiling = f64::from(sound.limiter.ceiling_mdb) / 1_000.0;
        let c = ui.add(
            egui::DragValue::new(&mut ceiling)
                .range(-12.0..=0.0)
                .speed(0.05)
                .fixed_decimals(1)
                .prefix("Ceiling ")
                .suffix(" dBTP"),
        );
        touched = touched.and(Touched {
            changed: c.changed(),
            busy: c.dragged() || c.has_focus(),
        });
        sound.limiter.ceiling_mdb = (ceiling * 1_000.0).round() as i32;
        ui.label(
            egui::RichText::new(format!("GR {}", reduction(lim_gr)))
                .color(theme.text_secondary)
                .monospace(),
        );
        if live.playing {
            ui.ctx().request_repaint();
        }
        self.master_edit(seq, sound, touched, asks);
    }
}

/// A gain reduction as the master's settings write it: "−2.4 dB", "0".
fn reduction(db: f32) -> String {
    if db <= -0.05 {
        format!("{db:.1} dB").replace('-', "−")
    } else {
        "0".to_owned()
    }
}

/// Room for the master's compressor and limiter beside its column.
pub const MASTER_SETTINGS: f32 = 150.0;

/// What the mixer asks of the window.
#[derive(Debug, Default)]
pub struct MixerAsks {
    pub commands: Vec<Command>,
    pub master: ui_kit::master::MasterResponse,
}

pub use cleanup::LearnFrom;
pub use overview::integrated_colour as overview_colour;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_are_found_by_name_and_drags_become_one_step() {
        assert_eq!(Tool::from_name("eq"), Some(Tool::Eq));
        assert_eq!(Tool::from_name("clean-up"), Some(Tool::Cleanup));
        assert_eq!(Tool::from_name("Meters"), Some(Tool::Meters));
        assert_eq!(Tool::from_name("reverb"), None);

        let p = Project::new("T", project::SequenceSettings::GAME_CLIP);
        let seq = p.active().unwrap().clone();
        let track = seq.audio_tracks[0].clone();
        let mut page = AudioPage::default();
        let mut asks = Asks::default();
        let mut sound = track.sound.clone();
        sound.compressor.on = true;
        // Dragged over three frames: shown at once, one step when let go.
        for busy in [true, true] {
            sound.compressor.threshold_mdb -= 1_000;
            page.track_edit(
                &seq,
                &track,
                sound.clone(),
                Touched { changed: true, busy },
                &mut asks,
            );
        }
        assert!(asks.commands.is_empty());
        let shown = page.shown(&p).unwrap();
        assert_eq!(
            shown.active().unwrap().audio_tracks[0].sound,
            sound,
            "heard while dragging"
        );
        page.track_edit(
            &seq,
            &track,
            sound.clone(),
            Touched {
                changed: false,
                busy: false,
            },
            &mut asks,
        );
        assert_eq!(asks.commands.len(), 1);
        assert!(page.shown(&p).is_none());
        // A click that changes nothing makes no step.
        page.track_edit(
            &seq,
            &track,
            track.sound.clone(),
            Touched {
                changed: true,
                busy: false,
            },
            &mut asks,
        );
        assert_eq!(asks.commands.len(), 1);
    }

    #[test]
    fn tags_name_the_effects_that_are_on_and_open_their_tool() {
        let p = Project::new("T", project::SequenceSettings::GAME_CLIP);
        let mut track = p.active().unwrap().audio_tracks[0].clone();
        assert!(tags(&track, None).is_empty());
        track.sound.high_pass.on = true;
        track.sound.ducking.on = true;
        let t = tags(&track, None);
        assert_eq!(
            t.iter().map(|(n, tool, _)| (*n, *tool)).collect::<Vec<_>>(),
            [("Low cut", Tool::Eq), ("Duck", Tool::Ducking)]
        );
    }
}
