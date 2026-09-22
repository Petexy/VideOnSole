//! A made-up folder of films, for pictures of this application.
//!
//! `--demo` exists for two reasons: to photograph the interface — every
//! picture in the README was taken with it — and to let somebody look at the
//! program on a machine with no films on it at all.
//!
//! **Nothing of the user's is read, and nothing of theirs is written.** The
//! folder is generated into this application's own cache and opened from
//! there, which means every page above it is the ordinary code path: the same
//! directory walk, the same demuxer, the same decoder, the same poster frame
//! written into the same shared thumbnail cache. A player photographed against
//! a special case would be a picture of the special case rather than of the
//! player.
//!
//! The films are drawn rather than filmed — there is no film to ship and none
//! of anybody else's to borrow — so each one is a scene made out of gradients
//! and noise, panned across. They are deterministic: the same build writes the
//! same films every time, so two shots taken a week apart differ by what
//! changed in the program and by nothing else.
//!
//! **What is encoded.** H.264 where this machine can write it and MPEG-4 part
//! 2 where it cannot — see `encoder_to_use`, which says why it is that way
//! round. The frames are made BT.709 by hand and the stream is told so, so the
//! colour is right whether a player reads the tag or falls back to the
//! standard for the size. See `Colour::of_frame` in `film.rs` for the other
//! end of that.

use std::path::{Path, PathBuf};

use ffmpeg_next as ffmpeg;

/// What the folder is called, and so what the head of the page reads.
///
/// It says what it is. A made-up folder wearing a real place's name is the one
/// screenshot somebody would go looking for the films from.
const FOLDER: &str = "Preview";

/// Bumped whenever a scene or a film changes, which is what makes a stale
/// cache regenerate rather than being shown for ever.
const GENERATION: u32 = 4;

/// What every film in the folder is. One size and one rate, because a folder
/// of films is a folder of films and not a test of the decoder.
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const RATE: i32 = 24;

/// How much wider the still is than the frame. The film is a pan across it, so
/// this is the whole of the movement — and a pan is honest motion for a
/// drawing, where a moving subject would only be a drawing that jitters.
const PAN: f32 = 1.55;

/// The made-up folder, generated if this machine has not got it already.
pub fn folder() -> Result<PathBuf, String> {
    let root = cache_root()?.join("preview");
    let folder = root.join(FOLDER);
    let stamp = root.join(format!(".generation-{GENERATION}"));
    if stamp.exists() && folder.is_dir() {
        return Ok(folder);
    }

    // A generation that is no longer wanted is removed outright rather than
    // written over: a renamed film left behind from the build before would
    // appear in the listing and in the count, and be the one thing on the page
    // nothing in this file explains.
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&folder).map_err(|err| format!("{}: {err}", folder.display()))?;

    // The library, and its log turned down to errors — x264 writes a paragraph
    // of statistics per film at the default level, and this application's
    // stderr is where somebody looks for the reason a controller does nothing.
    crate::player::start_ffmpeg();

    // Said once, because drawing and encoding eighty seconds of film takes long
    // enough that a silent minute reads as a program that has hung.
    eprintln!("videonsole: drawing the preview films, once…");
    for film in FILMS {
        let at = match film.within {
            Some(inner) => {
                let inner = folder.join(inner);
                std::fs::create_dir_all(&inner)
                    .map_err(|err| format!("{}: {err}", inner.display()))?;
                inner
            }
            None => folder.clone(),
        };
        write(&at.join(film.name), film)?;
        if let Some(subtitle) = film.subtitle {
            let beside = at.join(format!(
                "{}.en.srt",
                Path::new(film.name)
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or(film.name)
            ));
            std::fs::write(beside, subtitle).map_err(|err| format!("{err}"))?;
        }
    }

    std::fs::write(&stamp, b"").map_err(|err| format!("{}: {err}", stamp.display()))?;
    Ok(folder)
}

fn cache_root() -> Result<PathBuf, String> {
    if let Some(cache) = std::env::var_os("XDG_CACHE_HOME").filter(|it| !it.is_empty()) {
        return Ok(PathBuf::from(cache).join("videonsole"));
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| String::from("--demo needs somewhere to write, and HOME is not set"))?;
    Ok(PathBuf::from(home).join(".cache").join("videonsole"))
}

