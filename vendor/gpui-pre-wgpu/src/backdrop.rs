//! GPU backdrop effects for the web scene.

use bytemuck::{Pod, Zeroable};
use gpui::{
    BorderStyle, Bounds, BoxShadow, Corners, Edges, Hsla, PaintQuad, Pixels, Quad, Rgba, quad,
};
use wgpu::util::DeviceExt;

/// Creates a backdrop quad with a Gaussian blur, CSS saturation of 150% and a tint.
/// The optional base fills transparent pixels behind the scene, such as native vibrancy.
pub fn backdrop_quad(
    bounds: Bounds<Pixels>,
    corners: Corners<Pixels>,
    tint: Hsla,
    opaque_base: Option<Hsla>,
    sigma_device_px: f32,
) -> PaintQuad {
    // GPUI has no backdrop primitive. Carry the effect in a borderless solid quad
    // with reserved, out-of-range border values. GPUI scales both alphas by
    // element opacity. The tint's alpha is then recovered from their ratio.
    let mut base = opaque_base.unwrap_or_default();
    base.h = sigma_device_px.round().max(1.) + base.h.rem_euclid(1.);
    base.a = -tint.a;
    quad(
        bounds,
        corners,
        tint.alpha(1.),
        Edges::default(),
        base,
        if opaque_base.is_some() {
            BorderStyle::Solid
        } else {
            BorderStyle::Dashed
        },
    )
}

/// Marks the drop shadows of a translucent surface to clip their interior.
pub fn clip_backdrop_shadows(shadows: &mut [BoxShadow]) {
    for shadow in shadows.iter_mut().filter(|shadow| !shadow.inset) {
        // A negative blur radius (including -0 for a stroke) reserves the
        // outside-only variant. GPUI keeps the sign when scaling to device pixels.
        shadow.blur_radius = shadow.blur_radius.abs() * -1.;
    }
}

/// A sharp exterior stroke whose width survives GPUI's shadow bounds snapping.
pub fn backdrop_stroke(width: Pixels, mut color: Hsla) -> BoxShadow {
    // Carry the exact device-scaled width in the blur field. The reserved hue
    // range identifies a stroke so the shader uses it for coverage, not blur.
    color.h += 2.;
    BoxShadow::new(gpui::px(0.), gpui::px(0.), color)
        .spread_radius(width)
        .blur_radius(-width)
}

