//! The words under a film, and the one reason they are drawn here.
//!
//! **Why this file exists.** Every other word on the screen is set by
//! `lxb-render`, in the shell's own face, at the shell's own sizes. A subtitle
//! cannot be: the film is drawn *over* the frame the toolkit composed (see
//! `film.rs`), so a line the toolkit wrote at the bottom of the picture would
//! be underneath the picture and never seen. A subtitle belongs over the film
//! by every convention there is, so it is set in a pass of this application's
//! own, after the film's.
//!
//! What that does **not** mean is a second typeface. This uses `glyphon` —
//! the crate `lxb-render` sets all of its own type with, at the version it
//! asks for, so there is one rasteriser in the binary — and it loads
//! `lxb_toolkit::assets::FONT_REGULAR`, which is the face the rest of the
//! interface is written in. It is drawn in a different pass and it is the same
//! material.
//!
//! ## Reading the words
//!
//! Two sources, and they are deliberately the same kind of thing by the time
//! anything else sees them: a list of [`Cue`]s with a start, an end and some
//! text. One comes from a stream inside the film — decoded by `player.rs`,
//! which is the only place a decoder lives — and the other from a file sitting
//! beside it, parsed here.
//!
//! SubRip, WebVTT and SubStation are three spellings of one idea and are
//! parsed by one function, because the differences between them are smaller
//! than the difference between any of them and having no subtitles at all.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Wrap,
};

/// One line, and when it is on the screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub from: f64,
    pub to: f64,
    pub text: String,
}

/// The endings a subtitle file beside a film may have.
const ENDINGS: &[&str] = &["srt", "vtt", "ass", "ssa", "sub"];

/// The subtitle files sitting beside a film, in the order they should be
/// offered.
///
/// Matched on the film's own stem and then anything after it — `beach.srt`,
/// `beach.en.srt`, `beach.pl.forced.srt` — because that is how every tool that
/// writes one names it, and a viewer that only found the exact stem would miss
/// every file anybody has ever downloaded.
///
/// Sorted by name so the list is the same on every open. `read_dir` is not.
pub fn beside(film: &Path) -> Vec<PathBuf> {
    let Some(folder) = film.parent() else {
        return Vec::new();
    };
    let Some(stem) = film.file_stem().and_then(|stem| stem.to_str()) else {
        return Vec::new();
    };
    let folded = stem.to_lowercase();
    let Ok(reading) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = reading
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                return false;
            };
            let name = name.to_lowercase();
            let ending = path
                .extension()
                .and_then(|ending| ending.to_str())
                .map(str::to_lowercase);
            // The film itself has the same stem, so the ending is what keeps
            // it out — and a `.sub` beside a `.sub` cannot happen because a
            // film is never one.
            ending.is_some_and(|ending| ENDINGS.contains(&ending.as_str()))
                && name.starts_with(&folded)
        })
        .collect();
    found.sort();
    found
}

/// What a subtitle file is called, to somebody choosing between three of them.
///
/// The part of the name the film's own name does not account for — `en`,
/// `pl.forced` — because that is the whole of what tells two of them apart,
/// and a menu of three rows all reading `The Long Goodbye (1973).srt` is a
/// menu with nothing to choose from. Falls back to the file's own name where
/// there is nothing left over, which is the single-file case.
pub fn name_of(film: &Path, subtitle: &Path) -> String {
    let whole = subtitle
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    // Both without their ending, so what is compared is `beach` against
    // `beach.en` rather than `beach.mkv` against `beach.en.srt`.
    let film_stem = film
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    let stem = subtitle
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let rest = if !film_stem.is_empty()
        && stem.len() >= film_stem.len()
        && stem.is_char_boundary(film_stem.len())
        && stem[..film_stem.len()].eq_ignore_ascii_case(film_stem)
    {
        stem[film_stem.len()..].trim_start_matches('.')
    } else {
        ""
    };
    // Nothing left over — the file is named for the film and nothing else — so
    // its own name is all there is to say. `srt` would be a row saying only
    // what everybody can already see.
    if rest.is_empty() {
        whole.to_string()
    } else {
        rest.to_string()
    }
}

