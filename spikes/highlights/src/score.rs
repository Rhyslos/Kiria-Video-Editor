//! Scoring found moments against the moments a person picked (or the
//! events planted in a synthetic clip): each picked moment matches at most
//! one found moment within a window either side, closest pairs first.

use crate::detect::Moment;

/// A moment a person picked, or an event planted in a test clip.
#[derive(Clone, Debug, PartialEq)]
pub struct Truth {
    pub time: f64,
    /// What it is (`mic`, `game`, `cut`...), free text; may be empty.
    pub kind: String,
}

/// Reads a time written as seconds (`75.5`), minutes and seconds (`1:15`,
/// `1:15.5`) or hours, minutes and seconds (`1:02:03`).
pub fn parse_time(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut seconds = 0.0;
    for part in text.split(':') {
        let v: f64 = part.trim().replace(',', ".").parse().ok()?;
        if !v.is_finite() || v < 0.0 {
            return None;
        }
        seconds = seconds * 60.0 + v;
    }
    Some(seconds)
}

pub fn format_time(seconds: f64) -> String {
    let whole = seconds.max(0.0);
    let h = (whole / 3600.0).floor();
    let m = ((whole - h * 3600.0) / 60.0).floor();
    let s = whole - h * 3600.0 - m * 60.0;
    format!("{h}:{m:02}:{s:05.2}")
}

/// Reads a list of moments: one per line, the time first, then optionally
/// what it is. Commas, semicolons or tabs separate; a first line that isn't
/// a time (a heading) and lines starting with `#` are skipped.
pub fn parse_truth(text: &str) -> Result<Vec<Truth>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split([';', '\t', ',']);
        let first = fields.next().unwrap_or("");
        match parse_time(first) {
            Some(time) => out.push(Truth {
                time,
                kind: fields.next().unwrap_or("").trim().to_owned(),
            }),
            None if out.is_empty() && n == 0 => {} // a heading
            None => return Err(format!("line {}: \"{first}\" is not a time", n + 1)),
        }
    }
    out.sort_by(|a, b| a.time.total_cmp(&b.time));
    Ok(out)
}

/// Reads moments written by [`write_found`].
pub fn parse_found(text: &str) -> Result<Vec<Moment>, String> {
    Ok(parse_truth(text)?
        .into_iter()
        .map(|t| Moment {
            time: t.time,
            score: 0.0,
            sources: String::new(),
        })
        .collect())
}

pub fn write_found(moments: &[Moment]) -> String {
    let mut out = String::from("seconds,score,found by,time\n");
    for m in moments {
        out.push_str(&format!(
            "{:.2},{:.2},{},{}\n",
            m.time,
            m.score,
            m.sources,
            format_time(m.time)
        ));
    }
    out
}

/// How a detector did on one or more recordings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Score {
    pub picked: usize,
    pub found: usize,
    pub hits: usize,
    /// Per kind of picked moment: (kind, picked, hit).
    pub kinds: Vec<(String, usize, usize)>,
    /// Seconds of recording looked at, for false positives per hour.
    pub seconds: f64,
}

impl Score {
    pub fn false_positives(&self) -> usize {
        self.found - self.hits
    }
    pub fn missed(&self) -> usize {
        self.picked - self.hits
    }
    pub fn precision(&self) -> f64 {
        if self.found == 0 {
            0.0
        } else {
            self.hits as f64 / self.found as f64
        }
    }
    pub fn recall(&self) -> f64 {
        if self.picked == 0 {
            0.0
        } else {
            self.hits as f64 / self.picked as f64
        }
    }
    pub fn f1(&self) -> f64 {
        let (p, r) = (self.precision(), self.recall());
        if p + r == 0.0 { 0.0 } else { 2.0 * p * r / (p + r) }
    }
    pub fn false_per_hour(&self) -> f64 {
        if self.seconds <= 0.0 {
            0.0
        } else {
            self.false_positives() as f64 * 3600.0 / self.seconds
        }
    }

    pub fn add(&mut self, other: &Score) {
        self.picked += other.picked;
        self.found += other.found;
        self.hits += other.hits;
        self.seconds += other.seconds;
        for (kind, picked, hit) in &other.kinds {
            match self.kinds.iter_mut().find(|k| &k.0 == kind) {
                Some(k) => {
                    k.1 += picked;
                    k.2 += hit;
                }
                None => self.kinds.push((kind.clone(), *picked, *hit)),
            }
        }
    }

