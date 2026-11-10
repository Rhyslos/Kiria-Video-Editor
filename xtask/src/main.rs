//! Project chores, run with `cargo xtask <task>`.
//!
//! | Task | What it does |
//! |---|---|
//! | `check-media` | Checks `test-media/` against its manifest |
//! | `brand` | Prints the app's name and identifiers as `KEY=value` lines (for CI) |
//! | `keygen <secret-file>` | Makes a new release signing key; prints the public key |
//! | `manifest <out> <version> <files...>` | Writes a release manifest for the given files |
//! | `sign <manifest>` | Signs a manifest with `RELEASE_SIGNING_KEY` (base64) |
//! | `verify <manifest>` | Checks a manifest's signature against the key built into the app |
//! | `icons <out-dir>` | Draws the app icon as PNG and ICO |

mod icons;
mod media;
mod release;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = &args[args.len().min(1)..];
    let result = match args.first().map(String::as_str) {
        Some("check-media") => media::check_media(),
        Some("brand") => {
            release::brand();
            Ok(())
        }
        Some("keygen") => release::keygen(rest),
        Some("manifest") => release::manifest(rest),
        Some("sign") => release::sign(rest),
        Some("verify") => release::verify(rest),
        Some("icons") => icons::write(rest),
        _ => Err(
            "Usage: cargo xtask <check-media | brand | keygen | manifest | sign | verify | icons> [args]"
                .into(),
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
