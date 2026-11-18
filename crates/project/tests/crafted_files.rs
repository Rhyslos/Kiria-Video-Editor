//! Project and template files are checked on load: a value a file can
//! carry that no edit makes (a speed of zero, a sound rate of zero, a
//! picture millions of pixels wide, a setting outside its range) is
//! refused with an explained error, never loaded to panic, divide by zero
//! or allocate hugely later. Each case is a stored sample with one value
//! changed by hand.

// Test helpers may panic: a failure here is a failing test.
#![allow(clippy::expect_used)]

use project::file;
use project::templates;
use std::path::Path;

fn sample(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/samples")
        .join(name);
    std::fs::read_to_string(path).expect("the sample is there")
}

/// The latest project sample with the first `from` in its project (not its
/// summary) changed to `to`.
fn crafted_project(from: &str, to: &str) -> String {
    let text = sample(&format!("project-v{}.ron", file::FORMAT_VERSION));
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let project = lines
        .iter_mut()
        .find(|line| line.trim_start().starts_with("project:"))
        .expect("the sample has a project line");
    assert!(project.contains(from), "the sample's project has {from}");
    *project = project.replacen(from, to, 1);
    lines.join("\n")
}

/// The latest template sample with the first `from` changed to `to`.
fn crafted_template(from: &str, to: &str) -> String {
    let text = sample(&format!("template-v{}.ron", templates::FORMAT_VERSION));
    assert!(text.contains(from), "the template sample has {from}");
    text.replacen(from, to, 1)
}

/// Loads `text` as a project, expecting it refused; gives the error's text.
fn refused(text: &str) -> String {
    match file::from_str(text) {
        Ok(_) => panic!("the crafted project loaded"),
        Err(err) => err.to_string(),
    }
}

fn refused_template(text: &str) -> String {
    match templates::from_str(text) {
        Ok(_) => panic!("the crafted template loaded"),
        Err(err) => err.to_string(),
    }
}

#[test]
fn the_unchanged_samples_still_load() {
    file::from_str(&crafted_project("sample_rate:48000", "sample_rate:48000")).expect("the sample loads");
    templates::from_str(&sample(&format!("template-v{}.ron", templates::FORMAT_VERSION)))
        .expect("the template sample loads");
}

#[test]
fn a_speed_of_zero_or_over_nothing_is_refused() {
    for speed in [
        "(num:0,den:4)",
        "(num:1,den:0)",
        "(num:0,den:0)",
        "(num:1,den:101)",
    ] {
        let error = refused(&crafted_project("speed:(num:1,den:4)", &format!("speed:{speed}")));
        assert!(error.contains("damaged"), "{speed}: {error}");
    }
}

#[test]
fn a_speed_in_the_summary_or_written_unreduced_still_reads() {
    // 2/8 is a quarter, written the long way.
    let p = file::from_str(&crafted_project("speed:(num:1,den:4)", "speed:(num:2,den:8)")).expect("it loads");
    let speeds: Vec<(u32, u32)> = p
        .sequences
        .iter()
        .flat_map(|s| s.tracks().flat_map(|t| t.clips.iter()))
        .flat_map(|c| c.ramp.keys().iter().map(|k| (k.speed.num(), k.speed.den())))
        .collect();
    assert!(speeds.contains(&(1, 4)));
    assert!(!speeds.contains(&(2, 8)));
}

#[test]
fn timeline_settings_out_of_range_are_refused() {
    for (from, to) in [
        ("sample_rate:48000),video", "sample_rate:0),video"),
        ("sample_rate:48000),video", "sample_rate:4000000000),video"),
        ("width:1920", "width:4000000"),
        ("height:1080", "height:0"),
    ] {
        let error = refused(&crafted_project(from, to));
        assert!(error.contains("timeline settings can't be used"), "{to}: {error}");
    }
}

#[test]
fn track_and_master_volumes_out_of_range_are_refused() {
    let error = refused(&crafted_project("gain_db:-3.5", "gain_db:400.0"));
    assert!(error.contains("volume out of range"), "{error}");
    let error = refused(&crafted_project("gain_db:-3.5", "gain_db:NaN"));
    assert!(error.contains("volume out of range"), "{error}");
}

#[test]
fn track_and_master_sound_out_of_range_are_refused() {
    let error = refused(&crafted_project(
        "high_pass:(on:true,freq_hz:80)",
        "high_pass:(on:true,freq_hz:0)",
    ));
    assert!(error.contains("high pass"), "{error}");
    let error = refused(&crafted_project("ceiling_mdb:-1000", "ceiling_mdb:5000"));
    assert!(error.contains("ceiling"), "{error}");
}

#[test]
fn a_clips_settings_out_of_range_are_refused() {
    for (from, to, said) in [
        ("value:-9000)", "value:-900000)", "line can't be used"),
        ("amount:500000", "amount:500000000", "can't be used"),
        ("length:(1,1)", "length:(0,1)", "transition of no length"),
    ] {
        let error = refused(&crafted_project(from, to));
        assert!(error.contains(said), "{to}: {error}");
    }
}

#[test]
fn curves_and_masks_out_of_range_are_refused() {
    let error = refused(&crafted_project(
        "master:[(250000,220000),(750000,790000)]",
        "master:[(750000,220000),(250000,790000)]",
    ));
    assert!(error.contains("curve can't be used"), "{error}");
    let error = refused(&crafted_project(
        "master:[(250000,220000),(750000,790000)]",
        "master:[(250000,220000),(750000,7900000)]",
    ));
    assert!(error.contains("curve can't be used"), "{error}");
    let error = refused(&crafted_project("feather:300000", "feather:3000000"));
    assert!(error.contains("mask can't be used"), "{error}");
}

#[test]
fn a_template_is_checked_as_a_project_is() {
    let error = refused_template(&crafted_template("gain_mdb: -3000", "gain_mdb: 300000"));
    assert!(error.contains("volume or pan out of range"), "{error}");
    let error = refused_template(&crafted_template("num: 1,", "num: 0,"));
    assert!(error.contains("damaged"), "{error}");
}
