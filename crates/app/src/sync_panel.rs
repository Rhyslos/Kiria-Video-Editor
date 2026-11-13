//! Sync by sound (`Action::SyncBySound`): the selected clips lined up with
//! one of them by what they hear (`engine::sync`), on a worker.
//!
//! Each selected clip counts with the clips linked to it. The one lined up
//! with (the reference) is the one on the lowest video track, the earliest
//! there, unless another is chosen; with no picture among them, the one on
//! the first audio track. The result shows how far each other clip would
//! move and how sure the match is, in plain words. "Move" moves the sure
//! and likely ones, in one undoable step (`Command::LineUp`); unsure ones
//! only move with "Move anyway".
//!
//! A clip that would start before the timeline is cut off there (the
//! timeline starts at 0): the window says so before Move, and the status
//! line after (`cut_offs`).

use engine::sync::{self, ClipSound, Found, Sureness};
use jobs::{JobHandle, JobProgress, JobSystem, PoolKind, Priority};
use project::{Clip, ClipId, Project, Rate, Sequence, Speed, Time};
use ui_kit::Theme;

/// One selected clip with its linked clips, as the window shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    /// The clip whose sound is compared, and that is moved (its linked
    /// clips follow).
    pub clip: ClipId,
    /// "match.mkv on V1".
    pub name: String,
    /// Why its sound can't be compared, when it can't.
    pub problem: Option<&'static str>,
    /// For choosing the reference: no picture (pictures first), which
    /// track (lowest first), when it starts.
    order: (bool, usize, Time),
}

/// What became of one clip.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Where its file lines up with the reference's.
    Found(Found),
    /// Its sound couldn't be read or compared; why.
    Failed(String),
}

/// Clips and where each is to start.
type Moves = Vec<(ClipId, Time)>;

/// Each compared unit's place in the list and what was found.
type Results = Vec<(usize, Outcome)>;

#[derive(Default)]
pub struct SyncPanel {
    pub open: bool,
    units: Vec<Unit>,
    /// Which of `units` the others are lined up with.
    reference: usize,
    running: Option<JobHandle<Results>>,
    /// One per unit once compared (none for the reference).
    outcomes: Vec<Option<Outcome>>,
    /// Why the comparison as a whole stopped, when it did.
    failed: Option<String>,
}

/// What the window asked for this frame.
#[derive(Debug, PartialEq)]
pub enum Asked {
    Nothing,
    /// Move these clips to start at these times, as one step.
    Move(Moves),
}

impl SyncPanel {
    /// Starts comparing the `selected` clips of the open timeline.
    /// Returns what the status line says when it can't.
    pub fn start(&mut self, project: &Project, selected: &[ClipId], jobs: &JobSystem) -> Option<String> {
        let Some(seq) = project.active() else {
            return Some("Open a timeline first.".into());
        };
        let units = units(project, seq, selected);
        if units.len() < 2 {
            return Some("Select two or more clips to line up by their sound.".into());
        }
        if units.iter().filter(|u| u.problem.is_none()).count() < 2 {
            let why = units.iter().find_map(|u| u.problem).unwrap_or("it has no sound");
            return Some(format!("Two of the clips need sound to compare, but one {why}."));
        }
        self.cancel();
        *self = SyncPanel {
            open: true,
            reference: reference(&units),
            units,
            ..SyncPanel::default()
        };
        self.run(project, jobs);
        None
    }

    /// Nothing is being compared now (for screenshots).
    pub fn settled(&self) -> bool {
        self.running.is_none()
    }

    fn cancel(&mut self) {
        if let Some(job) = self.running.take() {
            job.cancel();
        }
    }