/// One made-up film: what it is called, how long it runs, and what is in it.
struct Film {
    name: &'static str,
    /// The subfolder it lives in, so that the listing has a way further in and
    /// the count on the page has a folder to count.
    within: Option<&'static str>,
    seconds: f32,
    scene: Scene,
    seed: u32,
    /// Written beside the film as a `.en.srt`, where there is one. A film with
    /// a file beside it is the case the Options menu's subtitle list exists
    /// for, and the one that shows a track being picked up without being asked
    /// for.
    subtitle: Option<&'static str>,
}

/// The four kinds of scene, each with the light it is lit by.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scene {
    Water(Light),
    Ridges(Light),
    City,
    Dunes,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Light {
    Dawn,
    Midday,
    Dusk,
    Cold,
}

const FILMS: &[Film] = &[
    Film {
        name: "Harbour at dawn.mp4",
        within: None,
        seconds: 12.0,
        scene: Scene::Water(Light::Dawn),
        seed: 11,
        subtitle: None,
    },
    Film {
        name: "The long ridge.mkv",
        within: None,
        seconds: 9.0,
        scene: Scene::Ridges(Light::Dusk),
        seed: 23,
        subtitle: None,
    },
    Film {
        name: "City, late.mp4",
        within: None,
        seconds: 8.0,
        scene: Scene::City,
        seed: 41,
        subtitle: None,
    },
    Film {
        name: "Dunes at six.mp4",
        within: None,
        seconds: 10.0,
        scene: Scene::Dunes,
        seed: 67,
        subtitle: None,
    },
    // The pair that says which order a listing is read in: `S01E09` before
    // `S01E10`, which plain byte order gets exactly backwards.
    Film {
        name: "Crossing S01E09.mp4",
        within: None,
        seconds: 7.0,
        scene: Scene::Water(Light::Midday),
        seed: 53,
        subtitle: Some(
            "1\n00:00:01,000 --> 00:00:04,000\nThe tide turns at six.\n\n\
             2\n00:00:04,500 --> 00:00:07,000\nWe should be gone by then.\n",
        ),
    },
    Film {
        name: "Crossing S01E10.mp4",
        within: None,
        seconds: 11.0,
        scene: Scene::Ridges(Light::Cold),
        seed: 37,
        subtitle: None,
    },
    Film {
        name: "Black sand.mp4",
        within: Some("Iceland"),
        seconds: 8.0,
        scene: Scene::Water(Light::Cold),
        seed: 97,
        subtitle: None,
    },
    Film {
        name: "Midnight.mkv",
        within: Some("Iceland"),
        seconds: 9.0,
        scene: Scene::Water(Light::Dusk),
        seed: 113,
        subtitle: None,
    },
];

// ------------------------------------------------------------- the encoding

