//! Looks: grade layers kept to put on any clip in a click (the Color
//! page's Looks tool), the ones the app comes with and the person's own.
//!
//! The person's belong to them, not to a project, like templates and the
//! labels offered everywhere: one small file, `looks.ron` in the app's
//! data folder, with its own format version. Putting a look on a clip adds
//! a copy of its layer to the clip's grade (an ordinary, undoable
//! `SetGrades`), at the strength chosen, so a project never depends on the
//! file; removing a look leaves the clips it was put on as they are.
//!
//! ```ron
//! (
//!   format_version: 1,
//!   looks: [
//!     (name: "Evening stream", layer: (name: "Evening stream", primary: (temperature: 120000))),
//!   ],
//! )
//! ```

use crate::file::FileKind;
use crate::grade::{Curve, Curves, GradeLayer, ONE, Primary, Wheel};
use diag::{AppError, Explain};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The format this build writes.
pub const FORMAT_VERSION: u32 = 1;

/// The file's name in the app's data folder.
pub const FILE_NAME: &str = "looks.ron";

/// The longest name a look may have.
pub const NAME_LIMIT: usize = 40;

/// A look: a name and the grade layer it puts on a clip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Look {
    pub name: String,
    pub layer: GradeLayer,
}

/// The person's own looks, in the order they were saved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Looks {
    pub looks: Vec<Look>,
}

#[derive(Serialize)]
struct FileOut<'a> {
    format_version: u32,
    looks: &'a [Look],
}

#[derive(Deserialize)]
struct FileIn {
    #[allow(dead_code)]
    format_version: u32,
    looks: Vec<Look>,
}

const LOOKS: FileKind = FileKind {
    newer: |version, current| {
        AppError::new("Your looks were saved by a newer version")
            .why(format!("They use format {version}; this version reads up to format {current}."))
            .fix("Update the app. Until then the looks the app comes with work, and the file is left as it is.")
    },
    damaged,
    no_step: |_| damaged(),
};

fn damaged() -> AppError {
    AppError::new("Your looks can't be read")
        .why("The file holding them is damaged.")
        .fix("Delete looks.ron in the app's data folder and save them again. Clips they were put on are not affected.")
}

/// Where the file is.
pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

impl Looks {
    /// Reads the person's looks: none when there is no file yet; refused
    /// when it was written by a newer version or is damaged (the file is
    /// then left as it is).
    pub fn load(data_dir: &Path) -> diag::Result<Looks> {
        let file = path(data_dir);
        if !file.exists() {
            return Ok(Looks::default());
        }
        let text = std::fs::read_to_string(&file).or_explain(|| {
            AppError::new("Your looks can't be read")
                .why(format!("{} could not be read.", file.display()))
                .fix("Check that the app's data folder can be read.")
        })?;
        Looks::parse(&text)
    }

    pub fn parse(text: &str) -> diag::Result<Looks> {
        let file: FileIn = crate::file::versioned_from_str(text, FORMAT_VERSION, &[], &LOOKS)?;
        Ok(Looks { looks: file.looks })
    }

    pub fn to_string(&self) -> diag::Result<String> {
        let config = ron::ser::PrettyConfig::new().indentor("  ".to_owned());
        ron::ser::to_string_pretty(
            &FileOut {
                format_version: FORMAT_VERSION,
                looks: &self.looks,
            },
            config,
        )
        .or_explain(|| {
            AppError::new("Your looks can't be saved").why("They could not be turned into file text.")
        })
    }

    /// Writes them to the app's data folder, whole or not at all. Touches
    /// the disk: call it from a worker.
    pub fn save(&self, data_dir: &Path) -> diag::Result<()> {
        let explain = || {
            AppError::new("Your looks can't be saved")
                .why(format!("Writing to {} failed.", data_dir.display()))
                .fix("Check that the disk is not full and that the app's data folder can be written to.")
        };
        std::fs::create_dir_all(data_dir).or_explain(explain)?;
        crate::disk::write_atomic(&path(data_dir), self.to_string()?.as_bytes()).or_explain(explain)
    }

