//! The index against projects built in code: what each query finds, that
//! syncs follow edits, and the phase 5 gate: with 10 000 files, filters
//! and search answer in under 100 ms.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use library::{Index, Live, Query, Sort, Type, Usage};
use project::{
    Command, Fingerprint, LabelColor, MediaId, MediaInfo, MediaKind, MediaLabel, MediaLabelId, MediaRef,
    Project, Time,
};
use std::time::{Duration, Instant};

fn file(name: &str, kind: MediaKind, secs: i64) -> MediaRef {
    MediaRef {
        id: MediaId(0),
        path: format!("recordings/{name}").into(),
        fingerprint: Fingerprint {
            size: name.len() as u64,
            sample_hash: name.into(),
        },
        kind,
        duration: Time::seconds(secs),
        info: MediaInfo {
            container: String::new(),
            video: (kind != MediaKind::Audio).then(|| project::VideoStream {
                index: 0,
                codec: "h264".into(),
                width: 1920,
                height: 1080,
                frame_rate: project::Rate::FPS_60,
                variable_frame_rate: false,
                rotation: 0,
            }),
            audio: Vec::new(),
            stated_length: None,
        },
    }
}

fn add(p: &Project, media: MediaRef) -> (Project, MediaId) {
    let p = Command::AddMedia { media }.apply(p).unwrap();
    let id = *p.media.keys().next_back().unwrap();
    (p, id)
}

fn label(p: &Project, name: &str, media: Vec<MediaId>) -> (Project, MediaLabelId) {
    let p = Command::AddMediaLabel {
        label: MediaLabel {
            id: MediaLabelId(0),
            name: name.into(),
            color: LabelColor::Blue,
        },
        media,
    }
    .apply(p)
    .unwrap();
    let id = p.library.labels.last().unwrap().id;
    (p, id)
}

struct Sample {
    project: Project,
    drone_clip: MediaId,
    interview: MediaId,
    music: MediaId,
    still: MediaId,
    drone: MediaLabelId,
}

fn sample() -> Sample {
    let p = Project::new_automatic("Trip");
    let (p, drone_clip) = add(&p, file("DJI_0042.mp4", MediaKind::Video, 60));
    let (p, interview) = add(&p, file("Interview_Anna.mkv", MediaKind::Video, 600));
    let (p, music) = add(&p, file("theme.mp3", MediaKind::Audio, 180));
    let (p, still) = add(&p, file("map.png", MediaKind::Image, 0));
    let (p, drone) = label(&p, "Drone", vec![drone_clip]);
    let p = Command::SetMediaNote {
        media: interview,
        note: "She laughs at 3:10, use it for the ending".into(),
    }
    .apply(&p)
    .unwrap();
    let sequence = p.active_sequence;
    let p = Command::PlaceMedia {
        sequence,
        media: interview,
    }
    .apply(&p)
    .unwrap();
    Sample {
        project: p,
        drone_clip,
        interview,
        music,
        still,
        drone,
    }
}

fn synced(p: &Project) -> Index {
    let mut index = Index::new().unwrap();
    index.sync(p).unwrap();
    index
}

#[test]
fn words_match_names_labels_and_notes_anywhere_in_any_case() {
    let s = sample();
    let index = synced(&s.project);
    let find = |text: &str| index.search(&Query::text(text)).unwrap();
    assert_eq!(find("dji"), [s.drone_clip], "part of a name");
    assert_eq!(find("0042"), [s.drone_clip], "digits inside a name");
    assert_eq!(find("DRONE"), [s.drone_clip], "a label's name, any case");
    assert_eq!(find("laughs ending"), [s.interview], "every word of a note");
    assert_eq!(
        find("laughs drone"),
        Vec::<MediaId>::new(),
        "every word must match"
    );
    assert_eq!(find("ma"), [s.still], "short words match too");
    assert_eq!(
        find("\"*)("),
        Vec::<MediaId>::new(),
        "search syntax is read as text"
    );
    assert_eq!(find(""), [s.drone_clip, s.interview, s.still, s.music], "by name");
}

