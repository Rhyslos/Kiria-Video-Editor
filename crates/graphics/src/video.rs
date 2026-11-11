//! Whether the window's device can decode video itself, and why not when it can't.

use crate::{Backend, Platform, Program};
use std::sync::Arc;
use video_device::VideoDevice;

/// What a start found out about decoding video on the window's own device.
#[derive(Debug, Clone)]
pub enum VideoDecoding {
    /// The device was opened here with its video decode queues: the handle
    /// is what the decoder needs to use it.
    OnDevice(Arc<VideoDevice>),
    /// Why not, in words that read after "not available".
    NotAvailable(String),
}

impl VideoDecoding {
    /// For a window whose device the toolkit opened by itself.
    pub(crate) fn without_own_device(
        program: Program,
        backend: Backend,
        platform: Platform,
    ) -> VideoDecoding {
        let reason = match (platform, backend) {
            _ if !program.plays_video() => "this window plays no video",
            (Platform::Other, _) => "only set up on Windows so far",
            (_, Backend::Direct3D12) => "the window is drawn with Direct3D 12, and this takes Vulkan",
            (_, Backend::ToolkitDefault) => {
                "the window is drawn with the system's default graphics, and this takes Vulkan"
            }
            (_, Backend::Vulkan) => "the device was not opened for it",
        };
        VideoDecoding::NotAvailable(reason.to_owned())
    }

    /// The handle for the decoder, when there is one.
    pub fn device(&self) -> Option<&Arc<VideoDevice>> {
        match self {
            VideoDecoding::OnDevice(device) => Some(device),
            VideoDecoding::NotAvailable(_) => None,
        }
    }

    /// One line for Diagnostics and the log: "H.264, H.265, AV1", or
    /// "not available (the reason)".
    pub fn describe(&self) -> String {
        match self {
            VideoDecoding::OnDevice(device) => device.codec_names(),
            VideoDecoding::NotAvailable(reason) => format!("not available ({reason})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_the_toolkit_opened_says_why_it_cannot_decode() {
        let editor =
            |backend, platform| VideoDecoding::without_own_device(Program::Editor, backend, platform);
        let direct3d = editor(Backend::Direct3D12, Platform::Windows);
        assert!(direct3d.device().is_none());
        assert_eq!(
            direct3d.describe(),
            "not available (the window is drawn with Direct3D 12, and this takes Vulkan)"
        );
        let last_resort = editor(Backend::ToolkitDefault, Platform::Windows);
        assert!(last_resort.describe().contains("default graphics"));
        for backend in [Backend::Vulkan, Backend::ToolkitDefault] {
            let elsewhere = editor(backend, Platform::Other);
            assert_eq!(
                elsewhere.describe(),
                "not available (only set up on Windows so far)"
            );
        }
    }

    #[test]
    fn the_launcher_never_asks_for_it() {
        for platform in [Platform::Windows, Platform::Other] {
            let launcher = VideoDecoding::without_own_device(Program::Launcher, Backend::Vulkan, platform);
            assert_eq!(launcher.describe(), "not available (this window plays no video)");
        }
    }
}