/// Read one subtitle file into cues. Never fails: a file that parses to
/// nothing is a track with nothing on it, which is what an empty list says.
pub fn read(path: &Path) -> Vec<Cue> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let text = decode(&bytes);
    let ending = path
        .extension()
        .and_then(|ending| ending.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    if ending == "ass" || ending == "ssa" {
        substation(&text)
    } else {
        timed_blocks(&text)
    }
}

/// Bytes to text.
///
/// UTF-8 with or without a mark, UTF-16 with one, and **Windows-1252 for
/// anything else** — which is a guess, and the honest one. A subtitle file
/// carries no declaration of its encoding: the format predates the question.
/// Nearly everything written this decade is UTF-8, so that is tried first and
/// accepted only if the whole file is valid; what is left is an older file in
/// somebody's local code page, and 1252 reads the most of those correctly.
/// A file in a different page shows the wrong letters under the accents rather
/// than nothing at all, which is the better of the two failures.
fn decode(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, false);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, true);
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|byte| windows_1252(*byte)).collect(),
    }
}

fn utf16(bytes: &[u8], big_endian: bool) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| {
            if big_endian {
                u16::from_be_bytes([pair[0], pair[1]])
            } else {
                u16::from_le_bytes([pair[0], pair[1]])
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}

/// The thirty-two places Windows-1252 differs from Latin-1. Everything else in
/// the page is its own code point.
fn windows_1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    match byte {
        0x80..=0x9F => HIGH[(byte - 0x80) as usize],
        other => char::from(other),
    }
}

/// SubRip and WebVTT, which are one format with two spellings of a decimal
/// point.
///
/// Blocks separated by a blank line; a line holding `-->` is the timing and
/// everything after it up to the blank line is the text. Anything before the
/// timing — a cue number, a cue name, `WEBVTT`, a `NOTE` — is not read, which
/// is what makes one parser cover both.
fn timed_blocks(text: &str) -> Vec<Cue> {
    let mut cues = Vec::new();
    let mut timing: Option<(f64, f64)> = None;
    let mut lines: Vec<&str> = Vec::new();

    let finish = |cues: &mut Vec<Cue>, timing: &mut Option<(f64, f64)>, lines: &mut Vec<&str>| {
        if let Some((from, to)) = timing.take() {
            let body = clean(&lines.join("\n"));
            if !body.is_empty() {
                cues.push(Cue {
                    from,
                    to,
                    text: body,
                });
            }
        }
        lines.clear();
    };

    for line in text.lines() {
        let trimmed = line.trim_end_matches('\r');
        if trimmed.trim().is_empty() {
            finish(&mut cues, &mut timing, &mut lines);
            continue;
        }
        if let Some((left, right)) = trimmed.split_once("-->") {
            // A second timing before the blank line ends the one before it,
            // which is what a file with a missing blank line looks like.
            finish(&mut cues, &mut timing, &mut lines);
            let from = stamp(left);
            // WebVTT writes its cue settings after the end time, on the same
            // line: `00:00:12.000 --> 00:00:15.000 line:90% align:center`.
            let to = stamp(right.split_whitespace().next().unwrap_or(right));
            if let (Some(from), Some(to)) = (from, to) {
                timing = Some((from, to.max(from)));
            }
            continue;
        }
        if timing.is_some() {
            lines.push(trimmed);
        }
    }
    finish(&mut cues, &mut timing, &mut lines);
    cues.sort_by(|left, right| left.from.total_cmp(&right.from));
    cues
}

/// SubStation Alpha: a `Dialogue:` line per cue, with the text as the last
/// field and everything before it fixed by the `Format:` line.
///
/// The format line is read rather than assumed, because the field order is
/// genuinely per-file — that is what the line is for — and a fixed count is
/// what makes a subtitle appear one field short with half its own text as a
/// style name.
fn substation(text: &str) -> Vec<Cue> {
    let mut cues = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.trim_start().strip_prefix("Format:") {
            fields = rest
                .split(',')
                .map(|field| field.trim().to_lowercase())
                .collect();
            continue;
        }
        let Some(rest) = line.trim_start().strip_prefix("Dialogue:") else {
            continue;
        };
        // The last field is the text and may hold commas of its own, so the
        // split is bounded by however many fields come before it.
        let count = if fields.is_empty() { 10 } else { fields.len() };
        let parts: Vec<&str> = rest.splitn(count, ',').collect();
        if parts.len() < count {
            continue;
        }
        let at = |name: &str, fallback: usize| -> usize {
            fields
                .iter()
                .position(|field| field == name)
                .unwrap_or(fallback)
        };
        let (Some(from), Some(to)) = (
            parts.get(at("start", 1)).and_then(|value| stamp(value)),
            parts.get(at("end", 2)).and_then(|value| stamp(value)),
        ) else {
            continue;
        };
        let body = clean(parts.last().copied().unwrap_or_default());
        if !body.is_empty() {
            cues.push(Cue {
                from,
                to: to.max(from),
                text: body,
            });
        }
    }
    cues.sort_by(|left, right| left.from.total_cmp(&right.from));
    cues
}

