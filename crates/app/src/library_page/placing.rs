//! "Add to timeline" from the Library: a file, or the part of it marked
//! with in and out, perhaps at a changed level, as one undo step.
//!
//! The whole file as it is goes in with the commands the media drawer uses
//! (`PlaceMedia` at the end, `InsertMedia` at the edit point). A marked part
//! or a changed level goes in as a template made on the spot: the file is
//! placed at the end of a copy of the open timeline exactly as `PlaceMedia`
//! would place it (its picture on the first video track, each sound stream
//! on its own audio track, all linked), the marked stretch of that is taken
//! as a template (`Template::capture`, which cuts clips at its edges), its
//! sound clips get the level, and `InsertTemplate` drops it in. So the
//! tracks a part lands on are the ones the whole file would take, and undo
//! takes it out in one step.

use diag::AppError;
use project::{Command, MediaId, Project, Region, Sound, Template, Time};

/// Where an added file goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddAt {
    /// After everything on the open timeline.
    End,
    /// At the edit point, pushing what is after it later.
    EditPoint,
}

/// What to add: a file, the part of it from `part.0` to `part.1` (seconds
/// into the file; the whole file when `None`), with its sound changed by
/// `gain_mdb` thousandths of a decibel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Adding {
    pub media: MediaId,
    pub part: Option<(f64, f64)>,
    pub gain_mdb: i32,
    pub at: AddAt,
}

/// What the status line says once `adding` is done for the file called `name`.
pub fn says(name: &str, adding: &Adding) -> String {
    let what = match adding.part {
        Some((from, to)) => format!("{} of {name}", ui_kit::time_text::length((to - from).max(0.0))),
        None => name.to_owned(),
    };
    let level = match adding.gain_mdb {
        0 => String::new(),
        mdb => format!(", its sound {:+.1} dB", f64::from(mdb) / 1000.0),
    };
    let place = match adding.at {
        AddAt::End => "at the end of the timeline",
        AddAt::EditPoint => "at the edit point",
    };
    format!("Added {what} {place}{level}")
}

/// The command that adds it to the open timeline of `project`, `edit_point`
/// being where the edit point is.
pub fn command(project: &Project, adding: Adding, edit_point: Time) -> diag::Result<Command> {
    let seq = project.active().ok_or_else(|| {
        AppError::new("There is no timeline to add to").fix("Make a timeline on the Edit page first.")
    })?;
    let sequence = seq.id;
    let media = adding.media;
    if adding.part.is_none() && adding.gain_mdb == 0 {
        return Ok(match adding.at {
            AddAt::End => Command::PlaceMedia { sequence, media },
            AddAt::EditPoint => Command::InsertMedia {
                sequence,
                media,
                at: edit_point,
            },
        });
    }
    let at = match adding.at {
        AddAt::End => seq.length(),
        AddAt::EditPoint => edit_point,
    };
    let template = part_template(project, adding)?;
    Ok(Command::InsertTemplate {
        sequence,
        template: Box::new(template),
        at,
    })
}