pub(crate) fn is_backdrop(quad: &Quad) -> bool {
    quad.border_color.a < 0.
        && quad.border_color.h >= 1.
        && quad.border_widths == Edges::default()
        && quad.background.as_solid().is_some_and(|color| color.a > 0.)
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Parameters {
    bounds: [f32; 4],
    radii: [f32; 4],
    tint: [f32; 4],
    base: [f32; 4],
    sigma: f32,
    saturation: f32,
    srgb_target: u32,
    opacity: f32,
    working_size: [f32; 2],
    working_pad: [f32; 2],
}

const DOWNSAMPLE: u32 = 4;

pub(crate) struct BackdropRenderer {
    layout: wgpu::BindGroupLayout,
    downsample: wgpu::RenderPipeline,
    horizontal: wgpu::RenderPipeline,
    vertical: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    format: wgpu::TextureFormat,
    targets: Option<Targets>,
}

struct Targets {
    width: u32,
    height: u32,
    scene: wgpu::TextureView,
    low_a: wgpu::TextureView,
    low_b: wgpu::TextureView,
}

impl BackdropRenderer {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("backdrop"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<Parameters>() as u64),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("backdrop"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("backdrop"),
            source: wgpu::ShaderSource::Wgsl(include_str!("backdrop.wgsl").into()),
        });
        let pipeline = |entry, blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_fullscreen"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("backdrop"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            downsample: pipeline("fs_downsample", None),
            horizontal: pipeline("fs_horizontal", None),
            vertical: pipeline("fs_vertical", None),
            composite: pipeline(
                "fs_composite",
                Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            ),
            blit: pipeline("fs_blit", None),
            layout,
            sampler,
            format,
            targets: None,
        }
    }

    pub(crate) fn prepare(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self
            .targets
            .as_ref()
            .is_some_and(|targets| (targets.width, targets.height) == (width, height))
        {
            return;
        }
        let create = |label, width, height| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        self.targets = Some(Targets {
            width,
            height,
            scene: create("backdrop scene", width, height),
            low_a: create(
                "backdrop A",
                width.div_ceil(DOWNSAMPLE),
                height.div_ceil(DOWNSAMPLE),
            ),
            low_b: create(
                "backdrop B",
                width.div_ceil(DOWNSAMPLE),
                height.div_ceil(DOWNSAMPLE),
            ),
        });
    }

    pub(crate) fn invalidate_targets(&mut self) {
        self.targets = None;
    }

    pub(crate) fn scene_view(&self) -> &wgpu::TextureView {
        &self
            .targets
            .as_ref()
            .expect("prepared backdrop targets")
            .scene
    }

    pub(crate) fn paint(
        &self,
        quad: &Quad,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let targets = self.targets.as_ref().expect("prepared backdrop targets");
        let bounds = quad.bounds;
        let mask = quad.content_mask.bounds;
        let scissor = |bounds: Bounds<gpui::ScaledPixels>| {
            let x = (bounds.left().0 as u32).min(targets.width);
            let y = (bounds.top().0 as u32).min(targets.height);
            let right = (bounds.right().0 as u32).min(targets.width);
            let bottom = (bounds.bottom().0 as u32).min(targets.height);
            (x, y, right.saturating_sub(x), bottom.saturating_sub(y))
        };
        let full = scissor(bounds);
        let visible = scissor(bounds.intersect(&mask));
        if visible.2 == 0 || visible.3 == 0 {
            return;
        }
        let surface = quad.background.as_solid().expect("backdrop tint");
        let opacity = surface.a;
        let tint = surface.alpha(-quad.border_color.a / opacity);
        let mut base = quad
            .border_color
            .alpha(f32::from(quad.border_style == BorderStyle::Solid));
        let sigma = base.h.floor();
        base.h = base.h.fract();
        let radii = quad.corner_radii;
        let base_rgb = rgba_array(base);
        let mut base = base_rgb.map(|component| component * base_rgb[3]);
        if self.format.is_srgb() {
            for channel in &mut base[..3] {
                *channel = srgb_to_linear(*channel);
            }
        }
        let low = (
            0,
            0,
            full.2.div_ceil(DOWNSAMPLE),
            full.3.div_ceil(DOWNSAMPLE),
        );
        let params = Parameters {
            bounds: [full.0 as f32, full.1 as f32, full.2 as f32, full.3 as f32],
            radii: [
                radii.top_left.0,
                radii.top_right.0,
                radii.bottom_right.0,
                radii.bottom_left.0,
            ],
            tint: rgba_array(tint),
            base,
            sigma,
            saturation: 1.5,
            srgb_target: u32::from(self.format.is_srgb()),
            opacity,
            working_size: [low.2 as f32, low.3 as f32],
            working_pad: [0.; 2],
        };
        let buffer = uniform(device, &params);
        let scene = self.bind(device, &targets.scene, &buffer);
        let low_a = self.bind(device, &targets.low_a, &buffer);
        let low_b = self.bind(device, &targets.low_b, &buffer);
        let clear = wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT);
        self.draw(
            encoder,
            &targets.low_a,
            &self.downsample,
            &scene,
            Some(low),
            clear,
        );
        self.draw(
            encoder,
            &targets.low_b,
            &self.horizontal,
            &low_a,
            Some(low),
            clear,
        );
        self.draw(
            encoder,
            &targets.low_a,
            &self.vertical,
            &low_b,
            Some(low),
            clear,
        );
        self.draw(
            encoder,
            &targets.scene,
            &self.composite,
            &low_a,
            Some(visible),
            wgpu::LoadOp::Load,
        );
    }

    pub(crate) fn present(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    ) {
        let buffer = uniform(device, &Parameters::zeroed());
        let bind_group = self.bind(device, self.scene_view(), &buffer);
        self.draw(
            encoder,
            target,
            &self.blit,
            &bind_group,
            None,
            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
        );
    }

    fn bind(
        &self,
        device: &wgpu::Device,
        source: &wgpu::TextureView,
        params: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("backdrop"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params.as_entire_binding(),
                },
            ],
        })
    }

    fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        pipeline: &wgpu::RenderPipeline,
        input: &wgpu::BindGroup,
        scissor: Option<(u32, u32, u32, u32)>,
        load: wgpu::LoadOp<wgpu::Color>,
    ) {
        let mut pass = render_pass(encoder, target, load, "backdrop");
        if let Some((x, y, width, height)) = scissor {
            pass.set_scissor_rect(x, y, width, height);
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, input, &[]);
        pass.draw(0..3, 0..1);
    }
}

pub(crate) fn render_pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
    label: &str,
) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
            depth_slice: None,
        })],
        ..Default::default()
    })
}

fn rgba_array(color: Hsla) -> [f32; 4] {
    let Rgba { r, g, b, a } = Rgba::from(color);
    [r, g, b, a]
}

fn uniform(device: &wgpu::Device, params: &Parameters) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("backdrop parameters"),
        contents: bytemuck::bytes_of(params),
        usage: wgpu::BufferUsages::UNIFORM,
    })
}

fn srgb_to_linear(color: f32) -> f32 {
    if color <= 0.04045 {
        color / 12.92
    } else {
        ((color + 0.055) / 1.055).powf(2.4)
    }
}
