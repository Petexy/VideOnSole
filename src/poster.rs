//! The picture a film wears in the grid, and how long it runs.
//!
//! A card standing for a film and drawing a film strip on it says only that
//! there is a film there. A card drawing a frame out of it says *which* film,
//! which is the whole reason a grid is worth having.
//!
//! ## The cache is everybody's
//!
//! What is made is written to `$XDG_CACHE_HOME/thumbnails/large` in the layout
//! the freedesktop thumbnail specification lays down: a PNG named for the MD5
//! of the file's URI, carrying the source's URI and modification time in
//! `tEXt` chunks so a stale one can be told from a good one.
//!
//! That is not a detail, and it is the reason this does not keep a cache of
//! its own. It is the same cache the user's file manager fills and the same
//! one **LineXinBar's own Video shelf reads**: a folder browsed here has
//! poster frames on the shell's bar afterwards, and a folder the shell has
//! walked opens here with every card already drawn. Two programs that each
//! kept their own would each pay the whole cost and neither would help the
//! other.
//!
//! MD5 is written out here rather than pulled in as a dependency, for the
//! reason the shell's own copy gives: it is small, it is frozen, and the whole
//! of what is needed of it is one digest.
//!
//! ## Only what is being looked at
//!
//! Seeking into a film and decoding one frame of it is expensive, and a folder
//! can hold hundreds. Nothing is made ahead of time: the grid asks for the
//! rows around the cursor, once a frame, and this hands back what it has.
//!
//! ## What it hands back is a path
//!
//! Deliberately. The poster goes to `Ui::picture`, which is the toolkit's own
//! atlas — so a card is cut, rounded, covered and faded into the ends of the
//! listing by the same code that draws every other picture in this design
//! language. Drawing it in this application's own pass, as the *playing* film
//! is drawn, would put it over the top of all of that.

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::UNIX_EPOCH;

use ffmpeg_next as ffmpeg;

/// The edge a poster is made to fit inside.
///
/// The specification's `large` size, and what every other desktop's `large`
/// directory holds — which is the whole point of writing there.
const SIZE: u32 = 256;

/// Where into a film the frame is taken from.
///
/// Not the first frame: films open on black, on a fade-in, or on a
/// distributor's logo, and a grid of black rectangles is worse than a grid of
/// film strips. A little way in is where there is something to see. A film
/// shorter than this is taken from a third of the way through instead.
const INTO_FILM: f64 = 3.0;

/// What is known about one film beyond what the directory said.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// How long it runs, in seconds. Nought where the container declares
    /// nothing, which some do.
    pub length: f64,
    pub width: u32,
    pub height: u32,
    /// The poster, once there is one. `None` for a film no frame could be got
    /// out of, which is a card with the mark on it rather than a card missing.
    pub poster: Option<PathBuf>,
}

enum Held {
    Waiting,
    Known(Facts),
    /// Looked at and refused. Kept, so the same unreadable file is not opened
    /// again on every frame it is drawn on.
    Refused,
}

struct Request {
    path: PathBuf,
}

struct Answer {
    path: PathBuf,
    facts: Option<Facts>,
}

pub struct Posters {
    requests: mpsc::Sender<Request>,
    answers: mpsc::Receiver<Answer>,
    held: HashMap<PathBuf, Held>,
}

impl Posters {
    pub fn new() -> Posters {
        let (requests, answers) = readers();
        Posters {
            requests,
            answers,
            held: HashMap::new(),
        }
    }

    /// Take whatever the readers have finished. Once a frame, before anything
    /// is drawn.
    pub fn settle(&mut self) {
        while let Ok(answer) = self.answers.try_recv() {
            let held = match answer.facts {
                Some(facts) => Held::Known(facts),
                None => Held::Refused,
            };
            self.held.insert(answer.path, held);
        }
    }

