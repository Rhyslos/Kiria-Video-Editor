//! A clip's grade (format 28): its colour as the Color page sets it, in
//! layers applied one after another after the clip's quick colour and
//! before its effects, in linear light. Every number is a whole millionth,
//! like a look's, and a layer at its defaults changes nothing.

use diag::AppError;
use serde::{Deserialize, Serialize};

/// One whole.
pub const ONE: i32 = 1_000_000;

/// A colour wheel: a push towards a colour (red, green and blue, each
/// -1 to 1 in millionths, the wheel's puck) and a brightness (its slider).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Wheel {
    pub red: i32,
    pub green: i32,
    pub blue: i32,
    pub brightness: i32,
}

impl Wheel {
    pub const PLAIN: Wheel = Wheel {
        red: 0,
        green: 0,
        blue: 0,
        brightness: 0,
    };

    pub fn is_plain(&self) -> bool {
        *self == Wheel::PLAIN
    }

    fn numbers(&self) -> [i32; 4] {
        [self.red, self.green, self.blue, self.brightness]
    }
}

/// The primary corrections of one layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Primary {
    /// Stops brighter (negative darker), -4 to 4.
    pub exposure: i32,
    /// Warmer (positive) or cooler, and greener or more magenta: -1 to 1.
    pub temperature: i32,
    pub tint: i32,
    /// How much harder (positive) or flatter, -1 to 1, around `pivot`.
    pub contrast: i32,
    /// The light level contrast turns around, 0.01 to 1 (0.18 is middle grey).
    pub pivot: i32,
    /// 0 is grey, 1 leaves it, 2 is twice the colour.
    pub saturation: i32,
    /// Stops brighter in the brightest parts and in the darkest: -2 to 2.
    pub highlights: i32,
    pub shadows: i32,
    /// The four wheels: the darkest parts, the middle, the brightest, all.
    pub shadows_wheel: Wheel,
    pub midtones_wheel: Wheel,
    pub highlights_wheel: Wheel,
    pub overall_wheel: Wheel,
}

impl Primary {
    pub const PLAIN: Primary = Primary {
        exposure: 0,
        temperature: 0,
        tint: 0,
        contrast: 0,
        pivot: 180_000,
        saturation: ONE,
        highlights: 0,
        shadows: 0,
        shadows_wheel: Wheel::PLAIN,
        midtones_wheel: Wheel::PLAIN,
        highlights_wheel: Wheel::PLAIN,
        overall_wheel: Wheel::PLAIN,
    };

    pub fn is_plain(&self) -> bool {
        *self == Primary::PLAIN
    }

    /// Refuses numbers outside what the page offers.
    pub fn check(&self) -> diag::Result<()> {
        let within = |what: &str, value: i32, least: i32, most: i32| {
            if (least..=most).contains(&value) {
                Ok(())
            } else {
                Err(AppError::new("That grade can't be used").why(format!("{what} is out of range.")))
            }
        };
        within("Exposure", self.exposure, -4 * ONE, 4 * ONE)?;
        within("Temperature", self.temperature, -ONE, ONE)?;
        within("Tint", self.tint, -ONE, ONE)?;
        within("Contrast", self.contrast, -ONE, ONE)?;
        within("Pivot", self.pivot, ONE / 100, ONE)?;
        within("Saturation", self.saturation, 0, 2 * ONE)?;
        within("Highlights", self.highlights, -2 * ONE, 2 * ONE)?;
        within("Shadows", self.shadows, -2 * ONE, 2 * ONE)?;
        for wheel in [
            self.shadows_wheel,
            self.midtones_wheel,
            self.highlights_wheel,
            self.overall_wheel,
        ] {
            for value in wheel.numbers() {
                within("A wheel", value, -ONE, ONE)?;
            }
        }
        Ok(())
    }
}

impl Default for Primary {
    fn default() -> Primary {
        Primary::PLAIN
    }
}

/// A curve: points (in, out), each 0 to [`ONE`], sorted by `in`, joined
/// smoothly without overshooting (monotone cubic). None, or the two ends
/// alone at (0, 0) and (1, 1), leaves the picture as it is.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Curve(pub Vec<(i32, i32)>);

