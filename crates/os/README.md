# os

Small questions to the operating system that the interface needs and the windowing crate does not answer: which keyboard layout is in use, and lowering a worker thread's priority below the interface thread's. Also the processor setting the sound card's thread needs, and, for tests, an allocator that counts.

This is the one crate besides the FFmpeg and GPU wrappers where `unsafe` may appear (working rule 6), and only for foreign calls into the system, each block with a `SAFETY:` comment saying why it is sound. Answers are in the system's own terms (`SystemLayout`); what they mean for the drawn keyboard is `ui_kit::keyboard::SystemKeyboard`'s job.

## Files

| File | What it holds |
|---|---|
| `src/lib.rs` | `SystemLayout` (a Windows language identifier, or an XKB layout name), `keyboard_layout_now` (ask once, may wait: for the command line) and `KeyboardWatch` (follow it while a window is open: `poll` never waits, so it runs every frame; `layout` is the latest answer). `ASK_EVERY` (500 ms) is how soon a switch shows |
| `src/windows.rs` | Windows: `GetKeyboardLayout` for the calling thread (the one foreign call, `unsafe`); the handle's low word is the language. The watch asks on the window's thread at most every `ASK_EVERY`; the call only reads the thread's state |
| `src/x11.rs` | Linux: the X server's keyboard extension over the X protocol in plain Rust (no `unsafe`): the active group from the keyboard's state, the symbols description ("pc+no+us:2+inet(evdev)") and that group's layout. Asking is a round trip to the server, so the watch keeps a thread of its own that asks every `ASK_EVERY` and sends what changed; it ends when the watch is dropped. Without an X server (no `DISPLAY`) it says once what the settings say |
| `src/thread_priority.rs` | `lower_this_thread`: the calling thread below normal priority for the rest of its life (Windows `THREAD_PRIORITY_BELOW_NORMAL`; Linux 10 nice steps lower with `setpriority`, which threads it starts inherit), false when the system refuses. `this_thread_nice` (Linux) reads it back for checks. The `jobs` crate lowers its analysis workers with it |
| `src/float.rs` | `denormals_as_zero`: makes the calling thread treat very small numbers as zero (the x86 control register's flush-to-zero and denormals-are-zero bits; the one `unsafe` call, deprecated intrinsics allowed). Filters and envelopes dying away otherwise fall into slow arithmetic and a block can take ten times as long; the sound card's callback calls it at its start. Elsewhere it does nothing and says so |
| `src/alloc_count.rs` | `Counting`, a global allocator that hands everything to the system's and counts what a thread asks for while marked (`counted`), for tests that prove the mixer never allocates or frees (`audio/tests/no_alloc.rs`). The app never installs it |
| `src/xkb.rs` | Plain text handling, tested on every system: a group's layout from the symbols description, the first of a layout list, `XKBLAYOUT=` in a settings file, and `from_settings` (`XKB_DEFAULT_LAYOUT`, then `/etc/default/keyboard`, then `/etc/vconsole.conf`) |

## What it costs

- `KeyboardWatch::poll` on the interface thread: about 10 ns on Linux (it only looks for a message from the thread). On Windows it calls `GetKeyboardLayout` at most twice a second, a call that reads the thread's own state.
- On Linux the thread's three requests to the X server took about 15 µs (median of 500, a virtual screen on a busy two-core machine; the slowest 4 ms), off the interface thread. Connecting took 0.2 ms, once.
- Lowering the analysis workers, measured at the phase 5 gate (two cores, software graphics, 500 files imported on the Edit page): interface time for 99 frames in 100 went from 22.8 ms to 11.6 ms and frames from 16 to 26 a second, with the interface's own work unchanged (about 5 ms of CPU for 99 frames in 100).

## Limits

- Linux follows layout switches only through the X server: desktops whose X support mirrors the active layout (most do) and X sessions. Without an X server the layout comes from the environment or the settings files once and does not follow switches.
- The XKB variant ("nodeadkeys") is left off: the drawing knows layouts, not variants.