    /// Compares every other clip with the reference, on a worker.
    fn run(&mut self, project: &Project, jobs: &JobSystem) {
        self.cancel();
        self.outcomes = vec![None; self.units.len()];
        self.failed = None;
        let Some(seq) = project.active() else { return };
        let Some((reference_clip, reference_sound)) = sound_of(project, seq, self.units[self.reference].clip)
        else {
            self.failed = Some("The clip to line up with isn't on the timeline any more.".into());
            return;
        };
        let mut pairs = Vec::new();
        for (i, unit) in self.units.iter().enumerate() {
            if i == self.reference || unit.problem.is_some() {
                continue;
            }
            let Some((clip, sound)) = sound_of(project, seq, unit.clip) else {
                continue;
            };
            let (reference_span, span) = sync::spans(&reference_clip, &clip);
            let reference = ClipSound {
                span: reference_span,
                ..reference_sound.clone()
            };
            pairs.push((i, reference, ClipSound { span, ..sound }));
        }
        let job = move |ctx: &jobs::JobContext| {
            let count = pairs.len().max(1) as f32;
            let cancelled = || ctx.is_cancelled();
            let mut results = Vec::with_capacity(pairs.len());
            for (n, (i, reference, other)) in pairs.iter().enumerate() {
                let progress = |share: f32| ctx.set_progress((n as f32 + share) / count);
                let watch = sync::Watch {
                    progress: &progress,
                    cancelled: &cancelled,
                };
                let found = sync::line_up_files(reference, std::slice::from_ref(other), &watch)?;
                let outcome = match found.into_iter().next() {
                    Some(Ok(found)) => Outcome::Found(found),
                    Some(Err(err)) => Outcome::Failed(err.to_string()),
                    None => Outcome::Failed("Nothing was compared.".into()),
                };
                results.push((*i, outcome));
            }
            Ok(results)
        };
        self.running = Some(jobs.submit(PoolKind::Analysis, Priority::Interactive, "sync by sound", job));
    }

    /// Takes in what the worker found. Every frame while the window is open.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.running else { return };
        match job.try_result() {
            Some(Ok(results)) => {
                for (i, outcome) in results {
                    if let Some(slot) = self.outcomes.get_mut(i) {
                        *slot = Some(outcome);
                    }
                }
                self.running = None;
            }
            Some(Err(err)) => {
                self.failed = Some(err.to_string());
                self.running = None;
            }
            None => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
        }
    }

    /// The window.
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        theme: &Theme,
        project: &Project,
        jobs: &JobSystem,
        undo: &str,
    ) -> Asked {
        let Some(seq) = project.active() else {
            return Asked::Nothing;
        };
        let rate = seq.settings.frame_rate;
        let mut asked = Asked::Nothing;
        let mut open = self.open;
        let mut rerun = false;
        // Over the middle of the window, under the page tabs.
        let middle = ctx.content_rect().center().x;
        egui::Window::new("Sync by sound")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .pivot(egui::Align2::CENTER_TOP)
            .default_pos([middle, 90.0])
            .default_width(460.0)
            .show(ctx, |ui| {
                let before = self.reference;
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Line up with").color(theme.text_secondary));
                    egui::ComboBox::from_id_salt("sync-reference")
                        .selected_text(&self.units[self.reference].name)
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            for (i, unit) in self.units.iter().enumerate() {
                                if unit.problem.is_none() {
                                    ui.selectable_value(&mut self.reference, i, &unit.name);
                                }
                            }
                        });
                });
                rerun = self.reference != before;
                ui.add_space(8.0);
                if let Some(job) = &self.running {
                    let share = match job.progress() {
                        JobProgress::Waiting => 0.0,
                        JobProgress::Running(share) => share,
                    };
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(egui::RichText::new("Listening to the clips…").color(theme.text_secondary));
                    });
                    ui.add(egui::ProgressBar::new(share).show_percentage());
                    ui.add_space(6.0);
                    if ui.button("Cancel").clicked() {
                        self.cancel();
                        self.failed = Some("Stopped before it finished.".into());
                    }
                    return;
                }
                if let Some(why) = &self.failed {
                    ui.label(egui::RichText::new(why).color(ui_kit::theme::status::ERROR));
                    return;
                }
                self.results_grid(ui, theme, seq, rate);
                ui.add_space(8.0);
                let (sure, unsure) = self.movable(seq);
                let note = if unsure.len() > sure.len() {
                    format!(
                        "Unsure matches stay where they are unless you move them anyway. Moving is one step: {undo} undoes it."
                    )
                } else {
                    format!("Their linked picture and sound move with them, as one step: {undo} undoes it.")
                };
                ui.label(egui::RichText::new(note).size(11.5).color(theme.muted));
                for (file, cut) in cut_offs(project, seq, &unsure) {
                    ui.label(
                        egui::RichText::new(cut_off_text(&file, cut, false))
                            .size(11.5)
                            .color(ui_kit::theme::status::WARNING),
                    );
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if !sure.is_empty() && ui_kit::accent_button(ui, theme, "Move").clicked() {
                        asked = Asked::Move(sure.clone());
                    }
                    if unsure.len() > sure.len() && ui.button("Move anyway").clicked() {
                        asked = Asked::Move(unsure.clone());
                    }
                });
            });
        if rerun {
            self.run(project, jobs);
        }
        self.open = open && matches!(asked, Asked::Nothing);
        if !self.open {
            self.cancel();
        }
        asked
    }

    /// One line per clip: its name, how far it would move, how sure.
    fn results_grid(&self, ui: &mut egui::Ui, theme: &Theme, seq: &Sequence, rate: Rate) {
        egui::Grid::new("sync-results")
            .num_columns(3)
            .spacing([18.0, 6.0])
            .show(ui, |ui| {
                for head in ["Clip", "Moves", "Match"] {
                    ui.label(egui::RichText::new(head).size(11.5).color(theme.faint));
                }
                ui.end_row();
                for (i, unit) in self.units.iter().enumerate() {
                    ui.label(egui::RichText::new(&unit.name).color(theme.text));
                    let (moves, sure, colour) = if i == self.reference {
                        (
                            "Stays: the others line up with it".to_owned(),
                            String::new(),
                            theme.muted,
                        )
                    } else if let Some(problem) = unit.problem {
                        (format!("Not compared: it {problem}"), String::new(), theme.muted)
                    } else {
                        match (&self.outcomes[i], self.to(seq, i)) {
                            (Some(Outcome::Found(found)), Some(to)) => {
                                let now = find(seq, unit.clip).map_or(to, |c| c.start);
                                (
                                    moves_text(to - now, rate),
                                    found.sureness.words().to_owned(),
                                    sureness_colour(theme, found.sureness),
                                )
                            }
                            (Some(Outcome::Found(_)), None) => (
                                "Nothing in common was heard".to_owned(),
                                Sureness::Unsure.words().to_owned(),
                                sureness_colour(theme, Sureness::Unsure),
                            ),
                            (Some(Outcome::Failed(why)), _) => (why.clone(), String::new(), theme.muted),
                            (None, _) => ("Not compared".to_owned(), String::new(), theme.muted),
                        }
                    };
                    ui.label(egui::RichText::new(moves).color(theme.text_secondary));
                    ui.label(egui::RichText::new(sure).color(colour));
                    ui.end_row();
                }
            });
    }

    /// Where unit `i` would start, lined up, when a match was found.
    fn to(&self, seq: &Sequence, i: usize) -> Option<Time> {
        let Some(Outcome::Found(found)) = self.outcomes.get(i)? else {
            return None;
        };
        let offset = found.offset?;
        let reference = find(seq, self.units[self.reference].clip)?;
        let clip = find(seq, self.units[i].clip)?;
        Some(sync::lined_up_start(&reference, &clip, offset))
    }

    /// The moves "Move" makes (sure and likely matches) and those "Move
    /// anyway" makes (every match found).
    fn movable(&self, seq: &Sequence) -> (Moves, Moves) {
        let mut sure = Vec::new();
        let mut all = Vec::new();
        for (i, unit) in self.units.iter().enumerate() {
            let (Some(Outcome::Found(found)), Some(to)) = (&self.outcomes[i], self.to(seq, i)) else {
                continue;
            };
            if found.sureness > Sureness::Unsure {
                sure.push((unit.clip, to));
            }
            all.push((unit.clip, to));
        }
        (sure, all)
    }
}

