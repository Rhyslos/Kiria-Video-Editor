//! The one error type used across the app.

use std::error::Error as StdError;
use std::fmt;

/// Shorthand for results that fail with an [`AppError`].
pub type Result<T, E = AppError> = std::result::Result<T, E>;

type Source = Box<dyn StdError + Send + Sync + 'static>;

/// Shown when an error has no specific fix.
const FALLBACK_FIX: &str =
    "If this keeps happening, copy the details from the Diagnostics panel and report it.";

/// An error a person can understand and act on.
///
/// ```
/// use diag::{AppError, FixAction};
///
/// let err = AppError::new("This clip plays slowly")
///     .why("It is 10-bit H.265 and your graphics card can't decode that format.")
///     .fix("Create a proxy, or lower the preview resolution.")
///     .action(FixAction::new("Create proxy", "media.create_proxy"));
/// assert_eq!(err.what(), "This clip plays slowly");
/// ```
#[derive(Debug)]
pub struct AppError(Box<Inner>);

// Boxed so that `Result<T, AppError>` stays one pointer wide on the error side.
#[derive(Debug)]
struct Inner {
    what: String,
    why: Option<String>,
    fix: Option<String>,
    action: Option<FixAction>,
    source: Option<Source>,
}

/// A one-click fix the interface can offer next to the error.
///
/// `command` names an app command (for example `media.create_proxy`); the
/// interface looks it up in its command table and runs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixAction {
    pub label: String,
    pub command: String,
}

impl FixAction {
    pub fn new(label: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            command: command.into(),
        }
    }
}

impl AppError {
    /// Starts an error with what happened, in plain words.
    pub fn new(what: impl Into<String>) -> Self {
        Self(Box::new(Inner {
            what: what.into(),
            why: None,
            fix: None,
            action: None,
            source: None,
        }))
    }

    /// Why it happened.
    pub fn why(mut self, why: impl Into<String>) -> Self {
        self.0.why = Some(why.into());
        self
    }

    /// How to fix it.
    pub fn fix(mut self, fix: impl Into<String>) -> Self {
        self.0.fix = Some(fix.into());
        self
    }

    /// A one-click fix to offer.
    pub fn action(mut self, action: FixAction) -> Self {
        self.0.action = Some(action);
        self
    }

    /// The technical cause, kept for logs and bug reports.
    pub fn source(mut self, source: impl StdError + Send + Sync + 'static) -> Self {
        self.0.source = Some(Box::new(source));
        self
    }

    pub fn what(&self) -> &str {
        &self.0.what
    }

    pub fn why_text(&self) -> Option<&str> {
        self.0.why.as_deref()
    }

    /// The fix to show; a general one when none was given.
    pub fn fix_text(&self) -> &str {
        self.0.fix.as_deref().unwrap_or(FALLBACK_FIX)
    }

    pub fn fix_action(&self) -> Option<&FixAction> {
        self.0.action.as_ref()
    }

    /// The full text for logs and bug reports: what, why, fix and the chain of causes.
    pub fn report(&self) -> String {
        let mut out = self.to_string();
        out.push_str("\nFix: ");
        out.push_str(self.fix_text());
        let mut cause = StdError::source(self);
        while let Some(err) = cause {
            out.push_str("\nCaused by: ");
            out.push_str(&err.to_string());
            cause = err.source();
        }
        out
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.what)?;
        if let Some(why) = &self.0.why {
            write!(f, ". {why}")?;
        }
        Ok(())
    }
}

impl StdError for AppError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.0.source.as_deref().map(|s| s as &(dyn StdError + 'static))
    }
}

/// Turns any error into an [`AppError`] while keeping it as the cause.
///
/// ```
/// use diag::{AppError, Explain};
///
/// let read = std::fs::read("missing-file.ron").or_explain(|| {
///     AppError::new("The project can't be opened").fix("Check that the file still exists.")
/// });
/// assert!(read.is_err());
/// ```
pub trait Explain<T> {
    fn or_explain(self, make: impl FnOnce() -> AppError) -> Result<T>;
}

impl<T, E> Explain<T> for std::result::Result<T, E>
where
    E: StdError + Send + Sync + 'static,
{
    fn or_explain(self, make: impl FnOnce() -> AppError) -> Result<T> {
        self.map_err(|err| make().source(err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn display_joins_what_and_why() {
        let err = AppError::new("Export can't start").why("Two clips are offline");
        assert_eq!(err.to_string(), "Export can't start. Two clips are offline");
        assert_eq!(AppError::new("Only what").to_string(), "Only what");
    }

    #[test]
    fn builder_keeps_every_part() {
        let err = AppError::new("what")
            .why("why")
            .fix("fix")
            .action(FixAction::new("Relink", "media.relink"));
        assert_eq!(err.what(), "what");
        assert_eq!(err.why_text(), Some("why"));
        assert_eq!(err.fix_text(), "fix");
        assert_eq!(err.fix_action(), Some(&FixAction::new("Relink", "media.relink")));
    }

    #[test]
    fn missing_fix_falls_back_to_a_general_one() {
        assert_eq!(AppError::new("what").fix_text(), FALLBACK_FIX);
    }

    #[test]
    fn report_lists_the_cause_chain() {
        let inner = AppError::new("disk full").source(io::Error::other("no space left on device"));
        let err = AppError::new("Autosave failed")
            .fix("Free some disk space.")
            .source(inner);
        let report = err.report();
        assert!(report.starts_with("Autosave failed\nFix: Free some disk space."));
        assert!(report.contains("Caused by: disk full"));
        assert!(report.contains("Caused by: no space left on device"));
    }

    #[test]
    fn explain_keeps_the_original_error_as_source() {
        let result: std::result::Result<(), io::Error> = Err(io::Error::other("boom"));
        let err = result.or_explain(|| AppError::new("Saving failed")).unwrap_err();
        assert_eq!(StdError::source(&err).map(|s| s.to_string()), Some("boom".into()));
    }
}