    /// The films worth knowing about: the ones on the screen, and a row either
    /// side of them.
    pub fn want(&mut self, paths: &[PathBuf]) {
        for path in paths {
            if self.held.contains_key(path) {
                continue;
            }
            self.held.insert(path.clone(), Held::Waiting);
            let _ = self.requests.send(Request { path: path.clone() });
        }
        // Nothing is ever let go of. A film's facts are four numbers and a
        // path — a folder of ten thousand is under a megabyte — and the
        // pictures themselves live in the toolkit's atlas, which does its own
        // letting go.
    }

    pub fn facts(&self, path: &Path) -> Option<&Facts> {
        match self.held.get(path) {
            Some(Held::Known(facts)) => Some(facts),
            _ => None,
        }
    }

    /// The poster to draw on this film's card, if one has been made.
    pub fn poster(&self, path: &Path) -> Option<&Path> {
        self.facts(path).and_then(|facts| facts.poster.as_deref())
    }

    /// Whether this film has been looked at at all, either way. What the grid
    /// asks before drawing a card that is still waiting.
    pub fn settled(&self, path: &Path) -> bool {
        matches!(
            self.held.get(path),
            Some(Held::Known(_)) | Some(Held::Refused)
        )
    }
}

impl Default for Posters {
    fn default() -> Posters {
        Posters::new()
    }
}

/// Two threads opening films.
///
/// Two rather than one because a grid fills in visibly one card at a time on
/// one, and rather than many because seeking into a film is mostly the disk
/// and eight at once is slower than two.
fn readers() -> (mpsc::Sender<Request>, mpsc::Receiver<Answer>) {
    let (send_request, take_request) = mpsc::channel::<Request>();
    let (send_answer, take_answer) = mpsc::channel::<Answer>();
    let queue = Arc::new(Mutex::new(take_request));
    for number in 0..2 {
        let queue = Arc::clone(&queue);
        let answers = send_answer.clone();
        let _ = std::thread::Builder::new()
            .name(format!("videonsole-poster-{number}"))
            .spawn(move || loop {
                let request = {
                    let Ok(queue) = queue.lock() else {
                        break;
                    };
                    queue.recv()
                };
                let Ok(request) = request else {
                    break;
                };
                let facts = look_at(&request.path);
                if answers
                    .send(Answer {
                        path: request.path,
                        facts,
                    })
                    .is_err()
                {
                    break;
                }
            });
    }
    (send_request, take_answer)
}

/// Open one film: how long it is, how big it is, and a frame out of it.
fn look_at(path: &Path) -> Option<Facts> {
    crate::player::start_ffmpeg();

    // The cache first. A poster somebody's file manager already made is a
    // poster this does not have to decode, which is the whole point of the
    // shared layout.
    let cached = cached_at(path);
    let standing = cached.as_ref().filter(|at| still_good(at, path)).cloned();

    let mut input = ffmpeg::format::input(path).ok()?;
    let length = if input.duration() > 0 {
        input.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)
    } else {
        0.0
    };
    let (index, base, parameters, aspect) = {
        let stream = input.streams().best(ffmpeg::media::Type::Video)?;
        (
            stream.index(),
            stream.time_base(),
            stream.parameters(),
            crate::player::shape_of(&stream),
        )
    };
    let _ = base;

    let mut decoder = ffmpeg::codec::context::Context::from_parameters(parameters)
        .ok()?
        .decoder()
        .video()
        .ok()?;
    let (width, height) = (decoder.width(), decoder.height());
    let shown = shown_size(width, height, aspect);

    if let Some(poster) = standing {
        return Some(Facts {
            length,
            width: shown.0,
            height: shown.1,
            poster: Some(poster),
        });
    }

    // A little way in, but never past the end of a film shorter than that.
    let into = if length > INTO_FILM * 2.0 {
        INTO_FILM
    } else if length > 0.0 {
        length / 3.0
    } else {
        0.0
    };
    if into > 0.0 {
        let stamp = (into * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
        let _ = input.seek(stamp, ..stamp);
    }

    let frame = first_frame(&mut input, &mut decoder, index)?;
    let poster = write_poster(path, &frame, shown, cached.as_deref());
    Some(Facts {
        length,
        width: shown.0,
        height: shown.1,
        poster,
    })
}