    /// "4/5" for a kind of moment, or "-" when there were none.
    pub fn kind_recall(&self, kind: &str) -> String {
        match self.kinds.iter().find(|k| k.0 == kind) {
            Some((_, picked, hit)) => format!("{hit}/{picked}"),
            None => "-".into(),
        }
    }
}

/// Matches found moments to picked ones within `window` seconds either
/// side, one to one, closest pairs first.
pub fn score(found: &[Moment], truth: &[Truth], window: f64, seconds: f64) -> Score {
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (i, t) in truth.iter().enumerate() {
        for (j, f) in found.iter().enumerate() {
            let gap = (f.time - t.time).abs();
            if gap <= window {
                pairs.push((gap, i, j));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut truth_hit = vec![false; truth.len()];
    let mut found_used = vec![false; found.len()];
    for (_, i, j) in pairs {
        if !truth_hit[i] && !found_used[j] {
            truth_hit[i] = true;
            found_used[j] = true;
        }
    }
    let mut s = Score {
        picked: truth.len(),
        found: found.len(),
        hits: truth_hit.iter().filter(|&&h| h).count(),
        kinds: Vec::new(),
        seconds,
    };
    for (t, hit) in truth.iter().zip(&truth_hit) {
        let kind = if t.kind.is_empty() {
            "any".to_owned()
        } else {
            t.kind.clone()
        };
        let one = Score {
            kinds: vec![(kind, 1, usize::from(*hit))],
            ..Score::default()
        };
        s.add(&one);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(times: &[f64]) -> Vec<Moment> {
        times
            .iter()
            .map(|&time| Moment {
                time,
                score: 1.0,
                sources: String::new(),
            })
            .collect()
    }

    #[test]
    fn times_in_every_form() {
        assert_eq!(parse_time("75.5"), Some(75.5));
        assert_eq!(parse_time("1:15"), Some(75.0));
        assert_eq!(parse_time("1:15,5"), Some(75.5));
        assert_eq!(parse_time("1:02:03"), Some(3723.0));
        assert_eq!(parse_time("loud"), None);
        assert_eq!(parse_time("-3"), None);
        assert_eq!(format_time(3723.5), "1:02:03.50");
    }

    #[test]
    fn truth_files_with_headings_kinds_and_any_separator() {
        let text = "\u{feff}time;what\n# warm-up\n1:10;mic\n0:30\tcut\n12.5,game shout\n\n";
        let t = parse_truth(text).expect("reads");
        assert_eq!(
            t.iter().map(|t| t.time).collect::<Vec<_>>(),
            vec![12.5, 30.0, 70.0]
        );
        assert_eq!(t[0].kind, "game shout");
        assert!(parse_truth("1:00\nlater\n").is_err());
    }

    #[test]
    fn found_files_read_back() {
        let found = at(&[1.0, 62.25]);
        let back = parse_found(&write_found(&found)).expect("reads");
        assert_eq!(back.iter().map(|m| m.time).collect::<Vec<_>>(), vec![1.0, 62.25]);
    }

    #[test]
    fn one_to_one_matching_within_the_window() {
        let truth = parse_truth("10\n12\n40\n70,cut").expect("reads");
        // 10.5 and 11.6 both near 10 and 12: each truth takes one.
        // 41.5 is outside the window; 90 is a false positive.
        let s = score(&at(&[10.5, 11.6, 41.5, 90.0]), &truth, 1.0, 3600.0);
        assert_eq!((s.picked, s.found, s.hits), (4, 4, 2));
        assert_eq!(s.false_positives(), 2);
        assert_eq!(s.missed(), 2);
        assert!((s.precision() - 0.5).abs() < 1e-9);
        assert!((s.recall() - 0.5).abs() < 1e-9);
        assert!((s.f1() - 0.5).abs() < 1e-9);
        assert_eq!(s.false_per_hour(), 2.0);
        assert_eq!(s.kind_recall("cut"), "0/1");
        assert_eq!(s.kind_recall("any"), "2/3");
        let wide = score(&at(&[10.5, 11.6, 41.5, 90.0]), &truth, 2.0, 3600.0);
        assert_eq!(wide.hits, 3);
    }
}