fn sureness_colour(theme: &Theme, sureness: Sureness) -> egui::Color32 {
    match sureness {
        Sureness::Sure => ui_kit::theme::status::GOOD,
        Sureness::Likely => theme.text,
        Sureness::Unsure => ui_kit::theme::status::WARNING,
    }
}

fn find(seq: &Sequence, id: ClipId) -> Option<Clip> {
    seq.tracks()
        .flat_map(|t| t.clips.iter())
        .find(|c| c.id == id)
        .cloned()
}

/// The clip `id` and its sound to compare, the whole part it plays.
fn sound_of(project: &Project, seq: &Sequence, id: ClipId) -> Option<(Clip, ClipSound)> {
    let clip = find(seq, id)?;
    let media = project.media.get(&clip.media)?;
    let on_audio = seq
        .audio_tracks
        .iter()
        .any(|t| t.clips.iter().any(|c| c.id == id));
    let stream = if on_audio {
        clip.stream
    } else {
        media.info.audio.first()?.index
    };
    let sound = ClipSound {
        path: media.path.clone(),
        stream,
        span: sync::Span { from: 0, length: 0 },
    };
    Some((clip, sound))
}

/// The selected clips, each with its linked clips counted once: the clip
/// of each whose sound is compared (on the first audio track that has
/// one, else the picture's file's first sound), in timeline order.
pub fn units(project: &Project, seq: &Sequence, selected: &[ClipId]) -> Vec<Unit> {
    let mut seen: Vec<ClipId> = Vec::new();
    let mut units = Vec::new();
    for id in selected {
        if seen.contains(id) {
            continue;
        }
        let Some(clip) = find(seq, *id) else { continue };
        let group: Vec<(bool, usize, &Clip)> = seq
            .video_tracks
            .iter()
            .enumerate()
            .map(|(n, t)| (true, n, t))
            .chain(seq.audio_tracks.iter().enumerate().map(|(n, t)| (false, n, t)))
            .flat_map(|(video, n, t)| t.clips.iter().map(move |c| (video, n, c)))
            .filter(|(_, _, c)| c.id == *id || (clip.link.is_some() && c.link == clip.link))
            .collect();
        seen.extend(group.iter().map(|(_, _, c)| c.id));
        let picture = group.iter().find(|(video, ..)| *video);
        let sound = group.iter().find(|(video, _, c)| !video && c.is_file());
        let (_, track, chosen) = match (sound, picture) {
            (Some(s), _) => *s,
            (None, Some(p)) => *p,
            (None, None) => continue,
        };
        // Named by its picture's track when it has one.
        let track_name = match picture {
            Some((_, n, _)) => seq.video_tracks.get(*n),
            None => seq.audio_tracks.get(track),
        }
        .map_or_else(String::new, |t| t.name.clone());
        let media = project.media.get(&chosen.media);
        let file = media
            .and_then(|m| m.path.file_name())
            .map_or_else(|| "Clip".to_owned(), |n| n.to_string_lossy().into_owned());
        let problem = if !chosen.is_file() {
            Some("plays a timeline, not a recording")
        } else if chosen.speed != Speed::NORMAL || !chosen.ramp.is_empty() {
            Some("plays at another speed")
        } else if media.is_none_or(|m| m.info.audio.is_empty()) {
            Some("has no sound")
        } else {
            None
        };
        // The reference is chosen by the group's picture when it has one.
        let order = match picture {
            Some((_, n, p)) => (false, *n, p.start),
            None => (true, track, chosen.start),
        };
        units.push(Unit {
            clip: chosen.id,
            name: format!("{file} on {track_name}"),
            problem,
            order,
        });
    }
    units.sort_by_key(|u| (u.order.2, u.order.0, u.order.1));
    units
}