/// `01:23:45,678`, `1:23:45.67`, `23:45.6` — hours optional, and the decimal
/// point written either way.
fn stamp(value: &str) -> Option<f64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let mut seconds = 0.0_f64;
    let mut any = false;
    for part in value.replace(',', ".").split(':') {
        let part = part.trim();
        let number: f64 = part.parse().ok()?;
        if !number.is_finite() || number < 0.0 {
            return None;
        }
        seconds = seconds * 60.0 + number;
        any = true;
    }
    any.then_some(seconds)
}

/// One cue's text, as it is drawn.
///
/// Three things come off it, and each of them appears in real files often
/// enough to be worth handling: SubStation's `{\...}` override blocks, its
/// `\N` line break, and the handful of HTML-ish tags SubRip picked up along the
/// way. What is left is words and newlines.
pub fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    let mut letters = text.chars().peekable();
    while let Some(letter) = letters.next() {
        match letter {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            '\\' => match letters.peek() {
                Some('N') | Some('n') => {
                    letters.next();
                    out.push('\n');
                }
                Some('h') => {
                    letters.next();
                    out.push(' ');
                }
                _ => out.push('\\'),
            },
            '<' => {
                // A tag, if it closes on this line and holds no spaces beyond
                // its own attributes; anything else is a less-than sign, which
                // subtitles do contain.
                let rest: String = letters.clone().take(24).collect();
                match rest.find('>') {
                    Some(end) if !rest[..end].contains('\n') => {
                        for _ in 0..=end {
                            letters.next();
                        }
                    }
                    _ => out.push('<'),
                }
            }
            other => out.push(other),
        }
    }
    // Trailing space on a line is invisible except where it is centred, where
    // it shifts the whole line.
    out.lines()
        .map(str::trim)
        .collect::<Vec<&str>>()
        .join("\n")
        .trim()
        .to_string()
}

/// The cue on the screen at this moment, if there is one.
///
/// A plain search rather than a cursor, because a film is seeked and a cursor
/// that had to be told about it is a cursor that is wrong after every jump.
/// Files run to a few thousand cues and this runs once a frame.
pub fn showing(cues: &[Cue], at: f64) -> Option<&str> {
    cues.iter()
        .find(|cue| at >= cue.from && at < cue.to)
        .map(|cue| cue.text.as_str())
}

// ---- putting them on the screen -----------------------------------------

/// Where one caption goes.
pub struct Caption {
    pub text: String,
    /// The film's own rectangle. The caption sits inside it, near the bottom —
    /// so when the film steps aside to make room for the transport, the words
    /// step aside with it rather than ending up behind the controls.
    pub within: [f32; 4],
    /// The size of one line, in pixels of this window.
    pub size: f32,
    pub opacity: f32,
}

/// The subtitle pass: one text renderer, drawn after the film.
pub struct Captions {
    fonts: FontSystem,
    swash: SwashCache,
    atlas: TextAtlas,
    viewport: glyphon::Viewport,
    renderer: TextRenderer,
    buffer: Buffer,
    /// What the buffer was last shaped for, so a caption that has not changed
    /// is not shaped again sixty times a second.
    shaped: (String, f32, f32),
    /// Whether `prepare` found anything to draw, which is what `draw` asks
    /// before opening a pass.
    anything: bool,
}

