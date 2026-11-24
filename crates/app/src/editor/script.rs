//! What an automated run scripts (`run_mode::Automation`): how the window
//! starts ([`settings`], [`arrange`]) and the steps it then takes by itself
//! once the project and its imports are in ([`step`], once per frame). A
//! normal run has none of it: every step finds nothing to do.

use super::Editor;
use crate::audio_page;
use crate::fake_pointer::FakePointer;
use crate::run_mode::Automation;
use crate::settings::Settings;
use project::{Command, Time};
use std::path::PathBuf;
use std::time::Duration;
use ui_kit::{Action, Page};

/// The longest a screenshot waits for imports and the viewer's picture,
/// unless `--screenshot-wait` says otherwise.
const SCREENSHOT_WAIT: Duration = Duration::from_secs(20);

/// How long a scripted pointer rests before the screenshot, so tooltips are up.
const POINTER_REST: Duration = Duration::from_millis(1500);

/// The steps still to take, each dropped once done.
#[derive(Debug, Default)]
pub(super) struct Script {
    /// `--pointer`: a scripted pointer for screenshots.
    pointer: Option<FakePointer>,
    /// The longest a screenshot waits for what it is of.
    screenshot_wait: Duration,
    /// `--action`: commands run on the first frames.
    actions: Vec<Action>,
    /// `--then`: commands run once the scripted pointer has done its part.
    then_actions: Vec<Action>,
    /// `--select-first-clip`: once there is a clip.
    select_first_clip: bool,
    /// `--sync-by-sound`: once the imports are in.
    sync_by_sound: bool,
    /// `--play`: as soon as the imports are in.
    play: bool,
    /// `--library-tab`: the tab to open once the project is in.
    library_tab: Option<String>,
    /// `--package-to`: open Package project with this folder, once.
    package_to: Option<PathBuf>,
    /// `--close-request`: ask about closing as soon as an export is unfinished.
    close_request: bool,
    /// What the command line asked of the Audio page and is still to do
    /// (its track, the demo tracks once imports are in).
    audio: audio_page::Start,
    /// The frame the `--audio-tracks` demo was made on: the responsiveness
    /// test starts counting a few frames after it, not during its set-up.
    audio_demo_made: Option<u64>,
}

impl Script {
    pub(super) fn new(automation: &Automation) -> Script {
        Script {
            pointer: automation.pointer,
            screenshot_wait: screenshot_wait(automation.screenshot_wait),
            actions: named(&automation.actions),
            then_actions: named(&automation.then_actions),
            select_first_clip: automation.select_first_clip,
            sync_by_sound: automation.sync_by_sound,
            play: automation.play,
            library_tab: automation.library_tab.clone(),
            package_to: automation.package_to.clone(),
            close_request: automation.close_request,
            audio: automation.audio,
            audio_demo_made: None,
        }
    }

    pub(super) fn pointer(&self) -> Option<&FakePointer> {
        self.pointer.as_ref()
    }

    /// The scripted pointer, if any, has done its part (and rested long
    /// enough for a tooltip to come up), `waited` into the screenshot's wait.
    pub(super) fn pointed(&self, frames: u64, waited: Duration) -> bool {
        self.pointer
            .is_none_or(|p| frames >= p.settled_at() && waited > POINTER_REST)
    }

    /// The screenshot has waited long enough: it is taken as things are.
    pub(super) fn waited_too_long(&self, waited: Duration) -> bool {
        waited > self.screenshot_wait
    }

    /// Sync by sound is still to be run (a screenshot waits for it).
    pub(super) fn syncing(&self) -> bool {
        self.sync_by_sound
    }

    /// The `--audio-tracks` demo is still to be made.
    pub(super) fn making_tracks(&self) -> bool {
        self.audio.tracks.is_some()
    }

    /// The project is being made for the test (`--audio-tracks`): set-up,
    /// not use, until a few frames after it was made.
    pub(super) fn setting_up(&self, frames: u64) -> bool {
        self.making_tracks() || self.audio_demo_made.is_some_and(|made| frames < made + 5)
    }
}

/// How long a screenshot waits: `--screenshot-wait` when it is a length a
/// [`Duration`] can hold, else the usual [`SCREENSHOT_WAIT`].
fn screenshot_wait(seconds: Option<f64>) -> Duration {
    seconds
        .and_then(|s| Duration::try_from_secs_f64(s).ok())
        .unwrap_or(SCREENSHOT_WAIT)
}

