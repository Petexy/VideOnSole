//! The film itself, at its own resolution, over the frame the toolkit
//! composed.
//!
//! **Why this file exists.** Everything else on the screen is drawn by
//! `lxb-render`, which is the point of building on the toolkit at all. But
//! `Ui::picture` reads a *file* into one 512-pixel cell of a shared atlas, and
//! a film is not a file that can be read once: its frames arrive twenty-four
//! times a second at their own size, and the whole purpose of the application
//! is to show them. So the frame is composed by `lxb-render` as everywhere
//! else and the film is drawn here, in a pass of this application's own,
//! afterwards.
//!
//! **What that costs, and what it does not.** The pass runs after `Ui::end`,
//! so the film is over everything the toolkit drew. Anything that has to
//! appear *over* a film therefore cannot simply be drawn on top of it. There
//! are two answers to that here, and which one is used is the difference
//! between a film that is playing and one that is not:
//!
//! * **The stage gets out of the way.** A stopped film is a picture on a
//!   page, and the page's furniture takes its room from the stage — see
//!   `view.rs`. This is what the whole application did at first.
//! * **The film is punched through.** A film that is *playing* fills the
//!   window, and every panel over it is a [`Hole`]: a rounded rectangle this
//!   pass leaves alone, so what the toolkit drew there survives. A panel is
//!   the shape of the hole, which is why the chrome over a playing film is
//!   panels and not loose words — a hole around a word would be a rectangle
//!   of wallpaper cut out of the picture.
//!
//! The second is also why this pass draws the **black** behind a playing film
//! rather than the toolkit doing it: `lxb-render` has no way to ask for a
//! plain opaque rectangle of a colour it did not choose, and a film's
//! letterbox is not the shell's wallpaper.
//!
//! ## The colour is done here, on the graphics card
//!
//! A decoder hands back three planes of luma and chroma, not pixels. Turning
//! those into red, green and blue is a matrix per pixel — twelve megabytes of
//! arithmetic per 4K frame — and a graphics card does it for nothing while a
//! processor doing it is a processor not decoding the next frame. So the
//! planes go up as they are and the shader below does the conversion, from the
//! coefficients the film itself declares.
//!
//! Two plane layouts cover everything that reaches here: three planes, which
//! is what a software decoder produces, and two, which is what comes back off
//! the hardware. Anything else the player converts before it arrives.
//!
//! **What this does not do is high dynamic range.** A film in BT.2020 with a
//! perceptual-quantiser curve is drawn through the same standard-range path as
//! everything else and will look flat. Doing it properly means the colour
//! protocols the shell speaks and a window that has asked for them, which this
//! application has not; saying so here is better than a picture that is
//! quietly wrong.

use ffmpeg::format::Pixel;
use ffmpeg_next as ffmpeg;

/// Where the film goes this frame.
///
/// A rectangle and how much of it is there. In the same pixels everything else
/// on the page is laid out in — the window's, top-left origin — because a
/// player that had to convert would convert one of the two wrongly.
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub centre: [f32; 2],
    /// Half the drawn width and height.
    pub half: [f32; 2],
    pub opacity: f32,
    /// How much light is left in it. One, except while something is being
    /// opened over it — a menu steps the whole page back and dims it, and a
    /// film that stayed bright while the page behind it dimmed would be the
    /// one surface that had not heard.
    pub dim: f32,
    /// How round the picture's own corners are.
    ///
    /// Nought for a film on a screen. A film opening out of a card carries the
    /// card's rounding and loses it on the way — see [`crate::view::View::stage_radius`].
    pub radius: f32,
    /// Nothing is drawn outside this. It is what keeps the film inside the
    /// stage it was given rather than over the transport under it.
    pub within: [f32; 4],
}

/// A rounded rectangle this pass leaves alone.
///
/// One per panel standing over a playing film. The rectangle is the panel's
/// own, grown by its rim, and the radius is the panel's — a hole that did not
/// match would show as a hairline of film along the panel's edge.
#[derive(Debug, Clone, Copy, Default)]
pub struct Hole {
    pub rect: [f32; 4],
    pub radius: f32,
}