/// What `moves` would cut off the start of clips because they would start
/// before the timeline does: each file's name and how much, the most of
/// any clip of its group (a clip and those linked to it).
pub fn cut_offs(project: &Project, seq: &Sequence, moves: &[(ClipId, Time)]) -> Vec<(String, Time)> {
    let mut out: Vec<(String, Time)> = Vec::new();
    for (id, to) in moves {
        let Some(clip) = find(seq, *id) else { continue };
        let by = *to - clip.start;
        let cut = seq
            .tracks()
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.id == clip.id || (clip.link.is_some() && c.link == clip.link))
            .map(|c| Time::ZERO - (c.start + by))
            .filter(|cut| *cut > Time::ZERO)
            .max();
        let Some(cut) = cut else { continue };
        let file = project
            .media
            .get(&clip.media)
            .and_then(|m| m.path.file_name())
            .map_or_else(|| "a clip".to_owned(), |n| n.to_string_lossy().into_owned());
        out.push((file, cut));
    }
    out
}

/// "The first 3.0 s of mic.wav will be cut off, because it would start
/// before the timeline does." (`done`: was cut off.)
pub fn cut_off_text(file: &str, cut: Time, done: bool) -> String {
    let length = ui_kit::time_text::length(cut.as_secs_f64());
    if done {
        format!(
            "The first {length} of {file} was cut off, because it would have started before the timeline does."
        )
    } else {
        format!(
            "The first {length} of {file} will be cut off, because it would start before the timeline does."
        )
    }
}

/// The usual reference: a picture on the lowest video track, the earliest
/// there; else sound on the first audio track. Only units with sound.
pub fn reference(units: &[Unit]) -> usize {
    units
        .iter()
        .enumerate()
        .filter(|(_, u)| u.problem.is_none())
        .min_by_key(|(_, u)| u.order)
        .map_or(0, |(i, _)| i)
}

/// "12.250 s earlier (735 frames)", "17 ms later (1 frame)", or
/// "Already in place": milliseconds under a second, seconds from one up.
pub fn moves_text(by: Time, rate: Rate) -> String {
    let seconds = by.as_secs_f64();
    let ms = (seconds * 1000.0).round() as i64;
    if ms == 0 {
        return "Already in place".into();
    }
    let frames = (seconds * rate.as_f64()).round().abs() as i64;
    let way = if ms < 0 { "earlier" } else { "later" };
    let plural = if frames == 1 { "" } else { "s" };
    let size = ms.unsigned_abs();
    let amount = if size < 1000 {
        format!("{size} ms")
    } else {
        format!("{}.{:03} s", thousands(size / 1000), size % 1000)
    };
    format!("{amount} {way} ({frames} frame{plural})")
}

