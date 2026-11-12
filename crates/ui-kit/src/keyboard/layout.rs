//! Keyboards as they are drawn: a shape (which keys there are and how big)
//! and what is printed on each key.
//!
//! Shortcuts belong to places, not characters, so a layout never changes
//! what a key does: it changes what the drawing prints on it. Where a
//! layout prints something else than a US keyboard, the drawing shows both,
//! so the "S" of a shortcut can be found on any keyboard.
//!
//! This part knows nothing about the system: it is the built-in layouts,
//! the lookups and the geometry. Asking the system which layout is in use
//! is a separate, small job (see `README.md` here).

use egui::{Key, Rect, pos2, vec2};
use serde::{Deserialize, Serialize};

/// The physical shape of a keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Wide Enter on one row, a wide left Shift (US keyboards).
    Ansi,
    /// Tall Enter over two rows and one more key beside the left Shift
    /// (most European keyboards).
    Iso,
    /// Tall Enter, one more key beside Backspace and beside the right
    /// Shift, and conversion keys around a short Space (Japanese keyboards).
    Jis,
}

/// A place on the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spot {
    /// A key a shortcut can use.
    Key(Key),
    Ctrl,
    Shift,
    Alt,
    /// The key between the left Shift and Z on ISO keyboards.
    Extra,
    /// The key left of Backspace on JIS keyboards.
    Yen,
    /// The key left of the right Shift on JIS keyboards.
    Ro,
    /// A key that is only drawn, with what is printed on it.
    Plain(&'static str),
}

impl Spot {
    /// The key a shortcut would use here, if this is one.
    pub fn key(self) -> Option<Key> {
        match self {
            Spot::Key(key) => Some(key),
            _ => None,
        }
    }
}

/// One key of a drawing, in key units (a letter key is 1 by 1) from the
/// keyboard's top left corner. Each key fills its cell: the drawing leaves
/// the gap between keys.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cap {
    pub spot: Spot,
    pub rect: Rect,
    /// The lower part of a tall Enter: narrower, on the row below.
    pub lower: Option<Rect>,
}

impl Cap {
    /// Whether `at` (in key units) is on this key.
    pub fn contains(&self, at: egui::Pos2) -> bool {
        self.rect.contains(at) || self.lower.is_some_and(|l| l.contains(at))
    }

    /// The corners of the key's outline, clockwise from the top left,
    /// drawn `inset` inside its cell (the gap between keys).
    pub fn outline(&self, inset: f32) -> Vec<egui::Pos2> {
        let r = self.rect.shrink(inset);
        match self.lower.map(|l| l.shrink(inset)) {
            None => vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()],
            Some(l) => vec![
                r.left_top(),
                r.right_top(),
                l.right_bottom(),
                l.left_bottom(),
                pos2(l.left(), r.bottom()),
                r.left_bottom(),
            ],
        }
    }
}

/// Width of every drawing, in key units: the main block, a gap, and the
/// editing keys and arrows.
pub const WIDTH: f32 = MAIN_WIDTH + SIDE_GAP + 3.0;
/// Height of every drawing: six rows and a gap under the function keys.
pub const HEIGHT: f32 = 6.0 + FUNCTION_GAP;

const MAIN_WIDTH: f32 = 15.0;
const SIDE_GAP: f32 = 0.5;
const FUNCTION_GAP: f32 = 0.25;

/// A piece of a row, left to right.
#[derive(Clone, Copy)]
enum Piece {
    Cap(Spot, f32),
    Gap(f32),
    /// A tall Enter: this wide on its row, and that wide on the row below.
    Tall(Spot, f32, f32),
}

fn keys(row: &mut Vec<Piece>, keys: &[Key]) {
    row.extend(keys.iter().map(|k| Piece::Cap(Spot::Key(*k), 1.0)));
}