/// The marked part of the file, at its level, as a template (see the top of this file).
pub fn part_template(project: &Project, adding: Adding) -> diag::Result<Template> {
    let seq = project.active().ok_or_else(|| {
        AppError::new("There is no timeline to add to").fix("Make a timeline on the Edit page first.")
    })?;
    let file = project
        .media
        .get(&adding.media)
        .ok_or_else(|| AppError::new("That file is no longer in the project").fix("Import it again."))?;
    let before: Vec<project::ClipId> = seq.tracks().flat_map(|t| t.clips.iter().map(|c| c.id)).collect();
    let placed = Command::PlaceMedia {
        sequence: seq.id,
        media: adding.media,
    }
    .apply(project)?;
    let placed_seq = placed
        .sequence(seq.id)
        .ok_or_else(|| AppError::new("The timeline went away while adding to it"))?;
    // Where the file landed: the start of its new clips.
    let start = placed_seq
        .tracks()
        .flat_map(|t| t.clips.iter())
        .filter(|c| !before.contains(&c.id))
        .map(|c| c.start)
        .min()
        .ok_or_else(|| {
            AppError::new("The file couldn't be placed")
                .why("It has no picture or sound to put on a timeline.")
        })?;
    let rate = placed_seq.settings.frame_rate;
    let length = file.duration.as_secs_f64();
    let (from, to) = adding.part.unwrap_or((0.0, length));
    let from = rate.snap(crate::editor::seconds(from.clamp(0.0, length)));
    let to = rate.snap(crate::editor::seconds(to.clamp(0.0, length)));
    let region = Region::between(start + from, start + to).ok_or_else(|| {
        AppError::new("The marked part is empty")
            .why("Its in and out marks are on the same frame.")
            .fix("Mark out later than in, or clear the marks to add the whole file.")
    })?;
    let mut template = Template::capture(&placed, seq.id, Some(region))?;
    if adding.gain_mdb != 0 {
        for track in &mut template.audio_tracks {
            for clip in track.clips.iter_mut() {
                clip.sound.gain_mdb =
                    (clip.sound.gain_mdb + adding.gain_mdb).clamp(Sound::SILENT, Sound::LOUDEST);
            }
        }
    }
    Ok(template)
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Fingerprint, MediaRef, TrackKind};

    fn game_recording() -> MediaRef {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/game-3-audio.mkv");
        let probed = media::probe(&path).unwrap();
        MediaRef {
            id: MediaId(0),
            path,
            fingerprint: Fingerprint {
                size: 5,
                sample_hash: "abc".into(),
            },
            kind: probed.kind,
            duration: probed.duration,
            info: probed.info,
        }
    }

    fn project_with(media: MediaRef) -> (Project, MediaId) {
        let p = Project::new_automatic("T");
        let p = Command::AddMedia { media }.apply(&p).unwrap();
        let id = *p.media.keys().next().unwrap();
        (p, id)
    }

    #[test]
    fn the_whole_file_goes_in_as_the_drawer_places_it() {
        let (p, media) = project_with(game_recording());
        let adding = Adding {
            media,
            part: None,
            gain_mdb: 0,
            at: AddAt::End,
        };
        assert!(matches!(
            command(&p, adding, Time::ZERO),
            Ok(Command::PlaceMedia { .. })
        ));
        let insert = Adding {
            at: AddAt::EditPoint,
            ..adding
        };
        assert!(matches!(
            command(&p, insert, Time::ZERO),
            Ok(Command::InsertMedia { .. })
        ));
    }

    #[test]
    fn a_marked_part_goes_in_cut_at_its_marks_as_one_step() {
        let (p, media) = project_with(game_recording());
        let length = p.media.get(&media).unwrap().duration.as_secs_f64();
        let adding = Adding {
            media,
            part: Some((0.5, length - 0.5)),
            gain_mdb: -3_000,
            at: AddAt::End,
        };
        let command = command(&p, adding, Time::ZERO).unwrap();
        assert!(matches!(command, Command::InsertTemplate { .. }));
        let after = command.apply(&p).unwrap();
        let seq = after.active().unwrap();
        let clips: Vec<_> = seq.tracks().flat_map(|t| t.clips.iter()).collect();
        assert!(clips.len() >= 2, "picture and sound");
        let rate = seq.settings.frame_rate;
        let half = rate.snap(crate::editor::seconds(0.5));
        for clip in &clips {
            assert_eq!(clip.start, Time::ZERO, "first on an empty timeline");
            assert_eq!(clip.source_in, half, "from the in mark");
        }
        for track in seq.audio_tracks.iter().filter(|t| t.kind == TrackKind::Audio) {
            for clip in &track.clips {
                assert_eq!(clip.sound.gain_mdb, -3_000, "the level goes on the sound");
            }
        }
        for track in &seq.video_tracks {
            for clip in &track.clips {
                assert_eq!(clip.sound.gain_mdb, 0, "not on the picture");
            }
        }
        // The same as placing the whole file, but shorter.
        let whole = Command::PlaceMedia {
            sequence: p.active_sequence,
            media,
        }
        .apply(&p)
        .unwrap();
        let whole_seq = whole.active().unwrap();
        assert_eq!(
            seq.tracks().filter(|t| !t.clips.is_empty()).count(),
            whole_seq.tracks().filter(|t| !t.clips.is_empty()).count(),
            "the same tracks as the whole file"
        );
        assert!(seq.length() < whole_seq.length());
    }

    #[test]
    fn the_status_line_says_what_went_where() {
        let adding = Adding {
            media: MediaId(1),
            part: Some((2.0, 7.5)),
            gain_mdb: 3_000,
            at: AddAt::End,
        };
        assert_eq!(
            says("a.mkv", &adding),
            "Added 5.5 s of a.mkv at the end of the timeline, its sound +3.0 dB"
        );
        let whole = Adding {
            part: None,
            gain_mdb: 0,
            at: AddAt::EditPoint,
            ..adding
        };
        assert_eq!(says("a.mkv", &whole), "Added a.mkv at the edit point");
    }

    #[test]
    fn an_empty_part_is_explained() {
        let (p, media) = project_with(game_recording());
        let adding = Adding {
            media,
            part: Some((1.0, 1.0)),
            gain_mdb: 0,
            at: AddAt::End,
        };
        let err = command(&p, adding, Time::ZERO).unwrap_err();
        assert_eq!(err.what(), "The marked part is empty");
    }
}
