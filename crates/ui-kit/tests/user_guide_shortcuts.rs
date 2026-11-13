//! The user guide's shortcut tables are made from the one shortcut table:
//! the part of `docs/user-guide/index.html` between the `shortcuts:start`
//! and `shortcuts:end` marks must be what [`tables`] writes, so the guide
//! can't fall behind a change of keys, names or mouse actions.
//!
//! After changing the shortcut table, write the part again with
//! `UPDATE_USER_GUIDE=1 cargo test -p ui-kit --test user_guide_shortcuts`.

use std::fmt::Write as _;
use std::path::PathBuf;
use ui_kit::shortcuts::mouse::MOUSE;
use ui_kit::{Action, Context, ShortcutTable};

const START: &str =
    "<!-- shortcuts:start (made from the shortcut table by crates/ui-kit/tests/user_guide_shortcuts.rs) -->";
const END: &str = "<!-- shortcuts:end -->";

fn guide() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/user-guide/index.html")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A key as printed on a US keyboard, where its name is a word.
fn printed(key: &str) -> &str {
    match key {
        "Comma" => ",",
        "Slash" => "/",
        "Backslash" => "\\",
        "Equals" => "=",
        "Minus" => "-",
        "Left" => "←",
        "Right" => "→",
        "Up" => "↑",
        "Down" => "↓",
        "Escape" => "Esc",
        other => other,
    }
}

/// "Ctrl+Shift+Z" as keys to read: each key in its own box.
fn keys_html(written: &str) -> String {
    // A key that is itself "+" is written "Plus" by the table, so splitting
    // on "+" is safe.
    written
        .split('+')
        .map(|k| format!("<kbd>{}</kbd>", escape(printed(k))))
        .collect::<Vec<_>>()
        .join("+")
}

/// Every command with its usual keys, by context, then the mouse.
fn tables() -> String {
    let table = ShortcutTable::default();
    let mut html = String::new();
    for context in [Context::Anywhere, Context::Viewer, Context::Timeline] {
        let id = context.label().to_lowercase().replace(' ', "-");
        let _ = writeln!(html, "<h3 id=\"keys-{id}\">{}</h3>", escape(context.label()));
        html.push_str(
            "<table class=\"keys\">\n<thead><tr><th>Command</th><th>Keys</th></tr></thead>\n<tbody>\n",
        );
        for action in Action::ALL.iter().filter(|a| a.context() == context) {
            let keys: Vec<String> = table
                .lines()
                .iter()
                .filter(|line| line.action == *action)
                .filter_map(|line| line.keys)
                .map(|keys| keys_html(&table.written(keys)))
                .collect();
            let keys = if keys.is_empty() {
                "<span class=\"none\">none (give it one on the shortcuts page)</span>".to_owned()
            } else {
                keys.join(" or ")
            };
            let _ = writeln!(
                html,
                "<tr><td>{}</td><td>{keys}</td></tr>",
                escape(action.label())
            );
        }
        html.push_str("</tbody>\n</table>\n");
    }
    html.push_str("<h3 id=\"keys-mouse\">Mouse</h3>\n");
    for context in Context::ALL {
        let lines: Vec<_> = MOUSE.iter().filter(|m| m.context == context).collect();
        if lines.is_empty() {
            continue;
        }
        let _ = writeln!(html, "<h4>{}</h4>", escape(context.label()));
        html.push_str("<table class=\"keys\">\n<thead><tr><th>With the mouse</th><th>What it does</th></tr></thead>\n<tbody>\n");
        for line in lines {
            let _ = writeln!(
                html,
                "<tr><td>{}</td><td>{}</td></tr>",
                escape(line.gesture),
                escape(&line.says(&table))
            );
        }
        html.push_str("</tbody>\n</table>\n");
    }
    html
}

#[test]
fn the_user_guide_lists_every_shortcut_as_the_table_has_it() {
    let path = guide();
    let page = std::fs::read_to_string(&path).expect("the user guide is in docs/user-guide");
    let (Some(start), Some(end)) = (page.find(START), page.find(END)) else {
        panic!("the user guide has no {START} … {END} part");
    };
    let made = format!("{START}\n{}{END}", tables());
    let now = &page[start..end + END.len()];
    if now == made {
        return;
    }
    if std::env::var_os("UPDATE_USER_GUIDE").is_some() {
        let updated = format!("{}{made}{}", &page[..start], &page[end + END.len()..]);
        std::fs::write(&path, updated).expect("the user guide can be written");
        return;
    }
    panic!(
        "the user guide's shortcut tables are out of date: run\n  UPDATE_USER_GUIDE=1 cargo test -p ui-kit --test user_guide_shortcuts\nto write them again from the shortcut table"
    );
}

#[test]
fn every_command_is_in_the_tables() {
    let made = tables();
    for action in Action::ALL {
        assert!(made.contains(&escape(action.label())), "{}", action.label());
    }
    assert!(made.contains("<kbd>F1</kbd>"), "the user guide's own key");
}