    /// Keeps `layer` as a look named `name` (one with that name, in any
    /// capitals, is replaced). Its range and mask are left out: a look is
    /// about colour, put on the whole picture.
    pub fn add(&mut self, name: &str, layer: &GradeLayer) -> diag::Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::new("A look needs a name").fix("Type at least one letter."));
        }
        if name.chars().count() > NAME_LIMIT {
            return Err(AppError::new("That name is too long")
                .why(format!("Names are up to {NAME_LIMIT} characters."))
                .fix("Use a shorter name."));
        }
        layer.primary.check()?;
        let look = Look {
            name: name.to_owned(),
            layer: GradeLayer {
                name: name.to_owned(),
                off: false,
                range: None,
                mask: None,
                ..layer.clone()
            },
        };
        let lower = name.to_lowercase();
        match self.looks.iter_mut().find(|l| l.name.to_lowercase() == lower) {
            Some(kept) => *kept = look,
            None => self.looks.push(look),
        }
        Ok(())
    }

    /// Stops keeping the look named `name`.
    pub fn remove(&mut self, name: &str) {
        let lower = name.trim().to_lowercase();
        self.looks.retain(|l| l.name.to_lowercase() != lower);
    }
}

/// `layer` at `strength` (0 to 1): every number and curve that far from
/// leaving the picture as it is towards the look, so a look can be put on
/// gently.
pub fn at_strength(layer: &GradeLayer, strength: f64) -> GradeLayer {
    let s = strength.clamp(0.0, 1.0);
    let towards = |plain: i32, value: i32| plain + (f64::from(value - plain) * s).round() as i32;
    let p = &layer.primary;
    let plain = Primary::PLAIN;
    let wheel = |w: Wheel| Wheel {
        red: towards(0, w.red),
        green: towards(0, w.green),
        blue: towards(0, w.blue),
        brightness: towards(0, w.brightness),
    };
    let curve = |c: &Curve| Curve(c.0.iter().map(|(x, y)| (*x, towards(*x, *y))).collect());
    GradeLayer {
        primary: Primary {
            exposure: towards(plain.exposure, p.exposure),
            temperature: towards(plain.temperature, p.temperature),
            tint: towards(plain.tint, p.tint),
            contrast: towards(plain.contrast, p.contrast),
            pivot: p.pivot,
            saturation: towards(plain.saturation, p.saturation),
            highlights: towards(plain.highlights, p.highlights),
            shadows: towards(plain.shadows, p.shadows),
            shadows_wheel: wheel(p.shadows_wheel),
            midtones_wheel: wheel(p.midtones_wheel),
            highlights_wheel: wheel(p.highlights_wheel),
            overall_wheel: wheel(p.overall_wheel),
        },
        curves: Curves {
            master: curve(&layer.curves.master),
            red: curve(&layer.curves.red),
            green: curve(&layer.curves.green),
            blue: curve(&layer.curves.blue),
        },
        ..layer.clone()
    }
}

/// Millionths of `value`.
const fn m(value: f64) -> i32 {
    (value * ONE as f64) as i32
}

