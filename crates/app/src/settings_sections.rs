//! The sections of the Settings window, by name and by the words that find
//! them in the search palette ("proxy" finds Proxies and Cache), so search
//! can open Settings scrolled to the one asked for.

/// A section of the Settings window, in the window's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Look,
    Editing,
    Audio,
    Resources,
    Proxies,
    Library,
    Cache,
    Graphics,
}

impl Section {
    pub const ALL: [Section; 8] = [
        Section::Look,
        Section::Editing,
        Section::Audio,
        Section::Resources,
        Section::Proxies,
        Section::Library,
        Section::Cache,
        Section::Graphics,
    ];

    /// Its heading in the window.
    pub fn name(self) -> &'static str {
        match self {
            Section::Look => "Look",
            Section::Editing => "Editing",
            Section::Audio => "Audio",
            Section::Resources => "Resources",
            Section::Proxies => "Proxies",
            Section::Library => "Library",
            Section::Cache => "Cache",
            Section::Graphics => "Graphics",
        }
    }

    /// Where Settings opens for it: a tab or row id of
    /// `editor::settings_ui::CATEGORIES`.
    pub fn id(self) -> &'static str {
        match self {
            Section::Look => "look",
            Section::Editing => "editing",
            Section::Audio => "audio",
            Section::Resources => "resources",
            Section::Proxies => "proxies",
            Section::Library => "library",
            Section::Cache => "cache",
            Section::Graphics => "graphics",
        }
    }

    /// What is set there, in a few words: the line search shows.
    pub fn about(self) -> &'static str {
        match self {
            Section::Look => "theme",
            Section::Editing => "edit at the playhead, what a click in the ruler moves",
            Section::Audio => "the loudness aimed for",
            Section::Resources => "worker threads, memory and disk cache, background work",
            Section::Proxies => "Off, On or Auto; what a jump shows without a proxy",
            Section::Library => "where marks and levels are kept",
            Section::Cache => "how much the cache holds, clear it for this project or all",
            Section::Graphics => "the graphics system that draws the window",
        }
    }

    /// The words search finds it by, besides its name and `about`.
    fn words(self) -> &'static str {
        match self {
            Section::Look => "appearance dark light colours colors",
            Section::Editing => "needle playhead ruler click",
            Section::Audio => "loudness lufs target sound volume",
            Section::Resources => "threads cpu ram memory performance",
            Section::Proxies => "proxy proxies preview smooth playback jump key frame",
            Section::Library => "marks in out levels session saved",
            Section::Cache => "proxy proxies clear disk space free delete",
            Section::Graphics => "vulkan direct3d gpu graphics card driver",
        }
    }

    /// Whether the lower-case words `q` find it.
    pub fn matches(self, q: &str) -> bool {
        let all = format!("settings {} {} {}", self.name(), self.about(), self.words()).to_lowercase();
        !q.is_empty() && q.split_whitespace().all(|word| all.contains(word))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_find_the_sections_they_are_about() {
        let found = |q: &str| -> Vec<Section> { Section::ALL.into_iter().filter(|s| s.matches(q)).collect() };
        assert_eq!(found("proxy"), [Section::Proxies, Section::Cache]);
        assert_eq!(found("clear cache"), [Section::Cache]);
        assert_eq!(found("lufs"), [Section::Audio]);
        assert!(found("").is_empty());
        assert_eq!(found("settings").len(), Section::ALL.len());
    }
}