/// One frame's worth of this pass: the film, the panels over it, and the
/// black behind it.
///
/// Handed over as one thing because the three are one decision — a hole that
/// did not arrive with the placement it was cut in would be a hole in the
/// wrong picture.
pub struct Showing<'a> {
    pub placement: Option<Placement>,
    pub holes: &'a [Hole],
    /// How much of the window is the film's own black rather than the shell's
    /// wallpaper. **Not** the film's opacity: the black arrives as a film
    /// starts playing and stays while it is seeking or between two frames,
    /// when there may be no picture to draw at all.
    pub blackout: f32,
    /// How much of the window that black covers, and how round it is at the
    /// corners. Everything, except the band the button hints are in: they are
    /// drawn by the toolkit and this pass runs after it, so black painted over
    /// them would stay over them. While a film is opening out of a card it is
    /// the card, rounded to match — see [`crate::view::View::letterbox`].
    pub curtain: Hole,
}

/// How many panels may stand over a film at once.
///
/// Four is what the player draws — the transport, the head, the details and
/// the menu — and the two spare are so that adding one is not a shader
/// change. Anything past this is dropped rather than drawn wrongly.
pub const HOLES: usize = 6;

/// How the planes of one frame are arranged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// Luma, then blue-difference and red-difference in planes of their own,
    /// each half as wide and half as tall. What a software decoder produces.
    Three,
    /// Luma, then one plane holding both differences side by side. What comes
    /// back from the hardware.
    Two,
}

/// The colour, as an affine transform from what is stored to what is shown.
///
/// Three rows of `rgb = row · yuv + row.w`, worked out from the coefficients
/// the film declares and whether its levels use the whole range of a byte.
/// Kept as a transform rather than as a name, because the shader should not
/// have a table of standards in it — the standard is data.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Colour {
    rows: [[f32; 4]; 3],
}

impl Colour {
    /// From the two coefficients that define every one of these standards and
    /// whether the levels are studio or full.
    fn of(kr: f32, kb: f32, full: bool) -> Colour {
        let kg = (1.0 - kr - kb).max(0.0001);
        // The three chroma coefficients every one of these standards derives
        // the same way.
        let cr_r = 2.0 * (1.0 - kr);
        let cb_b = 2.0 * (1.0 - kb);
        let cb_g = -2.0 * kb * (1.0 - kb) / kg;
        let cr_g = -2.0 * kr * (1.0 - kr) / kg;

        // Studio levels put black at 16 and white at 235, which is what
        // broadcast has always done and what nearly every film on a disk uses;
        // full levels use the whole byte. Reading one as the other is the
        // washed-out picture everybody has seen.
        let (luma, chroma, black, middle) = if full {
            (1.0, 1.0, 0.0, 128.0 / 255.0)
        } else {
            (255.0 / 219.0, 255.0 / 224.0, 16.0 / 255.0, 128.0 / 255.0)
        };
        let row = |cb: f32, cr: f32| -> [f32; 4] {
            let (cb, cr) = (chroma * cb, chroma * cr);
            [luma, cb, cr, -(luma * black + cb * middle + cr * middle)]
        };
        Colour {
            rows: [row(0.0, cr_r), row(cb_g, cr_g), row(cb_b, 0.0)],
        }
    }

    /// What a frame says about itself.
    ///
    /// A film that declares nothing is read as the standard for its size,
    /// which is the guess every player makes and the right one: standard
    /// definition predates the newer coefficients and high definition
    /// postdates them, so the height really does say which.
    fn of_frame(frame: &ffmpeg::frame::Video) -> Colour {
        use ffmpeg::color::{Range, Space};
        let full = frame.color_range() == Range::JPEG
            || matches!(
                frame.format(),
                Pixel::YUVJ420P | Pixel::YUVJ422P | Pixel::YUVJ444P
            );
        match frame.color_space() {
            Space::BT470BG | Space::SMPTE170M | Space::FCC => Colour::of(0.299, 0.114, full),
            Space::SMPTE240M => Colour::of(0.212, 0.087, full),
            Space::BT2020NCL | Space::BT2020CL => Colour::of(0.2627, 0.0593, full),
            Space::BT709 => Colour::of(0.2126, 0.0722, full),
            _ if frame.height() <= 576 => Colour::of(0.299, 0.114, full),
            _ => Colour::of(0.2126, 0.0722, full),
        }
    }
}