#[test]
fn filters_by_type_label_list_and_use_and_sorts() {
    let s = sample();
    let p = Command::AddMediaList {
        name: "Ending".into(),
        media: vec![s.music, s.interview],
    }
    .apply(&s.project)
    .unwrap();
    let list = p.library.lists[0].id;
    let index = synced(&p);
    let find = |q: Query| index.search(&q).unwrap();
    assert_eq!(
        find(Query {
            types: vec![Type::Audio, Type::Image],
            ..Query::default()
        }),
        [s.still, s.music]
    );
    assert_eq!(
        find(Query {
            labels: vec![s.drone],
            ..Query::default()
        }),
        [s.drone_clip]
    );
    assert_eq!(
        find(Query {
            list: Some(list),
            sort: Sort::ListOrder,
            ..Query::default()
        }),
        [s.music, s.interview],
        "a list keeps its own order"
    );
    assert_eq!(
        find(Query {
            list: Some(list),
            ..Query::default()
        }),
        [s.interview, s.music]
    );
    assert_eq!(
        find(Query {
            usage: Usage::Used,
            ..Query::default()
        }),
        [s.interview]
    );
    assert_eq!(
        find(Query {
            usage: Usage::Unused,
            sort: Sort::Longest,
            ..Query::default()
        }),
        [s.music, s.drone_clip, s.still]
    );
    assert_eq!(
        find(Query {
            sort: Sort::Newest,
            ..Query::default()
        }),
        [s.still, s.music, s.interview, s.drone_clip]
    );
    assert_eq!(
        find(Query {
            sort: Sort::Type,
            ..Query::default()
        }),
        [s.drone_clip, s.interview, s.music, s.still]
    );
}

#[test]
fn syncs_follow_edits_and_undo() {
    let s = sample();
    let mut index = synced(&s.project);
    let renamed = Command::EditMediaLabel {
        label: s.drone,
        name: "Aerial".into(),
        color: LabelColor::Green,
    }
    .apply(&s.project)
    .unwrap();
    let done = index.sync(&renamed).unwrap();
    assert_eq!(
        (done.added, done.changed, done.removed),
        (0, 1, 0),
        "only the labelled file is rewritten"
    );
    assert_eq!(index.search(&Query::text("aerial")).unwrap(), [s.drone_clip]);
    assert!(index.search(&Query::text("drone")).unwrap().is_empty());
    // Undo is the earlier snapshot synced again.
    index.sync(&s.project).unwrap();
    assert_eq!(index.search(&Query::text("drone")).unwrap(), [s.drone_clip]);
    let usage = index.usage(s.interview);
    assert_eq!(usage.len(), 1);
    assert!(usage[0].1 >= 1, "the placed file is used");
    let same = index
        .with_fingerprint(&s.project.media[&s.music].fingerprint)
        .unwrap();
    assert_eq!(same, [s.music]);
    let none = index.sync(&s.project).unwrap();
    assert_eq!((none.added, none.changed, none.removed), (0, 0, 0));
}

#[test]
fn the_worker_answers_without_being_waited_for() {
    let s = sample();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut live = Live::start(move || {
        let _ = tx.send(());
    });
    live.follow(&s.project);
    live.ask("drawer", &Query::text("drone"));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !live.is_current("drawer") {
        assert!(Instant::now() < deadline, "the worker answers");
        let _ = rx.recv_timeout(Duration::from_millis(100));
    }
    assert_eq!(live.answer("drawer").unwrap().media, [s.drone_clip]);
    // An edit is followed, and the query answered again.
    let tagged = Command::SetMediaLabel {
        media: vec![s.music],
        label: s.drone,
        on: true,
    }
    .apply(&s.project)
    .unwrap();
    live.follow(&tagged);
    while !live.is_current("drawer") {
        assert!(Instant::now() < deadline, "the worker answers again");
        let _ = rx.recv_timeout(Duration::from_millis(100));
    }
    assert_eq!(live.answer("drawer").unwrap().media, [s.drone_clip, s.music]);
    assert!(live.take_error().is_none());
}

