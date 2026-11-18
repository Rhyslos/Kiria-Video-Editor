//! The proxies a worker reads: which file has one, and their open decoders.

use super::{Convert, OPEN_DECODERS, Target, Worker};
use cache::proxy::{ProxyMode, Shelf};
use media::{Decoded, VideoDecoder};
use project::{Fingerprint, MediaId};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The proxies a worker reads.
#[derive(Default)]
pub(super) struct Proxies {
    shelf: Option<Arc<Shelf>>,
    /// The shelf's generation the remembered answers are from.
    generation: u64,
    /// Each file's proxy, or none, as last looked up.
    files: HashMap<(MediaId, u32), Option<PathBuf>>,
    /// Open proxies, most recently used first.
    decoders: VecDeque<((MediaId, u32), VideoDecoder)>,
    /// Proxies that could not be read: not tried again this session.
    broken: HashSet<PathBuf>,
}

impl Proxies {
    /// Takes the shelf a request came with.
    pub(super) fn take(&mut self, shelf: Option<&Arc<Shelf>>) {
        let same = match (&self.shelf, shelf) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.shelf = shelf.cloned();
            self.forget();
        }
        self.notice_changes();
    }

    fn forget(&mut self) {
        self.files.clear();
        self.decoders.clear();
    }

    /// A proxy was added or removed, or the mode changed: look again, and
    /// let go of open proxies (one being deleted can't go while it is open).
    pub(super) fn notice_changes(&mut self) {
        let Some(generation) = self.shelf.as_ref().map(|shelf| shelf.generation()) else {
            return;
        };
        if generation != self.generation {
            self.generation = generation;
            self.forget();
        }
    }

    pub(super) fn mode(&self) -> ProxyMode {
        self.shelf.as_ref().map_or(ProxyMode::Off, |shelf| shelf.mode())
    }

    /// The proxy of a file, when it has one and proxies aren't off.
    pub(super) fn of(&mut self, key: (MediaId, u32), fingerprint: &Fingerprint) -> Option<PathBuf> {
        if self.mode() == ProxyMode::Off {
            return None;
        }
        let shelf = self.shelf.as_ref()?;
        if let Some(known) = self.files.get(&key) {
            return known.clone();
        }
        let found = shelf
            .ready(fingerprint, key.1)
            .filter(|path| !self.broken.contains(path));
        self.files.insert(key, found.clone());
        found
    }

    /// The frame on screen at `target`'s moment, from the proxy at `path`.
    /// `None` when it can't be read (it is not tried again).
    pub(super) fn picture(&mut self, target: &Target<'_>, path: &Path, convert: Convert) -> Option<Decoded> {
        let key = target.key;
        let found = self.decoders.iter().position(|(k, _)| *k == key);
        let mut decoder = match found.and_then(|i| self.decoders.remove(i)) {
            Some((_, decoder)) => decoder,
            None => match VideoDecoder::open_proxy(path, target.rotation) {
                Ok(decoder) => {
                    if let Some(shelf) = &self.shelf {
                        shelf.used(path);
                    }
                    decoder
                }
                Err(err) => {
                    tracing::warn!(file = %path.display(), "a proxy could not be opened: {}", err.report());
                    self.broken.insert(path.to_path_buf());
                    self.files.remove(&key);
                    return None;
                }
            },
        };
        let (w, h) = target.size;
        let got = match convert {
            Convert::OnCpu => decoder.frame_at(target.moment, w, h).map(Decoded::Rgba),
            Convert::WhileDrawing { .. } => decoder.picture_at(target.moment, w, h),
        };
        match got {
            Ok(picture) => {
                self.decoders.push_front((key, decoder));
                self.decoders.truncate(OPEN_DECODERS);
                Some(picture)
            }
            Err(err) => {
                tracing::warn!(file = %path.display(), "a proxy could not be read: {}", err.report());
                self.broken.insert(path.to_path_buf());
                self.files.remove(&key);
                None
            }
        }
    }
}

impl Worker {
    /// With proxies On, the picture of `target` from its file's proxy.
    pub(super) fn proxy_only(&mut self, target: &Target<'_>) -> Option<Decoded> {
        if self.proxies.mode() != ProxyMode::On {
            return None;
        }
        let path = self.proxies.of(target.key, target.fingerprint)?;
        self.proxies.picture(target, &path, self.convert)
    }
}