/// The rows of the main block for `shape`.
fn rows(shape: Shape) -> [Vec<Piece>; 6] {
    use Piece::{Cap, Gap, Tall};
    let key = |k: Key, w: f32| Cap(Spot::Key(k), w);
    let tall_enter = shape != Shape::Ansi;

    let mut function = vec![key(Key::Escape, 1.0), Gap(1.0)];
    keys(&mut function, &[Key::F1, Key::F2, Key::F3, Key::F4]);
    function.push(Gap(0.5));
    keys(&mut function, &[Key::F5, Key::F6, Key::F7, Key::F8]);
    function.push(Gap(0.5));
    keys(&mut function, &[Key::F9, Key::F10, Key::F11, Key::F12]);

    let mut numbers = Vec::new();
    keys(
        &mut numbers,
        &[
            Key::Backtick,
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
            Key::Num0,
            Key::Minus,
            Key::Equals,
        ],
    );
    if shape == Shape::Jis {
        numbers.push(Cap(Spot::Yen, 1.0));
        numbers.push(key(Key::Backspace, 1.0));
    } else {
        numbers.push(key(Key::Backspace, 2.0));
    }

    let mut top = vec![key(Key::Tab, 1.5)];
    keys(
        &mut top,
        &[
            Key::Q,
            Key::W,
            Key::E,
            Key::R,
            Key::T,
            Key::Y,
            Key::U,
            Key::I,
            Key::O,
            Key::P,
            Key::OpenBracket,
            Key::CloseBracket,
        ],
    );
    top.push(if tall_enter {
        Tall(Spot::Key(Key::Enter), 1.5, 1.25)
    } else {
        key(Key::Backslash, 1.5)
    });

    let mut home = vec![Cap(Spot::Plain("Caps"), 1.75)];
    keys(
        &mut home,
        &[
            Key::A,
            Key::S,
            Key::D,
            Key::F,
            Key::G,
            Key::H,
            Key::J,
            Key::K,
            Key::L,
            Key::Semicolon,
            Key::Quote,
        ],
    );
    // With a tall Enter, the key that sits above Enter on US keyboards is
    // here, left of it.
    home.push(if tall_enter {
        key(Key::Backslash, 1.0)
    } else {
        key(Key::Enter, 2.25)
    });

    let mut bottom = match shape {
        Shape::Iso => vec![Cap(Spot::Shift, 1.25), Cap(Spot::Extra, 1.0)],
        Shape::Ansi | Shape::Jis => vec![Cap(Spot::Shift, 2.25)],
    };
    keys(
        &mut bottom,
        &[
            Key::Z,
            Key::X,
            Key::C,
            Key::V,
            Key::B,
            Key::N,
            Key::M,
            Key::Comma,
            Key::Period,
            Key::Slash,
        ],
    );
    if shape == Shape::Jis {
        bottom.push(Cap(Spot::Ro, 1.0));
        bottom.push(Cap(Spot::Shift, 1.75));
    } else {
        bottom.push(Cap(Spot::Shift, 2.75));
    }

    let system = Cap(Spot::Plain(""), 1.25);
    let space = if shape == Shape::Jis {
        vec![
            Cap(Spot::Plain("Muhen"), 1.25),
            key(Key::Space, 4.5),
            Cap(Spot::Plain("Henkan"), 1.25),
            Cap(Spot::Plain("Kana"), 1.25),
        ]
    } else {
        vec![key(Key::Space, 7.0), Cap(Spot::Alt, 1.25), system]
    };
    let mut modifiers = vec![Cap(Spot::Ctrl, 1.5), system, Cap(Spot::Alt, 1.25)];
    modifiers.extend(space);
    if shape == Shape::Jis {
        modifiers.push(Cap(Spot::Alt, 1.25));
    }
    modifiers.push(Cap(Spot::Ctrl, 1.5));

    [function, numbers, top, home, bottom, modifiers]
}

/// The editing keys and arrows right of the main block: key, column, row.
const SIDE: [(Key, f32, usize); 10] = [
    (Key::Insert, 0.0, 1),
    (Key::Home, 1.0, 1),
    (Key::PageUp, 2.0, 1),
    (Key::Delete, 0.0, 2),
    (Key::End, 1.0, 2),
    (Key::PageDown, 2.0, 2),
    (Key::ArrowUp, 1.0, 4),
    (Key::ArrowLeft, 0.0, 5),
    (Key::ArrowDown, 1.0, 5),
    (Key::ArrowRight, 2.0, 5),
];

/// Where row `row` starts, from the top.
fn row_top(row: usize) -> f32 {
    row as f32 + if row > 0 { FUNCTION_GAP } else { 0.0 }
}