/// The planes of one film, resident on the card.
struct Resident {
    planes: [wgpu::Texture; 3],
    bind: wgpu::BindGroup,
    layout: Layout,
    /// The luma plane's size, which is the film's.
    size: (u32, u32),
    format: Pixel,
    colour: Colour,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Placed {
    centre: [f32; 2],
    half: [f32; 2],
    screen: [f32; 2],
    opacity: f32,
    planes: f32,
    colour: [[f32; 4]; 3],
    /// `dim`, how many holes are real, how round this quad's own corners are,
    /// and one word of nothing — a uniform buffer counts in sixteens and a
    /// lone float would push the arrays after it off their alignment.
    about: [f32; 4],
    holes: [[f32; 4]; HOLES],
    /// The radius of each hole in `x`. The rest is room the alignment was
    /// going to take anyway.
    corners: [[f32; 4]; HOLES],
}

impl Placed {
    /// The holes, in the shape the card wants them.
    fn cut(holes: &[Hole]) -> ([[f32; 4]; HOLES], [[f32; 4]; HOLES], f32) {
        let mut rects = [[0.0; 4]; HOLES];
        let mut corners = [[0.0; 4]; HOLES];
        let mut count = 0;
        for hole in holes
            .iter()
            .filter(|hole| hole.rect[2] > 0.0 && hole.rect[3] > 0.0)
        {
            if count == HOLES {
                break;
            }
            rects[count] = hole.rect;
            corners[count] = [hole.radius.max(0.0), 0.0, 0.0, 0.0];
            count += 1;
        }
        (rects, corners, count as f32)
    }
}

pub struct Films {
    pipeline: wgpu::RenderPipeline,
    /// The black behind a playing film, with the same holes cut out of it.
    /// Its own pipeline and its own copy of the uniform because it is drawn
    /// in the same pass as the film and one buffer cannot hold two values.
    curtain: wgpu::RenderPipeline,
    picture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    buffer: wgpu::Buffer,
    placing: wgpu::BindGroup,
    curtain_buffer: wgpu::Buffer,
    curtain_placing: wgpu::BindGroup,
    resident: Option<Resident>,
}

impl Films {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Films {
        let placing_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("where a film goes"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        // Three textures and a sampler, whatever the layout is. A two-plane
        // frame binds its chroma plane twice rather than needing a second
        // layout: the shader reads whichever the count says are real.
        let mut entries: Vec<wgpu::BindGroupLayoutEntry> = (0..3)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            })
            .collect();
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
        let picture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("a film"),
            entries: &entries,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("film"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("film"),
            bind_group_layouts: &[Some(&placing_layout), Some(&picture_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("film"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // The curtain wants nothing but the placement, so it has a layout of
        // its own rather than binding a film it never reads.
        let curtain_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("the black behind a film"),
            bind_group_layouts: &[Some(&placing_layout)],
            immediate_size: 0,
        });
        let curtain = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("the black behind a film"),
            layout: Some(&curtain_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("curtain_vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("curtain_fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // Linear, which is what upsamples the half-size chroma planes and what
        // fits a film to a screen that is not its own size. No mip chain: a
        // film is shown at about its own size or larger, never at a fraction
        // of it, which is the case a chain is for.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("film"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("where a film goes"),
            size: std::mem::size_of::<Placed>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let placing = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("where a film goes"),
            layout: &placing_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        let curtain_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("the black behind a film"),
            size: std::mem::size_of::<Placed>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let curtain_placing = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("the black behind a film"),
            layout: &placing_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: curtain_buffer.as_entire_binding(),
            }],
        });

        Films {
            pipeline,
            curtain,
            picture_layout,
            sampler,
            buffer,
            placing,
            curtain_buffer,
            curtain_placing,
            resident: None,
        }
    }

    /// Forget the film. What the player does when it moves to another one, so
    /// the last frame of the one before it is not left on the screen.
    pub fn clear(&mut self) {
        self.resident = None;
    }