fn write(path: &Path, film: &Film) -> Result<(), String> {
    let wide = (WIDTH as f32 * PAN) as u32 & !1;
    let still = still(film, wide, HEIGHT);
    let frames = (film.seconds * RATE as f32).round() as i64;

    let mut out = ffmpeg::format::output(&path).map_err(|err| say(path, err))?;
    let codec = encoder_to_use()?;

    let global = out
        .format()
        .flags()
        .contains(ffmpeg::format::Flags::GLOBAL_HEADER);
    let mut stream = out.add_stream(codec).map_err(|err| say(path, err))?;
    let index = stream.index();
    stream.set_time_base(ffmpeg::Rational(1, RATE));

    let mut encoder = ffmpeg::codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .map_err(|err| say(path, err))?;
    encoder.set_width(WIDTH);
    encoder.set_height(HEIGHT);
    encoder.set_format(ffmpeg::format::Pixel::YUV420P);
    encoder.set_time_base(ffmpeg::Rational(1, RATE));
    encoder.set_frame_rate(Some(ffmpeg::Rational(RATE, 1)));
    encoder.set_bit_rate(3_000_000);
    encoder.set_gop(24);
    encoder.set_max_b_frames(0);
    // Said, and carried by H.264 though MPEG-4 part 2 has nowhere to put it:
    // the frames below are made with the BT.709 matrix, and at 720 lines every
    // player's fallback for an untagged film is BT.709 as well. So it is right
    // either way about, and right for the reason rather than by luck.
    encoder.set_colorspace(ffmpeg::color::Space::BT709);
    encoder.set_color_range(ffmpeg::color::Range::MPEG);
    if global {
        encoder.set_flags(ffmpeg::codec::Flags::GLOBAL_HEADER);
    }
    let mut encoder = encoder.open_as(codec).map_err(|err| say(path, err))?;
    stream.set_parameters(&encoder);
    out.write_header().map_err(|err| say(path, err))?;

    // *After* the header, and this is the whole of why a film came out saying
    // it was twenty milliseconds long. A muxer picks its own timescale — mp4
    // wants 1/12288 — and writes it over whatever the stream was given, during
    // `write_header`. Rescaling into the base the stream had *before* that is
    // rescaling into a base nothing will ever read the film back with.
    let stream_base = out
        .stream(index)
        .ok_or_else(|| format!("{}: the stream went away", path.display()))?
        .time_base();

    let mut frame = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::YUV420P, WIDTH, HEIGHT);
    for number in 0..frames {
        // A pan that eases in and out at its ends. A pan at a constant rate
        // starts and stops on a cut, which is the one thing a locked-off
        // camera never does.
        let along = if frames > 1 {
            number as f32 / (frames - 1) as f32
        } else {
            0.0
        };
        let from = ((wide - WIDTH) as f32 * smoothstep(along)).round() as u32;
        fill(&mut frame, &still, wide, from);
        frame.set_pts(Some(number));
        encoder.send_frame(&frame).map_err(|err| say(path, err))?;
        drain(&mut encoder, &mut out, index, stream_base)?;
    }
    encoder.send_eof().map_err(|err| say(path, err))?;
    drain(&mut encoder, &mut out, index, stream_base)?;
    out.write_trailer().map_err(|err| say(path, err))?;
    Ok(())
}

/// H.264 where this machine can write it, and MPEG-4 part 2 where it cannot.
///
/// H.264 first because it is what a film somebody owns actually is: it is the
/// one the graphics card decodes, so a demo film exercises the hardware path
/// and the details pane says something true about this machine rather than
/// *software* every time. MPEG-4 part 2 behind it because it is in every build
/// of ffmpeg there has ever been, and a machine whose ffmpeg was built without
/// libx264 still has to be able to draw the folder — a demo that cannot be
/// generated is worse than no demo.
fn encoder_to_use() -> Result<ffmpeg::Codec, String> {
    ffmpeg::encoder::find_by_name("libx264")
        .or_else(|| ffmpeg::encoder::find(ffmpeg::codec::Id::H264))
        .or_else(|| ffmpeg::encoder::find(ffmpeg::codec::Id::MPEG4))
        .ok_or_else(|| String::from("this ffmpeg can write no video at all, so --demo cannot draw"))
}

fn drain(
    encoder: &mut ffmpeg::encoder::Video,
    out: &mut ffmpeg::format::context::Output,
    index: usize,
    base: ffmpeg::Rational,
) -> Result<(), String> {
    let mut packet = ffmpeg::Packet::empty();
    while encoder.receive_packet(&mut packet).is_ok() {
        packet.set_stream(index);
        packet.rescale_ts(ffmpeg::Rational(1, RATE), base);
        packet
            .write_interleaved(out)
            .map_err(|err| format!("{err}"))?;
    }
    Ok(())
}

fn say(path: &Path, err: ffmpeg::Error) -> String {
    format!("{}: {err}", path.display())
}