impl Captions {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Captions {
        // The interface's own face, and only it. Nothing here consults
        // fontconfig: a subtitle set in whatever the machine happens to
        // consider a sans-serif would be the one thing on the screen that was
        // not the shell's material.
        let fonts = FontSystem::new_with_fonts([glyphon::fontdb::Source::Binary(Arc::new(
            lxb_toolkit::assets::FONT_REGULAR,
        ))]);
        let cache = Cache::new(device);
        let viewport = glyphon::Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let renderer =
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        let buffer = Buffer::new_empty(Metrics::new(16.0, 20.0));
        Captions {
            fonts,
            swash: SwashCache::new(),
            atlas,
            viewport,
            renderer,
            buffer,
            shaped: (String::new(), 0.0, 0.0),
            anything: false,
        }
    }

    /// Shape the caption and hand its glyphs to the GPU. Once a frame, before
    /// anything is drawn.
    pub fn settle(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: [f32; 2],
        caption: Option<&Caption>,
    ) {
        self.anything = false;
        self.viewport.update(
            queue,
            Resolution {
                width: screen[0].max(1.0) as u32,
                height: screen[1].max(1.0) as u32,
            },
        );
        let Some(caption) = caption.filter(|caption| caption.opacity > 0.01) else {
            return;
        };
        if caption.text.is_empty() {
            return;
        }

        // A caption is given the middle of the film's width to sit in, because
        // a line running the whole way across a wide screen is a line the eye
        // has to travel rather than take in.
        let room = (caption.within[2] * 0.82).max(64.0);
        let line = caption.size * 1.24;
        if self.shaped.0 != caption.text
            || (self.shaped.1 - caption.size).abs() > 0.01
            || (self.shaped.2 - room).abs() > 0.5
        {
            self.buffer.set_metrics(Metrics::new(caption.size, line));
            self.buffer.set_wrap(Wrap::Word);
            self.buffer.set_size(Some(room), None);
            // Centred, which is where a subtitle has always been — and set
            // here rather than on each line afterwards, because a line added
            // by wrapping would not have been there to set.
            self.buffer.set_text(
                &caption.text,
                &Attrs::new().family(Family::Name("Roboto")),
                Shaping::Advanced,
                Some(glyphon::cosmic_text::Align::Center),
            );
            self.buffer.shape_until_scroll(&mut self.fonts, false);
            self.shaped = (caption.text.clone(), caption.size, room);
        }

        let lines = self.buffer.layout_runs().count().max(1) as f32;
        let height = lines * line;
        let left = caption.within[0] + (caption.within[2] - room) * 0.5;
        // Up from the bottom of the film by about a line, which is where a
        // subtitle has always sat: far enough in that a television's overscan
        // cannot eat it, and not so far that it covers a face.
        let bottom = caption.within[1] + caption.within[3] - caption.size * 1.1;
        let top = (bottom - height).max(caption.within[1]);

        let alpha = |amount: f32| (amount.clamp(0.0, 1.0) * 255.0).round() as u8;
        let bounds = TextBounds {
            left: caption.within[0] as i32,
            top: caption.within[1] as i32,
            right: (caption.within[0] + caption.within[2]) as i32,
            bottom: (caption.within[1] + caption.within[3]) as i32,
        };

        // The halo the shell puts behind its own words over the wallpaper,
        // asked of the toolkit rather than invented here — which is why a
        // subtitle over a white sky reads exactly as the clock on the start
        // screen does.
        let mut areas: Vec<TextArea> = Vec::new();
        for (dx, dy, strength) in lxb_toolkit::typography::halo_copies(caption.size, 1.0) {
            areas.push(TextArea {
                buffer: &self.buffer,
                left: left + dx,
                top: top + dy,
                scale: 1.0,
                bounds,
                default_color: Color::rgba(0, 0, 0, alpha(strength * caption.opacity)),
                custom_glyphs: &[],
            });
        }
        areas.push(TextArea {
            buffer: &self.buffer,
            left,
            top,
            scale: 1.0,
            bounds,
            default_color: Color::rgba(255, 255, 255, alpha(caption.opacity)),
            custom_glyphs: &[],
        });

        self.anything = self
            .renderer
            .prepare(
                device,
                queue,
                &mut self.fonts,
                &mut self.atlas,
                &self.viewport,
                areas,
                &mut self.swash,
            )
            .is_ok();
    }

