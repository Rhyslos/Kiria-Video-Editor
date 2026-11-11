//! Measures the ways to put the timeline's picture together that
//! docs/research/rendering.md compares, on this machine's graphics device,
//! and prints the results as Markdown.

mod compose;
mod contend;
mod gpu;
mod measure;

use compose::{Compositor, Place, Source, Targets, Way};
use contend::Composing;
use gpu::Gpu;
use std::collections::BTreeMap;
use std::process::ExitCode;

const P1080: (u32, u32) = (1920, 1080);
const P1440: (u32, u32) = (2560, 1440);
const P2160: (u32, u32) = (3840, 2160);

struct Options {
    quick: bool,
    backends: wgpu::Backends,
    seconds: f64,
}

fn main() -> ExitCode {
    match options().and_then(|options| run(&options)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("render-bench: {why}");
            ExitCode::FAILURE
        }
    }
}

fn options() -> Result<Options, String> {
    let mut options = Options {
        quick: false,
        backends: wgpu::Backends::VULKAN,
        seconds: 4.0,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--quick" => options.quick = true,
            "--backend" => {
                options.backends = match args.next().as_deref() {
                    Some("vulkan") => wgpu::Backends::VULKAN,
                    Some("dx12") => wgpu::Backends::DX12,
                    Some("gl") => wgpu::Backends::GL,
                    Some("any") => wgpu::Backends::all(),
                    other => return Err(format!("--backend takes vulkan, dx12, gl or any, not {other:?}")),
                }
            }
            "--seconds" => {
                options.seconds = args
                    .next()
                    .and_then(|text| text.parse().ok())
                    .ok_or("--seconds takes a number")?;
            }
            other => {
                return Err(format!(
                    "unknown option {other}. Usage: render-bench [--quick] [--backend vulkan|dx12|gl|any] [--seconds N]"
                ));
            }
        }
    }
    Ok(options)
}

fn run(options: &Options) -> Result<(), String> {
    let gpu = gpu::open(options.backends)?;
    println!("# Rendering measurements\n");
    println!("- Device: {}", gpu.about);
    println!(
        "- The card's own clock: {}",
        if gpu.stamps {
            "used for the \"on the card\" column"
        } else {
            "not offered here, so \"on the card\" is empty"
        }
    );
    let compositor = Compositor::new(&gpu);
    println!(
        "- Making the shaders and pipelines took {:.1} ms\n",
        compositor.made_in.as_secs_f64() * 1000.0
    );

    check(&gpu, &compositor)?;
    costs(&gpu, &compositor, options)?;
    disturbance(&gpu, &compositor, options)
}

/// The same pictures through every way: that they come out as they should
/// says the measurements below are of working code.
fn check(gpu: &Gpu, compositor: &Compositor) -> Result<(), String> {
    let size = (64, 64);
    // A mid grey, and a white laid over it at half strength.
    let grey = Source::flat(gpu, size, [126, 128, 128]);
    let white = Source::flat(gpu, size, [235, 128, 128]);
    let window = compose::window(gpu, size);
    let view = window.create_view(&wgpu::TextureViewDescriptor::default());
    println!("## Check\n");
    println!("| Way | grey alone | white at half strength over grey |");
    println!("| --- | --- | --- |");
    let mut alone = Vec::new();
    for way in Way::ALL {
        let targets = Targets::new(gpu, compositor, way, size);
        let mut shown = Vec::new();
        for layers in [
            vec![(&grey, Place::FULL)],
            vec![(&grey, Place::FULL), (&white, Place::MIXED)],
        ] {
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            compositor.draw(gpu, &mut encoder, way, &targets, &layers, Some(&view), None);
            gpu.queue.submit([encoder.finish()]);
            shown.push(compose::pixel(gpu, &window, (32, 32))?[0]);
        }
        println!("| {} | {} | {} |", way.name(), shown[0], shown[1]);
        alone.push(shown[0]);
    }
    println!();
    // Grey 126 in limited range is (126-16)/219 = 0.502, so about 128 on screen.
    if alone.iter().any(|&value| value.abs_diff(128) > 1) {
        return Err(format!(
            "the grey picture came out as {alone:?}, not 128: the compositor is wrong"
        ));
    }
    println!(
        "Every way shows the grey as it is. Mixed in display colours, half white over it lands halfway between the two values; mixed in linear light it is brighter, as light itself would mix.\n"
    );
    Ok(())
}