/// One frame: a window on to the wide still, in the planes a codec wants.
///
/// The conversion is done here rather than through a scaler because there is
/// nothing to scale — the crop is already the size of the frame — and because
/// doing it here is what makes the matrix this application's own choice rather
/// than whichever one `swscale` would have picked for the size.
fn fill(frame: &mut ffmpeg::frame::Video, still: &[u8], wide: u32, from: u32) {
    let at = |x: u32, y: u32| {
        let i = ((y * wide + from + x) * 3) as usize;
        (
            still[i] as f32 / 255.0,
            still[i + 1] as f32 / 255.0,
            still[i + 2] as f32 / 255.0,
        )
    };
    // BT.709, studio range: the coefficients `film.rs` reads back out.
    let luma = |r: f32, g: f32, b: f32| 0.2126 * r + 0.7152 * g + 0.0722 * b;

    let stride = frame.stride(0);
    let plane = frame.data_mut(0);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let (r, g, b) = at(x, y);
            let value = 16.0 + 219.0 * luma(r, g, b);
            plane[y as usize * stride + x as usize] = value.clamp(0.0, 255.0) as u8;
        }
    }

    // Chroma, at half in both directions — so each sample is the average of
    // the four pixels it stands for rather than one of them picked out.
    for (which, coefficient) in [(1usize, 1.8556f32), (2, 1.5748)] {
        let stride = frame.stride(which);
        let plane = frame.data_mut(which);
        for y in 0..HEIGHT / 2 {
            for x in 0..WIDTH / 2 {
                let mut total = 0.0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (r, g, b) = at(x * 2 + dx, y * 2 + dy);
                    let y_of = luma(r, g, b);
                    total += (if which == 1 { b } else { r } - y_of) / coefficient;
                }
                let value = 128.0 + 224.0 * (total / 4.0);
                plane[y as usize * stride + x as usize] = value.clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// The whole scene, once, wider than the frame — the film is a window sliding
/// across this. Drawing it once rather than per frame is the difference
/// between a folder that takes a minute to make and one that takes twenty.
fn still(film: &Film, width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width as f32, height as f32);
    let mut pixels = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        let v = (y as f32 + 0.5) / h;
        for x in 0..width {
            let u = (x as f32 + 0.5) / w;
            let mut colour = match film.scene {
                Scene::Water(light) => water(u, v, light, film.seed),
                Scene::Ridges(light) => ridges(u, v, light, film.seed),
                Scene::City => city(u, v, film.seed),
                Scene::Dunes => dunes(u, v, film.seed),
            };
            // A lens is darker at its edges than at its middle. It is put on
            // the still rather than on each frame, because a vignette that
            // slid about with the pan would be a lens moving inside the camera.
            let corner = ((u - 0.5) * 1.20).powi(2) + ((v - 0.5) * 1.35).powi(2);
            let vignette = 1.0 - 0.30 * corner;
            let grain = (value(x as i32, y as i32, film.seed ^ 0x9e37) - 0.5) * 0.010;
            for channel in &mut colour {
                *channel = (*channel * vignette + grain).clamp(0.0, 1.0);
            }
            pixels.extend_from_slice(&[byte(colour[0]), byte(colour[1]), byte(colour[2])]);
        }
    }
    pixels
}

