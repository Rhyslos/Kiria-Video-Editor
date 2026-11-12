//! What the mouse does, for the Mouse section of the keyboard shortcuts
//! page and the cheat sheet. Shown, not changeable: the widgets read the
//! mouse themselves, so a line here describes one of them and is updated
//! with it.

use super::Context;

/// One thing the mouse does somewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseAction {
    /// What the hand does, for example "Ctrl+wheel".
    pub gesture: &'static str,
    /// What happens.
    pub does: &'static str,
    pub context: Context,
}

const fn on(context: Context, gesture: &'static str, does: &'static str) -> MouseAction {
    MouseAction {
        gesture,
        does,
        context,
    }
}

/// Every mouse action, grouped by context.
pub const MOUSE: &[MouseAction] = &[
    on(
        Context::Anywhere,
        "Drag the handle between the viewer and the timeline",
        "Share the height between them; double-click for the usual split",
    ),
    on(
        Context::Viewer,
        "Click the buttons over the picture",
        "Play or pause, step one frame back or forward, stop",
    ),
    on(
        Context::Viewer,
        "Drag the selected clip's picture",
        "Move it in the frame, with handles on the picture switched on (Q); it snaps to the frame's centre and edges",
    ),
    on(
        Context::Viewer,
        "Drag a corner of the selected clip's picture",
        "Resize it, the opposite corner staying where it is (handles on)",
    ),
    on(
        Context::Viewer,
        "Double-click the selected clip's picture",
        "Fit it: as large as fits the frame, centred (handles on)",
    ),
    on(
        Context::Viewer,
        "Click the diamond beside a value in the inspector",
        "Add a keyframe for it at the playhead, or remove the one there; the arrows beside it move the playhead to its previous or next keyframe",
    ),
    on(
        Context::Viewer,
        "Right-click a filled diamond in the inspector",
        "Choose how the value goes on to the next keyframe: linear, hold or smooth",
    ),
    on(
        Context::Viewer,
        "Double-click the picture in the viewer's own window",
        "Fill that screen with it, or give the screen back",
    ),
    on(
        Context::Timeline,
        "Wheel",
        "Scroll the tracks under the pointer (all of them with the divider off); pan over the ruler and the scroll bar",
    ),
    on(
        Context::Timeline,
        "Ctrl+wheel, or a pinch",
        "Zoom in or out at the pointer",
    ),
    on(
        Context::Timeline,
        "Shift+wheel, or a sideways wheel",
        "Pan along the timeline",
    ),
    on(
        Context::Timeline,
        "Alt+wheel",
        "Taller or shorter tracks in the area under the pointer (not with compact tracks)",
    ),
    on(
        Context::Timeline,
        "Drag with the middle button",
        "Pan along the timeline and scroll the tracks",
    ),
    on(
        Context::Timeline,
        "Click or drag in the ruler",
        "Move the playhead; or the edit needle, when the Display menu says clicking the timeline moves it",
    ),
    on(
        Context::Timeline,
        "Drag the playhead's handle at the top of the ruler",
        "Move the playhead, whatever a click moves",
    ),
    on(Context::Timeline, "Shift+drag in the ruler", "Mark a region"),
    on(
        Context::Timeline,
        "Drag the red handle in the ruler",
        "Move the edit needle",
    ),
    on(
        Context::Timeline,
        "Click a label",
        "Go to it; double-click to change it",
    ),
    on(
        Context::Timeline,
        "Click a clip",
        "Select it; with Shift or Ctrl, add it to the selection",
    ),
    on(Context::Timeline, "Click an empty place", "Select nothing"),
    on(
        Context::Timeline,
        "Click a diamond along the selected clip's bottom edge",
        "Move the playhead to that keyframe",
    ),
    on(
        Context::Timeline,
        "Drag a clip, or its edge",
        "Move it, or trim it (what a drag does follows the tool)",
    ),
    on(
        Context::Timeline,
        "Drag a clip up or down",
        "Move it to another track of its kind; its sound finds a free track",
    ),
    on(
        Context::Timeline,
        "Drag a clip above the top video track, or under the bottom audio track",
        "Make a new track there for it",
    ),
    on(
        Context::Timeline,
        "Right-click",
        "The menu for that clip or place",
    ),
    on(
        Context::Timeline,
        "Right-click a track's name",
        "Add a track; rename, hide or remove that one",
    ),
    on(
        Context::Timeline,
        "Double-click a track's name",
        "Type a new name for it",
    ),
    on(
        Context::Timeline,
        "Click the eye by a video track's name",
        "Hide the track from the picture, or show it again",
    ),
    on(
        Context::Timeline,
        "Click the level button by an audio track's name",
        "Show or hide a line across each of its clips at the clip's volume",
    ),
    on(
        Context::Timeline,
        "Drag a clip's level line up or down",
        "That clip's volume: +12 dB at the top, Off at the bottom; double-click it for 0 dB",
    ),
    on(
        Context::Timeline,
        "Drag a file from the media drawer onto a track",
        "Lay it there: its picture on the video track under the pointer, its sound where there is room",
    ),
    on(
        Context::Timeline,
        "Drag the edge of a track's name",
        "That track's height; double-click for the usual height (not with compact tracks)",
    ),
    on(
        Context::Timeline,
        "Drag the line between video and audio",
        "Share the timeline's height between them; the video area still grows for a track that is added (the Display menu can take the line away)",
    ),
    on(
        Context::Timeline,
        "Drag a volume fader",
        "That track's volume, or the mix's; double-click for 0 dB",
    ),
    on(
        Context::Dragging,
        "Hold Alt while dragging a clip",
        "No snapping for this drag",
    ),
    on(
        Context::Dragging,
        "Hold Alt while dragging a picture or its corner in the viewer",
        "No snapping for this drag",
    ),
    on(
        Context::Dragging,
        "Hold Alt while dragging a level line",
        "Change the volume more finely",
    ),
    on(
        Context::Dragging,
        "Hold Ctrl while dropping a file on the timeline",
        "Insert it, pushing later clips along, instead of overwriting",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_line_says_something_and_none_is_there_twice() {
        for (i, a) in MOUSE.iter().enumerate() {
            assert!(!a.gesture.is_empty() && !a.does.is_empty());
            assert!(!a.does.ends_with('.'), "no full stops, like the commands");
            for b in &MOUSE[i + 1..] {
                assert!(
                    a.gesture != b.gesture || a.context != b.context,
                    "{} is there twice",
                    a.gesture
                );
            }
        }
    }

    #[test]
    fn lines_of_a_context_stay_together() {
        // The page and the sheet print them in this order under one heading each.
        let mut seen = Vec::new();
        for action in MOUSE {
            if seen.last() != Some(&action.context) {
                assert!(
                    !seen.contains(&action.context),
                    "{:?} is split up",
                    action.context
                );
                seen.push(action.context);
            }
        }
        assert!(seen.contains(&Context::Dragging));
    }
}
