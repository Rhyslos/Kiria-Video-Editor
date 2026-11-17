//! Titles (format 26): clips of their own on video tracks
//! (`ClipKind::Title`) that show words instead of a file, laid out in one of
//! three ways. A title's text and how it looks are `Clip::title`; its place,
//! size, opacity, colour, effects and keyframes are a clip's like any other.

use crate::time::Time;
use diag::AppError;

/// How long a new title lasts.
pub const TITLE_LENGTH: Time = Time::seconds(4);
use serde::{Deserialize, Serialize};

/// How a title's words are laid out in the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TitleStyle {
    /// Low down on the left, on a dark band: a name and what they do.
    LowerThird,
    /// In the middle of the frame.
    Centred,
    /// In the middle of a dark card filling the frame: the end.
    EndCard,
}

impl TitleStyle {
    pub const ALL: [TitleStyle; 3] = [TitleStyle::LowerThird, TitleStyle::Centred, TitleStyle::EndCard];

    /// Its name in the interface.
    pub fn name(self) -> &'static str {
        match self {
            TitleStyle::LowerThird => "Lower third",
            TitleStyle::Centred => "Centred title",
            TitleStyle::EndCard => "End card",
        }
    }

    /// What a new one says.
    fn first_words(self) -> &'static str {
        match self {
            TitleStyle::LowerThird => "Name\nWhat they do",
            TitleStyle::Centred => "Title",
            TitleStyle::EndCard => "Thanks for watching",
        }
    }

    /// How tall a new one's words are, in millionths of the frame's height.
    fn usual_size(self) -> i32 {
        match self {
            TitleStyle::LowerThird => 50_000,
            TitleStyle::Centred => 100_000,
            TitleStyle::EndCard => 80_000,
        }
    }
}

/// A title's words and how they look.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Title {
    /// What it says; a new line starts a new line.
    pub text: String,
    pub style: TitleStyle,
    /// How tall its letters are, in millionths of the frame's height.
    pub size: i32,
    /// The colour of its letters, red, green and blue.
    pub colour: [u8; 3],
    /// A dark band behind it (a dark card filling the frame for an end card).
    pub backing: bool,
}

impl Title {
    /// The smallest and largest letters, in millionths of the frame's height.
    pub const SIZES: (i32, i32) = (10_000, 400_000);

    /// The most it can say.
    pub const MOST_CHARACTERS: usize = 2_000;

    /// A new title of `style`, with words to change.
    pub fn new(style: TitleStyle) -> Title {
        Title {
            text: style.first_words().to_owned(),
            style,
            size: style.usual_size(),
            colour: [255, 255, 255],
            backing: style != TitleStyle::Centred,
        }
    }

    /// Its first line, as the timeline names it.
    pub fn first_line(&self) -> &str {
        self.text.lines().next().unwrap_or("").trim()
    }

    /// Refuses a title that can't be shown.
    pub fn check(&self) -> diag::Result<()> {
        let (least, most) = Title::SIZES;
        if !(least..=most).contains(&self.size) {
            return Err(AppError::new("That title size can't be used")
                .why("A title's letters can be from 1 % to 40 % of the frame's height."));
        }
        if self.text.chars().count() > Title::MOST_CHARACTERS {
            return Err(AppError::new("That title is too long")
                .why(format!(
                    "A title can say up to {} characters.",
                    Title::MOST_CHARACTERS
                ))
                .fix("Split it over several titles."));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_title_has_words_to_change_and_fits() {
        for style in TitleStyle::ALL {
            let title = Title::new(style);
            assert!(title.check().is_ok(), "{}", style.name());
            assert!(!title.first_line().is_empty());
        }
        assert_eq!(Title::new(TitleStyle::LowerThird).first_line(), "Name");
        assert!(!Title::new(TitleStyle::Centred).backing);
    }

    #[test]
    fn sizes_and_lengths_out_of_range_are_refused() {
        let mut title = Title::new(TitleStyle::Centred);
        title.size = 5_000;
        assert!(title.check().is_err());
        title.size = 100_000;
        title.text = "x".repeat(Title::MOST_CHARACTERS + 1);
        assert!(title.check().is_err());
    }
}
