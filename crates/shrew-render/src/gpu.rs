//! GPU side: instance formats, the two pipelines, and growable instance buffers.

use bytemuck::{Pod, Zeroable};

use crate::Color;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct RectInstance {
    pub rect: [f32; 4],
    pub color: Color,
    pub border_color: Color,
    /// Corner radius, border width.
    pub params: [f32; 2],
    /// `[x0, y0, x1, y1]`.
    pub clip: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct GlyphInstance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub uv: [f32; 2],
    pub color: Color,
    /// 0 = coverage mask tinted by `color`, 1 = color glyph.
    pub kind: u32,
    pub clip: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct Globals {
    pub viewport: [f32; 2],
    pub srgb_target: u32,
    pub _pad: u32,
}

const RECT_ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x4, 1 => Unorm8x4, 2 => Unorm8x4, 3 => Float32x2, 4 => Float32x4
];
const GLYPH_ATTRS: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
    0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Unorm8x4, 4 => Uint32, 5 => Float32x4
];

pub(crate) struct Pipelines {
    pub layout: wgpu::BindGroupLayout,
    pub rect: wgpu::RenderPipeline,
    pub glyph: wgpu::RenderPipeline,
}

impl Pipelines {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shrew shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let tex = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shrew bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<Globals>() as u64),
                    },
                    count: None,
                },
                tex(1),
                tex(2),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shrew pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let blend = wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING;
        let make = |label, vs, fs, stride: usize, attrs: &[wgpu::VertexAttribute]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: stride as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: attrs,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let rect = make("shrew rects", "rect_vs", "rect_fs", size_of::<RectInstance>(), &RECT_ATTRS);
        let glyph = make("shrew glyphs", "glyph_vs", "glyph_fs", size_of::<GlyphInstance>(), &GLYPH_ATTRS);
        Pipelines { layout, rect, glyph }
    }
}

/// A vertex buffer that grows (to the next power of two) when a frame needs more room.
pub(crate) struct InstanceBuffer {
    pub buffer: wgpu::Buffer,
    label: &'static str,
}

impl InstanceBuffer {
    pub fn new(device: &wgpu::Device, label: &'static str, bytes: u64) -> Self {
        InstanceBuffer { buffer: Self::create(device, label, bytes), label }
    }

    fn create(device: &wgpu::Device, label: &'static str, bytes: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: bytes.max(256),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    pub fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        if data.len() as u64 > self.buffer.size() {
            self.buffer = Self::create(device, self.label, (data.len() as u64).next_power_of_two());
        }
        queue.write_buffer(&self.buffer, 0, data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_layouts_match_the_shader() {
        assert_eq!(size_of::<RectInstance>(), 48);
        assert_eq!(size_of::<GlyphInstance>(), 48);
        assert_eq!(size_of::<Globals>(), 16);
        assert_eq!(RECT_ATTRS[4].offset, 32);
        assert_eq!(GLYPH_ATTRS[5].offset, 32);
    }
}