fn byte(value: f32) -> u8 {
    (value * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

// ---------------------------------------------------------------- the scenes

/// The three colours a sky is made of, and the sun that is in it.
struct Sky {
    top: [f32; 3],
    middle: [f32; 3],
    horizon: [f32; 3],
    sun: [f32; 3],
    /// Where the sun stands, across and down. Below the horizon is a sun that
    /// has set, which is what leaves the glow without the disc.
    sun_at: [f32; 2],
    /// How large the disc is, as a fraction of the width.
    sun_size: f32,
    deep: [f32; 3],
}

fn sky_of(light: Light) -> Sky {
    match light {
        Light::Dawn => Sky {
            top: [0.07, 0.11, 0.26],
            middle: [0.36, 0.26, 0.42],
            horizon: [0.96, 0.66, 0.38],
            sun: [1.0, 0.93, 0.74],
            sun_at: [0.66, 0.50],
            sun_size: 0.030,
            deep: [0.05, 0.08, 0.15],
        },
        Light::Midday => Sky {
            top: [0.16, 0.38, 0.68],
            middle: [0.40, 0.62, 0.82],
            horizon: [0.76, 0.85, 0.90],
            sun: [1.0, 0.99, 0.94],
            sun_at: [0.28, 0.16],
            sun_size: 0.018,
            deep: [0.10, 0.27, 0.36],
        },
        Light::Dusk => Sky {
            top: [0.04, 0.05, 0.14],
            middle: [0.17, 0.12, 0.29],
            horizon: [0.55, 0.26, 0.31],
            sun: [1.0, 0.78, 0.55],
            sun_at: [0.38, 0.575],
            sun_size: 0.026,
            deep: [0.03, 0.04, 0.09],
        },
        Light::Cold => Sky {
            top: [0.20, 0.30, 0.44],
            middle: [0.44, 0.55, 0.66],
            horizon: [0.72, 0.78, 0.83],
            sun: [0.96, 0.96, 0.98],
            sun_at: [0.74, 0.30],
            sun_size: 0.016,
            deep: [0.09, 0.13, 0.18],
        },
    }
}

/// The sky alone, above a horizon at `horizon`.
fn sky_at(u: f32, v: f32, horizon: f32, sky: &Sky, seed: u32, aspect: f32) -> [f32; 3] {
    let t = (v / horizon).clamp(0.0, 1.0);
    let mut colour = if t < 0.55 {
        mix(sky.top, sky.middle, smoothstep(t / 0.55))
    } else {
        mix(sky.middle, sky.horizon, smoothstep((t - 0.55) / 0.45))
    };

    // Cloud, as bands that are stretched flat near the horizon — which is what
    // perspective does to anything lying in a layer.
    let stretch = 1.0 + 6.0 * t * t;
    let cloud = fbm(u * 2.6, v * 5.0 * stretch, seed ^ 0x51ed, 4);
    let amount = 0.30 * smoothstep(((cloud - 0.46) / 0.30).clamp(0.0, 1.0)) * (0.25 + 0.75 * t);
    colour = mix(colour, lighten(sky.horizon, 0.25), amount);

    // The sun: a disc, and a glow that reaches much further than the disc
    // does. The glow is what says which way the light is coming from in every
    // other part of the picture, so it is added rather than mixed.
    let dx = (u - sky.sun_at[0]) * aspect;
    let dy = v - sky.sun_at[1];
    let distance = (dx * dx + dy * dy).sqrt();
    let glow = (-distance / 0.16).exp() * 0.9;
    colour = add(colour, scale(sky.sun, glow * 0.55));
    if distance < sky.sun_size && sky.sun_at[1] < horizon {
        let edge = smoothstep(((sky.sun_size - distance) / (sky.sun_size * 0.35)).clamp(0.0, 1.0));
        colour = mix(colour, sky.sun, edge);
    }
    colour
}

fn water(u: f32, v: f32, light: Light, seed: u32) -> [f32; 3] {
    const HORIZON: f32 = 0.56;
    let sky = sky_of(light);
    if v < HORIZON {
        return sky_at(u, v, HORIZON, &sky, seed, 1.5);
    }

    // Below the horizon the sky is what is being reflected, so the water
    // starts as the sky's own colours and darkens with distance from the eye.
    let down = ((v - HORIZON) / (1.0 - HORIZON)).clamp(0.0, 1.0);
    let mut colour = mix(darken(sky.horizon, 0.35), sky.deep, smoothstep(down));

    // Ripples: bands that grow further apart towards the bottom of the frame,
    // because the near water is closer to the eye than the far water is.
    let spacing = 26.0 + 150.0 * down * down;
    let wobble = fbm(u * 3.0, v * 9.0, seed ^ 0x2f1b, 3) * 5.5;
    let band = ((v * spacing + wobble).sin() * 0.5 + 0.5).powf(2.2);
    colour = add(
        colour,
        scale(lighten(sky.horizon, 0.2), band * 0.16 * (0.25 + down)),
    );

    // And the column under the sun, which widens as it comes towards the eye
    // and is the brightest thing in the picture.
    let width = 0.02 + 0.34 * down;
    let across = ((u - sky.sun_at[0]) / width).abs();
    if across < 1.0 {
        let strength = (1.0 - across).powi(2) * (1.0 - 0.45 * down);
        colour = add(colour, scale(sky.sun, strength * band * 0.85));
    }
    colour
}

fn ridges(u: f32, v: f32, light: Light, seed: u32) -> [f32; 3] {
    const HORIZON: f32 = 0.72;
    let sky = sky_of(light);
    let mut colour = sky_at(u, v, HORIZON, &sky, seed, 1.5);

    // Painter's order, far to near: each layer stands lower on the frame, is
    // rougher than the one behind it, and has less of the sky's haze mixed
    // into it. That mixture is the whole of the depth here.
    let rock = match light {
        Light::Cold => [0.21, 0.24, 0.31],
        Light::Midday => [0.24, 0.31, 0.26],
        _ => [0.16, 0.13, 0.20],
    };
    for layer in 0..4 {
        let far = 1.0 - layer as f32 / 3.0;
        let base = 0.40 + 0.16 * layer as f32;
        let amplitude = 0.05 + 0.055 * layer as f32;
        let frequency = 1.6 + 2.2 * layer as f32;
        let height =
            base - amplitude * (ridge(u * frequency, seed.wrapping_add(layer as u32 * 17)) - 0.45);
        if v < height {
            continue;
        }
        let mut here = mix(rock, sky.horizon, far * 0.62);
        here = darken(here, (1.0 - far) * 0.35);

        // Snow lies along the tops, and lies deeper the higher the top is —
        // so it is the shape somebody can see that puts it there, rather than
        // a second noise they have no way to tie to the ridge.
        if light == Light::Cold {
            let below = ((v - height) / 0.055).clamp(0.0, 1.0);
            let high = smoothstep(((0.58 - height) / 0.16).clamp(0.0, 1.0));
            let cap = smoothstep((1.0 - below) * high);
            let snow = mix([0.93, 0.95, 0.98], sky.horizon, far * 0.55);
            here = mix(here, snow, cap * 0.95);
        }
        colour = here;
    }
    colour
}

fn city(u: f32, v: f32, seed: u32) -> [f32; 3] {
    // A night sky, and the orange the ground throws back up into it — which is
    // why a city never has a black sky over it.
    let mut colour = mix([0.03, 0.04, 0.10], [0.16, 0.11, 0.16], smoothstep(v / 0.78));
    colour = add(
        colour,
        scale(
            [0.55, 0.32, 0.16],
            (v / 0.78).clamp(0.0, 1.0).powi(3) * 0.55,
        ),
    );

    // Stars, in the top of the frame only, where the glow has not washed them
    // out. One pixel each, so they survive being scaled down to a card.
    if v < 0.42 {
        let star = value((u * 900.0) as i32, (v * 600.0) as i32, seed ^ 0x77af);
        if star > 0.9992 {
            colour = add(colour, [0.7, 0.7, 0.8]);
        }
    }

    // Two ranks of buildings. The far rank is hazier and lower; the near rank
    // is nearly black, which is what an eye adjusted to a lit window sees.
    for rank in 0..2 {
        let near = rank == 1;
        let skyline = if near { 0.56 } else { 0.44 };
        let width = if near { 0.085 } else { 0.055 };
        let block = (u / width).floor();
        let key = seed
            .wrapping_add(rank as u32 * 977)
            .wrapping_add(block as u32);
        let top = skyline - 0.26 * value(block as i32, rank, key) - if near { 0.0 } else { 0.02 };
        if v < top {
            continue;
        }
        let body = if near {
            [0.035, 0.035, 0.055]
        } else {
            [0.10, 0.10, 0.15]
        };
        colour = body;

        // Windows. A grid inside the block, a margin so the rows do not run
        // into the edges, and a third of them lit — a building with every
        // window lit is an office nobody has ever worked in.
        let across = ((u - block * width) / width - 0.5).abs();
        if across > 0.40 {
            continue;
        }
        let column = ((u - block * width) / (width / 6.0)).floor();
        let row = ((v - top) / 0.020).floor();
        let inside_column = ((u - block * width) / (width / 6.0)).fract();
        let inside_row = ((v - top) / 0.020).fract();
        if !(0.22..0.78).contains(&inside_column) || !(0.25..0.75).contains(&inside_row) {
            continue;
        }
        let lit = value(column as i32 + block as i32 * 97, row as i32, key ^ 0x1234);
        if lit > 0.66 {
            let warmth = 0.55 + 0.45 * value(row as i32, column as i32, key ^ 0x5678);
            let window = [1.0, 0.82, 0.52];
            colour = mix(colour, window, if near { 0.85 } else { 0.55 } * warmth);
        }
    }
    colour
}

fn dunes(u: f32, v: f32, seed: u32) -> [f32; 3] {
    const HORIZON: f32 = 0.30;
    let sky = sky_of(Light::Dusk);
    if v < HORIZON {
        return sky_at(u, v, HORIZON, &sky, seed, 1.5);
    }

    // The haze at the foot of the sky, so that the gap between two crests is
    // distance rather than a hole in the picture.
    let sun = sky.sun_at[0];
    let mut colour = darken(sky.horizon, 0.30);

    // Sand, far to near. What makes a dune read as a dune and not as a stripe
    // is that the face turned away from the sun is in shade — and that shade
    // is read off the *slope* of the crest at this point. Which is why a crest
    // is two slow waves and one slow octave of noise and nothing rougher: the
    // slope of a rough curve is a stripe, and the picture would be combed.
    for layer in 0..5 {
        let near = layer as f32 / 4.0;
        let base = HORIZON + 0.02 + 0.175 * layer as f32;
        let frequency = 0.9 + 1.0 * layer as f32;
        let phase = seed.wrapping_add(layer as u32 * 31);
        let turn = value(layer, 0, seed) * std::f32::consts::TAU;
        let amplitude = 0.018 + 0.05 * near;
        let crest = |x: f32| {
            let wave = ((x * frequency * std::f32::consts::TAU + turn).sin() * 0.6
                + (x * frequency * 2.7 + turn * 1.7).sin() * 0.4)
                * 0.5;
            base - amplitude * (wave + (noise(x * frequency * 1.1, 0.5, phase) - 0.5))
        };
        let top = crest(u);
        if v < top {
            continue;
        }
        // Read either side, and far enough either side that what comes back is
        // the lie of the dune rather than the grain on it.
        let step = 0.02;
        let slope = (crest(u + step) - crest(u - step)) / (2.0 * step);
        // One direction for the whole frame. Taking the sun's side *per point*
        // flips the sign as the picture passes under it, and a sign that flips
        // is a seam straight down the middle of the sand.
        let towards = if sun < 0.5 { -1.0 } else { 1.0 };
        let facing = (slope * towards * 2.6).clamp(-1.0, 1.0);

        let mut here = mix([0.60, 0.44, 0.37], [0.84, 0.58, 0.36], near);
        here = mix(here, darken(sky.horizon, 0.12), (1.0 - near).powi(2) * 0.66);
        here = scale(here, (1.0 + 0.30 * facing).clamp(0.55, 1.30));
        // Ripples run along the crest rather than down it, which is the one
        // thing that says which way the wind was going.
        let ripple = (fbm(u * 22.0, (v - top) * 80.0, phase ^ 0x3c3c, 2) - 0.5) * 0.06 * near;
        here = add(here, [ripple, ripple * 0.86, ripple * 0.7]);
        // And the near face falls into shadow as it comes down.
        here = darken(here, ((v - top) * 0.7).clamp(0.0, 0.22));
        colour = here;
    }
    colour
}

// ------------------------------------------------------------ the arithmetic

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f32; 3], by: f32) -> [f32; 3] {
    [a[0] * by, a[1] * by, a[2] * by]
}