fn costs(gpu: &Gpu, compositor: &Compositor, options: &Options) -> Result<(), String> {
    let cases: &[(&str, (u32, u32), (u32, u32))] = if options.quick {
        &[("1080p into 1080p", P1080, P1080)]
    } else {
        &[
            ("1080p into 1080p", P1080, P1080),
            ("1440p into 1440p", P1440, P1440),
            ("1440p into 1080p", P1440, P1080),
            ("4K into 4K", P2160, P2160),
            ("4K into 1440p", P2160, P1440),
        ]
    };
    let pictures = if options.quick { 5 } else { 40 };
    println!("## What a picture costs\n");
    println!(
        "Milliseconds for one picture, medians of {pictures}. \"Thread\" is the asking thread's own time (building the commands and handing them over); \"on the card\" is from the start of the first pass to the end of the last by the card's clock; \"alone\" is from asking to finished, one at a time; \"back to back\" is per picture when they are handed over without waiting; \"first\" is the very first picture.\n"
    );
    println!("| Layers into frame | Layers | Way | Thread | On the card | Alone | Back to back | First |");
    println!("| --- | --- | --- | --- | --- | --- | --- | --- |");
    let mut sources = Sources::default();
    for &(name, layer_size, frame) in cases {
        for count in [1, 2, 4] {
            for way in Way::ALL {
                let cost = measure::cost(
                    gpu,
                    compositor,
                    sources.of(gpu, layer_size, count),
                    frame,
                    way,
                    pictures,
                )?;
                let card = cost.card.map_or("–".to_string(), |ms| format!("{ms:.2}"));
                println!(
                    "| {name} | {count} | {} | {:.2} | {card} | {:.2} | {:.2} | {:.1} |",
                    way.name(),
                    cost.thread,
                    cost.alone,
                    cost.back_to_back,
                    cost.first
                );
            }
        }
    }
    println!();
    Ok(())
}

fn disturbance(gpu: &Gpu, compositor: &Compositor, options: &Options) -> Result<(), String> {
    let pictures: &[(&str, (u32, u32), usize)] = if options.quick {
        &[("1080p, 2 layers", P1080, 2)]
    } else {
        &[("1440p, 2 layers", P1440, 2), ("4K, 4 layers", P2160, 4)]
    };
    let rates: &[f64] = if options.quick { &[60.0] } else { &[60.0, 144.0] };
    let seconds = if options.quick { 0.5 } else { options.seconds };
    println!("## While the interface draws\n");
    println!(
        "A loop stands in for the interface: every frame it draws the newest composed picture (a float frame in linear light) onto a stand-in window, for {seconds} s. Meanwhile 60 pictures a second are composed by nobody, by that same thread, or by a worker. \"Interface thread\" is that thread's own time per frame until its commands are handed over; \"frame finished\" is from the frame's start until the card is done with it. Median / worst, in ms. A frame is late when it finished after one frame's time.\n"
    );
    println!(
        "| Picture | Interface | Composed by | Frames | Late | Lost | Interface thread | Frame finished | Pictures | Composing thread |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    let mut sources = Sources::default();
    for &(name, size, count) in pictures {
        for &rate in rates {
            for composing in Composing::ALL {
                let run = contend::Run {
                    gpu,
                    compositor,
                    sources: sources.of(gpu, size, count),
                    frame: size,
                    frames_a_second: rate,
                    pictures_a_second: 60.0,
                    seconds,
                };
                let outcome = contend::run(&run, composing)?;
                let pair = |(median, worst): (f64, f64)| format!("{median:.2} / {worst:.2}");
                println!(
                    "| {name} | {rate:.0} a second | {} | {} | {} | {} | {} | {} | {} | {} |",
                    composing.name(),
                    outcome.frames,
                    outcome.late,
                    outcome.lost,
                    pair(outcome.thread),
                    pair(outcome.finished),
                    outcome.pictures,
                    outcome.composing.map_or("–".to_string(), pair),
                );
            }
        }
    }
    println!();
    Ok(())
}

/// Stand-in decoded pictures, made once per size.
#[derive(Default)]
struct Sources(BTreeMap<(u32, u32), Vec<Source>>);

impl Sources {
    fn of(&mut self, gpu: &Gpu, size: (u32, u32), count: usize) -> &[Source] {
        let made = self.0.entry(size).or_insert_with(|| {
            (0..compose::MOST_LAYERS as u32)
                .map(|seed| Source::pattern(gpu, size, seed))
                .collect()
        });
        &made[..count.min(made.len())]
    }
}