impl Shape {
    /// Every key of a keyboard of this shape.
    pub fn caps(self) -> Vec<Cap> {
        let mut caps = Vec::new();
        for (row, pieces) in rows(self).iter().enumerate() {
            let (mut x, y) = (0.0, row_top(row));
            for piece in pieces {
                let width = match *piece {
                    Piece::Gap(width) => width,
                    Piece::Cap(spot, width) => {
                        caps.push(Cap {
                            spot,
                            rect: Rect::from_min_size(pos2(x, y), vec2(width, 1.0)),
                            lower: None,
                        });
                        width
                    }
                    Piece::Tall(spot, width, lower) => {
                        let right = x + width;
                        caps.push(Cap {
                            spot,
                            rect: Rect::from_min_size(pos2(x, y), vec2(width, 1.0)),
                            lower: Some(Rect::from_min_max(
                                pos2(right - lower, row_top(row + 1)),
                                pos2(right, row_top(row + 1) + 1.0),
                            )),
                        });
                        width
                    }
                };
                x += width;
            }
        }
        for (key, column, row) in SIDE {
            caps.push(Cap {
                spot: Spot::Key(key),
                rect: Rect::from_min_size(pos2(MAIN_WIDTH + SIDE_GAP + column, row_top(row)), vec2(1.0, 1.0)),
                lower: None,
            });
        }
        caps
    }
}

/// What a US keyboard prints at `spot`: the name shortcuts go by. Empty
/// for places a US keyboard doesn't have.
pub fn us_legend(spot: Spot) -> &'static str {
    match spot {
        Spot::Ctrl => "Ctrl",
        Spot::Shift => "Shift",
        Spot::Alt => "Alt",
        Spot::Plain(text) => text,
        Spot::Extra | Spot::Yen | Spot::Ro => "",
        Spot::Key(key) => match key {
            Key::Escape => "Esc",
            Key::Insert => "Ins",
            Key::Delete => "Del",
            Key::PageUp => "PgUp",
            Key::PageDown => "PgDn",
            Key::ArrowUp => "↑",
            Key::ArrowLeft => "←",
            Key::ArrowDown => "↓",
            Key::ArrowRight => "→",
            Key::Minus => "-",
            Key::Quote => "'",
            _ => key.symbol_or_name(),
        },
    }
}

/// A keyboard layout: its shape and what is printed where.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub shape: Shape,
    /// What this layout prints where a US keyboard prints something else
    /// (or has no key).
    printed: &'static [(Spot, &'static str)],
}

impl Layout {
    /// What is printed at `spot`.
    pub fn legend(&self, spot: Spot) -> &'static str {
        self.printed
            .iter()
            .find(|(s, _)| *s == spot)
            .map_or_else(|| us_legend(spot), |(_, text)| text)
    }

    /// What is printed on `cap`: its legend, shortened on a key too
    /// narrow for it.
    pub fn printed_on(&self, cap: &Cap) -> &'static str {
        match cap.spot {
            Spot::Key(Key::Backspace) if cap.rect.width() < 1.5 => "Bksp",
            spot => self.legend(spot),
        }
    }

    /// What a US keyboard prints at `spot`, when this layout prints
    /// something else: the second, smaller mark on the key.
    pub fn us_mark(&self, spot: Spot) -> Option<&'static str> {
        let us = us_legend(spot);
        (!us.is_empty() && self.legend(spot) != us).then_some(us)
    }

    /// A shortcut's keys as this keyboard prints them: "Ctrl+-" for
    /// Ctrl+Slash on a Norwegian keyboard, and the usual name ("Ctrl+Slash")
    /// where it prints what a US keyboard does. A printed "+" is written
    /// "Plus", so "Alt+Plus" never reads as two keys.
    pub fn written(&self, keys: crate::Shortcut) -> String {
        match self
            .us_mark(Spot::Key(keys.key))
            .map(|_| self.legend(Spot::Key(keys.key)))
        {
            Some("+") => keys.written("Plus"),
            Some(printed) => keys.written(printed),
            None => keys.label(),
        }
    }

    /// A shortcut's keys as this keyboard prints them and, when that is not
    /// what the shortcut is called, the name too: "Ctrl+- (Slash on a US
    /// keyboard)". For the shortcuts page and the cheat sheet, where a
    /// shortcut is looked up as well as read.
    pub fn describe(&self, keys: crate::Shortcut) -> String {
        match self.us_mark(Spot::Key(keys.key)) {
            Some(_) => format!("{} ({} on a US keyboard)", self.written(keys), keys.key.name()),
            None => keys.label(),
        }
    }
}

