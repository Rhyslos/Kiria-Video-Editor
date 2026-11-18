//! The one place a recording's fingerprint becomes part of a file name.
//!
//! Every store here keeps its files by the recording's fingerprint, and
//! the fingerprint comes from a project file, which anyone can write. So
//! the name is made only from a fingerprint of the form the app makes
//! (`media::fingerprint`: the size, a number, and 32 hexadecimal digits):
//! anything else (a `/`, `..`, an empty hash) is refused, and a store
//! treats a refused fingerprint as "not cached": it reads nothing and
//! writes nothing for it.

use project::Fingerprint;

/// How many hexadecimal digits a fingerprint's hash has (the first 16
/// bytes of its digest, `media::fingerprint`).
pub const HASH_DIGITS: usize = 32;

/// The stem of the names of a recording's cached files, `size-hash`, when
/// `fingerprint` is one the app makes; `None` otherwise.
pub fn stem(fingerprint: &Fingerprint) -> Option<String> {
    let hash = &fingerprint.sample_hash;
    let well_formed = hash.len() == HASH_DIGITS && hash.bytes().all(|b| b.is_ascii_hexdigit());
    if !well_formed {
        tracing::debug!("a fingerprint that is not one the app makes was given no cache file");
        return None;
    }
    Some(format!("{}-{hash}", fingerprint.size))
}

/// Fingerprints a store must refuse (for the stores' tests): every one
/// would put a file outside the cache folder, or is not the app's.
#[cfg(test)]
pub(crate) fn hostile() -> Vec<Fingerprint> {
    [
        "../../escaped",
        "../0123456789abcdef0123456789abcd",
        "0123456789abcdef/../../../escaped",
        "0123456789abcdef0123456789abcde/",
        "0123456789abcdef0123456789abcde\\",
        "",
        "0123456789abcdef",
        "0123456789abcdef0123456789abcdef0",
        "0123456789abcdef0123456789abcdeg",
    ]
    .into_iter()
    .map(|hash| Fingerprint {
        size: 1,
        sample_hash: hash.to_owned(),
    })
    .collect()
}

/// A well-formed fingerprint numbered `n` (for the stores' tests).
#[cfg(test)]
pub(crate) fn print(n: u64) -> Fingerprint {
    Fingerprint {
        size: n,
        sample_hash: format!("{n:032x}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_apps_own_fingerprints_name_files() {
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/voice.mp3");
        let real = media::fingerprint(&fixture).unwrap();
        assert_eq!(stem(&real), Some(format!("{}-{}", real.size, real.sample_hash)));
        assert_eq!(
            stem(&print(10)).as_deref(),
            Some("10-0000000000000000000000000000000a")
        );
    }

    #[test]
    fn anything_else_is_refused() {
        for fingerprint in hostile() {
            assert_eq!(stem(&fingerprint), None, "{fingerprint:?}");
        }
    }
}
