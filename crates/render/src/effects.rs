//! Picture effects: each a pass that reads a picture the compositor made
//! and writes it again, changed, into another of the same size. A clip's
//! effects run one after another between two such pictures, after its
//! picture is drawn upright at the size it takes in the frame and before
//! it is placed (`engine::compose`). Numbers arrive worked out (`Effect`);
//! nothing here knows of projects.

use crate::compositor::{
    Frame, Working, pipeline, sampler_entry, smooth_sampler, texture_binding, texture_entry, uniform_buffer,
    uniform_entry,
};

/// One effect with its numbers, as the shader takes them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Effect {
    /// Darker towards the edges: `amount` 0-1, from `size` (0-1 of the way
    /// to an edge's middle), reaching it `softness` further on.
    Vignette { amount: f32, size: f32, softness: f32 },
}

impl Effect {
    fn numbers(&self) -> [f32; 4] {
        match *self {
            Effect::Vignette {
                amount,
                size,
                softness,
            } => [0.0, amount, size, softness],
        }
    }
}

const PASS_BYTES: u64 = 32;

/// The effect passes for pictures made in one working space.
#[derive(Debug)]
pub struct Effects {
    working: Working,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl Effects {
    pub fn new(device: &wgpu::Device, working: Working) -> Effects {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("effects"),
            source: wgpu::ShaderSource::Wgsl(include_str!("effects.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("effect"),
            entries: &[uniform_entry(PASS_BYTES), sampler_entry(), texture_entry(2)],
        });
        let pipeline = pipeline(
            device,
            "effect",
            &layout,
            &module,
            ("whole", "effect"),
            working.format(),
            // Every pixel is written as worked out: nothing is blended.
            None,
        );
        Effects {
            working,
            pipeline,
            layout,
            sampler: smooth_sampler(device),
        }
    }

    /// Adds to `encoder` a pass per effect, `picture` and `spare` (pictures
    /// of the same size, made by a compositor in this working space) taking
    /// turns. Returns the one holding the result first, then the other.
    pub fn apply(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        picture: Frame,
        spare: Frame,
        effects: &[Effect],
    ) -> (Frame, Frame) {
        let (mut from, mut to) = (picture, spare);
        let (w, h) = from.size();
        let linear = if self.working == Working::Linear { 1.0 } else { 0.0 };
        for effect in effects {
            // Each pass its own numbers: they are written when the queue is
            // submitted, after every pass has been recorded.
            let uniforms = uniform_buffer(device, PASS_BYTES);
            let numbers = effect.numbers();
            let rows = [numbers, [linear, w as f32, h as f32, 0.0]];
            let bytes: Vec<u8> = rows.as_flattened().iter().flat_map(|n| n.to_ne_bytes()).collect();
            queue.write_buffer(&uniforms, 0, &bytes);
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("effect"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    texture_binding(2, from.view()),
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("effect"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: to.view(),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..4, 0..1);
            drop(pass);
            std::mem::swap(&mut from, &mut to);
        }
        (from, to)
    }
}