const US: Layout = Layout {
    shape: Shape::Ansi,
    printed: &[],
};

const UK: Layout = Layout {
    shape: Shape::Iso,
    printed: &[(Spot::Key(Key::Backslash), "#"), (Spot::Extra, "\\")],
};

const NORWEGIAN: Layout = Layout {
    shape: Shape::Iso,
    printed: &[
        (Spot::Key(Key::Backtick), "|"),
        (Spot::Key(Key::Minus), "+"),
        (Spot::Key(Key::Equals), "\\"),
        (Spot::Key(Key::OpenBracket), "Å"),
        (Spot::Key(Key::CloseBracket), "¨"),
        (Spot::Key(Key::Semicolon), "Ø"),
        (Spot::Key(Key::Quote), "Æ"),
        (Spot::Key(Key::Backslash), "'"),
        (Spot::Key(Key::Slash), "-"),
        (Spot::Extra, "<"),
    ],
};

const JAPANESE: Layout = Layout {
    shape: Shape::Jis,
    printed: &[
        (Spot::Key(Key::Backtick), "H/Z"),
        (Spot::Key(Key::Equals), "^"),
        (Spot::Yen, "¥"),
        (Spot::Key(Key::OpenBracket), "@"),
        (Spot::Key(Key::CloseBracket), "["),
        (Spot::Key(Key::Quote), ":"),
        (Spot::Key(Key::Backslash), "]"),
        (Spot::Ro, "\\"),
    ],
};

/// The layouts the drawing knows, as settings and the dropdown name them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LayoutId {
    #[default]
    Us,
    Uk,
    Norwegian,
    Japanese,
}

impl LayoutId {
    pub const ALL: [LayoutId; 4] = [
        LayoutId::Us,
        LayoutId::Uk,
        LayoutId::Norwegian,
        LayoutId::Japanese,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LayoutId::Us => "US English",
            LayoutId::Uk => "UK English",
            LayoutId::Norwegian => "Norwegian",
            LayoutId::Japanese => "Japanese",
        }
    }

    pub fn layout(self) -> &'static Layout {
        match self {
            LayoutId::Us => &US,
            LayoutId::Uk => &UK,
            LayoutId::Norwegian => &NORWEGIAN,
            LayoutId::Japanese => &JAPANESE,
        }
    }

    /// The layout for a Windows language identifier (the low word of the
    /// active keyboard layout's handle), if the drawing knows it.
    pub fn from_windows_language(language: u16) -> Option<LayoutId> {
        // The low ten bits are the language, the rest the country.
        match (language, language & 0x03FF) {
            (0x0809, _) => Some(LayoutId::Uk),
            (_, 0x09) => Some(LayoutId::Us),
            (_, 0x14) => Some(LayoutId::Norwegian),
            (_, 0x11) => Some(LayoutId::Japanese),
            _ => None,
        }
    }

    /// The layout for an XKB layout name ("no", or the first of
    /// "no,us"), if the drawing knows it.
    pub fn from_xkb_name(names: &str) -> Option<LayoutId> {
        match names.split(',').next()?.trim() {
            "us" => Some(LayoutId::Us),
            "gb" => Some(LayoutId::Uk),
            "no" => Some(LayoutId::Norwegian),
            "jp" => Some(LayoutId::Japanese),
            _ => None,
        }
    }
}

/// Which keyboard the page draws and shortcuts are written for: the one
/// the system reports, or one picked by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum KeyboardChoice {
    /// Whatever the system reports, following switches.
    #[default]
    Automatic,
    /// This one, whatever the system says.
    Fixed(LayoutId),
}

impl KeyboardChoice {
    /// The layout to draw: the picked one, or the system's when the drawing
    /// knows it, or US English.
    pub fn resolve(self, system: &SystemKeyboard) -> LayoutId {
        match self {
            KeyboardChoice::Fixed(id) => id,
            KeyboardChoice::Automatic => system.layout.unwrap_or_default(),
        }
    }
}

/// What the system said about its keyboard, as the page shows it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SystemKeyboard {
    /// The layout, when the drawing knows it.
    pub layout: Option<LayoutId>,
    /// The system's own name for a layout the drawing doesn't know ("ru"),
    /// to say which one it is.
    pub unknown: Option<String>,
}

