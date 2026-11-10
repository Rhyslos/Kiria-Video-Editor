//! Release chores: identifiers for CI, the signing key, manifests and signatures.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use std::path::Path;
use updater::{Channel, Manifest, ManifestFile, RELEASE_PUBLIC_KEY};

/// `KEY=value` lines a workflow can append to `$GITHUB_ENV`.
pub fn brand() {
    println!("APP_NAME={}", brand::APP_NAME);
    println!("APP_ID={}", brand::APP_ID);
    println!("APP_VERSION={}", brand::VERSION);
    println!("PROJECT_FILE_EXTENSION={}", brand::PROJECT_FILE_EXTENSION);
}

/// `beta-version <run number>`: `KEY=value` lines for an automatic test build
/// of `main`. The version is the next patch after the workspace version with a
/// beta number, so it sorts after the last release and before the next one:
/// 0.1.0 with run 12 gives 0.1.1-beta.12.
pub fn beta_version(args: &[String]) -> Result<(), String> {
    let [run] = args else {
        return Err("Usage: cargo xtask beta-version <run number>".into());
    };
    let version = next_beta(env!("CARGO_PKG_VERSION"), run)?;
    println!("APP_VERSION={version}");
    println!("BUILD_VERSION={version}");
    println!("RELEASE_TAG=v{version}");
    Ok(())
}

fn next_beta(base: &str, run: &str) -> Result<String, String> {
    let run: u64 = run.parse().map_err(|_| format!("{run} is not a run number"))?;
    let core = base.split('-').next().unwrap_or(base);
    let parts: Vec<u64> = core
        .split('.')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|_| format!("{base} is not a version"))?;
    let [major, minor, patch] = parts[..] else {
        return Err(format!("{base} is not a version"));
    };
    Ok(format!("{major}.{minor}.{}-beta.{run}", patch + 1))
}

/// Writes a new secret key (base64) to a file and prints the public key as Rust.
pub fn keygen(args: &[String]) -> Result<(), String> {
    let [out] = args else {
        return Err("Usage: cargo xtask keygen <file for the secret key>".into());
    };
    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret).map_err(|e| format!("No randomness available: {e}"))?;
    std::fs::write(out, BASE64.encode(secret)).map_err(|e| format!("Can't write {out}: {e}"))?;
    let public = updater::manifest::public_key_of(&secret);
    println!(
        "Secret key written to {out}. Store it as the RELEASE_SIGNING_KEY repository secret, then delete the file."
    );
    println!("Public key for RELEASE_PUBLIC_KEY:");
    for row in public.chunks(16) {
        let bytes: Vec<String> = row.iter().map(|b| format!("0x{b:02x}")).collect();
        println!("    {},", bytes.join(", "));
    }
    Ok(())
}

/// `manifest <out> <version> <file>...`: the channel is beta when the version has a pre-release part.
pub fn manifest(args: &[String]) -> Result<(), String> {
    let [out, version, files @ ..] = args else {
        return Err("Usage: cargo xtask manifest <out.json> <version> <file>...".into());
    };
    if files.is_empty() {
        return Err("A manifest needs at least one file.".into());
    }
    let channel = if version.contains('-') {
        Channel::Beta
    } else {
        Channel::Stable
    };
    let files = files
        .iter()
        .map(|path| {
            let bytes = std::fs::read(path).map_err(|e| format!("Can't read {path}: {e}"))?;
            let name = Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .ok_or_else(|| format!("{path} has no file name"))?;
            Ok(ManifestFile {
                name,
                size: bytes.len() as u64,
                sha256: updater::manifest::sha256_hex(&bytes),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let manifest = Manifest {
        version: version.trim_start_matches('v').to_owned(),
        channel,
        min_launcher_version: env!("CARGO_PKG_VERSION").to_owned(),
        files,
    };
    std::fs::write(out, manifest.to_bytes()).map_err(|e| format!("Can't write {out}: {e}"))?;
    println!(
        "Wrote {out} for version {} ({} files).",
        manifest.version,
        manifest.files.len()
    );
    Ok(())
}

/// Signs a manifest with the secret key in `RELEASE_SIGNING_KEY`, writing `<manifest>.sig`.
pub fn sign(args: &[String]) -> Result<(), String> {
    let [path] = args else {
        return Err("Usage: cargo xtask sign <manifest.json>".into());
    };
    let secret = std::env::var("RELEASE_SIGNING_KEY")
        .map_err(|_| "RELEASE_SIGNING_KEY is not set. Add it as a repository secret.".to_owned())?;
    let secret: [u8; 32] = BASE64
        .decode(secret.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or("RELEASE_SIGNING_KEY is not a base64 key of 32 bytes.")?;
    if updater::manifest::public_key_of(&secret) != RELEASE_PUBLIC_KEY {
        return Err("RELEASE_SIGNING_KEY does not belong to the public key built into the app.".into());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Can't read {path}: {e}"))?;
    let signature = updater::manifest::sign(&bytes, &secret);
    std::fs::write(format!("{path}.sig"), signature)
        .map_err(|e| format!("Can't write the signature: {e}"))?;
    println!("Signed {path}.");
    Ok(())
}

/// Checks `<manifest>.sig` against the key built into the app.
pub fn verify(args: &[String]) -> Result<(), String> {
    let [path] = args else {
        return Err("Usage: cargo xtask verify <manifest.json>".into());
    };
    let bytes = std::fs::read(path).map_err(|e| format!("Can't read {path}: {e}"))?;
    let signature = std::fs::read_to_string(format!("{path}.sig"))
        .map_err(|e| format!("Can't read the signature: {e}"))?;
    Manifest::verify_and_parse(&bytes, &signature, &RELEASE_PUBLIC_KEY).map_err(|e| e.report())?;
    println!("{path} is signed with the release key.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn betas_sort_after_the_release_they_build_on() {
        assert_eq!(next_beta("0.1.0", "12").unwrap(), "0.1.1-beta.12");
        assert_eq!(next_beta("1.4.9", "3").unwrap(), "1.4.10-beta.3");
        assert!(next_beta("0.1", "1").is_err());
        assert!(next_beta("0.1.0", "x").is_err());
    }
}
