//! Every command a shortcut can trigger, with its name and the context it
//! belongs to.

use egui::Color32;
use serde::{Deserialize, Serialize};

/// Where a command acts. The shortcuts page colours and filters by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Context {
    /// Works wherever you are in the window.
    Anywhere,
    /// The picture and its playback.
    Viewer,
    /// Clips, tracks and the timeline's view.
    Timeline,
    /// Keys held while something is being dragged (mouse actions only).
    Dragging,
}

impl Context {
    pub const ALL: [Context; 4] = [
        Context::Anywhere,
        Context::Viewer,
        Context::Timeline,
        Context::Dragging,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Context::Anywhere => "Anywhere",
            Context::Viewer => "Viewer",
            Context::Timeline => "Timeline",
            Context::Dragging => "While dragging",
        }
    }

    /// The context's colour: its chip, and its dots on the keys.
    pub fn colour(self) -> Color32 {
        use crate::theme::context;
        match self {
            Context::Anywhere => context::ANYWHERE,
            Context::Viewer => context::VIEWER,
            Context::Timeline => context::TIMELINE,
            Context::Dragging => context::DRAGGING,
        }
    }
}

/// One list for the commands: the enum, its order, each command's context
/// and its name, so none can be added without all three.
macro_rules! actions {
    ($($name:ident, $context:ident, $label:literal;)*) => {
        /// Everything a shortcut can trigger.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum Action {
            $($name,)*
        }

        impl Action {
            /// Every command, in the order the shortcuts page lists them.
            pub const ALL: &'static [Action] = &[$(Action::$name,)*];

            pub fn label(self) -> &'static str {
                match self {
                    $(Action::$name => $label,)*
                }
            }

            pub fn context(self) -> Context {
                match self {
                    $(Action::$name => Context::$context,)*
                }
            }

            /// The name settings and `--action` know the command by.
            pub fn name(self) -> &'static str {
                match self {
                    $(Action::$name => stringify!($name),)*
                }
            }
        }
    };
}