impl SystemKeyboard {
    /// From a Windows language identifier.
    pub fn from_windows_language(language: u16) -> SystemKeyboard {
        let layout = LayoutId::from_windows_language(language);
        SystemKeyboard {
            layout,
            unknown: layout.is_none().then(|| format!("language {language:04X}")),
        }
    }

    /// From an XKB layout name.
    pub fn from_xkb_name(name: &str) -> SystemKeyboard {
        let layout = LayoutId::from_xkb_name(name);
        SystemKeyboard {
            layout,
            unknown: layout.is_none().then(|| format!("\"{name}\"")),
        }
    }

    /// One line on what the system reports: "Norwegian", or why US English
    /// is drawn instead.
    pub fn summary(&self) -> String {
        match (self.layout, &self.unknown) {
            (Some(id), _) => id.label().to_owned(),
            (None, Some(name)) => format!("{name}, not drawn here yet"),
            (None, None) => "not reported".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Shortcut;

    fn spots(shape: Shape) -> Vec<Spot> {
        shape.caps().iter().map(|c| c.spot).collect()
    }

    #[test]
    fn every_shape_fills_its_rows_exactly_and_no_keys_overlap() {
        for shape in [Shape::Ansi, Shape::Iso, Shape::Jis] {
            for (row, pieces) in rows(shape).iter().enumerate() {
                let width: f32 = pieces
                    .iter()
                    .map(|p| match p {
                        Piece::Cap(_, w) | Piece::Gap(w) | Piece::Tall(_, w, _) => *w,
                    })
                    .sum();
                // The row under a tall Enter stops where its lower part begins.
                let under_enter = shape != Shape::Ansi && row == 3;
                let expected = if under_enter {
                    MAIN_WIDTH - 1.25
                } else {
                    MAIN_WIDTH
                };
                assert_eq!(width, expected, "{shape:?} row {row}");
            }
            let caps = shape.caps();
            let parts: Vec<Rect> = caps
                .iter()
                .flat_map(|c| std::iter::once(c.rect).chain(c.lower))
                .collect();
            for (i, a) in parts.iter().enumerate() {
                assert!(a.min.x >= 0.0 && a.max.x <= WIDTH && a.max.y <= HEIGHT);
                for b in &parts[i + 1..] {
                    assert!(
                        !a.shrink(0.01).intersects(b.shrink(0.01)),
                        "{shape:?}: {a:?} and {b:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_shape_has_every_key_a_us_keyboard_has_once() {
        let ansi = spots(Shape::Ansi);
        for shape in [Shape::Ansi, Shape::Iso, Shape::Jis] {
            let here = spots(shape);
            for spot in ansi.iter().filter(|s| s.key().is_some()) {
                assert_eq!(
                    here.iter().filter(|s| *s == spot).count(),
                    1,
                    "{shape:?} {spot:?}"
                );
            }
        }
    }

    #[test]
    fn the_shapes_differ_where_real_keyboards_do() {
        let (ansi, iso, jis) = (Shape::Ansi.caps(), Shape::Iso.caps(), Shape::Jis.caps());
        let find = |caps: &[Cap], spot: Spot| *caps.iter().find(|c| c.spot == spot).unwrap();
        let enter = Spot::Key(Key::Enter);
        let backslash = Spot::Key(Key::Backslash);

        // ANSI: a wide Enter on the home row, the backslash key above it.
        assert_eq!(find(&ansi, enter).lower, None);
        assert_eq!(find(&ansi, enter).rect.width(), 2.25);
        assert!(find(&ansi, backslash).rect.top() < find(&ansi, enter).rect.top());
        assert!(!spots(Shape::Ansi).contains(&Spot::Extra));

        for tall in [&iso, &jis] {
            // A tall Enter over two rows, with that key left of its lower part.
            let (enter, key) = (find(tall, enter), find(tall, backslash));
            let lower = enter.lower.unwrap();
            assert_eq!((enter.rect.width(), lower.width()), (1.5, 1.25));
            assert_eq!(lower.top(), enter.rect.bottom());
            assert_eq!(lower.right(), enter.rect.right());
            assert_eq!((key.rect.right(), key.rect.top()), (lower.left(), lower.top()));
            assert_eq!(enter.outline(0.0).len(), 6);
            assert_eq!(enter.outline(0.1)[4], pos2(lower.left() + 0.1, lower.top() - 0.1));
            assert!(enter.contains(lower.center()) && enter.contains(enter.rect.center()));
        }

        // ISO: one more key between a short left Shift and Z.
        let extra = find(&iso, Spot::Extra);
        assert_eq!(extra.rect.right(), find(&iso, Spot::Key(Key::Z)).rect.left());
        assert_eq!(find(&iso, Spot::Shift).rect.width(), 1.25);

        // JIS: a key before a short Backspace, one before the right Shift,
        // and conversion keys around a short Space.
        assert_eq!(
            find(&jis, Spot::Yen).rect.right(),
            find(&jis, Spot::Key(Key::Backspace)).rect.left()
        );
        assert_eq!(find(&jis, Spot::Key(Key::Backspace)).rect.width(), 1.0);
        assert_eq!(
            find(&jis, Spot::Ro).rect.left(),
            find(&jis, Spot::Key(Key::Slash)).rect.right()
        );
        assert!(
            find(&jis, Spot::Key(Key::Space)).rect.width() < find(&ansi, Spot::Key(Key::Space)).rect.width()
        );
        assert!(spots(Shape::Jis).contains(&Spot::Plain("Kana")));
    }

    #[test]
    fn a_us_keyboard_prints_what_shortcuts_are_called() {
        let us = LayoutId::Us.layout();
        assert_eq!(us.legend(Spot::Key(Key::S)), "S");
        assert_eq!(us.legend(Spot::Key(Key::Slash)), "/");
        assert_eq!(us.legend(Spot::Key(Key::Minus)), "-", "a plain hyphen, as typed");
        assert_eq!(us.legend(Spot::Key(Key::F5)), "F5");
        assert_eq!(us.legend(Spot::Shift), "Shift");
        for cap in Shape::Ansi.caps() {
            assert_eq!(us.us_mark(cap.spot), None);
        }
        assert_eq!(us.describe(Shortcut::ctrl(Key::Slash)), "Ctrl+Slash");
    }

    #[test]
    fn other_layouts_print_their_own_characters_and_keep_the_us_one_findable() {
        let no = LayoutId::Norwegian.layout();
        assert_eq!(no.shape, Shape::Iso);
        assert_eq!(no.legend(Spot::Key(Key::Semicolon)), "Ø");
        assert_eq!(no.legend(Spot::Key(Key::Quote)), "Æ");
        assert_eq!(no.legend(Spot::Key(Key::OpenBracket)), "Å");
        assert_eq!(no.us_mark(Spot::Key(Key::Semicolon)), Some(";"));
        assert_eq!(no.legend(Spot::Key(Key::S)), "S", "letters stay where they are");
        assert_eq!(no.us_mark(Spot::Key(Key::S)), None);
        assert_eq!(no.legend(Spot::Extra), "<");
        assert_eq!(no.us_mark(Spot::Extra), None, "a US keyboard has no such key");
        assert_eq!(
            no.describe(Shortcut::ctrl(Key::Slash)),
            "Ctrl+- (Slash on a US keyboard)"
        );
        assert_eq!(no.describe(Shortcut::ctrl(Key::S)), "Ctrl+S");
        assert_eq!(no.written(Shortcut::ctrl(Key::Slash)), "Ctrl+-");
        assert_eq!(no.written(Shortcut::alt(Key::Minus)), "Alt+Plus", "never Alt++");
        assert_eq!(
            no.written(Shortcut::ctrl(Key::Comma)),
            "Ctrl+Comma",
            "printed as on US keyboards"
        );
        assert_eq!(no.written(Shortcut::ctrl_shift(Key::Z)), "Ctrl+Shift+Z");

        let uk = LayoutId::Uk.layout();
        assert_eq!(uk.shape, Shape::Iso);
        assert_eq!(uk.legend(Spot::Key(Key::Backslash)), "#");
        assert_eq!(uk.us_mark(Spot::Key(Key::Backslash)), Some("\\"));
        assert_eq!(uk.legend(Spot::Extra), "\\");

        let jp = LayoutId::Japanese.layout();
        assert_eq!(jp.shape, Shape::Jis);
        assert_eq!(jp.legend(Spot::Yen), "¥");
        assert_eq!(jp.legend(Spot::Key(Key::OpenBracket)), "@");
        assert_eq!(jp.us_mark(Spot::Key(Key::OpenBracket)), Some("["));
        let backspace = |shape: Shape| {
            let caps = shape.caps();
            *caps.iter().find(|c| c.spot == Spot::Key(Key::Backspace)).unwrap()
        };
        assert_eq!(jp.printed_on(&backspace(Shape::Jis)), "Bksp", "a short key");
        assert_eq!(uk.printed_on(&backspace(Shape::Iso)), "Backspace");
    }

    #[test]
    fn a_layout_only_prints_on_keys_its_shape_has() {
        for id in LayoutId::ALL {
            let layout = id.layout();
            let there = spots(layout.shape);
            for (spot, text) in layout.printed {
                assert!(there.contains(spot), "{id:?} prints {text} on a key it lacks");
                assert_ne!(
                    *text,
                    us_legend(*spot),
                    "{id:?}: {text} is what a US keyboard prints"
                );
            }
            // Every key says something, except the two system keys.
            for cap in layout.shape.caps() {
                assert!(
                    !layout.legend(cap.spot).is_empty() || cap.spot == Spot::Plain(""),
                    "{id:?} {:?}",
                    cap.spot
                );
            }
        }
    }

    #[test]
    fn layouts_are_found_from_what_systems_call_them() {
        assert_eq!(LayoutId::from_windows_language(0x0409), Some(LayoutId::Us));
        assert_eq!(LayoutId::from_windows_language(0x0809), Some(LayoutId::Uk));
        assert_eq!(LayoutId::from_windows_language(0x0414), Some(LayoutId::Norwegian));
        assert_eq!(LayoutId::from_windows_language(0x0814), Some(LayoutId::Norwegian));
        assert_eq!(LayoutId::from_windows_language(0x0411), Some(LayoutId::Japanese));
        assert_eq!(LayoutId::from_windows_language(0x0419), None, "not drawn yet");
        assert_eq!(LayoutId::from_xkb_name("no"), Some(LayoutId::Norwegian));
        assert_eq!(LayoutId::from_xkb_name("gb,us"), Some(LayoutId::Uk));
        assert_eq!(LayoutId::from_xkb_name("jp"), Some(LayoutId::Japanese));
        assert_eq!(LayoutId::from_xkb_name("us"), Some(LayoutId::Us));
        assert_eq!(LayoutId::from_xkb_name("ru"), None);
        assert_eq!(LayoutId::from_xkb_name(""), None);
    }

    #[test]
    fn automatic_follows_the_system_and_a_fixed_choice_does_not() {
        let norwegian = SystemKeyboard::from_xkb_name("no");
        assert_eq!(norwegian.summary(), "Norwegian");
        assert_eq!(KeyboardChoice::Automatic.resolve(&norwegian), LayoutId::Norwegian);
        assert_eq!(
            KeyboardChoice::Fixed(LayoutId::Uk).resolve(&norwegian),
            LayoutId::Uk
        );

        // A layout the drawing doesn't know, or none reported: US English.
        let russian = SystemKeyboard::from_windows_language(0x0419);
        assert_eq!(russian.layout, None);
        assert_eq!(russian.summary(), "language 0419, not drawn here yet");
        assert_eq!(KeyboardChoice::Automatic.resolve(&russian), LayoutId::Us);
        assert_eq!(
            SystemKeyboard::from_xkb_name("ru").summary(),
            "\"ru\", not drawn here yet"
        );
        let nothing = SystemKeyboard::default();
        assert_eq!(nothing.summary(), "not reported");
        assert_eq!(KeyboardChoice::Automatic.resolve(&nothing), LayoutId::Us);
        assert_eq!(
            SystemKeyboard::from_windows_language(0x0411).layout,
            Some(LayoutId::Japanese)
        );
        assert_eq!(KeyboardChoice::default(), KeyboardChoice::Automatic);
    }

    #[test]
    fn japanese_and_uk_keyboards_write_their_own_characters() {
        let jp = LayoutId::Japanese.layout();
        assert_eq!(jp.written(Shortcut::key(Key::Equals)), "^");
        assert_eq!(jp.written(Shortcut::key(Key::Backslash)), "]");
        let uk = LayoutId::Uk.layout();
        assert_eq!(uk.written(Shortcut::alt(Key::Backslash)), "Alt+#");
        let us = LayoutId::Us.layout();
        assert_eq!(us.written(Shortcut::ctrl(Key::Slash)), "Ctrl+Slash");
    }
}