    /// Draw the caption, over the film that was drawn over the frame.
    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, into: &wgpu::TextureView) {
        if !self.anything {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("subtitle"),
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
        let _ = self.renderer.render(&self.atlas, &self.viewport, &mut pass);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subrip_is_read() {
        let file = "1\n00:00:01,500 --> 00:00:03,000\nHello\nthere\n\n\
                    2\n00:00:04,000 --> 00:00:05,000\nAgain\n";
        let cues = timed_blocks(file);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].from, 1.5);
        assert_eq!(cues[0].to, 3.0);
        assert_eq!(cues[0].text, "Hello\nthere");
        assert_eq!(cues[1].text, "Again");
    }

    #[test]
    fn webvtt_is_the_same_parser() {
        let file = "WEBVTT\n\nNOTE something\n\ncue-1\n\
                    00:01.000 --> 00:03.000 line:90% align:center\nWords\n";
        let cues = timed_blocks(file);
        assert_eq!(cues.len(), 1, "{cues:?}");
        assert_eq!(cues[0].from, 1.0);
        assert_eq!(cues[0].to, 3.0);
        assert_eq!(cues[0].text, "Words");
    }

    /// A file with a missing blank line still gives two cues rather than one
    /// cue holding the second one's timing as its text.
    #[test]
    fn a_missing_blank_line_does_not_swallow_the_next_cue() {
        let file = "00:00:01,000 --> 00:00:02,000\nOne\n00:00:03,000 --> 00:00:04,000\nTwo\n";
        let cues = timed_blocks(file);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text, "One");
        assert_eq!(cues[1].text, "Two");
    }

    #[test]
    fn substation_reads_its_fields_off_its_own_format_line() {
        let file = "[Events]\n\
            Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
            Dialogue: 0,0:00:01.50,0:00:03.00,Default,,0,0,0,,{\\i1}Hello,\\Nthere{\\i0}\n";
        let cues = substation(file);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].from, 1.5);
        // The comma inside the text survives, which a fixed field count eats.
        assert_eq!(cues[0].text, "Hello,\nthere");
    }

    #[test]
    fn overrides_and_tags_come_off_and_the_words_stay() {
        assert_eq!(clean("{\\an8}Up <i>there</i>"), "Up there");
        assert_eq!(clean("a \\N b"), "a\nb");
        // Not every angle bracket is a tag.
        assert_eq!(clean("5 < 6"), "5 < 6");
    }

    #[test]
    fn a_stamp_reads_with_or_without_hours_and_either_point() {
        assert_eq!(stamp("00:00:01,500"), Some(1.5));
        assert_eq!(stamp("0:00:01.50"), Some(1.5));
        assert_eq!(stamp("01.500"), Some(1.5));
        assert_eq!(stamp("1:01.5"), Some(61.5));
        assert_eq!(stamp("rubbish"), None);
        assert_eq!(stamp(""), None);
    }

    #[test]
    fn the_cue_on_the_screen_is_the_one_whose_time_it_is() {
        let cues = vec![
            Cue {
                from: 1.0,
                to: 2.0,
                text: String::from("one"),
            },
            Cue {
                from: 3.0,
                to: 4.0,
                text: String::from("two"),
            },
        ];
        assert_eq!(showing(&cues, 0.5), None);
        assert_eq!(showing(&cues, 1.5), Some("one"));
        assert_eq!(showing(&cues, 2.5), None);
        assert_eq!(showing(&cues, 3.0), Some("two"));
        assert_eq!(showing(&cues, 4.0), None, "the end is not inclusive");
    }

    #[test]
    fn a_file_that_is_not_utf8_is_still_read() {
        // `café` in Windows-1252.
        let bytes = [b'c', b'a', b'f', 0xE9];
        assert_eq!(decode(&bytes), "café");
        // And a mark is taken off rather than drawn.
        assert_eq!(decode(&[0xEF, 0xBB, 0xBF, b'a']), "a");
    }

    #[test]
    fn a_subtitle_is_named_by_what_the_film_does_not_account_for() {
        let film = Path::new("/f/The Long Goodbye (1973).mkv");
        assert_eq!(
            name_of(film, Path::new("/f/The Long Goodbye (1973).en.srt")),
            "en"
        );
        assert_eq!(
            name_of(film, Path::new("/f/The Long Goodbye (1973).pl.forced.srt")),
            "pl.forced"
        );
        // Nothing left over: the file's own name is all there is to say.
        assert_eq!(
            name_of(film, Path::new("/f/The Long Goodbye (1973).srt")),
            "The Long Goodbye (1973).srt"
        );
    }
}
