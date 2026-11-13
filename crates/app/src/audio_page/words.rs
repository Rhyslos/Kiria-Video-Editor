//! What each tool is doing now, in one plain sentence: from its settings,
//! and while playing from what the mixer measures.

use project::track_sound::{BandShape, Cleanup, Compressor, Ducking, Eq, HighPass, Loudness};

/// Decibels in thousandths as text: "3", "2.5".
fn db(mdb: i32) -> String {
    let v = f64::from(mdb.unsigned_abs()) / 1_000.0;
    if (v - v.round()).abs() < 0.05 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

/// A frequency as text: "80 Hz", "2.5 kHz".
pub fn hz(freq: u32) -> String {
    if freq >= 1_000 {
        let k = f64::from(freq) / 1_000.0;
        if (k - k.round()).abs() < 0.05 {
            format!("{k:.0} kHz")
        } else {
            format!("{k:.1} kHz")
        }
    } else {
        format!("{freq} Hz")
    }
}

/// A live reduction in dB is worth saying from this much.
const AUDIBLE_DB: f32 = 0.5;

/// The EQ and the high pass.
pub fn eq(track: &str, eq: &Eq, high_pass: &HighPass) -> String {
    let mut parts = Vec::new();
    if eq.on {
        for band in eq.bands.iter().filter(|b| b.on && b.gain_mdb.abs() >= 100) {
            let way = if band.gain_mdb > 0 { "lifts" } else { "cuts" };
            let place = match band.shape {
                BandShape::LowShelf => format!("below {}", hz(band.freq_hz)),
                BandShape::HighShelf => format!("above {}", hz(band.freq_hz)),
                BandShape::Peak => format!("around {}", hz(band.freq_hz)),
            };
            parts.push(format!("{way} {} dB {place}", db(band.gain_mdb)));
        }
    }
    let rumble = high_pass
        .on
        .then(|| format!("rumble under {} is taken out", hz(high_pass.freq_hz)));
    match (parts.is_empty(), rumble) {
        (true, None) if !eq.on => format!("The EQ is off: {track} sounds as recorded."),
        (true, None) => format!("Every band is flat: {track} sounds as recorded."),
        (true, Some(r)) => format!("On {track}, {r}."),
        (false, rumble) => {
            let mut said = capital(&join(&parts));
            if let Some(r) = rumble {
                said.push_str("; ");
                said.push_str(&r);
            }
            format!("{said}.")
        }
    }
}

/// The compressor: what it is set to do, or (while playing) how far it is
/// turning the track down now.
pub fn compressor(track: &str, c: &Compressor, reducing_db: Option<f32>) -> String {
    if !c.on {
        return format!(
            "The compressor is off: {track}'s loud and quiet moments stay as far apart as recorded."
        );
    }
    let ratio = f64::from(c.ratio_milli) / 1_000.0;
    let share = if (ratio - ratio.round()).abs() < 0.05 {
        format!("1/{ratio:.0}")
    } else {
        format!("1/{ratio:.1}")
    };
    match reducing_db {
        Some(gr) if gr <= -AUDIBLE_DB => format!(
            "Turns down {track}'s loudest moments by about {:.0} dB now, so it stays even.",
            -gr
        ),
        Some(_) => format!(
            "{track} is under −{} dB just now, so it is left alone.",
            db(c.threshold_mdb)
        ),
        None => format!(
            "Above −{} dB, {track} rises only {share} as fast, then all of it is lifted {} dB.",
            db(c.threshold_mdb),
            db(c.makeup_mdb)
        ),
    }
}

/// The clean-up.
pub fn cleanup(track: &str, c: &Cleanup, reducing_db: Option<f32>) -> String {
    if c.profile.is_none() {
        return "Nothing learned yet: mark a pause where only the noise is heard, then learn it.".to_owned();
    }
    if !c.on {
        return format!("The noise is learned; turn Clean-up on to take it out of {track}.");
    }
    match reducing_db {
        Some(r) if r <= -AUDIBLE_DB => {
            format!(
                "Taking about {:.0} dB of the learned noise out of {track} now.",
                -r
            )
        }
        _ => format!(
            "Turns down what is less than {} dB over the learned noise by up to {} dB.",
            db(c.threshold_mdb),
            db(c.reduction_mdb)
        ),
    }
}

/// Ducking: under which tracks, how far, and how far now.
pub fn ducking(track: &str, d: &Ducking, keys: &[&str], ducked_db: Option<f32>) -> String {
    if keys.is_empty() {
        return "No track is dialogue yet: make one Dialogue (right-click its name or its strip) for ducking to listen to."
            .to_owned();
    }
    if !d.on {
        return format!(
            "Ducking is off: {track} stays as loud while {} speaks.",
            join_or(keys)
        );
    }
    match ducked_db {
        Some(now) if now <= -AUDIBLE_DB => format!(
            "{} is speaking: {track} is down {:.0} dB of {} now.",
            capital(&join_or(keys)),
            -now,
            db(d.depth_mdb)
        ),
        _ => format!(
            "Turns {track} down {} dB while {} speaks, and back up when they stop.",
            db(d.depth_mdb),
            join_or(keys)
        ),
    }
}

/// The Loudness tool: where it keeps the track, and its gain now.
pub fn leveller(track: &str, l: &Loudness, gain_db: Option<f32>) -> String {
    if !l.on {
        return format!("The Loudness tool is off: {track} goes up and down as recorded.");
    }
    let aim = db(l.target_mlufs);
    match gain_db {
        Some(g) if g >= AUDIBLE_DB => format!("Keeping {track} near −{aim} LUFS: lifting it {g:.1} dB now."),
        Some(g) if g <= -AUDIBLE_DB => {
            format!("Keeping {track} near −{aim} LUFS: lowering it {:.1} dB now.", -g)
        }
        _ => format!(
            "Keeps {track} near −{aim} LUFS, slowly, moving it at most {} dB up or {} dB down.",
            db(l.max_boost_mdb as i32),
            db(l.max_cut_mdb as i32)
        ),
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// "a, b and c".
fn join(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// "A, B or C"; past three, "one of the 12 dialogue tracks".
fn join_or(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        many if many.len() > 3 => format!("one of the {} dialogue tracks", many.len()),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_tool_says_what_it_does_in_one_sentence() {
        let mut eq_set = Eq {
            on: true,
            ..Eq::default()
        };
        eq_set.bands[1].gain_mdb = -3_000;
        eq_set.bands[3].gain_mdb = 2_500;
        let hp = HighPass {
            on: true,
            freq_hz: 80,
        };
        assert_eq!(
            eq("Ingrid", &eq_set, &hp),
            "Cuts 3 dB around 250 Hz and lifts 2.5 dB around 3 kHz; rumble under 80 Hz is taken out."
        );
        assert_eq!(
            eq("Ingrid", &Eq::default(), &HighPass::default()),
            "The EQ is off: Ingrid sounds as recorded."
        );
        let comp = Compressor {
            on: true,
            ..Compressor::default()
        };
        assert_eq!(
            compressor("Ingrid", &comp, Some(-4.2)),
            "Turns down Ingrid's loudest moments by about 4 dB now, so it stays even."
        );
        assert!(compressor("Ingrid", &comp, None).starts_with("Above −24 dB, Ingrid rises only 1/3 as fast"));
        let duck = Ducking {
            on: true,
            ..Ducking::default()
        };
        assert_eq!(
            ducking("Music", &duck, &["Ingrid", "Ola"], Some(-8.0)),
            "Ingrid or Ola is speaking: Music is down 8 dB of 12 now."
        );
        assert!(ducking("Music", &duck, &[], None).starts_with("No track is dialogue"));
        assert_eq!(
            ducking("Music", &duck, &["A", "B", "C", "D"], Some(-3.0)),
            "One of the 4 dialogue tracks is speaking: Music is down 3 dB of 12 now."
        );
        let level = Loudness {
            on: true,
            ..Loudness::default()
        };
        assert_eq!(
            leveller("Voice", &level, Some(3.5)),
            "Keeping Voice near −18 LUFS: lifting it 3.5 dB now."
        );
        assert!(cleanup("Voice", &Cleanup::default(), None).starts_with("Nothing learned yet"));
        assert_eq!(hz(2_500), "2.5 kHz");
    }
}