actions! {
    Undo, Anywhere, "Undo";
    Redo, Anywhere, "Redo";
    Save, Anywhere, "Save";
    Search, Anywhere, "Search";
    OpenSettings, Anywhere, "Settings";
    KeyboardShortcuts, Anywhere, "Keyboard shortcuts";
    Import, Anywhere, "Import media";
    Versions, Anywhere, "Saved versions: keep this version, or go back to one…";
    Templates, Anywhere, "Templates: drop in an intro or outro…";
    Prune, Anywhere, "Prune unused media: take the files no timeline uses out of the project…";
    PackageProject, Anywhere, "Package project: copy it and the media it uses into one folder, to open on another machine…";
    SaveTemplate, Anywhere, "Save as a template: the region, or the whole timeline…";
    Copy, Anywhere, "Copy: the chosen keyframe marks on the timeline";
    Paste, Anywhere, "Paste: copied keyframes at the playhead, on the selected clip";
    PlayPause, Viewer, "Play / pause";
    Stop, Viewer, "Stop playing here";
    PreviousFrame, Viewer, "Back one frame";
    NextFrame, Viewer, "Forward one frame";
    GoToStart, Viewer, "Go to the start";
    GoToEnd, Viewer, "Go to the end";
    PreviousKeyframe, Viewer, "Back to the selected clip's previous keyframe";
    NextKeyframe, Viewer, "On to the selected clip's next keyframe";
    ToggleLoop, Viewer, "Loop playback inside the region";
    MarkIn, Viewer, "Mark in: where the region begins (Edit page), or where the part of a file to add begins (Library)";
    MarkOut, Viewer, "Mark out: where the region ends (Edit page), or where the part of a file to add ends (Library)";
    FullScreenViewer, Viewer, "Viewer full screen, and back";
    PictureHandles, Viewer, "Move, resize, crop and turn the selected clip on the picture: handles on or off";
    SplitView, Viewer, "Split view: the selected clip's own picture beside the timeline's";
    ViewerWindow, Viewer, "Viewer in its own window, for another screen or monitor";
    SaveFrame, Viewer, "Save this frame as a picture…";
    ViewerZoomFit, Viewer, "Fit the picture in the viewer";
    ViewerZoom100, Viewer, "Picture at 100 %: one pixel of the timeline on one of the screen";
    ViewerZoomIn, Viewer, "Zoom in on the picture";
    ViewerZoomOut, Viewer, "Zoom out of the picture";
    Split, Timeline, "Split at the edit needle";
    Delete, Timeline, "Delete the selected clip";
    RippleDelete, Timeline, "Delete and close the gap";
    AddLabel, Timeline, "Add a timeline label";
    ClearSelection, Timeline, "Select nothing (and leave full screen)";
    ClearRegion, Timeline, "Clear the region";
    RemoveSilence, Timeline, "Remove silence…";
    SelectTool, Timeline, "Select tool: pick, move and trim clips";
    BladeTool, Timeline, "Blade tool: cut clips where clicked";
    RippleTool, Timeline, "Ripple tool: trims move everything after";
    RollTool, Timeline, "Roll tool: move the cut between two clips";
    SlipTool, Timeline, "Slip tool: show another part of a clip";
    SlideTool, Timeline, "Slide tool: move a clip between its neighbours";
    ToggleSnapping, Timeline, "Snapping on or off";
    ZoomToFit, Timeline, "Show the whole timeline";
    ZoomIn, Timeline, "Zoom in on the timeline";
    ZoomOut, Timeline, "Zoom out of the timeline";
    TallerTracks, Timeline, "Taller tracks (the area under the pointer, or all)";
    ShorterTracks, Timeline, "Shorter tracks (the area under the pointer, or all)";
    UsualTrackHeights, Timeline, "Tracks back to their usual height";
    RoughCut, Timeline, "Timeline view: rough cut or full";
    CompactTracks, Timeline, "Compact tracks: every track a thin strip, to see many at a glance";
    TrackDivider, Timeline, "Divider between video and audio tracks, or one list that scrolls together";
    ClickMovesNeedle, Timeline, "Clicking the timeline moves the edit needle (the playhead by its handle), or the playhead";
    AddVideoTrack, Timeline, "Add a video track (on top)";
    AddAudioTrack, Timeline, "Add an audio track (at the bottom)";
    NewTimeline, Timeline, "New timeline, the same size and rate as the open one";
    DuplicateTimeline, Timeline, "Duplicate the open timeline";
    NestSelection, Timeline, "Nest the selected clips: move them into a timeline of their own";
    SyncBySound, Timeline, "Sync by sound: line up the selected clips by what they hear…";
    AddAdjustmentLayer, Timeline, "Add an adjustment layer at the playhead: its colour changes everything below it";
    VolumeLines, Timeline, "Volume and pan lines: draw them on audio tracks and clips, or work clips again";
}

impl Action {
    /// The command called `name` (see [`Action::name`]).
    pub fn from_name(name: &str) -> Option<Action> {
        Action::ALL.iter().copied().find(|a| a.name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_found_by_their_names() {
        for action in Action::ALL {
            assert_eq!(Action::from_name(action.name()), Some(*action));
        }
        assert_eq!(Action::Split.name(), "Split");
        assert_eq!(Action::from_name("Nothing"), None);
    }

    #[test]
    fn every_command_has_a_name_people_can_read_and_no_two_share_one() {
        for (i, a) in Action::ALL.iter().enumerate() {
            assert!(!a.label().is_empty());
            for b in &Action::ALL[i + 1..] {
                assert_ne!(a.label(), b.label());
            }
        }
    }

    #[test]
    fn commands_only_use_the_contexts_keys_can_have() {
        // "While dragging" is for the mouse section: no key lives there.
        assert!(Action::ALL.iter().all(|a| a.context() != Context::Dragging));
        for context in [Context::Anywhere, Context::Viewer, Context::Timeline] {
            assert!(Action::ALL.iter().any(|a| a.context() == context));
        }
    }

    #[test]
    fn contexts_can_be_told_apart() {
        for (i, a) in Context::ALL.iter().enumerate() {
            for b in &Context::ALL[i + 1..] {
                assert_ne!(a.colour(), b.colour());
                assert_ne!(a.label(), b.label());
            }
        }
    }
}
