//! The waveforms the timeline draws: read once per file and sound stream on
//! a worker, then kept for the session.

use cache::Peaks;
use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use project::{MediaId, MediaRef};
use std::collections::HashMap;

enum State {
    Reading(JobHandle<Peaks>),
    Ready(Peaks),
    /// Couldn't be read (missing file, broken sound): drawn without a waveform.
    Failed,
}

#[derive(Default)]
pub struct Waveforms {
    streams: HashMap<(MediaId, u32), State>,
}

impl Waveforms {
    /// The peaks of this stream if they are ready; starts reading them otherwise.
    pub fn get(
        &mut self,
        jobs: &JobSystem,
        ctx: &egui::Context,
        media: &MediaRef,
        stream: u32,
    ) -> Option<&Peaks> {
        let key = (media.id, stream);
        let state = self.streams.entry(key).or_insert_with(|| {
            let (path, ctx) = (media.path.clone(), ctx.clone());
            State::Reading(
                jobs.submit(PoolKind::Analysis, Priority::Background, "waveform", move |_| {
                    let result = cache::waveform::peaks(&path, stream);
                    ctx.request_repaint();
                    result
                }),
            )
        });
        match state {
            State::Ready(p) => Some(p),
            _ => None,
        }
    }

    /// Collects finished readings.
    pub fn poll(&mut self) {
        for state in self.streams.values_mut() {
            if let State::Reading(handle) = state
                && let Some(result) = handle.try_result()
            {
                *state = match result {
                    Ok(p) => State::Ready(p),
                    Err(err) => {
                        tracing::info!("no waveform: {}", err.report());
                        State::Failed
                    }
                };
            }
        }
    }
}