    /// Put one decoded frame on the card.
    ///
    /// The textures are made once and written over on every frame after that:
    /// a film is thousands of pictures of one size, and allocating for each of
    /// them is the one thing that would make this expensive.
    pub fn show(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &ffmpeg::frame::Video,
    ) -> bool {
        let Some(layout) = layout_of(frame.format()) else {
            return false;
        };
        let size = (frame.width(), frame.height());
        if size.0 == 0 || size.1 == 0 {
            return false;
        }
        let colour = Colour::of_frame(frame);

        let same = self
            .resident
            .as_ref()
            .is_some_and(|resident| resident.size == size && resident.format == frame.format());
        if !same {
            self.resident = Some(self.make(device, layout, frame));
        }
        let Some(resident) = self.resident.as_mut() else {
            return false;
        };
        resident.colour = colour;

        let used = planes_of(layout);
        for index in 0..used {
            if index >= frame.planes() {
                return false;
            }
            let rows = frame.plane_height(index);
            let stride = frame.stride(index) as u32;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &resident.planes[index],
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                frame.data(index),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    // The decoder's own row length, which is padded out to
                    // whatever the instruction set liked. Handed over as it is
                    // rather than repacked: a copy of every row of a 4K frame,
                    // sixty times a second, to save the graphics driver a step
                    // it takes anyway.
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(rows),
                },
                wgpu::Extent3d {
                    width: frame.plane_width(index),
                    height: rows,
                    depth_or_array_layers: 1,
                },
            );
        }
        true
    }

    fn make(
        &self,
        device: &wgpu::Device,
        layout: Layout,
        frame: &ffmpeg::frame::Video,
    ) -> Resident {
        let used = planes_of(layout);
        let planes: [wgpu::Texture; 3] = std::array::from_fn(|index| {
            // The third plane of a two-plane frame is never read; it is made a
            // single texel rather than left out, so one bind group layout
            // serves both arrangements.
            let real = index < used && index < frame.planes();
            let (width, height) = if real {
                (frame.plane_width(index), frame.plane_height(index))
            } else {
                (1, 1)
            };
            let format = match (layout, index) {
                // The one plane holding both differences: two channels to the
                // texel, which is exactly what a texture of two channels is
                // for — and what makes the sampler upsample them together.
                (Layout::Two, 1) => wgpu::TextureFormat::Rg8Unorm,
                _ => wgpu::TextureFormat::R8Unorm,
            };
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("film plane"),
                size: wgpu::Extent3d {
                    width: width.max(1),
                    height: height.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        });
        let views: [wgpu::TextureView; 3] =
            std::array::from_fn(|index| planes[index].create_view(&Default::default()));
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("a film"),
            layout: &self.picture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&views[2]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        Resident {
            planes,
            bind,
            layout,
            size: (frame.width(), frame.height()),
            format: frame.format(),
            colour: Colour::of_frame(frame),
        }
    }

    /// Draw the black and then the film, over the frame the toolkit composed.
    ///
    /// One pass, loading rather than clearing — everything under it is the
    /// wallpaper, the panes and the words, and some of it is still wanted:
    /// whatever falls inside a `hole` is what the toolkit drew there and is
    /// left exactly as it is.
    ///
    pub fn draw(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        into: &wgpu::TextureView,
        screen: [f32; 2],
        showing: Showing<'_>,
    ) {
        let Showing {
            placement,
            holes,
            blackout,
            curtain: over,
        } = showing;
        let placed = placement.filter(|placement| placement.opacity > 0.001);
        let film = placed.zip(self.resident.as_ref());
        let curtain = blackout > 0.001 && over.rect[2] > 0.0 && over.rect[3] > 0.0;
        if film.is_none() && !curtain {
            return;
        }
        let (cut, corners, count) = Placed::cut(holes);

        if curtain {
            queue.write_buffer(
                &self.curtain_buffer,
                0,
                bytemuck::bytes_of(&Placed {
                    centre: [
                        over.rect[0] + over.rect[2] * 0.5,
                        over.rect[1] + over.rect[3] * 0.5,
                    ],
                    half: [over.rect[2] * 0.5, over.rect[3] * 0.5],
                    screen,
                    opacity: blackout.clamp(0.0, 1.0),
                    planes: 0.0,
                    colour: [[0.0; 4]; 3],
                    about: [1.0, count, over.radius.max(0.0), 0.0],
                    holes: cut,
                    corners,
                }),
            );
        }
        if let Some((placement, resident)) = film {
            queue.write_buffer(
                &self.buffer,
                0,
                bytemuck::bytes_of(&Placed {
                    centre: placement.centre,
                    half: placement.half,
                    screen,
                    opacity: placement.opacity,
                    planes: planes_of(resident.layout) as f32,
                    colour: resident.colour.rows,
                    about: [
                        placement.dim.clamp(0.0, 1.0),
                        count,
                        placement.radius.max(0.0),
                        0.0,
                    ],
                    holes: cut,
                    corners,
                }),
            );
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("film"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: into,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if curtain {
            pass.set_pipeline(&self.curtain);
            pass.set_bind_group(0, &self.curtain_placing, &[]);
            pass.draw(0..6, 0..1);
        }
        // The film second, and inside the stage it was given: a scissor
        // rectangle rather than a clip in the shader, because the stage is
        // whole pixels of the window and the card can do it for nothing.
        let Some((placement, resident)) = film else {
            return;
        };
        let Some([x, y, width, height]) = whole_pixels(placement.within, screen) else {
            return;
        };
        pass.set_pipeline(&self.pipeline);
        pass.set_scissor_rect(x, y, width, height);
        pass.set_bind_group(0, &self.placing, &[]);
        pass.set_bind_group(1, &resident.bind, &[]);
        pass.draw(0..6, 0..1);
    }
}

/// How many of the three textures an arrangement really uses.
fn planes_of(layout: Layout) -> usize {
    match layout {
        Layout::Three => 3,
        Layout::Two => 2,
    }
}

/// Which arrangement of planes a pixel format is, or `None` for one this does
/// not draw — which the player converts before it gets here.
fn layout_of(format: Pixel) -> Option<Layout> {
    match format {
        Pixel::YUV420P | Pixel::YUVJ420P => Some(Layout::Three),
        Pixel::NV12 => Some(Layout::Two),
        _ => None,
    }
}

/// Whether a decoded frame can be drawn as it stands, or has to be converted
/// first. Asked by the player, so the two cannot disagree about what this
/// draws.
pub fn draws(format: Pixel) -> bool {
    layout_of(format).is_some()
}

/// A scissor rectangle has to be whole pixels inside the target, and a
/// rectangle that came out empty is not a rectangle to draw nothing with —
/// it is a draw to skip.
fn whole_pixels([x, y, width, height]: [f32; 4], screen: [f32; 2]) -> Option<[u32; 4]> {
    let left = x.floor().max(0.0);
    let top = y.floor().max(0.0);
    let right = (x + width).ceil().min(screen[0]);
    let bottom = (y + height).ceil().min(screen[1]);
    if right <= left || bottom <= top {
        return None;
    }
    Some([
        left as u32,
        top as u32,
        (right - left) as u32,
        (bottom - top) as u32,
    ])
}

const SHADER: &str = r#"
const HOLES: i32 = 6;

struct Placed {
    centre: vec2<f32>,
    half: vec2<f32>,
    screen: vec2<f32>,
    opacity: f32,
    planes: f32,
    colour0: vec4<f32>,
    colour1: vec4<f32>,
    colour2: vec4<f32>,
    // How much light is left in the picture, how many of the holes below are
    // real, and how round this quad's own corners are.
    about: vec4<f32>,
    holes: array<vec4<f32>, 6>,
    corners: array<vec4<f32>, 6>,
};

@group(0) @binding(0) var<uniform> placed: Placed;
@group(1) @binding(0) var luma: texture_2d<f32>;
@group(1) @binding(1) var blue: texture_2d<f32>;
@group(1) @binding(2) var red: texture_2d<f32>;
@group(1) @binding(3) var sampling: sampler;

struct Drawn {
    @builtin(position) at: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Where this is on the window, which is what the holes are measured in.
    @location(1) pixel: vec2<f32>,
};

fn place(index: u32) -> Drawn {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[index];
    let pixel = placed.centre + corner * placed.half;

    var out: Drawn;
    out.at = vec4<f32>(
        pixel.x / placed.screen.x * 2.0 - 1.0,
        1.0 - pixel.y / placed.screen.y * 2.0,
        0.0,
        1.0,
    );
    out.uv = corner * vec2<f32>(0.5, 0.5) + vec2<f32>(0.5, 0.5);
    out.pixel = pixel;
    return out;
}

@vertex
fn vs(@builtin(vertex_index) index: u32) -> Drawn {
    return place(index);
}

/// The distance from a rounded rectangle, negative inside it.
///
/// The same field the toolkit's own shader draws its panels with, so a hole
/// and the panel that asked for it agree to the pixel along their whole
/// edge rather than only on the straight parts.
fn rounded(local: vec2<f32>, half: vec2<f32>, radius: f32) -> f32 {
    let corner = min(radius, min(half.x, half.y));
    let q = abs(local) - half + vec2<f32>(corner, corner);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0, 0.0))) - corner;
}

