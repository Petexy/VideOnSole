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
    /// How much of the film the panels standing over it show through — the
    /// same [`crate::view::View::on_black`] the panels were stained with, so
    /// the picture behind the glass arrives on the frame the accent leaves it.
    ///
    /// Nought while the film is a picture on the shell's page: there a panel
    /// has the wallpaper behind it and refracts it, which is what the
    /// toolkit's own material is for and what this has no business replacing.
    pub behind: f32,
}

/// How long the longer edge of the film laid down for the panels is.
///
/// Small is the point rather than a compromise: a panel shows the film through
/// frosted glass, and the shrinking *is* the frosting — done once, by the
/// sampler, on the way down, instead of by a chain of passes on the way back
/// up. The shell frosts what is behind its own glass from a picture of 256
/// along the longer edge; a panel here is a bar two feet wide with a film
/// under a third of it, so it wants rather more blur than that, not less.
const BEHIND_EDGE: u32 = 96;

/// What a stained panel lets through of the film behind it.
///
/// **Tinted glass absorbs what it transmits.** Everything in this interface is
/// drawn to be read against something deliberately dark, which is what lets a
/// panel's own colour stay thin enough to see through. Let a film through at
/// full strength and the panel stops being a pane and becomes a window: over a
/// bright frame the white clock and the white marks on it go out, and a
/// control nobody can read is worse than a bar of the wrong colour. The
/// shell's own panes over another client's window settled within a hair of
/// this, for the same reason and against the same wallpaper.
const TRANSMITTED: f32 = 0.30;

/// The colour the film is laid down in for the panels.
///
/// Floating point and linear, because what goes in has already had the film's
/// own curve taken off it and what comes out is added straight into a frame
/// that is still in light. Eight bits of a linear channel would band the
/// shadows, and the shadows are most of what is behind a panel.
const BEHIND_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

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
    /// The same film again, small, for the panels standing over it.
    behind: Behind,
}

/// The film laid down at a fraction of its size, for the panels to show.
///
/// **Why there is a second copy at all.** A panel over a playing film is a
/// pane of glass, and what a pane shows is what is behind it out of focus. The
/// film is sharp and the size of the screen, and reading it blurred where it
/// stands would be a ring of taps per pixel of every panel, on a picture that
/// moves twenty-four times a second — which is how a blur shimmers. One pass a
/// frame at [`BEHIND_EDGE`] costs a fraction of a panel's own area and cannot
/// shimmer, because the sampler averages the same footprint every frame.
struct Behind {
    picture: wgpu::TextureView,
    read: wgpu::BindGroup,
    /// What was made, in texels. The shrinking pass is told this so that its
    /// taps cover exactly one of them.
    made: (u32, u32),
}