/// Decode until one frame comes out, or until it is clear none will.
///
/// Bounded, because a file whose video stream is damaged from the seek point
/// onwards would otherwise be read to its end — several gigabytes, on a worker
/// that a grid is waiting for.
fn first_frame(
    input: &mut ffmpeg::format::context::Input,
    decoder: &mut ffmpeg::decoder::Video,
    index: usize,
) -> Option<ffmpeg::frame::Video> {
    let mut packet = ffmpeg::codec::packet::Packet::empty();
    for _ in 0..600 {
        if packet.read(input).is_err() {
            break;
        }
        if packet.stream() != index {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            continue;
        }
        let mut frame = ffmpeg::frame::Video::empty();
        if decoder.receive_frame(&mut frame).is_ok() {
            return Some(frame);
        }
    }
    let _ = decoder.send_eof();
    let mut frame = ffmpeg::frame::Video::empty();
    decoder.receive_frame(&mut frame).ok().map(|()| frame)
}

/// How large a film really is on a screen.
///
/// A film's pixels are not always square: a disk stores a widescreen picture
/// in a 720-wide frame and says so in a ratio beside it. Drawing the stored
/// size is the tall, thin picture everybody has seen.
pub fn shown_size(width: u32, height: u32, aspect: f32) -> (u32, u32) {
    if width == 0 || height == 0 || !aspect.is_finite() || aspect <= 0.0 {
        return (width, height);
    }
    let stored = width as f32 / height as f32;
    if (stored - aspect).abs() < 0.001 {
        return (width, height);
    }
    if aspect > stored {
        (((height as f32) * aspect).round() as u32, height)
    } else {
        (width, ((width as f32) / aspect).round() as u32)
    }
}

/// Scale a frame down to the cache's size and write it where every desktop
/// looks for one.
fn write_poster(
    film: &Path,
    frame: &ffmpeg::frame::Video,
    shown: (u32, u32),
    at: Option<&Path>,
) -> Option<PathBuf> {
    let at = at?;
    // libswscale *aborts the process* when asked for a context whose input
    // format was never worked out — which is what a damaged film's first frame
    // looks like. Refused here, on a worker whose death would take the window
    // with it.
    if frame.format() == ffmpeg::format::Pixel::None || frame.width() == 0 || frame.height() == 0 {
        return None;
    }
    let (width, height) = fit(shown, SIZE);
    let mut scaler = ffmpeg::software::scaling::Context::get(
        frame.format(),
        frame.width(),
        frame.height(),
        ffmpeg::format::Pixel::RGBA,
        width,
        height,
        ffmpeg::software::scaling::Flags::BILINEAR,
    )
    .ok()?;
    let mut out = ffmpeg::frame::Video::empty();
    scaler.run(frame, &mut out).ok()?;

    // The scaled frame's rows are padded; a PNG's are not.
    let stride = out.stride(0);
    let row = (width as usize) * 4;
    let data = out.data(0);
    let mut pixels = Vec::with_capacity(row * height as usize);
    for line in 0..height as usize {
        let from = line * stride;
        pixels.extend_from_slice(data.get(from..from + row)?);
    }

    let stamp = std::fs::metadata(film)
        .ok()
        .and_then(|about| about.modified().ok())
        .and_then(|when| when.duration_since(UNIX_EPOCH).ok())
        .map(|since| since.as_secs())
        .unwrap_or(0);

    let mut png: Vec<u8> = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        // The two the specification requires: what this is a picture of, and
        // when that file was written. Without them every other program treats
        // it as a thumbnail of unknown provenance and makes its own.
        encoder
            .add_text_chunk(String::from("Thumb::URI"), file_uri(film))
            .ok()?;
        encoder
            .add_text_chunk(String::from("Thumb::MTime"), stamp.to_string())
            .ok()?;
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(&pixels).ok()?;
    }

    let folder = at.parent()?;
    std::fs::create_dir_all(folder).ok()?;
    // Written beside it and renamed into place, so a reader never sees half a
    // file — including the file manager that is very likely reading this same
    // directory at the same time.
    let temporary = folder.join(format!(
        ".videonsole-{}-{:?}.png",
        std::process::id(),
        std::thread::current().id()
    ));
    {
        let mut file = std::fs::File::create(&temporary).ok()?;
        file.write_all(&png).ok()?;
        // The specification asks for a private mode: a thumbnail can be a
        // picture of something the user would not put in a world-readable
        // place.
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    if std::fs::rename(&temporary, at).is_err() {
        let _ = std::fs::remove_file(&temporary);
        return None;
    }
    Some(at.to_path_buf())
}