/// How much of this pixel a panel has already taken.
///
/// Feathered over the same three quarters of a pixel the toolkit feathers
/// its own edges over — a hard-edged hole shows as a stair down the side of
/// every panel, and a wider one as a halo of film around it.
/// How much of this pixel is inside the quad's own rounded corners.
///
/// One for a square-cornered quad everywhere but the last three quarters of a
/// pixel at its edge, which is where the toolkit's own edges land as well. A
/// film opening out of a card is rounded to the card and straightens as it
/// grows, and a picture that arrived on the screen still visibly a rounded
/// card would have arrived as something else.
fn inside(pixel: vec2<f32>) -> f32 {
    let d = rounded(pixel - placed.centre, placed.half, placed.about.z);
    return 1.0 - smoothstep(-0.75, 0.75, d);
}

fn covered(pixel: vec2<f32>) -> f32 {
    var most = 0.0;
    for (var index = 0; index < HOLES; index = index + 1) {
        if (f32(index) >= placed.about.y) {
            break;
        }
        let hole = placed.holes[index];
        let half = vec2<f32>(hole.z, hole.w) * 0.5;
        let centre = vec2<f32>(hole.x, hole.y) + half;
        let d = rounded(pixel - centre, half, placed.corners[index].x);
        most = max(most, 1.0 - smoothstep(-0.75, 0.75, d));
    }
    return most;
}