/// What the shrinking pass is told: what it is making, and the colour the film
/// declares itself to be.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Shrinking {
    made: [f32; 2],
    planes: f32,
    /// A uniform buffer counts in sixteens; the colour after this has to start
    /// on one.
    nothing: f32,
    colour: [[f32; 4]; 3],
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
    /// and — for the pass that puts the film behind the panels, and nothing
    /// else — how much of it they let through.
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
    /// The film laid down small, once a frame, for the panels to show. A pass
    /// of its own because it draws into a texture rather than into the window.
    shrink: wgpu::RenderPipeline,
    /// That small picture put inside the holes, and nowhere else. Laid over a
    /// panel that is already whole, at the fraction of it the stain lets
    /// through — see [`TRANSMITTED`].
    behind: wgpu::RenderPipeline,
    picture_layout: wgpu::BindGroupLayout,
    behind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    buffer: wgpu::Buffer,
    placing: wgpu::BindGroup,
    curtain_buffer: wgpu::Buffer,
    curtain_placing: wgpu::BindGroup,
    behind_buffer: wgpu::Buffer,
    behind_placing: wgpu::BindGroup,
    shrink_buffer: wgpu::Buffer,
    shrinking: wgpu::BindGroup,
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

        // What the shrinking pass is told, and what the panels read back.
        let shrinking_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("the film, small"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let behind_read_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("the film behind a panel"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        // Read in both stages: the quad is grown by how far the
                        // blur reaches, and that is measured off this texture.
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
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
                ],
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

        // The film laid down small. Its own shader module rather than another
        // pair of entry points in the one below, because it is the only pass
        // here that does not draw into the window and the only one whose first
        // bind group is not a placement.
        let shrink_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("the film, small"),
            bind_group_layouts: &[Some(&shrinking_layout), Some(&picture_layout)],
            immediate_size: 0,
        });
        let shrinking_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("the film, small"),
            source: wgpu::ShaderSource::Wgsl(SHRINK_SHADER.into()),
        });
        let shrink = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("the film, small"),
            layout: Some(&shrink_layout),
            vertex: wgpu::VertexState {
                module: &shrinking_shader,
                entry_point: Some("shrink_vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shrinking_shader,
                entry_point: Some("shrink_fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: BEHIND_FORMAT,
                    blend: None,
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

        // And that picture put behind the panels, **over** rather than added.
        //
        // Added was the first answer and it is wrong, in the way this whole
        // material is built not to be: a panel over a bright frame went white,
        // and the count in its far corner went with it. Tinted glass does not
        // add what is behind it to itself — it absorbs, and what comes out is
        // the picture at the fraction the stain lets through, standing in for
        // that much of the glass rather than on top of all of it. Which is
        // also the only form whose panel cannot be brighter than the frame
        // behind it, and so the only one a white clock survives.
        //
        // The colour is blended against the source's alpha; the destination's
        // own alpha is left exactly as it was, because this is a window that
        // is already opaque and nothing here is entitled to open a hole in it.
        let behind_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("the film behind a panel"),
            bind_group_layouts: &[
                Some(&placing_layout),
                Some(&picture_layout),
                Some(&behind_read_layout),
            ],
            immediate_size: 0,
        });
        let behind = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("the film behind a panel"),
            layout: Some(&behind_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("behind_vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("behind_fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
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
        let behind_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("the film behind a panel"),
            size: std::mem::size_of::<Placed>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let behind_placing = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("the film behind a panel"),
            layout: &placing_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: behind_buffer.as_entire_binding(),
            }],
        });
        let shrink_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("the film, small"),
            size: std::mem::size_of::<Shrinking>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shrinking = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("the film, small"),
            layout: &shrinking_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: shrink_buffer.as_entire_binding(),
            }],
        });

        Films {
            pipeline,
            curtain,
            shrink,
            behind,
            picture_layout,
            behind_layout: behind_read_layout,
            sampler,
            buffer,
            placing,
            curtain_buffer,
            curtain_placing,
            behind_buffer,
            behind_placing,
            shrink_buffer,
            shrinking,
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
            behind: self.lay_down_small(device, frame.width(), frame.height()),
        }
    }

    /// Make the texture the film is laid down small into.
    fn lay_down_small(&self, device: &wgpu::Device, width: u32, height: u32) -> Behind {
        let made = small_enough(width, height);
        let picture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("the film, small"),
            size: wgpu::Extent3d {
                width: made.0,
                height: made.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: BEHIND_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let picture = picture.create_view(&Default::default());
        let read = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("the film behind a panel"),
            layout: &self.behind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&picture),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        Behind {
            picture,
            read,
            made,
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
            behind,
        } = showing;
        let placed = placement.filter(|placement| placement.opacity > 0.001);
        let film = placed.zip(self.resident.as_ref());
        let curtain = blackout > 0.001 && over.rect[2] > 0.0 && over.rect[3] > 0.0;
        if film.is_none() && !curtain {
            return;
        }
        let (cut, corners, count) = Placed::cut(holes);
        // Only with a film, only with a panel standing on it, and only once
        // the panels are the ones that stand on black — a film that is a
        // picture on the page has the wallpaper behind its panels and the
        // toolkit's own glass is already showing it.
        let behind = behind.clamp(0.0, 1.0);
        let showing_behind = behind > 0.001 && count > 0.0 && film.is_some();

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
        if let Some((placement, resident)) = film.filter(|_| showing_behind) {
            queue.write_buffer(
                &self.shrink_buffer,
                0,
                bytemuck::bytes_of(&Shrinking {
                    made: [resident.behind.made.0 as f32, resident.behind.made.1 as f32],
                    planes: planes_of(resident.layout) as f32,
                    nothing: 0.0,
                    colour: resident.colour.rows,
                }),
            );
            // The same quad the film is drawn as, so the panels read the
            // picture off exactly the rectangle the film is standing in — the
            // holes are measured on the window and the two cannot be worked
            // out separately without coming apart as a film is opened.
            queue.write_buffer(
                &self.behind_buffer,
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
                        behind * TRANSMITTED,
                    ],
                    holes: cut,
                    corners,
                }),
            );

            // Laid down before the window's own pass begins, because a texture
            // cannot be drawn into and read from in one pass.
            let mut small = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("the film, small"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &resident.behind.picture,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            small.set_pipeline(&self.shrink);
            small.set_bind_group(0, &self.shrinking, &[]);
            small.set_bind_group(1, &resident.bind, &[]);
            small.draw(0..6, 0..1);
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

        // And the film behind the panels last of all: it is the one thing here
        // drawn *into* the holes rather than around them, so it goes down over
        // a panel that is already whole.
        if showing_behind {
            pass.set_pipeline(&self.behind);
            pass.set_bind_group(0, &self.behind_placing, &[]);
            pass.set_bind_group(2, &resident.behind.read, &[]);
            pass.draw(0..6, 0..1);
        }
    }
}

