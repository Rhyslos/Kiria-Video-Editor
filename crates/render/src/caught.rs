//! Graphics errors caught where they happen instead of ending the thread.
//!
//! The graphics library hands a mistake (a size over the card's limit, a
//! buffer used while it is still being read back) to the device's
//! uncaptured-error handler, which by default ends the thread that made it:
//! a dead preview worker would leave the viewer black for the rest of the
//! session. Work run through `caught` has its errors collected on this
//! thread instead, so it can be told as an error for that frame.

use std::future::Future;
use std::task::{Context, Poll, Waker};

/// Runs `work` with every graphics error it causes on `device` from this
/// thread caught: its result, or the first error.
///
/// What `work` made may hold broken resources when it failed, so it is
/// dropped; whoever keeps resources between calls should make them afresh.
pub fn caught<T>(device: &wgpu::Device, work: impl FnOnce() -> T) -> Result<T, wgpu::Error> {
    // One scope for each kind of error; popped in the reverse order.
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let done = work();
    let errors = [validation.pop(), memory.pop(), internal.pop()].map(now);
    match errors.into_iter().flatten().next() {
        Some(error) => Err(error),
        None => Ok(done),
    }
}

/// What a popped scope caught. The scopes of a native device are answered at
/// once; one that isn't (never seen) counts as nothing caught.
fn now(popped: impl Future<Output = Option<wgpu::Error>>) -> Option<wgpu::Error> {
    let mut popped = std::pin::pin!(popped);
    match popped.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(caught) => caught,
        Poll::Pending => None,
    }
}

/// An error in a few words for a message: the graphics library's own text,
/// on one line.
pub fn describe(error: &wgpu::Error) -> String {
    let text = match error {
        wgpu::Error::OutOfMemory { .. } => "the graphics card ran out of memory".to_owned(),
        other => other.to_string(),
    };
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> Option<wgpu::Device> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
        let (device, _) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
        Some(device)
    }

    fn texture(device: &wgpu::Device, width: u32) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test"),
            size: wgpu::Extent3d {
                width,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }

    #[test]
    fn a_mistake_is_an_error_not_the_end_of_the_thread() {
        let Some(device) = device() else {
            eprintln!("no graphics device: skipped");
            return;
        };
        let too_wide = device.limits().max_texture_dimension_2d + 1;
        let error = caught(&device, || texture(&device, too_wide)).err();
        let error = error.expect("a texture over the card's limit is refused");
        assert!(matches!(error, wgpu::Error::Validation { .. }));
        assert!(!describe(&error).contains('\n'));
        // Nothing is left pushed: the next work is checked on its own.
        assert!(caught(&device, || texture(&device, 4)).is_ok());
    }
}