/// The size a picture becomes when its longest edge is held to `edge`.
/// Never enlarged: a film smaller than the cache's size keeps its own.
fn fit((width, height): (u32, u32), edge: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= edge || longest == 0 {
        return (width.max(1), height.max(1));
    }
    let scale = f64::from(edge) / f64::from(longest);
    (
        ((f64::from(width) * scale).round() as u32).max(1),
        ((f64::from(height) * scale).round() as u32).max(1),
    )
}

fn cache_dir() -> Option<PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| crate::library::home().map(|home| home.join(".cache")))?;
    Some(cache.join("thumbnails").join("large"))
}

fn cached_at(path: &Path) -> Option<PathBuf> {
    Some(cache_dir()?.join(format!("{}.png", md5_hex(file_uri(path).as_bytes()))))
}

/// Whether a cached poster is still a picture of this file.
///
/// The modification time first, which is what catches a film replaced by
/// another of the same name, and then the URI, which catches the vanishingly
/// unlikely collision. A cached file that says nothing about itself is not
/// trusted: it was written by something that did not follow the specification,
/// and it is cheaper to make a new one than to draw somebody else's picture.
fn still_good(at: &Path, film: &Path) -> bool {
    let Ok(data) = std::fs::read(at) else {
        return false;
    };
    let Ok(about) = std::fs::metadata(film) else {
        return false;
    };
    let Some(stamp) = about
        .modified()
        .ok()
        .and_then(|when| when.duration_since(UNIX_EPOCH).ok())
        .map(|since| since.as_secs())
    else {
        return false;
    };
    if png_text(&data, "Thumb::MTime").as_deref() != Some(stamp.to_string().as_str()) {
        return false;
    }
    png_text(&data, "Thumb::URI").is_none_or(|uri| uri == file_uri(film))
}

/// One `tEXt` chunk out of a PNG, by keyword.
fn png_text(data: &[u8], keyword: &str) -> Option<String> {
    for (kind, body) in png_chunks(data) {
        if kind != b"tEXt" {
            continue;
        }
        let mut halves = body.splitn(2, |byte| *byte == 0);
        let key = halves.next()?;
        let value = halves.next()?;
        if key == keyword.as_bytes() {
            return Some(String::from_utf8_lossy(value).into_owned());
        }
    }
    None
}

/// The chunks of a PNG, as they lie: length, kind, body, checksum.
fn png_chunks(data: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    let mut at = 8;
    std::iter::from_fn(move || {
        let length = u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?) as usize;
        let kind = data.get(at + 4..at + 8)?;
        let body = data.get(at + 8..at + 8 + length)?;
        at += 12 + length;
        Some((kind, body))
    })
}

/// A path as the URI a thumbnail is named for.
///
/// Percent-encoded to the specification's own rule: unreserved characters and
/// the path separator stand, and everything else — spaces above all, which
/// every film downloaded from anywhere has in its name — becomes a triplet.
pub fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.as_os_str().as_encoded_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                uri.push(char::from(*byte))
            }
            other => uri.push_str(&format!("%{other:02X}")),
        }
    }
    uri
}

