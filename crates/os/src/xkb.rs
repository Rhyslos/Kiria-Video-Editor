//! Reading XKB layout names: from the X server's description of the
//! keyboard, from the environment and from the system's settings files.
//! Plain text handling, tested on every system.
#![cfg_attr(windows, allow(dead_code))]

use crate::SystemLayout;

/// The layout of keyboard group `group` (0 for the first) in the X
/// server's symbols description, such as "pc+no+us:2+inet(evdev)": the
/// first group's layout is the first part after "pc", and each further
/// group's carries its number after a colon. The variant is left off.
pub fn layout_of_group(symbols: &str, group: usize) -> Option<String> {
    let wanted = group + 1;
    symbols
        .split('+')
        .filter_map(|part| match part.split_once(':') {
            Some((name, number)) => Some((name, number.trim().parse::<usize>().ok()?)),
            None => Some((part, 1)),
        })
        .filter(|(name, _)| bare(name) != "pc")
        .find(|(_, number)| *number == wanted)
        .map(|(name, _)| bare(name).to_owned())
        .filter(|name| !name.is_empty())
}

/// A name without its variant: "no" from "no(nodeadkeys)".
fn bare(name: &str) -> &str {
    name.split('(').next().unwrap_or(name).trim()
}

/// The first layout of a list as settings write it: "no" from "no,us" or
/// from "no(winkeys), us".
pub fn first_of_list(list: &str) -> Option<String> {
    let first = bare(list.split(',').next()?.trim_matches(|c| c == '"' || c == '\''));
    (!first.is_empty()).then(|| first.to_owned())
}

/// The layout list in a settings file of the `XKBLAYOUT="no,us"` kind.
pub fn layout_in_file(text: &str) -> Option<String> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("XKBLAYOUT="))
        .find_map(first_of_list)
}

/// Where the system keeps the keyboard it starts with.
const SETTINGS_FILES: [&str; 2] = ["/etc/default/keyboard", "/etc/vconsole.conf"];

/// The layout the system is set up with, when there is no X server to ask:
/// `XKB_DEFAULT_LAYOUT`, then the settings files. It does not follow
/// switches.
pub fn from_settings() -> Option<SystemLayout> {
    let from_env = std::env::var("XKB_DEFAULT_LAYOUT")
        .ok()
        .and_then(|list| first_of_list(&list));
    from_env
        .or_else(|| {
            SETTINGS_FILES
                .iter()
                .filter_map(|path| std::fs::read_to_string(path).ok())
                .find_map(|text| layout_in_file(&text))
        })
        .map(SystemLayout::Xkb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_group_has_its_layout_in_the_symbols() {
        let symbols = "pc+no+us:2+ru:3+inet(evdev)+group(alt_shift_toggle)";
        assert_eq!(layout_of_group(symbols, 0).as_deref(), Some("no"));
        assert_eq!(layout_of_group(symbols, 1).as_deref(), Some("us"));
        assert_eq!(layout_of_group(symbols, 2).as_deref(), Some("ru"));
        assert_eq!(layout_of_group(symbols, 3), None);
        assert_eq!(
            layout_of_group("pc+gb(extd)+inet(evdev)", 0).as_deref(),
            Some("gb")
        );
        assert_eq!(layout_of_group("pc+jp+inet(evdev)", 0).as_deref(), Some("jp"));
        assert_eq!(layout_of_group("pc", 0), None);
        assert_eq!(layout_of_group("", 0), None);
    }

    #[test]
    fn settings_files_and_lists_give_their_first_layout() {
        assert_eq!(first_of_list("no,us").as_deref(), Some("no"));
        assert_eq!(first_of_list("\"gb(extd), us\"").as_deref(), Some("gb"));
        assert_eq!(first_of_list(""), None);
        let debian =
            "# KEYBOARD CONFIGURATION FILE\nXKBMODEL=\"pc105\"\nXKBLAYOUT=\"no,us\"\nXKBVARIANT=\"\"\n";
        assert_eq!(layout_in_file(debian).as_deref(), Some("no"));
        assert_eq!(layout_in_file("KEYMAP=us\nXKBLAYOUT=jp\n").as_deref(), Some("jp"));
        assert_eq!(layout_in_file("KEYMAP=us\n"), None);
        assert_eq!(layout_in_file("XKBLAYOUT=\"\"\n"), None);
    }
}