/// 12250 as "12 250".
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, d) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{2009}');
        }
        out.push(d);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{
        AudioStream, Command, Fingerprint, MediaId, MediaInfo, MediaKind, MediaRef, SequenceSettings,
        VideoStream,
    };

    fn file(name: &str, picture: bool) -> MediaRef {
        MediaRef {
            id: MediaId(0),
            path: name.into(),
            fingerprint: Fingerprint {
                size: 1,
                sample_hash: name.into(),
            },
            kind: if picture {
                MediaKind::Video
            } else {
                MediaKind::Audio
            },
            duration: Time::seconds(60),
            info: MediaInfo {
                container: "matroska,webm".into(),
                video: picture.then_some(VideoStream {
                    index: 0,
                    codec: "hevc".into(),
                    width: 1920,
                    height: 1080,
                    frame_rate: Rate::FPS_60,
                    rotation: 0,
                    variable_frame_rate: false,
                }),
                audio: vec![AudioStream {
                    index: 1,
                    codec: "aac".into(),
                    channels: 2,
                    sample_rate: 48_000,
                    name: "Sound".into(),
                }],
                stated_length: None,
            },
        }
    }

    #[test]
    fn the_game_recording_is_the_reference_and_linked_clips_count_once() {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let sequence = p.active_sequence;
        let import = |p: &Project, media| Command::Import { sequence, media }.apply(p).unwrap();
        // The mic first on the timeline, then the game recording.
        let p = import(&import(&p, file("mic.wav", false)), file("game.mkv", true));
        let seq = p.active().unwrap();
        let mic = seq.audio_tracks[0].clips[0].id;
        let game_picture = seq.video_tracks[0].clips[0].id;
        let game_sound = seq.audio_tracks[0].clips[1].id;
        // The game picked by its picture and by its sound: once.
        let found = units(&p, seq, &[mic, game_picture, game_sound]);
        assert_eq!(found.len(), 2);
        let game = &found[reference(&found)];
        assert_eq!(game.clip, game_sound, "its sound is compared and moved");
        assert_eq!(game.name, "game.mkv on V1");
        assert!(found.iter().all(|u| u.problem.is_none()));

        // A clip at another speed can't be compared.
        let p = Command::SetSpeed {
            sequence,
            clip: mic,
            speed: Speed::new(1, 2).unwrap(),
        }
        .apply(&p)
        .unwrap();
        let seq = p.active().unwrap();
        let found = units(&p, seq, &[mic, game_picture]);
        assert_eq!(
            found.iter().find(|u| u.clip == mic).unwrap().problem,
            Some("plays at another speed")
        );
    }

    /// A mic that started recording 3 s before the screen recording, both
    /// at 0 on the timeline: lining it up would start it 3 s before the
    /// timeline, so its first 3 s are cut off, and that is said.
    #[test]
    fn a_move_before_the_timeline_says_what_it_cuts_off() {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let sequence = p.active_sequence;
        let p = Command::Import {
            sequence,
            media: file("mic.wav", false),
        }
        .apply(&p)
        .unwrap();
        let seq = p.active().unwrap();
        let mic = seq.audio_tracks[0].clips[0].id;
        let cuts = cut_offs(&p, seq, &[(mic, Time::seconds(-3))]);
        assert_eq!(cuts, [("mic.wav".to_owned(), Time::seconds(3))]);
        assert_eq!(
            cut_off_text(&cuts[0].0, cuts[0].1, false),
            "The first 3.0 s of mic.wav will be cut off, because it would start before the timeline does."
        );
        assert!(
            cut_offs(&p, seq, &[(mic, Time::seconds(2))]).is_empty(),
            "later: nothing cut"
        );
    }

    #[test]
    fn moves_are_said_in_milliseconds_or_seconds_and_frames() {
        let rate = Rate::FPS_60;
        assert_eq!(
            moves_text(Time::from_ratio(-49, 4).unwrap(), rate),
            "12.250 s earlier (735 frames)"
        );
        assert_eq!(
            moves_text(Time::from_ratio(1, 60).unwrap(), rate),
            "17 ms later (1 frame)"
        );
        assert_eq!(
            moves_text(Time::seconds(4000), rate),
            "4\u{2009}000.000 s later (240000 frames)"
        );
        assert_eq!(
            moves_text(Time::from_ratio(1, 4000).unwrap(), rate),
            "Already in place"
        );
    }
}