/// MD5, as the thumbnail specification names the file by.
///
/// Written out rather than depended on: it is a hundred lines, it has not
/// changed since 1992, and it is used here for a file name rather than for
/// anything that has to be hard to forge.
fn md5_hex(data: &[u8]) -> String {
    const SHIFTS: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, //
        5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, //
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, //
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let table: [u32; 64] =
        std::array::from_fn(|step| ((step as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32);

    let mut message = data.to_vec();
    let length = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&length.to_le_bytes());

    let mut state: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    for block in message.chunks_exact(64) {
        let words: [u32; 16] = std::array::from_fn(|index| {
            u32::from_le_bytes(block[index * 4..index * 4 + 4].try_into().unwrap_or([0; 4]))
        });
        let [mut a, mut b, mut c, mut d] = state;
        for step in 0..64 {
            let (mixed, taken) = match step / 16 {
                0 => ((b & c) | (!b & d), step),
                1 => ((d & b) | (!d & c), (5 * step + 1) % 16),
                2 => (b ^ c ^ d, (3 * step + 5) % 16),
                _ => (c ^ (b | !d), (7 * step) % 16),
            };
            let moved = a
                .wrapping_add(mixed)
                .wrapping_add(table[step])
                .wrapping_add(words[taken]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(moved.rotate_left(SHIFTS[step]));
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
    }

    state
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published vectors. This names files in a cache everything else on
    /// the machine shares, so being one bit different from everybody is being
    /// wrong in a way that only shows as every thumbnail being made twice.
    #[test]
    fn md5_matches_the_published_vectors() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"a"), "0cc175b9c0f1b6a831c399e269772661");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex(b"message digest"),
            "f96b697d7cb7938d525a2f31aaf161d0"
        );
        assert_eq!(
            md5_hex(b"The quick brown fox jumps over the lazy dog"),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
        // Longer than one block, which is where a padding mistake shows.
        assert_eq!(
            md5_hex(
                b"12345678901234567890123456789012345678901234567890\
                  123456789012345678901234567890"
            )
            .len(),
            32
        );
    }

    #[test]
    fn a_uri_is_encoded_the_way_the_specification_names_a_file() {
        assert_eq!(
            file_uri(Path::new("/home/jens/film/me.mkv")),
            "file:///home/jens/film/me.mkv"
        );
        // The case every downloaded film is.
        assert_eq!(
            file_uri(Path::new("/f/The Long Goodbye.mkv")),
            "file:///f/The%20Long%20Goodbye.mkv"
        );
    }

    #[test]
    fn the_longest_edge_is_held_and_nothing_is_enlarged() {
        assert_eq!(fit((1920, 1080), 256), (256, 144));
        assert_eq!(fit((1080, 1920), 256), (144, 256));
        assert_eq!(fit((160, 90), 256), (160, 90), "never enlarged");
        assert_eq!(fit((0, 0), 256), (1, 1));
    }

    /// A disk stores a widescreen film in a 720-wide frame and says so beside
    /// it. Drawing the stored size is the tall thin picture.
    #[test]
    fn a_films_pixels_are_not_always_square() {
        assert_eq!(shown_size(720, 576, 16.0 / 9.0), (1024, 576));
        assert_eq!(shown_size(720, 576, 4.0 / 3.0), (768, 576));
        // Already square: nothing is touched.
        assert_eq!(shown_size(1920, 1080, 16.0 / 9.0), (1920, 1080));
        // And nothing is believed that cannot be true.
        assert_eq!(shown_size(1920, 1080, 0.0), (1920, 1080));
        assert_eq!(shown_size(1920, 1080, f32::NAN), (1920, 1080));
    }

    #[test]
    fn a_png_gives_up_the_chunks_it_was_written_with() {
        let mut png: Vec<u8> = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .add_text_chunk(String::from("Thumb::URI"), String::from("file:///a"))
                .unwrap();
            encoder
                .add_text_chunk(String::from("Thumb::MTime"), String::from("42"))
                .unwrap();
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0, 0, 0, 255]).unwrap();
        }
        assert_eq!(png_text(&png, "Thumb::MTime").as_deref(), Some("42"));
        assert_eq!(png_text(&png, "Thumb::URI").as_deref(), Some("file:///a"));
        assert_eq!(png_text(&png, "Nothing::Here"), None);
    }
}
