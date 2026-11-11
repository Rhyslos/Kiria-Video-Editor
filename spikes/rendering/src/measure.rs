//! What putting one picture together costs, each way.

use crate::compose::{Compositor, Place, Source, Targets, Way, window};
use crate::gpu::{Gpu, Stamps};
use std::time::Instant;

/// Milliseconds for one picture, medians over the run.
pub struct Cost {
    /// On the thread that asks: building the commands and handing them over.
    pub thread: f64,
    /// On the card, by its own clock; nothing when it has none.
    pub card: Option<f64>,
    /// From asking to finished, one picture at a time.
    pub alone: f64,
    /// Per picture when they are handed over back to back.
    pub back_to_back: f64,
    /// The very first picture, with whatever is set up on first use.
    pub first: f64,
}

pub fn cost(
    gpu: &Gpu,
    compositor: &Compositor,
    sources: &[Source],
    frame: (u32, u32),
    way: Way,
    pictures: u32,
) -> Result<Cost, String> {
    let targets = Targets::new(gpu, compositor, way, frame);
    let window = window(gpu, frame).create_view(&wgpu::TextureViewDescriptor::default());
    let places = Place::standard(sources.len());
    let layers: Vec<(&Source, Place)> = sources.iter().zip(places).collect();
    gpu.wait()?;
    let one = |stamps: Option<(&Stamps, u32)>| {
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        compositor.draw(gpu, &mut encoder, way, &targets, &layers, Some(&window), stamps);
        gpu.queue.submit([encoder.finish()]);
    };

    let start = Instant::now();
    one(None);
    gpu.wait()?;
    let first = ms(start);
    for _ in 0..2 {
        one(None);
    }
    gpu.wait()?;

    let stamps = Stamps::new(gpu, pictures);
    let (mut thread, mut alone) = (Vec::new(), Vec::new());
    for picture in 0..pictures {
        let start = Instant::now();
        one(stamps.as_ref().map(|stamps| (stamps, picture)));
        thread.push(ms(start));
        gpu.wait()?;
        alone.push(ms(start));
    }
    let card = match &stamps {
        Some(stamps) => Some(median(stamps.read(gpu)?)),
        None => None,
    };

    let start = Instant::now();
    for _ in 0..pictures {
        one(None);
    }
    gpu.wait()?;
    let back_to_back = ms(start) / f64::from(pictures);

    Ok(Cost {
        thread: median(thread),
        card,
        alone: median(alone),
        back_to_back,
        first,
    })
}

pub fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

pub fn median(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

pub fn worst(values: &[f64]) -> f64 {
    values.iter().copied().fold(0.0, f64::max)
}