/// How big the film is laid down for the panels standing over it.
///
/// **In the film's own proportion**, not a square: a texel of it has to come
/// out the same size on the screen across as it is down, or a portrait film
/// behind a panel is blurred further one way than the other and the smear
/// reads as a picture somebody has stretched.
///
/// Never nought in either direction, because a texture of no width is not a
/// texture — a film one pixel tall and four thousand wide is a thing that
/// exists, and it is not worth a crash.
fn small_enough(width: u32, height: u32) -> (u32, u32) {
    let longer = width.max(height).max(1);
    let shrunk = |edge: u32| (edge * BEHIND_EDGE).div_ceil(longer).clamp(1, BEHIND_EDGE);
    (shrunk(width), shrunk(height))
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
// The same film laid down small, in light, for the panels standing over it.
// A group of its own rather than a fifth plane, because the pass that makes it
// binds the planes while drawing into this one.
@group(2) @binding(0) var behind_picture: texture_2d<f32>;
@group(2) @binding(1) var behind_sampling: sampler;

struct Drawn {
    @builtin(position) at: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Where this is on the window, which is what the holes are measured in.
    @location(1) pixel: vec2<f32>,
};

fn corner_of(index: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    );
    return corners[index];
}

fn place(index: u32) -> Drawn {
    let corner = corner_of(index);
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

/// How far the blur reaches past the pixel it belongs to, in window pixels.
///
/// The tent below is one texel of the small picture either side, and the
/// sampler's own magnification carries it about half a texel further. It is
/// worked out from what is bound rather than passed in, so that changing
/// `BEHIND_EDGE` cannot leave the soft edge of the picture measured for the
/// old one.
fn behind_reach() -> f32 {
    let made = max(vec2<f32>(textureDimensions(behind_picture)), vec2<f32>(1.0));
    let texel = placed.half * 2.0 / made;
    return max(texel.x, texel.y) * 1.5;
}

/// The film's own quad, grown by that reach.
///
/// **Grown, because the letterbox is part of the picture.** Where the film
/// stops, a panel goes back to being plain stained glass, and a pane of glass
/// does not show one half of itself out of focus and put a razor down the
/// middle. The edge has to dissolve over the same width as everything else in
/// the picture does, which means the fade straddles it — half of it outside
/// the film, on ground the film's own quad never covers.
@vertex
fn behind_vs(@builtin(vertex_index) index: u32) -> Drawn {
    let corner = corner_of(index);
    let pixel = placed.centre + corner * (placed.half + vec2<f32>(behind_reach()));

    var out: Drawn;
    out.at = vec4<f32>(
        pixel.x / placed.screen.x * 2.0 - 1.0,
        1.0 - pixel.y / placed.screen.y * 2.0,
        0.0,
        1.0,
    );
    // The film's own nought to one, which runs a little past both ends over
    // the ground the quad was grown on to. The sampler holds its edge there,
    // and the fade has taken the picture to nothing by the time it matters.
    out.uv = (pixel - placed.centre + placed.half) / max(placed.half * 2.0, vec2<f32>(1.0));
    out.pixel = pixel;
    return out;
}

/// The film behind the panels standing on it.
///
/// The same quad as the film, turned inside out: where the film leaves a hole
/// this fills one, and everywhere else it does nothing at all. What comes out
/// is premultiplied — the picture at the fraction the stain lets through, and
/// that fraction as the alpha the panel underneath is taken back by. See
/// `TRANSMITTED` for why it is that way round and not added.
@fragment
fn behind_fs(drawn: Drawn) -> @location(0) vec4<f32> {
    // Inside a panel, and inside the film: what is behind a panel out over the
    // letterbox is the letterbox, and nothing is added there. The film's edge
    // is feathered over the width of the blur rather than the width of a
    // pixel — see `behind_vs`.
    let reach = behind_reach();
    let edge = rounded(drawn.pixel - placed.centre, placed.half, placed.about.z);
    let shown = (1.0 - smoothstep(-reach, reach, edge)) * covered(drawn.pixel);
    if (shown <= 0.0) {
        discard;
    }
    // Nine taps in a tent on a picture already several times smaller than the
    // screen. The sampler's own magnification is most of the blur; the tent is
    // what keeps the creases of a magnified texel out of a panel a yard wide.
    // `textureSampleLevel` rather than `textureSample`, because the `discard`
    // above means this is not reached by every pixel of the quad and a tap
    // that needed neighbouring pixels to know its own size would be undefined.
    let step = 1.0 / vec2<f32>(textureDimensions(behind_picture));
    var light = vec3<f32>(0.0);
    var total = 0.0;
    for (var down = -1; down <= 1; down = down + 1) {
        for (var across = -1; across <= 1; across = across + 1) {
            let weight = (2.0 - abs(f32(across))) * (2.0 - abs(f32(down)));
            let at = drawn.uv + vec2<f32>(f32(across), f32(down)) * step;
            light = light + textureSampleLevel(
                behind_picture, behind_sampling, at, 0.0).rgb * weight;
            total = total + weight;
        }
    }
    // How much of it the panel lets through, how much of the film is there at
    // all, and how much light is left in the page under an open menu.
    let through = placed.about.w * placed.opacity * placed.about.x * shown;
    return vec4<f32>(light / total * through, through);
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

/// Laying the film down small.
///
/// A module of its own: it is the only pass here that draws into a texture
/// rather than into the window, and the only one whose first bind group is
/// what it is making rather than where the film goes.
const SHRINK_SHADER: &str = r#"
struct Shrinking {
    // How many texels across and down the picture being made is.
    made: vec2<f32>,
    planes: f32,
    nothing: f32,
    colour0: vec4<f32>,
    colour1: vec4<f32>,
    colour2: vec4<f32>,
};

@group(0) @binding(0) var<uniform> shrinking: Shrinking;
@group(1) @binding(0) var luma: texture_2d<f32>;
@group(1) @binding(1) var blue: texture_2d<f32>;
@group(1) @binding(2) var red: texture_2d<f32>;
@group(1) @binding(3) var sampling: sampler;

struct Made {
    @builtin(position) at: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn shrink_vs(@builtin(vertex_index) index: u32) -> Made {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[index];
    var out: Made;
    out.at = vec4<f32>(corner, 0.0, 1.0);
    out.uv = vec2<f32>(corner.x * 0.5 + 0.5, corner.y * -0.5 + 0.5);
    return out;
}

// The same curve the film's own pass takes off, for the same reason: what is
// stored is not light, and a picture averaged before the curve came off would
// be a picture averaged in the wrong space.
fn to_light(value: f32) -> f32 {
    if (value <= 0.04045) {
        return value / 12.92;
    }
    return pow((value + 0.055) / 1.055, 2.4);
}

/// Sixteen taps covering exactly one texel of what is being made.
///
/// **Exactly one, not a radius somebody liked.** The sampler averages two
/// texels by two of the film around each tap, so sixteen taps spread over one
/// finished texel really do read the whole of the footprint that texel stands
/// for. Fewer, or narrower, and what is left out comes back as a picture that
/// crawls while the film runs: a blur made of samples that miss half of what
/// they are averaging is a different blur every frame.
@fragment
fn shrink_fs(made: Made) -> @location(0) vec4<f32> {
    let step = 1.0 / shrinking.made;
    var stored = vec3<f32>(0.0);
    for (var down = 0; down < 4; down = down + 1) {
        for (var across = 0; across < 4; across = across + 1) {
            let offset = (vec2<f32>(f32(across), f32(down)) + 0.5) * 0.25 - 0.5;
            let at = made.uv + offset * step;
            let y = textureSampleLevel(luma, sampling, at, 0.0).r;
            var u: f32;
            var v: f32;
            if (shrinking.planes < 2.5) {
                let both = textureSampleLevel(blue, sampling, at, 0.0).rg;
                u = both.r;
                v = both.g;
            } else {
                u = textureSampleLevel(blue, sampling, at, 0.0).r;
                v = textureSampleLevel(red, sampling, at, 0.0).r;
            }
            stored = stored + vec3<f32>(y, u, v);
        }
    }
    // Averaged as stored and converted once. The conversion is a matrix, so
    // the two orders agree to the bit — and one of them is sixteen times the
    // arithmetic.
    stored = stored * (1.0 / 16.0);
    let signal = vec3<f32>(
        dot(shrinking.colour0.xyz, stored) + shrinking.colour0.w,
        dot(shrinking.colour1.xyz, stored) + shrinking.colour1.w,
        dot(shrinking.colour2.xyz, stored) + shrinking.colour2.w,
    );
    let held = clamp(signal, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(to_light(held.r), to_light(held.g), to_light(held.b), 1.0);
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

    /// The picture the panels read is the film's own shape, small. A square
    /// one would blur a portrait film further down the screen than across it,
    /// and the smear behind the transport would read as a stretched picture.
    #[test]
    fn the_film_is_laid_down_small_in_its_own_proportion() {
        for (width, height) in [(1920, 1080), (1080, 1920), (640, 480), (460, 816)] {
            let (across, down) = small_enough(width, height);
            assert_eq!(
                across.max(down),
                BEHIND_EDGE,
                "{width}x{height} is not small"
            );
            let wanted = width as f32 / height as f32;
            let got = across as f32 / down as f32;
            assert!(
                (got / wanted - 1.0).abs() < 0.02,
                "{width}x{height} came out {across}x{down}, which is not its shape"
            );
        }
    }

    /// A film with an edge far shorter than a hundredth of its other one is a
    /// thing that exists — a strip of titles, a picture a decoder got wrong —
    /// and a texture of no width is not a texture.
    #[test]
    fn no_film_is_laid_down_at_nothing_across() {
        assert_eq!(small_enough(4000, 1), (BEHIND_EDGE, 1));
        assert_eq!(small_enough(1, 4000), (1, BEHIND_EDGE));
        assert_eq!(small_enough(0, 0), (1, 1));
        // And nothing is ever laid down larger than it was asked to be.
        let (across, down) = small_enough(64, 48);
        assert!(across <= BEHIND_EDGE && down <= BEHIND_EDGE);
    }

    #[test]
    fn the_shrinking_uniform_is_the_size_the_shader_says() {
        // Two floats of size and two more to see the colour on to a sixteen,
        // then three rows of it. A scalar out of place here would be a film
        // laid down small in somebody else's colours.
        assert_eq!(std::mem::size_of::<Shrinking>(), 16 + 48);
        assert_eq!(std::mem::size_of::<Shrinking>() % 16, 0);
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
