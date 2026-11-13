//! The printable cheat sheet: one HTML file with nothing to fetch, made
//! from the shortcut table as it is now. Keyboard pictures (one per
//! combination of Ctrl, Shift and Alt that does something), then every
//! command by context, then the mouse.

use super::layout::{Cap, HEIGHT, Layout, Spot, WIDTH};
use crate::shortcuts::mouse::{MOUSE, MouseAction};
use crate::shortcuts::{Context, Shortcut, ShortcutTable};
use std::fmt::Write;

/// Points per key unit in the pictures.
const UNIT: f32 = 40.0;

const STYLE: &str = "
body { font: 14px/1.4 system-ui, sans-serif; color: #111; background: #fff; margin: 24px auto; max-width: 980px; padding: 0 16px; }
h1 { font-size: 22px; margin: 0 0 4px; }
h2 { font-size: 16px; margin: 20px 0 6px; }
h3 { font-size: 13px; margin: 14px 0 4px; }
p.note { color: #555; margin: 0 0 12px; }
.dot { display: inline-block; width: 10px; height: 10px; border-radius: 50%; margin-right: 6px; }
.chips span.chip { margin-right: 16px; white-space: nowrap; }
figure { margin: 12px 0; break-inside: avoid; }
figcaption { font-weight: 600; margin-bottom: 4px; }
.small { display: grid; grid-template-columns: 1fr 1fr; gap: 0 24px; }
svg { width: 100%; height: auto; display: block; }
svg text { font: 11px system-ui, sans-serif; text-anchor: middle; dominant-baseline: central; fill: #111; }
svg text.us { font-size: 8px; fill: #555; }
svg .cap { fill: #fff; stroke: #999; stroke-width: 1; stroke-linejoin: round; }
svg .held .cap { fill: #ccc; }
svg .idle text { fill: #999; }
.small svg text { font-size: 14px; }
.small svg text.us { font-size: 10px; }
h2, h3 { break-after: avoid; }
table { border-collapse: collapse; width: 100%; table-layout: fixed; }
tr { break-inside: avoid; }
td { padding: 3px 10px 3px 0; border-bottom: 1px solid #ddd; vertical-align: top; width: 30%; }
td.keys { font-family: ui-monospace, monospace; font-size: 13px; width: 20%; }
td.gesture { width: 20%; }
td.none { color: #777; }
td.gap { border-bottom: none; }
* { print-color-adjust: exact; }
@media print { body { margin: 0; max-width: none; } }
";

/// Text made safe to put in HTML.
fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// A colour as CSS writes it.
fn css(colour: egui::Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", colour.r(), colour.g(), colour.b())
}

fn dot(context: Context) -> String {
    format!(
        "<span class=\"dot\" style=\"background:{}\"></span>",
        css(context.colour())
    )
}

/// The combinations of Ctrl, Shift and Alt the table uses, plain keys
/// first, then one modifier, then more.
pub fn modifier_sets(table: &ShortcutTable) -> Vec<(bool, bool, bool)> {
    let mut sets = vec![(false, false, false)];
    for (_, keys) in table.bindings() {
        let set = (keys.ctrl, keys.shift, keys.alt);
        if !sets.contains(&set) {
            sets.push(set);
        }
    }
    let count = |s: &(bool, bool, bool)| u8::from(s.0) + u8::from(s.1) + u8::from(s.2);
    sets.sort_by_key(|s| (count(s), !s.0, !s.1, !s.2));
    sets
}

/// "Plain keys", "With Ctrl", "With Ctrl+Shift".
pub fn modifier_title((ctrl, shift, alt): (bool, bool, bool)) -> String {
    let held: Vec<&str> = [(ctrl, "Ctrl"), (shift, "Shift"), (alt, "Alt")]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| *name)
        .collect();
    if held.is_empty() {
        "Plain keys".to_owned()
    } else {
        format!("With {}", held.join("+"))
    }
}

/// One keyboard picture: what every key does with `set` held.
fn keyboard(out: &mut String, table: &ShortcutTable, layout: &Layout, set: (bool, bool, bool)) {
    let _ = write!(
        out,
        "<svg viewBox=\"0 0 {} {}\" role=\"img\">",
        WIDTH * UNIT,
        HEIGHT * UNIT
    );
    for cap in layout.shape.caps() {
        key(out, table, layout, set, &cap);
    }
    out.push_str("</svg>");
}

fn key(out: &mut String, table: &ShortcutTable, layout: &Layout, set: (bool, bool, bool), cap: &Cap) {
    let (ctrl, shift, alt) = set;
    let action = cap.spot.key().and_then(|key| {
        let keys = Shortcut {
            key,
            ctrl,
            shift,
            alt,
        };
        table.bindings().find(|(_, s)| *s == keys).map(|(a, _)| a)
    });
    let held = matches!(cap.spot, Spot::Ctrl if ctrl)
        || matches!(cap.spot, Spot::Shift if shift)
        || matches!(cap.spot, Spot::Alt if alt);
    let points: Vec<String> = cap
        .outline(0.05)
        .iter()
        .map(|p| format!("{},{}", p.x * UNIT, p.y * UNIT))
        .collect();
    let class = match (action, held) {
        (Some(_), _) => "",
        (None, true) => " class=\"held\"",
        (None, false) => " class=\"idle\"",
    };
    let _ = write!(out, "<g{class}>");
    if let Some(action) = action {
        let _ = write!(out, "<title>{}</title>", escaped(action.label()));
    }
    let fill = action.map_or_else(String::new, |a| {
        format!(" style=\"fill:{};fill-opacity:0.45\"", css(a.context().colour()))
    });
    let _ = write!(
        out,
        "<polygon class=\"cap\"{fill} points=\"{}\"/>",
        points.join(" ")
    );
    let centre = cap.rect.center();
    let _ = write!(
        out,
        "<text x=\"{}\" y=\"{}\">{}</text>",
        centre.x * UNIT,
        centre.y * UNIT,
        escaped(layout.printed_on(cap))
    );
    if let Some(us) = layout.us_mark(cap.spot) {
        let _ = write!(
            out,
            "<text class=\"us\" x=\"{}\" y=\"{}\">{}</text>",
            (cap.rect.left() + 0.25) * UNIT,
            (cap.rect.bottom() - 0.25) * UNIT,
            escaped(us)
        );
    }
    out.push_str("</g>");
}

/// One context's lines under its heading, two abreast: the first half
/// down the left, the rest down the right. Nothing for no lines.
fn section(out: &mut String, context: Context, cells: &[[String; 2]]) {
    if cells.is_empty() {
        return;
    }
    let _ = write!(
        out,
        "<section><h3>{}{}</h3><table>",
        dot(context),
        escaped(context.label())
    );
    let half = cells.len().div_ceil(2);
    for (i, left) in cells.iter().take(half).enumerate() {
        let _ = write!(out, "<tr>{}{}", left[0], left[1]);
        match cells.get(half + i) {
            Some(right) => {
                let _ = write!(out, "{}{}", right[0], right[1]);
            }
            None => out.push_str("<td class=\"gap\"></td><td class=\"gap\"></td>"),
        }
        out.push_str("</tr>");
    }
    out.push_str("</table></section>\n");
}

/// The whole sheet. `app` is the app's name and `layout_name` what the
/// layout is called, for the heading and the note under it.
/// A mouse line, naming keys as this sheet's keyboard prints them.
fn mouse_says(line: &MouseAction, table: &ShortcutTable, layout: &Layout) -> String {
    line.says_with(|action| {
        table
            .shortcut_for(action)
            .map_or_else(|| action.label().to_owned(), |keys| layout.describe(keys))
    })
}

pub fn html(table: &ShortcutTable, layout: &Layout, layout_name: &str, app: &str) -> String {
    let mut out = String::with_capacity(96 * 1024);
    let title = format!("{} keyboard shortcuts", escaped(app));
    let _ = write!(
        out,
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>{title}</title>\n<style>{STYLE}</style>\n</head>\n<body>\n<h1>{title}</h1>\n"
    );
    let _ = writeln!(
        out,
        "<p class=\"note\">Keyboard: {}. Shortcuts belong to the key's place, so they sit in the same spot on every keyboard; keys are named as on a US keyboard.</p>",
        escaped(layout_name)
    );
    out.push_str("<p class=\"chips\">");
    for context in Context::ALL {
        let _ = write!(
            out,
            "<span class=\"chip\">{}{}</span>",
            dot(context),
            escaped(context.label())
        );
    }
    out.push_str("</p>\n");

    let sets = modifier_sets(table);
    for (i, set) in sets.iter().enumerate() {
        // The plain keyboard at full width, the others two abreast.
        if i == 1 {
            out.push_str("<div class=\"small\">\n");
        }
        let _ = write!(out, "<figure><figcaption>{}</figcaption>", modifier_title(*set));
        keyboard(&mut out, table, layout, *set);
        out.push_str("</figure>\n");
    }
    if sets.len() > 1 {
        out.push_str("</div>\n");
    }

    out.push_str("<h2>Keys</h2>\n");
    for context in Context::ALL {
        let cells: Vec<[String; 2]> = table
            .lines()
            .iter()
            .filter(|b| b.action.context() == context)
            .map(|binding| {
                let keys = match binding.keys {
                    Some(keys) => format!("<td class=\"keys\">{}</td>", escaped(&layout.describe(keys))),
                    None => "<td class=\"keys none\">no shortcut</td>".to_owned(),
                };
                [keys, format!("<td>{}</td>", escaped(binding.action.label()))]
            })
            .collect();
        section(&mut out, context, &cells);
    }
    out.push_str("<h2>Mouse</h2>\n");
    for context in Context::ALL {
        let cells: Vec<[String; 2]> = MOUSE
            .iter()
            .filter(|m| m.context == context)
            .map(|line| {
                [
                    format!("<td class=\"gesture\">{}</td>", escaped(line.gesture)),
                    format!("<td>{}</td>", escaped(&mouse_says(line, table, layout))),
                ]
            })
            .collect();
        section(&mut out, context, &cells);
    }
    out.push_str("</body>\n</html>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Action;
    use crate::keyboard::layout::LayoutId;
    use egui::Key;

    fn sheet(table: &ShortcutTable, layout: LayoutId) -> String {
        html(table, layout.layout(), layout.label(), "Some editor")
    }

    #[test]
    fn the_sheet_lists_every_command_with_its_keys_under_its_context() {
        let table = ShortcutTable::default();
        let page = sheet(&table, LayoutId::Us);
        assert!(page.starts_with("<!DOCTYPE html>"));
        assert!(page.contains("<title>Some editor keyboard shortcuts</title>"));
        for binding in table.lines() {
            let keys = binding
                .keys
                .map_or_else(|| "no shortcut".to_owned(), |k| k.label());
            let row = format!(
                "{}</td><td>{}</td>",
                escaped(&keys),
                escaped(binding.action.label())
            );
            assert!(page.contains(&row), "{row}");
        }
        // Grouped by context, in order, each under its heading.
        let at = |text: &str| page.find(text).unwrap_or_else(|| panic!("{text} is missing"));
        let keys = at("<h2>Keys</h2>");
        let (anywhere, viewer, timeline) = (
            keys + page[keys..].find("Anywhere</h3>").unwrap(),
            keys + page[keys..].find("Viewer</h3>").unwrap(),
            keys + page[keys..].find("Timeline</h3>").unwrap(),
        );
        assert!(anywhere < at("<td>Undo</td>") && at("<td>Undo</td>") < viewer);
        assert!(viewer < at("<td>Play / pause</td>") && at("<td>Play / pause</td>") < timeline);
        assert!(timeline < at("<td>Split at the edit needle</td>"));
        assert!(at("<td>Split at the edit needle</td>") < at("<h2>Mouse</h2>"));
    }

    #[test]
    fn the_sheet_shows_the_keys_in_use_not_the_usual_ones() {
        let mut table = ShortcutTable::default();
        let split = table
            .lines()
            .iter()
            .position(|b| b.action == Action::Split)
            .unwrap();
        table.swap(split, Shortcut::ctrl(Key::S));
        let page = sheet(&table, LayoutId::Us);
        assert!(page.contains("<td class=\"keys\">Ctrl+S</td><td>Split at the edit needle</td>"));
        assert!(page.contains("<td class=\"keys\">S</td><td>Save</td>"));
        assert!(!page.contains("<td class=\"keys\">S</td><td>Split at the edit needle</td>"));
        // And the picture: the plain S key now says Save when pointed at.
        let plain = &page[page.find("Plain keys").unwrap()..page.find("With Ctrl").unwrap()];
        assert!(plain.contains("<title>Save</title>"));
        assert!(!plain.contains("<title>Split at the edit needle</title>"));
    }

    #[test]
    fn the_sheet_has_a_picture_for_every_combination_in_use() {
        let table = ShortcutTable::default();
        let sets = modifier_sets(&table);
        assert_eq!(
            sets,
            [
                (false, false, false),
                (true, false, false),
                (false, true, false),
                (false, false, true),
                (true, true, false),
            ]
        );
        let page = sheet(&table, LayoutId::Us);
        assert_eq!(page.matches("<svg").count(), sets.len());
        assert_eq!(page.matches("</svg>").count(), sets.len());
        for title in [
            "Plain keys",
            "With Ctrl",
            "With Shift",
            "With Alt",
            "With Ctrl+Shift",
        ] {
            assert!(
                page.contains(&format!("<figcaption>{title}</figcaption>")),
                "{title}"
            );
        }
        // A used key is filled with its context's colour.
        assert!(page.contains(&format!("fill:{}", css(Context::Timeline.colour()))));
    }

    #[test]
    fn a_context_s_lines_go_two_abreast_first_half_on_the_left() {
        let cell = |text: &str| [format!("<td>{text}</td>"), format!("<td>does {text}</td>")];
        let mut out = String::new();
        section(&mut out, Context::Viewer, &[cell("a"), cell("b"), cell("c")]);
        assert!(out.contains("<tr><td>a</td><td>does a</td><td>c</td><td>does c</td></tr>"));
        assert!(
            out.contains("<tr><td>b</td><td>does b</td><td class=\"gap\"></td><td class=\"gap\"></td></tr>")
        );
        assert!(out.contains("Viewer</h3>"));
        let mut nothing = String::new();
        section(&mut nothing, Context::Viewer, &[]);
        assert!(nothing.is_empty(), "no heading over nothing");
    }

    #[test]
    fn the_sheet_lists_the_mouse() {
        let page = sheet(&ShortcutTable::default(), LayoutId::Us);
        let mouse = &page[page.find("<h2>Mouse</h2>").unwrap()..];
        for line in MOUSE {
            assert!(mouse.contains(&escaped(line.gesture)), "{}", line.gesture);
            assert!(
                mouse.contains(&escaped(&line.says(&ShortcutTable::default()))),
                "{}",
                line.does
            );
        }
        assert!(mouse.contains("While dragging</h3>"));
    }

    #[test]
    fn the_sheet_stands_alone_and_is_well_formed() {
        for layout in LayoutId::ALL {
            let page = sheet(&ShortcutTable::default(), layout);
            for outside in [
                "http://", "https://", "src=", "href=", "url(", "@import", "<script",
            ] {
                assert!(!page.contains(outside), "{layout:?}: {outside}");
            }
            for tag in [
                "g", "svg", "table", "section", "figure", "div", "tr", "td", "text", "h3",
            ] {
                let opened =
                    page.matches(&format!("<{tag}>")).count() + page.matches(&format!("<{tag} ")).count();
                assert_eq!(
                    opened,
                    page.matches(&format!("</{tag}>")).count(),
                    "{layout:?} <{tag}>"
                );
            }
            assert!(page.len() < 400 * 1024, "small enough to write in one go");
        }
    }

    #[test]
    fn the_sheet_draws_the_chosen_layout_and_escapes_what_it_prints() {
        let page = sheet(&ShortcutTable::default(), LayoutId::Norwegian);
        assert!(page.contains("Keyboard: Norwegian."));
        assert!(page.contains(">Ø</text>"));
        assert!(page.contains("<text class=\"us\""), "the US character beside it");
        assert!(
            page.contains(">&lt;</text>"),
            "the key beside the left Shift, escaped"
        );
        assert!(page.contains("Ctrl+- (Slash on a US keyboard)"));
        assert_eq!(escaped("a<b & \"c\">"), "a&lt;b &amp; &quot;c&quot;&gt;");
        // The tall Enter is one six-cornered outline.
        let enter = page.find(">Enter</text>").unwrap();
        let polygon = &page[page[..enter].rfind("<polygon").unwrap()..enter];
        assert_eq!(polygon.matches(',').count(), 6);
    }
}