impl Curve {
    /// The most points a curve has.
    pub const MOST_POINTS: usize = 16;

    /// Whether it leaves the picture as it is.
    pub fn is_plain(&self) -> bool {
        self.0.iter().all(|(x, y)| x == y)
    }

    /// The points as shares of one, with the ends at 0 and 1 made when
    /// missing (as straight on).
    fn knots(&self) -> Vec<(f64, f64)> {
        let one = f64::from(ONE);
        let mut knots: Vec<(f64, f64)> = self
            .0
            .iter()
            .map(|(x, y)| (f64::from(*x) / one, f64::from(*y) / one))
            .collect();
        if knots.first().is_none_or(|(x, _)| *x > 0.0) {
            knots.insert(0, (0.0, 0.0));
        }
        if knots.last().is_none_or(|(x, _)| *x < 1.0) {
            knots.push((1.0, 1.0));
        }
        knots
    }

    /// Its value at `x` (0 to 1).
    pub fn at(&self, x: f64) -> f64 {
        let k = self.knots();
        let x = x.clamp(0.0, 1.0);
        let n = k.len();
        // Slopes between knots, then at each knot (Fritsch and Carlson's
        // monotone cubic: no overshoot between two points).
        let d: Vec<f64> = (0..n - 1)
            .map(|i| (k[i + 1].1 - k[i].1) / (k[i + 1].0 - k[i].0).max(1e-9))
            .collect();
        let mut m = vec![0.0; n];
        m[0] = d[0];
        m[n - 1] = d[n - 2];
        for i in 1..n - 1 {
            m[i] = if d[i - 1] * d[i] <= 0.0 {
                0.0
            } else {
                let (a, b) = (d[i - 1], d[i]);
                3.0 * (a + b) / ((2.0 * b + a) / a + (b + 2.0 * a) / b)
            };
        }
        let i = (0..n - 1).find(|i| x <= k[i + 1].0).unwrap_or(n - 2);
        let h = (k[i + 1].0 - k[i].0).max(1e-9);
        let t = (x - k[i].0) / h;
        let (t2, t3) = (t * t, t * t * t);
        let y = (2.0 * t3 - 3.0 * t2 + 1.0) * k[i].1
            + (t3 - 2.0 * t2 + t) * h * m[i]
            + (-2.0 * t3 + 3.0 * t2) * k[i + 1].1
            + (t3 - t2) * h * m[i + 1];
        y.clamp(0.0, 1.0)
    }

    /// Refuses points out of range, out of order or too many.
    pub fn check(&self) -> diag::Result<()> {
        let fine = self.0.len() <= Curve::MOST_POINTS
            && self
                .0
                .iter()
                .all(|(x, y)| (0..=ONE).contains(x) && (0..=ONE).contains(y))
            && self.0.windows(2).all(|w| w[0].0 < w[1].0);
        match fine {
            true => Ok(()),
            false => Err(AppError::new("That curve can't be used")
                .why("A curve has up to 16 points from 0 to 1, each further along than the last.")),
        }
    }
}

/// A layer's curves: brightness (master) first, then red, green and blue.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Curves {
    #[serde(skip_serializing_if = "crate::defaults::is_default")]
    pub master: Curve,
    #[serde(skip_serializing_if = "crate::defaults::is_default")]
    pub red: Curve,
    #[serde(skip_serializing_if = "crate::defaults::is_default")]
    pub green: Curve,
    #[serde(skip_serializing_if = "crate::defaults::is_default")]
    pub blue: Curve,
}

impl Curves {
    pub fn is_plain(&self) -> bool {
        [&self.master, &self.red, &self.green, &self.blue]
            .iter()
            .all(|c| c.is_plain())
    }

    pub fn all(&self) -> [&Curve; 4] {
        [&self.master, &self.red, &self.green, &self.blue]
    }
}