@vertex
fn curtain_vs(@builtin(vertex_index) index: u32) -> Drawn {
    return place(index);
}

/// The black a playing film sits on.
///
/// `lxb-render` will draw glass, light and words and not a plain rectangle of
/// a colour nobody chose, so the letterbox around a film is painted here.
@fragment
fn curtain_fs(drawn: Drawn) -> @location(0) vec4<f32> {
    let shown = inside(drawn.pixel) * (1.0 - covered(drawn.pixel));
    return vec4<f32>(0.0, 0.0, 0.0, placed.opacity * shown);
}

// What is stored in a film is not light: it is light put through a curve so
// that the values a byte can hold are spread the way an eye notices them. The
// target this draws into converts the other way on the way out, so the curve
// has to come off here or the picture is drawn through it twice — which is the
// washed-out, milky look of a player that did not.
//
// The sRGB curve rather than the one broadcast specifies, which differ by less
// than a step of a byte over most of their range and by rather more than that
// from getting it wrong altogether.
fn to_light(value: f32) -> f32 {
    if (value <= 0.04045) {
        return value / 12.92;
    }
    return pow((value + 0.055) / 1.055, 2.4);
}

@fragment
fn fs(drawn: Drawn) -> @location(0) vec4<f32> {
    let y = textureSample(luma, sampling, drawn.uv).r;
    var u: f32;
    var v: f32;
    if (placed.planes < 2.5) {
        // Both differences in one plane, two channels to the texel.
        let both = textureSample(blue, sampling, drawn.uv).rg;
        u = both.r;
        v = both.g;
    } else {
        u = textureSample(blue, sampling, drawn.uv).r;
        v = textureSample(red, sampling, drawn.uv).r;
    }

    let stored = vec3<f32>(y, u, v);
    let signal = vec3<f32>(
        dot(placed.colour0.xyz, stored) + placed.colour0.w,
        dot(placed.colour1.xyz, stored) + placed.colour1.w,
        dot(placed.colour2.xyz, stored) + placed.colour2.w,
    );
    let held = clamp(signal, vec3<f32>(0.0), vec3<f32>(1.0));
    let light = vec3<f32>(to_light(held.r), to_light(held.g), to_light(held.b));
    // Dimmed rather than faded: a film faded toward the page behind it would
    // show the page through itself, and what is behind a film here is its own
    // black.
    let shown = inside(drawn.pixel) * (1.0 - covered(drawn.pixel));
    return vec4<f32>(light * placed.about.x, placed.opacity * shown);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// Black and white have to come out black and white, or every film is
    /// drawn through a tint nobody asked for.
    #[test]
    fn studio_levels_put_black_at_sixteen_and_white_at_two_hundred_and_thirty_five() {
        let colour = Colour::of(0.2126, 0.0722, false);
        let apply = |y: f32, u: f32, v: f32| -> [f32; 3] {
            std::array::from_fn(|channel| {
                let row = colour.rows[channel];
                row[0] * y + row[1] * u + row[2] * v + row[3]
            })
        };
        let black = apply(16.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0);
        let white = apply(235.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0);
        for channel in 0..3 {
            assert!(black[channel].abs() < 0.001, "black came out {black:?}");
            assert!(
                (white[channel] - 1.0).abs() < 0.005,
                "white came out {white:?}"
            );
        }
    }

    #[test]
    fn full_levels_put_black_at_nought_and_white_at_the_top_of_the_byte() {
        let colour = Colour::of(0.2126, 0.0722, true);
        // Grey of every brightness has both differences sitting at the middle
        // of the byte, which is what makes it grey. Asking about the luma
        // alone measures a colour nothing ever stores.
        let apply = |y: f32| -> [f32; 3] {
            std::array::from_fn(|channel| {
                let row = colour.rows[channel];
                row[0] * y + row[1] * 0.501_960_8 + row[2] * 0.501_960_8 + row[3]
            })
        };
        let black = apply(0.0);
        let white = apply(1.0);
        for channel in 0..3 {
            assert!(black[channel].abs() < 0.005, "black came out {black:?}");
            assert!(
                (white[channel] - 1.0).abs() < 0.005,
                "white came out {white:?}"
            );
        }
    }

    /// The coefficients every one of these standards derives the same way,
    /// against the numbers the standard itself prints.
    #[test]
    fn the_coefficients_are_the_ones_the_standard_names() {
        let colour = Colour::of(0.2126, 0.0722, true);
        // Red from the red difference, and blue from the blue.
        assert!((colour.rows[0][2] - 1.5748).abs() < 0.0005);
        assert!((colour.rows[2][1] - 1.8556).abs() < 0.0005);
        // And the two that pull green back.
        assert!((colour.rows[1][1] + 0.1873).abs() < 0.0005);
        assert!((colour.rows[1][2] + 0.4681).abs() < 0.0005);
    }

    #[test]
    fn only_the_two_arrangements_the_shader_reads_are_drawn() {
        assert!(draws(Pixel::YUV420P));
        assert!(draws(Pixel::YUVJ420P));
        assert!(draws(Pixel::NV12));
        assert!(!draws(Pixel::YUV420P10LE), "ten bits is converted first");
        assert!(!draws(Pixel::RGB24));
        assert!(!draws(Pixel::VAAPI), "a handle is not a picture");
    }

    #[test]
    fn a_scissor_outside_the_screen_is_no_draw_at_all() {
        assert!(whole_pixels([-40.0, 0.0, 20.0, 20.0], [800.0, 600.0]).is_none());
        assert!(whole_pixels([0.0, 0.0, 0.0, 20.0], [800.0, 600.0]).is_none());
        assert_eq!(
            whole_pixels([10.4, 10.6, 100.0, 100.0], [800.0, 600.0]),
            Some([10, 10, 101, 101])
        );
    }

    #[test]
    fn the_uniform_is_the_size_the_shader_says() {
        // Three vec2s and two floats, three vec4s of colour, one of `about`,
        // and then the holes twice over. Every one of those is on a sixteen,
        // which is the alignment this is really checking: a scalar left
        // between the colour and the arrays would push both off their stride
        // and the holes would be read as somebody else's numbers.
        assert_eq!(std::mem::size_of::<Placed>(), 32 + 48 + 16 + 32 * HOLES);
        assert_eq!(std::mem::size_of::<Placed>() % 16, 0);
    }

    #[test]
    fn a_hole_with_no_room_in_it_is_not_a_hole() {
        let (rects, corners, count) = Placed::cut(&[
            Hole {
                rect: [10.0, 20.0, 100.0, 40.0],
                radius: 8.0,
            },
            // A panel that has not opened yet, which every panel is for a
            // frame. Counted as a hole it would punch a point out of the film.
            Hole {
                rect: [0.0, 0.0, 0.0, 0.0],
                radius: 8.0,
            },
        ]);
        assert_eq!(count, 1.0);
        assert_eq!(rects[0], [10.0, 20.0, 100.0, 40.0]);
        assert_eq!(corners[0][0], 8.0);
    }

    #[test]
    fn more_holes_than_the_shader_holds_are_dropped_rather_than_written_past() {
        let many: Vec<Hole> = (0..HOLES + 4)
            .map(|index| Hole {
                rect: [index as f32, 0.0, 10.0, 10.0],
                radius: 1.0,
            })
            .collect();
        let (rects, _, count) = Placed::cut(&many);
        assert_eq!(count, HOLES as f32);
        assert_eq!(rects[HOLES - 1][0], (HOLES - 1) as f32);
    }
}
