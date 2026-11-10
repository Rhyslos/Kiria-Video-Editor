# project

The project model, edit commands, undo/redo and autosave.

Projects, sequences, tracks, clips, media references, timeline labels, the region and the edit needle. Time is stored as exact fractions, never floats. Every change is a command applied to an immutable snapshot, which makes undo, autosave and background rendering safe. The project file is versioned from the first commit, with tested migrations. Built in Phase 0.3 and 0.4.

## Status

Placeholder: nothing here yet.