/// Commands by name; a name that is none is logged and left out.
fn named(names: &[String]) -> Vec<Action> {
    names
        .iter()
        .filter_map(|name| {
            let found = Action::from_name(name);
            if found.is_none() {
                tracing::warn!("--action {name}: no such command");
            }
            found
        })
        .collect()
}

/// The Audio page's settings the command line asks for, before the
/// settings are kept (so they are never written as changes).
pub(super) fn settings(settings: &mut Settings, audio: &audio_page::Start) {
    if let Some(band) = audio.band {
        settings.audio_band = band;
    }
    if let Some(sizes) = audio.sizes {
        settings.audio_heights = sizes;
    }
    if let Some((bottom, way)) = audio.view {
        settings.audio_bottom = bottom;
        if let Some(way) = way {
            settings.audio_split = way;
        }
    }
}

/// How the window starts: its page, drawer, full screen, the Export page's
/// demo queue, the Settings window and the shortcuts page.
pub(super) fn arrange(editor: &mut Editor, automation: &Automation) {
    let page = if automation.export_page || automation.export_queue {
        Page::Export
    } else {
        automation.page.unwrap_or_default()
    };
    editor.page = page;
    editor.shown_page = page;
    editor.drawer = crate::media_drawer::MediaDrawer::new(
        automation.media_drawer || automation.effects_drawer,
        automation.find.clone().unwrap_or_default(),
    );
    if automation.effects_drawer {
        editor.drawer.tab = ui_kit::drawer::DrawerTab::Effects;
    }
    editor.full_screen = automation.full_screen;
    editor.full_screen_changed = automation.full_screen;
    if let Some(tool) = automation.audio.tool {
        editor.audio_page.tool = tool;
    }
    if automation.export_queue {
        editor.export_page.demo_queue();
    }
    editor.shortcuts_page = automation
        .shortcuts_page
        .map(|state| ui_kit::keyboard::ShortcutsPage::preview(state, &editor.shortcuts));
    editor.show_settings = automation.settings.is_some() || automation.settings_search.is_some();
    if let Some(id) = &automation.settings
        && !editor.settings_view.open_at(super::settings_ui::CATEGORIES, id)
    {
        tracing::warn!("--settings: no tab or setting is called {id}");
    }
    if let Some(words) = &automation.settings_search {
        editor.settings_view.set_query(words.clone());
    }
}

/// The scripted steps due this frame, each taken once.
pub(super) fn step(editor: &mut Editor) {
    if editor.frames == 2 {
        for action in std::mem::take(&mut editor.script.actions) {
            editor.run_action(action);
        }
    }
    if !editor.script.then_actions.is_empty()
        && editor
            .script
            .pointer
            .is_none_or(|p| editor.frames >= p.settled_at())
    {
        for action in std::mem::take(&mut editor.script.then_actions) {
            editor.run_action(action);
        }
    }
    let imported = editor.importer.busy() == 0;
    if editor.script.select_first_clip
        && imported
        && let Some(clip) = editor
            .document
            .as_ref()
            .and_then(|d| d.project().active())
            .and_then(crate::timeline_panel::first_clip)
    {
        editor.script.select_first_clip = false;
        editor.selected = vec![clip];
    }
    if editor.script.sync_by_sound
        && imported
        && let Some(seq) = editor.document.as_ref().and_then(|d| d.project().active())
        && seq.tracks().any(|t| !t.clips.is_empty())
    {
        editor.script.sync_by_sound = false;
        editor.selected = seq.tracks().flat_map(|t| t.clips.iter().map(|c| c.id)).collect();
        editor.run_action(Action::SyncBySound);
    }
    if editor.script.play && imported && editor.document.is_some() {
        editor.script.play = false;
        editor.run_action(Action::PlayPause);
    }
    if let Some(document) = &editor.document {
        if let Some(name) = editor.script.library_tab.take()
            && !editor.library_page.open_tab_named(&name, document.project())
        {
            tracing::warn!("--library-tab {name}: no such tab");
        }
        if let Some(into) = editor.script.package_to.take() {
            editor
                .package
                .open_into(&editor.jobs, document.project(), document.path(), into);
        }
    }
    if editor.script.close_request && editor.export_page.unfinished().is_some() {
        editor.script.close_request = false;
        editor.close_prompt.ask();
    }
    if imported {
        audio_demo(editor);
    }
}