/// One layer of a grade.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GradeLayer {
    pub name: String,
    /// Switched off: kept, not applied.
    #[serde(default, skip_serializing_if = "crate::defaults::is_default")]
    pub off: bool,
    #[serde(default, skip_serializing_if = "crate::defaults::is_default")]
    pub primary: Primary,
    /// Its curves (format 29), applied after the primary corrections.
    #[serde(default, skip_serializing_if = "crate::defaults::is_default")]
    pub curves: Curves,
}

impl GradeLayer {
    /// A layer that changes nothing, called `name`.
    pub fn new(name: impl Into<String>) -> GradeLayer {
        GradeLayer {
            name: name.into(),
            off: false,
            primary: Primary::PLAIN,
            curves: Curves::default(),
        }
    }
}

/// A clip's grade: its layers in the order they are applied. None at
/// first: an ungraded clip.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Grade(pub Vec<GradeLayer>);

impl Grade {
    /// The name of a grade's first layer.
    pub const FIRST_LAYER: &'static str = "Base correction";

    /// Whether it changes anything.
    pub fn is_graded(&self) -> bool {
        self.applied().next().is_some()
    }

    /// The layers that change something, in order.
    pub fn applied(&self) -> impl Iterator<Item = &GradeLayer> {
        self.0
            .iter()
            .filter(|l| !l.off && !(l.primary.is_plain() && l.curves.is_plain()))
    }

    /// Layer `index`, made (with the layers before it) when missing.
    pub fn layer_mut(&mut self, index: usize) -> &mut GradeLayer {
        while self.0.len() <= index {
            let name = match self.0.len() {
                0 => Grade::FIRST_LAYER.to_owned(),
                n => format!("Layer {}", n + 1),
            };
            self.0.push(GradeLayer::new(name));
        }
        &mut self.0[index]
    }

    /// Refuses a grade that can't be used.
    pub fn check(&self) -> diag::Result<()> {
        if self.0.len() > 16 {
            return Err(
                AppError::new("That grade has too many layers").why("A grade can have up to 16 layers.")
            );
        }
        self.0.iter().try_for_each(|l| {
            l.primary.check()?;
            l.curves.all().iter().try_for_each(|c| c.check())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_grade_changes_nothing_and_a_layer_is_made_when_set() {
        let mut grade = Grade::default();
        assert!(!grade.is_graded());
        grade.layer_mut(0).primary.exposure = ONE / 2;
        assert!(grade.is_graded());
        assert_eq!(grade.0[0].name, Grade::FIRST_LAYER);
        grade.layer_mut(2);
        assert_eq!(grade.0.len(), 3);
        assert_eq!(grade.0[2].name, "Layer 3");
        grade.0[0].off = true;
        assert!(!grade.is_graded(), "the one that changes something is off");
    }

    #[test]
    fn a_curve_goes_through_its_points_smoothly_without_overshoot() {
        let plain = Curve::default();
        assert!(plain.is_plain());
        assert!((plain.at(0.3) - 0.3).abs() < 1e-9, "no points: straight");
        // An S: darker shadows, brighter highlights.
        let s = Curve(vec![(250_000, 180_000), (750_000, 820_000)]);
        assert!(!s.is_plain());
        assert!((s.at(0.25) - 0.18).abs() < 1e-9 && (s.at(0.75) - 0.82).abs() < 1e-9);
        let mut last = 0.0;
        for i in 0..=100 {
            let y = s.at(f64::from(i) / 100.0);
            assert!(y >= last - 1e-12, "rising all the way");
            last = y;
        }
        assert_eq!(s.at(1.0), 1.0);
        assert!(
            Curve(vec![(500_000, 100), (400_000, 100)]).check().is_err(),
            "out of order"
        );
    }

    #[test]
    fn numbers_out_of_range_are_refused() {
        let mut grade = Grade::default();
        grade.layer_mut(0).primary.saturation = 3 * ONE;
        assert!(grade.check().is_err());
        grade.layer_mut(0).primary.saturation = ONE;
        grade.layer_mut(0).primary.midtones_wheel.red = ONE + 1;
        assert!(grade.check().is_err());
        grade.layer_mut(0).primary.midtones_wheel.red = ONE;
        assert!(grade.check().is_ok());
    }
}