/// The phase 5 gate, in code: 10 000 files with labels, lists and notes;
/// every kind of query answers in under 100 ms (and the build itself is
/// reported). In practice they take a few milliseconds.
#[test]
fn ten_thousand_files_answer_in_under_100_ms() {
    let mut p = Project::new_automatic("Big");
    let words = [
        "match",
        "drone",
        "interview",
        "broll",
        "voice",
        "sunset",
        "boss",
        "replay",
    ];
    let kinds = [
        MediaKind::Video,
        MediaKind::Video,
        MediaKind::Audio,
        MediaKind::Image,
    ];
    for i in 0..10_000u64 {
        let id: MediaId = p.allocate_id();
        let name = format!("{}_{i:05}.mkv", words[i as usize % words.len()]);
        p.media.insert(
            id,
            MediaRef {
                id,
                ..file(&name, kinds[i as usize % kinds.len()], (i % 900) as i64)
            },
        );
    }
    let ids: Vec<MediaId> = p.media.keys().copied().collect();
    let (q, a) = label(&p, "Highlight", ids.iter().step_by(7).copied().collect());
    let (q, b) = label(&q, "Keep", ids.iter().step_by(3).copied().collect());
    let mut p = Command::AddMediaList {
        name: "Best of".into(),
        media: ids.iter().step_by(11).copied().collect(),
    }
    .apply(&q)
    .unwrap();
    for m in ids.iter().step_by(13) {
        p.library.notes.insert(*m, format!("great moment number {}", m.0));
    }
    assert!(p.problems().is_empty());
    let list = p.library.lists[0].id;

    let mut index = Index::new().unwrap();
    let started = Instant::now();
    index.sync(&p).unwrap();
    let built = started.elapsed();
    assert_eq!(index.len(), 10_000);
    eprintln!("10 000 files indexed in {built:?}");

    let queries = [
        Query::default(),
        Query::text("drone"),
        Query::text("sun 0042"),
        Query::text("great moment"),
        Query::text("highlight"),
        Query {
            types: vec![Type::Audio],
            sort: Sort::Longest,
            ..Query::default()
        },
        Query {
            labels: vec![a, b],
            ..Query::default()
        },
        Query {
            list: Some(list),
            sort: Sort::ListOrder,
            ..Query::default()
        },
        Query {
            text: "match".into(),
            types: vec![Type::Video],
            labels: vec![b],
            usage: Usage::Unused,
            sort: Sort::Newest,
            ..Query::default()
        },
    ];
    for query in &queries {
        let started = Instant::now();
        let found = index.search(query).unwrap();
        let took = started.elapsed();
        eprintln!("{query:?}: {} found in {took:?}", found.len());
        assert!(took < Duration::from_millis(100), "{query:?} took {took:?}");
    }
    assert_eq!(index.search(&Query::text("drone")).unwrap().len(), 1_250);
    assert_eq!(
        index
            .search(&Query {
                labels: vec![a, b],
                ..Query::default()
            })
            .unwrap()
            .len(),
        ids.iter().enumerate().filter(|(i, _)| i % 21 == 0).count()
    );
    // A small edit afterwards is a small write.
    let one = Command::SetMediaNote {
        media: ids[5],
        note: "renamed".into(),
    }
    .apply(&p)
    .unwrap();
    let started = Instant::now();
    let done = index.sync(&one).unwrap();
    eprintln!("one changed file synced in {:?}", started.elapsed());
    assert_eq!((done.added, done.changed, done.removed), (0, 1, 0));
}

/// The phase 5 gate on a real project: `GATE_PROJECT=<project file> cargo
/// test -p library --test index -- --ignored --nocapture`. Loads the project
/// (one with hundreds of imported files), indexes it and times each kind
/// of query, both straight on the index and asked of the worker the pages
/// ask (`Live`), from asking to the answer being there.
#[test]
#[ignore = "needs GATE_PROJECT: a project file with imported media"]
fn a_real_project_answers_in_under_100_ms() {
    let Some(path) = std::env::var_os("GATE_PROJECT") else {
        eprintln!("GATE_PROJECT not set: nothing to measure");
        return;
    };
    let p = project::file::load(std::path::Path::new(&path)).unwrap();
    let mut index = Index::new().unwrap();
    let started = Instant::now();
    index.sync(&p).unwrap();
    eprintln!("{} files indexed in {:?}", index.len(), started.elapsed());

    let queries = [
        Query::default(),
        Query::text("h264"),
        Query::text("tone 1"),
        Query::text("o"),
        Query {
            types: vec![Type::Audio],
            sort: Sort::Longest,
            ..Query::default()
        },
        Query {
            types: vec![Type::Image],
            sort: Sort::Name,
            ..Query::default()
        },
        Query {
            types: vec![Type::Video],
            usage: Usage::Used,
            sort: Sort::MostUsed,
            ..Query::default()
        },
        Query {
            text: "5".into(),
            usage: Usage::Unused,
            sort: Sort::Newest,
            ..Query::default()
        },
    ];
    let mut live = Live::start(|| {});
    live.follow(&p);
    // The worker indexes the project first, as when a project opens.
    let opened = Instant::now();
    live.ask("gate", &Query::default());
    while !live.is_current("gate") {
        std::thread::sleep(Duration::from_micros(200));
    }
    eprintln!(
        "the worker's first answer, indexing included: {:?}",
        opened.elapsed()
    );
    for query in &queries {
        let started = Instant::now();
        let found = index.search(query).unwrap();
        let took = started.elapsed();
        let asked = Instant::now();
        live.ask("gate", query);
        while !live.is_current("gate") {
            std::thread::sleep(Duration::from_micros(200));
        }
        let answered = asked.elapsed();
        eprintln!(
            "{query:?}: {} found in {took:?}; asked of the worker: {answered:?}",
            found.len()
        );
        assert!(took < Duration::from_millis(100), "{query:?} took {took:?}");
        assert!(
            answered < Duration::from_millis(100),
            "{query:?} answered in {answered:?}"
        );
    }
    assert!(live.take_error().is_none());
}