/// What the command line asked of the Audio page once the project and its
/// imports are in: many audio tracks (`--audio-tracks`, the project made
/// anew as one undoable `Restore`), the region checked for clarity
/// (`--audio-clarity-region`) and the track the tools start on
/// (`--audio-track`).
fn audio_demo(editor: &mut Editor) {
    if let Some(n) = editor.script.audio.tracks {
        let Some(p) = editor.document.as_ref().map(|d| d.project()) else {
            return;
        };
        let has_sound = p
            .active()
            .is_some_and(|s| s.audio_tracks.iter().any(|t| !t.clips.is_empty()));
        if !has_sound {
            return; // Its imports are not in yet.
        }
        editor.script.audio.tracks = None;
        editor.script.audio_demo_made = Some(editor.frames);
        if let Some(project) = audio_page::start::many_tracks(p, n) {
            if let Err(err) = editor.with_document(|d| {
                d.apply(Command::Restore {
                    project: Box::new(project),
                })
            }) {
                editor.errors.push(&err);
            }
            // The first track's noise, learned from its first two seconds,
            // so the Clean-up tool has a profile to show.
            if let Some((track, document)) = editor
                .document
                .as_ref()
                .and_then(|d| Some((d.project().active()?.audio_tracks.front()?.id, d)))
            {
                editor.audio_page.learn(
                    &editor.jobs,
                    document.project(),
                    track,
                    (Time::ZERO, Time::seconds(2)),
                    None,
                );
            }
            // Looked at from the top, fitted, as a project opened so.
            let mut fresh = ui_kit::TimelineView::default();
            fresh.zoom = editor.audio_page.timeline.zoom;
            editor.audio_page.timeline = fresh;
            editor.audio_page.fitted = false;
        }
    }
    if editor.script.audio.clarity_region && editor.script.audio.tracks.is_none() {
        editor.script.audio.clarity_region = false;
        let whole = editor.document.as_ref().and_then(|d| {
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
            && let Err(err) = editor.with_document(|d| d.apply(command))
        {
            editor.errors.push(&err);
        }
        editor.audio_page.clarity.scope = audio_page::clarity::Scope::Region;
    }
    if let Some(n) = editor.script.audio.track {
        let track = editor
            .document
            .as_ref()
            .and_then(|d| d.project().active())
            .and_then(|s| s.audio_tracks.get(n.saturating_sub(1)))
            .map(|t| t.id);
        if track.is_some() {
            editor.audio_page.track = track;
            editor.script.audio.track = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_normal_run_has_nothing_scripted() {
        let script = Script::new(&Automation::default());
        assert!(script.pointer().is_none());
        assert!(!script.syncing() && !script.making_tracks() && !script.setting_up(0));
        assert!(script.pointed(0, Duration::ZERO), "no pointer to wait for");
        assert!(!script.waited_too_long(Duration::from_secs(19)));
        assert!(script.waited_too_long(Duration::from_secs(21)));
    }

    #[test]
    fn the_script_takes_what_the_command_line_asked_for() {
        let automation = Automation {
            actions: vec!["Templates".into(), "NoSuchCommand".into()],
            then_actions: vec!["Copy".into()],
            sync_by_sound: true,
            screenshot_wait: Some(60.0),
            audio: audio_page::Start {
                tracks: Some(12),
                ..Default::default()
            },
            ..Automation::default()
        };
        let script = Script::new(&automation);
        assert_eq!(script.actions, [Action::Templates], "an unknown name is left out");
        assert_eq!(script.then_actions, [Action::Copy]);
        assert!(script.syncing() && script.making_tracks() && script.setting_up(100));
        assert!(!script.waited_too_long(Duration::from_secs(59)));
    }

    #[test]
    fn a_screenshot_wait_too_long_to_hold_falls_back_to_the_usual_one() {
        assert_eq!(screenshot_wait(None), SCREENSHOT_WAIT);
        assert_eq!(screenshot_wait(Some(2.5)), Duration::from_millis(2500));
        assert_eq!(screenshot_wait(Some(1e300)), SCREENSHOT_WAIT);
        assert_eq!(screenshot_wait(Some(f64::MAX)), SCREENSHOT_WAIT);
    }

    #[test]
    fn the_audio_settings_asked_for_replace_the_kept_ones() {
        let mut kept = Settings::default();
        let band = audio_page::band::View::from_name("stereo");
        settings(
            &mut kept,
            &audio_page::Start {
                band,
                ..Default::default()
            },
        );
        assert_eq!(Some(kept.audio_band), band);
    }
}