fn lighten(a: [f32; 3], by: f32) -> [f32; 3] {
    mix(a, [1.0, 1.0, 1.0], by)
}

fn darken(a: [f32; 3], by: f32) -> [f32; 3] {
    mix(a, [0.0, 0.0, 0.0], by)
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A number between nought and one for a point on a lattice, out of nothing
/// but the point itself — so the same build draws the same picture on every
/// machine, with no state carried from one pixel to the next.
fn value(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32)
        .wrapping_mul(0x8da6_b343)
        .wrapping_add((y as u32).wrapping_mul(0xd8163841))
        .wrapping_add(seed.wrapping_mul(0xcb1a_b31f));
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    h as f32 / u32::MAX as f32
}

/// Value noise: the lattice, read between its points.
fn noise(x: f32, y: f32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (smoothstep(x - x0), smoothstep(y - y0));
    let (ix, iy) = (x0 as i32, y0 as i32);
    let top = value(ix, iy, seed) + (value(ix + 1, iy, seed) - value(ix, iy, seed)) * fx;
    let bottom =
        value(ix, iy + 1, seed) + (value(ix + 1, iy + 1, seed) - value(ix, iy + 1, seed)) * fx;
    top + (bottom - top) * fy
}

/// Several octaves of it, which is what turns a lattice into cloud, haze or
/// grain depending on the scale it is asked for.
fn fbm(x: f32, y: f32, seed: u32, octaves: u32) -> f32 {
    let mut total = 0.0;
    let mut amplitude = 0.5;
    let mut frequency = 1.0;
    let mut weight = 0.0;
    for octave in 0..octaves {
        total += noise(x * frequency, y * frequency, seed.wrapping_add(octave * 7)) * amplitude;
        weight += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    total / weight.max(f32::EPSILON)
}

/// A skyline: one dimension of noise, with the smooth octaves folded so that
/// the peaks come to a point. A ridge drawn from plain noise is a row of
/// hills, and a mountain is not a hill.
fn ridge(x: f32, seed: u32) -> f32 {
    let mut total = 0.0;
    let mut amplitude = 0.5;
    let mut frequency = 1.0;
    let mut weight = 0.0;
    for octave in 0..5 {
        let here = noise(x * frequency, 0.5, seed.wrapping_add(octave * 13));
        total += (1.0 - (here * 2.0 - 1.0).abs()) * amplitude;
        weight += amplitude;
        amplitude *= 0.55;
        frequency *= 2.0;
    }
    total / weight.max(f32::EPSILON)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The folder has something in it, has somewhere further in, has the pair
    /// that says which order a listing is read in, and has the one film with a
    /// subtitle file beside it.
    #[test]
    fn the_made_up_folder_is_a_folder_somebody_could_have() {
        assert!(FILMS.len() >= 6);
        assert!(FILMS.iter().any(|one| one.within.is_some()));
        assert!(FILMS.iter().any(|one| one.name.contains("S01E09")));
        assert!(FILMS.iter().any(|one| one.name.contains("S01E10")));
        assert!(FILMS.iter().any(|one| one.subtitle.is_some()));
        // Both containers, because a folder that only ever holds one is a
        // folder that never says the other is read.
        assert!(FILMS.iter().any(|one| one.name.ends_with(".mp4")));
        assert!(FILMS.iter().any(|one| one.name.ends_with(".mkv")));
        assert!(FILMS
            .iter()
            .all(|one| crate::library::is_film(Path::new(one.name))));
    }

    /// Every name is its own, in its own folder — two films of the same name
    /// would be one film and a listing that counts wrong.
    #[test]
    fn no_two_films_share_a_name() {
        let mut seen: Vec<(Option<&str>, &str)> =
            FILMS.iter().map(|one| (one.within, one.name)).collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), before);
    }

    /// The frame is a window on the still and the still is wider than it, so
    /// there is something to pan across and the window never falls off the end.
    #[test]
    fn the_pan_stays_inside_the_still() {
        let wide = (WIDTH as f32 * PAN) as u32 & !1;
        assert!(wide > WIDTH);
        for along in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let from = ((wide - WIDTH) as f32 * smoothstep(along)).round() as u32;
            assert!(from + WIDTH <= wide, "{along} ran off the end");
        }
    }

    /// Nothing a scene draws leaves the range a colour has.
    #[test]
    fn every_scene_stays_inside_a_colour() {
        for film in FILMS {
            for step in 0..64 {
                let t = step as f32 / 63.0;
                for (u, v) in [(t, 0.12), (t, 0.5), (t, 0.88), (0.5, t)] {
                    let colour = match film.scene {
                        Scene::Water(light) => water(u, v, light, film.seed),
                        Scene::Ridges(light) => ridges(u, v, light, film.seed),
                        Scene::City => city(u, v, film.seed),
                        Scene::Dunes => dunes(u, v, film.seed),
                    };
                    for channel in colour {
                        assert!(
                            channel.is_finite() && (-0.01..1.45).contains(&channel),
                            "{} at ({u}, {v}) answered {channel}",
                            film.name
                        );
                    }
                }
            }
        }
    }
}