/// The looks the app comes with.
pub fn built_in() -> Vec<Look> {
    let look = |name: &str, primary: Primary, curves: Curves| Look {
        name: name.to_owned(),
        layer: GradeLayer {
            primary,
            curves,
            ..GradeLayer::new(name)
        },
    };
    let p = Primary::PLAIN;
    let s_curve = |amount: f64| {
        Curve(vec![
            (0, 0),
            (m(0.25), m(0.25 - amount)),
            (m(0.75), m(0.75 + amount)),
            (ONE, ONE),
        ])
    };
    vec![
        look(
            "Warm",
            Primary {
                temperature: m(0.25),
                tint: m(0.03),
                saturation: m(1.05),
                ..p
            },
            Curves::default(),
        ),
        look(
            "Cool",
            Primary {
                temperature: m(-0.25),
                saturation: m(0.95),
                ..p
            },
            Curves::default(),
        ),
        look(
            "Punchy",
            Primary {
                contrast: m(0.25),
                saturation: m(1.3),
                ..p
            },
            Curves::default(),
        ),
        look(
            "Teal and orange",
            Primary {
                shadows_wheel: Wheel {
                    red: m(-0.12),
                    green: m(0.02),
                    blue: m(0.14),
                    brightness: 0,
                },
                highlights_wheel: Wheel {
                    red: m(0.14),
                    green: m(0.04),
                    blue: m(-0.12),
                    brightness: 0,
                },
                contrast: m(0.1),
                saturation: m(1.15),
                ..p
            },
            Curves::default(),
        ),
        look(
            "Faded film",
            Primary {
                saturation: m(0.8),
                temperature: m(0.08),
                ..p
            },
            Curves {
                master: Curve(vec![(0, m(0.08)), (m(0.5), m(0.52)), (ONE, m(0.93))]),
                ..Curves::default()
            },
        ),
        look(
            "Crisp",
            Primary {
                contrast: m(0.15),
                saturation: m(1.1),
                ..p
            },
            Curves {
                master: s_curve(0.04),
                ..Curves::default()
            },
        ),
        look(
            "Night",
            Primary {
                exposure: m(-0.7),
                temperature: m(-0.4),
                saturation: m(0.7),
                ..p
            },
            Curves::default(),
        ),
        look(
            "Black and white",
            Primary {
                saturation: 0,
                contrast: m(0.15),
                ..p
            },
            Curves::default(),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_look_the_app_comes_with_can_be_used() {
        let looks = built_in();
        assert!(looks.len() >= 6);
        for look in &looks {
            look.layer
                .primary
                .check()
                .unwrap_or_else(|e| panic!("{}: {}", look.name, e.report()));
            for curve in look.layer.curves.all() {
                curve
                    .check()
                    .unwrap_or_else(|e| panic!("{}: {}", look.name, e.report()));
            }
            assert!(!at_strength(&look.layer, 1.0).primary.is_plain() || !look.layer.curves.is_plain());
        }
    }

    #[test]
    fn a_look_at_no_strength_leaves_the_picture_and_half_is_half_way() {
        let warm = built_in().remove(0);
        let none = at_strength(&warm.layer, 0.0);
        assert!(none.primary.is_plain() && none.curves.is_plain());
        let half = at_strength(&warm.layer, 0.5);
        assert_eq!(half.primary.temperature, warm.layer.primary.temperature / 2);
        assert_eq!(at_strength(&warm.layer, 1.0), warm.layer);
        let faded = built_in().into_iter().find(|l| l.name == "Faded film").unwrap();
        let half = at_strength(&faded.layer, 0.5);
        assert_eq!(half.curves.master.0[0], (0, m(0.04)));
    }

    #[test]
    fn saved_and_read_back_without_range_or_mask() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Looks::load(dir.path()).unwrap(), Looks::default());
        let mut looks = Looks::default();
        let mut layer = built_in().remove(0).layer;
        layer.mask = Some(crate::grade::Mask::MIDDLE);
        looks.add(" Evening ", &layer).unwrap();
        looks.add("evening", &layer).unwrap();
        assert_eq!(looks.looks.len(), 1, "one name once");
        assert_eq!(looks.looks[0].layer.mask, None);
        assert_eq!(looks.looks[0].layer.name, "evening", "the newer replaces it");
        looks.save(dir.path()).unwrap();
        assert_eq!(Looks::load(dir.path()).unwrap(), looks);
        looks.remove("EVENING");
        assert!(looks.looks.is_empty());
        assert!(looks.add("  ", &layer).is_err());
        std::fs::write(path(dir.path()), "(format_version: 99, looks: [])").unwrap();
        assert!(Looks::load(dir.path()).is_err(), "newer is refused");
    }
}
