//! Playing one film: opening it, decoding it, keeping time, and making a
//! noise.
//!
//! Everything in here runs on **one worker thread**, and that is a decision
//! rather than an omission. Demuxing, decoding the picture, decoding the
//! soundtrack and decoding the subtitles are four jobs that all read from one
//! file in one order, and splitting them across threads buys parallelism that
//! is not needed — a modern machine decodes 1080p several times faster than it
//! plays — at the price of four things that have to agree about where a seek
//! left them. What it costs is bounded and named below: the queues are soft,
//! so a badly interleaved file grows one of them rather than starving the
//! other.
//!
//! ## What keeps time
//!
//! **The sound card.** It consumes samples at exactly the rate it says it
//! does, so the number of samples it has taken *is* the position in the film,
//! and the picture is fitted to that. It is the only clock worth using: the
//! ear notices a tenth of a second out of step and the eye does not, so a
//! player that timed the sound to the picture would be audibly wrong to be
//! invisibly right. A film with no soundtrack, or one whose device would not
//! open, keeps time by the monotonic clock instead — see [`Beat`].
//!
//! A frame is never waited for. `frame_due` hands over the newest frame whose
//! time has come and drops any older ones on the way past, so a machine that
//! cannot keep up loses frames and stays in time rather than staying complete
//! and falling behind.
//!
//! ## What decodes
//!
//! `ffmpeg`, the same library LineXinBar decodes a wallpaper film with —
//! through VA-API where this machine has it and in software where it does not.
//! The picture is **not** converted to RGB here. It is handed on in the planes
//! the decoder produced, and `film.rs` does the colour on the GPU, which is
//! both faster and more correct: a 4K frame is twelve megabytes of conversion
//! sixty times a second that a graphics card does for nothing.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use ffmpeg::format::Pixel;
use ffmpeg_next as ffmpeg;

use crate::subtitles::{self, Cue};

/// How much sound is kept ahead of the device.
///
/// Long enough that a slow frame — a keyframe on a 4K film, a folder being
/// read — cannot be heard, and short enough that the ring is emptied and
/// refilled quickly after a seek. Everything after a seek is silent until this
/// much has been decoded again, so a second of it would be a second of silence
/// on every jump.
const SOUND_AHEAD: f64 = 0.75;

/// And the hard ceiling, for a file whose streams are interleaved badly enough
/// that the picture is a long way from the sound it belongs with.
const SOUND_MOST: f64 = 8.0;

/// How many decoded frames are held ahead of the one being shown.
///
/// Small: a frame is a whole picture, and eight 4K frames is a hundred
/// megabytes. What this has to cover is the jitter between decoding and
/// drawing, not a buffer against slowness — a machine that cannot decode in
/// time will not catch up in eight frames.
const FRAMES_AHEAD: usize = 6;

/// How near a frame's time has to be before it is shown.
///
/// Half a sixtieth of a second. A frame due in five milliseconds is shown on
/// this refresh rather than the next: early by five milliseconds is invisible
/// and late by sixteen is a stutter.
const DUE: f64 = 0.008;

/// How far into a film a seek is allowed to decode before giving up on landing
/// exactly.
///
/// A seek lands on the keyframe at or before what was asked for, and the
/// frames between the two are decoded and thrown away so that the film really
/// starts where somebody pointed. Keyframes are seconds apart, so this is
/// generous — it exists for the file whose index lies, where decoding to a
/// point that is never reached would hang the player rather than miss.
const SEEK_PATIENCE: f64 = 30.0;

/// One subtitle track, as a menu offers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// What to call it: the stream's language or title, or what a file beside
    /// the film is called.
    pub name: String,
    /// Whether it came from a file rather than from inside the film. Worth
    /// saying on the row, because it is the difference between a track the
    /// film has always had and one somebody put there.
    pub from_a_file: bool,
    /// The film marks this one as one that has to be shown — the lines nobody
    /// is meant to be able to turn off, for the scene in another language.
    pub forced: bool,
}

/// Which subtitle track to show when nobody has said.
///
/// Two things count as somebody having meant it, and neither of them is the
/// ordinary `default` flag:
///
/// * **A track the film marks forced.** That flag exists for exactly one
///   purpose — the scene in another language, the sign the plot turns on — and
///   a film that carries one is a film that is wrong without it.
/// * **A file sitting beside the film.** Nobody downloads a `.srt` by
///   accident.
///
/// A plain `default` disposition is deliberately *not* enough. Half the films
/// anybody owns carry an English subtitle track marked default, and turning
/// subtitles on for every one of them is the behaviour people go looking for a
/// setting to stop.
fn worth_showing(tracks: &[Track]) -> Option<usize> {
    tracks
        .iter()
        .position(|track| track.forced)
        .or_else(|| tracks.iter().position(|track| track.from_a_file))
}

/// A frame, and when it should be on the screen.
struct Shown {
    at: f64,
    frame: ffmpeg::frame::Video,
}

/// What the window asks the player to do. Everything that changes playback
/// goes through here, so the worker is the only thing that touches the
/// decoders.
enum Order {
    Seek(f64),
    Subtitles(Option<usize>),
    /// Read a subtitle file that was not beside the film when it was opened.
    AddSubtitles(PathBuf),
    Stop,
}

/// Everything the window may read at any moment, and the worker may write.
struct Shared {
    length: AtomicU64,
    width: AtomicU32,
    height: AtomicU32,
    /// How wide the film really is against how tall, which is not always what
    /// its stored size says — see [`shape_of`].
    aspect: AtomicU32,
    /// A first frame has been decoded, so there is something to draw.
    ready: AtomicBool,
    /// The film has run out and everything decoded has been shown.
    ended: AtomicBool,
    /// Set by the window, read by the worker and by the sound. The one piece
    /// of state that is not the worker's, because a pause has to stop the
    /// clock in the same instant it is pressed.
    playing: AtomicBool,
    /// A seek is in flight, so what is on the screen is not where the film is.
    seeking: AtomicBool,
    has_sound: AtomicBool,
    trouble: Mutex<Option<String>>,
    tracks: Mutex<Vec<Track>>,
    chosen: Mutex<Option<usize>>,
    /// What is doing the decoding, for the details pane. "VA-API" or the
    /// decoder's own name.
    decoded_by: Mutex<String>,
    cues: Mutex<Vec<Cue>>,
    frames: Mutex<VecDeque<Shown>>,
}

pub struct Player {
    path: PathBuf,
    shared: Arc<Shared>,
    sound: Arc<Sound>,
    clock: Arc<Clock>,
    orders: mpsc::Sender<Order>,
    worker: Option<std::thread::JoinHandle<()>>,
    /// Whether anything has been drawn yet, so the very first frame is shown
    /// without waiting for a clock that has not started.
    shown: AtomicBool,
}

impl Player {
    /// Open a film and start playing it from `from` seconds in.
    ///
    /// Never fails and never blocks: opening a file over a network share can
    /// take seconds, and a window that stopped drawing while it happened would
    /// look like a crash. What went wrong, if anything did, is in
    /// [`Player::trouble`] a moment later.
    pub fn open(path: &Path, from: f64, wanted: Option<usize>) -> Player {
        let shared = Arc::new(Shared {
            length: AtomicU64::new(0),
            width: AtomicU32::new(0),
            height: AtomicU32::new(0),
            aspect: AtomicU32::new(0f32.to_bits()),
            ready: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            playing: AtomicBool::new(true),
            seeking: AtomicBool::new(false),
            has_sound: AtomicBool::new(false),
            trouble: Mutex::new(None),
            tracks: Mutex::new(Vec::new()),
            chosen: Mutex::new(wanted),
            decoded_by: Mutex::new(String::new()),
            cues: Mutex::new(Vec::new()),
            frames: Mutex::new(VecDeque::new()),
        });
        // Made here rather than in the worker, because the window sets the
        // volume on it and the clock reads it: one object, so a press on the
        // volume cannot land on a sound nothing is playing.
        let sound = Arc::new(Sound::new());
        let clock = Arc::new(Clock::new(from));
        let (orders, taking) = mpsc::channel();

        let worker = {
            let path = path.to_path_buf();
            let shared = Arc::clone(&shared);
            let sound = Arc::clone(&sound);
            let clock = Arc::clone(&clock);
            std::thread::Builder::new()
                .name(String::from("videonsole-film"))
                .spawn(move || {
                    if let Err(trouble) = run(&path, from, &shared, &sound, &clock, &taking) {
                        *shared.trouble.lock().unwrap_or_else(|it| it.into_inner()) = Some(trouble);
                        shared.ended.store(true, Ordering::Release);
                    }
                })
                .ok()
        };

        Player {
            path: path.to_path_buf(),
            shared,
            sound,
            clock,
            orders,
            worker,
            shown: AtomicBool::new(false),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How far into the film we are.
    ///
    /// Never past the end and never before the beginning — but only held to a
    /// length the container actually declared, because a stream that declares
    /// none would otherwise be pinned at nought for its whole duration.
    pub fn at(&self) -> f64 {
        let at = self.clock.at().max(0.0);
        let length = self.length();
        if length > 0.0 {
            at.min(length)
        } else {
            at
        }
    }

    /// How long the film is, as the container declares it. Nought where it
    /// declares nothing, which some streams genuinely do not.
    pub fn length(&self) -> f64 {
        f64::from_bits(self.shared.length.load(Ordering::Relaxed))
    }

    /// How large the film is as it was stored.
    pub fn size(&self) -> Option<(u32, u32)> {
        let width = self.shared.width.load(Ordering::Relaxed);
        let height = self.shared.height.load(Ordering::Relaxed);
        (width > 0 && height > 0).then_some((width, height))
    }

    /// How large the film is on a screen, which is what the stage is measured
    /// against. Not the same thing as [`Player::size`] for a film whose pixels
    /// are not square — a disk's widescreen picture is stored 720 wide and
    /// shown 1024.
    pub fn shown_size(&self) -> Option<(u32, u32)> {
        let (width, height) = self.size()?;
        let aspect = f32::from_bits(self.shared.aspect.load(Ordering::Relaxed));
        Some(crate::poster::shown_size(width, height, aspect))
    }

    pub fn ready(&self) -> bool {
        self.shared.ready.load(Ordering::Acquire)
    }

    pub fn ended(&self) -> bool {
        self.shared.ended.load(Ordering::Acquire)
    }

    pub fn seeking(&self) -> bool {
        self.shared.seeking.load(Ordering::Acquire)
    }

    pub fn has_sound(&self) -> bool {
        self.shared.has_sound.load(Ordering::Acquire)
    }

    pub fn playing(&self) -> bool {
        self.shared.playing.load(Ordering::Acquire) && !self.ended()
    }

    pub fn trouble(&self) -> Option<String> {
        self.shared
            .trouble
            .lock()
            .unwrap_or_else(|it| it.into_inner())
            .clone()
    }

    pub fn tracks(&self) -> Vec<Track> {
        self.shared
            .tracks
            .lock()
            .unwrap_or_else(|it| it.into_inner())
            .clone()
    }

    pub fn chosen(&self) -> Option<usize> {
        *self
            .shared
            .chosen
            .lock()
            .unwrap_or_else(|it| it.into_inner())
    }

    pub fn decoded_by(&self) -> String {
        self.shared
            .decoded_by
            .lock()
            .unwrap_or_else(|it| it.into_inner())
            .clone()
    }

    pub fn play(&self, playing: bool) {
        self.shared.playing.store(playing, Ordering::Release);
        self.clock.run(playing);
        self.sound.run(playing);
    }

    /// Go to a point in the film.
    ///
    /// The clock is moved **here**, in the instant the press is answered,
    /// rather than when the worker gets there: the groove has to follow the
    /// thumb, and a bar that waited for a decoder would lag behind every jump
    /// by however long a keyframe took to find.
    pub fn seek_to(&self, to: f64) {
        let to = to.clamp(0.0, (self.length() - 0.1).max(0.0));
        self.shared.seeking.store(true, Ordering::Release);
        self.shared.ended.store(false, Ordering::Release);
        self.clock.set(to);
        self.sound.reset();
        let _ = self.orders.send(Order::Seek(to));
    }

    /// Read a subtitle file that is neither in the film nor beside it.
    ///
    /// Answers the number the new track was given, or `None` for a file with
    /// nothing in it this can read — which is the whole of the check, and is
    /// done **here** rather than on the worker so that the press that chose
    /// the file is the thing that says whether it worked. A few tens of
    /// kilobytes read twice is cheaper than an answer that arrives a frame
    /// later with nowhere to go.
    pub fn add_subtitles(&self, file: &Path) -> Option<usize> {
        if subtitles::read(file).is_empty() {
            return None;
        }
        let mut tracks = self
            .shared
            .tracks
            .lock()
            .unwrap_or_else(|it| it.into_inner());
        // A file already on the list is chosen again rather than added twice.
        let name = subtitles::name_of(&self.path, file);
        if let Some(known) = tracks
            .iter()
            .position(|track| track.from_a_file && track.name == name)
        {
            return Some(known);
        }
        tracks.push(Track {
            name,
            from_a_file: true,
            forced: false,
        });
        let number = tracks.len() - 1;
        drop(tracks);
        let _ = self.orders.send(Order::AddSubtitles(file.to_path_buf()));
        Some(number)
    }

    pub fn choose_subtitles(&self, track: Option<usize>) {
        *self
            .shared
            .chosen
            .lock()
            .unwrap_or_else(|it| it.into_inner()) = track;
        let _ = self.orders.send(Order::Subtitles(track));
    }

    /// How loud, from nought to one, and whether it is muted at all.
    pub fn set_volume(&self, volume: f32, muted: bool) {
        self.sound.set_volume(if muted { 0.0 } else { volume });
    }

    /// The frame to draw now, if a new one has come due.
    ///
    /// `None` means the frame already on the screen is still the right one,
    /// which for a 24-frame film on a 60-hertz screen is most refreshes.
    pub fn frame_due(&self) -> Option<ffmpeg::frame::Video> {
        let now = self.clock.at();
        let mut frames = self
            .shared
            .frames
            .lock()
            .unwrap_or_else(|it| it.into_inner());
        let first = !self.shown.load(Ordering::Relaxed);
        let mut taken = None;
        while let Some(front) = frames.front() {
            // The first frame of all is shown whenever it arrives: the clock
            // is counting from a base the worker has only just set, and a
            // window that waited for the two to agree would open on black.
            if !(first && taken.is_none()) && front.at > now + DUE {
                break;
            }
            taken = frames.pop_front();
        }
        if taken.is_some() {
            self.shown.store(true, Ordering::Relaxed);
        }
        taken.map(|shown| shown.frame)
    }

    /// The subtitle on the screen at this moment.
    pub fn caption(&self) -> Option<String> {
        let at = self.clock.at();
        let cues = self.shared.cues.lock().unwrap_or_else(|it| it.into_inner());
        subtitles::showing(&cues, at).map(str::to_string)
    }
}

impl Drop for Player {
    /// Stop the worker and wait for it.
    ///
    /// Waited for rather than detached, because the thread holds the open
    /// file, the decoders and the sound device — and stepping to the next film
    /// makes a new player before this one is dropped. Two players feeding one
    /// device is two soundtracks at once.
    fn drop(&mut self) {
        let _ = self.orders.send(Order::Stop);
        self.sound.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

// ---- what says how far in we are ----------------------------------------

/// The film's own clock.
///
/// `base` is the point the film was last put at; the beat is how far past it
/// playing has got since.
struct Clock {
    base: Mutex<f64>,
    beat: Mutex<Beat>,
}

/// The monotonic clock, stopped and started with the film.
///
/// Kept even when the sound card is the one keeping time, because it is what
/// says the sound card is lying. See [`Clock::at`].
#[derive(Debug)]
struct Wall {
    since: Option<Instant>,
    at: f64,
}

impl Wall {
    fn seconds(&self) -> f64 {
        self.at
            + self
                .since
                .map(|since| since.elapsed().as_secs_f64())
                .unwrap_or(0.0)
    }

    fn reset(&mut self) {
        self.at = 0.0;
        if self.since.is_some() {
            self.since = Some(Instant::now());
        }
    }

    fn run(&mut self, running: bool) {
        match (running, self.since) {
            (true, None) => self.since = Some(Instant::now()),
            (false, Some(from)) => {
                self.at += from.elapsed().as_secs_f64();
                self.since = None;
            }
            _ => {}
        }
    }
}

/// What counts the time.
enum Beat {
    /// The sound card, which is the good one. See the note at the top of this
    /// file. The wall clock beside it is the guard described in [`Clock::at`].
    Sound { sound: Arc<Sound>, wall: Wall },
    /// A film with no soundtrack, or one whose device would not open.
    Alone(Wall),
}

/// How far ahead of real time the sound card is allowed to be.
///
/// A device consumes samples at the rate it says it does, so this is not there
/// for drift — it is there for the *start*, where the device takes a whole
/// buffer at once to fill itself, and for the moment after a seek where it
/// does the same. Half a second is far more than any device's buffer and far
/// less than the error this is guarding against.
const SOUND_MAY_LEAD: f64 = 0.5;

impl Clock {
    fn new(from: f64) -> Clock {
        Clock {
            base: Mutex::new(from),
            beat: Mutex::new(Beat::Alone(Wall {
                since: None,
                at: 0.0,
            })),
        }
    }

    /// Hand timekeeping to the sound card, once there is one.
    ///
    /// The wall clock carries across rather than being made afresh: it is
    /// still counting, and what it has counted so far is what the guard
    /// measures the card against.
    fn keep_time_by(&self, sound: Arc<Sound>, running: bool) {
        let mut beat = self.beat.lock().unwrap_or_else(|it| it.into_inner());
        let mut wall = match std::mem::replace(
            &mut *beat,
            Beat::Alone(Wall {
                since: None,
                at: 0.0,
            }),
        ) {
            Beat::Sound { wall, .. } | Beat::Alone(wall) => wall,
        };
        wall.at = 0.0;
        wall.since = None;
        wall.run(running);
        *beat = Beat::Sound { sound, wall };
    }

    /// How far past `base` the film has got.
    ///
    /// **The sound card keeps the time, and the monotonic clock keeps the
    /// sound card honest.** The number of samples a device has taken is the
    /// position in the film — that is the whole argument at the top of this
    /// file — and it holds for as long as the device really does take them at
    /// the rate it claims. A device that takes them faster does not exist in
    /// hardware and does exist in software: a machine whose default output is
    /// ALSA's `null`, which accepts everything the instant it is offered,
    /// plays a twenty-second film in ten seconds and every frame of it is
    /// technically in time with the soundtrack.
    ///
    /// That is not a hypothetical. It is what this project's own nested test
    /// harness does — it routes the session's audio to `null` so a screenshot
    /// run cannot be heard — and the first film played under it ran at twice
    /// speed. So the wall clock is kept beside the card and the smaller of the
    /// two wins. Both only ever go forwards, so the smaller of them does too.
    fn at(&self) -> f64 {
        let base = *self.base.lock().unwrap_or_else(|it| it.into_inner());
        let beat = self.beat.lock().unwrap_or_else(|it| it.into_inner());
        base + match &*beat {
            Beat::Sound { sound, wall } => sound.seconds().min(wall.seconds() + SOUND_MAY_LEAD),
            Beat::Alone(wall) => wall.seconds(),
        }
    }

    /// Put the film at a point, and start counting again from there.
    fn set(&self, to: f64) {
        // The base first and the beat second: a reader between the two sees
        // the new base with an old count, which is a fraction of a second out.
        // The other order shows the *old* base with a count of nought, which
        // is the film jumping back to where it was before the seek.
        *self.base.lock().unwrap_or_else(|it| it.into_inner()) = to;
        let mut beat = self.beat.lock().unwrap_or_else(|it| it.into_inner());
        match &mut *beat {
            Beat::Sound { sound, wall } => {
                sound.reset();
                wall.reset();
            }
            Beat::Alone(wall) => wall.reset(),
        }
    }

    fn run(&self, running: bool) {
        let mut beat = self.beat.lock().unwrap_or_else(|it| it.into_inner());
        match &mut *beat {
            Beat::Sound { wall, .. } | Beat::Alone(wall) => wall.run(running),
        }
    }
}

// ---- the soundtrack ------------------------------------------------------

/// The samples on their way to the device, and the count of the ones that have
/// arrived — which is the film's clock.
pub struct Sound {
    ring: Mutex<VecDeque<f32>>,
    /// Frames — one per channel set — handed to the device since the last
    /// reset. Outside the lock because it is written for every sample the
    /// device takes and read once a frame by the window.
    played: AtomicU64,
    /// Bumped by a seek. The bridge drops whatever it was holding when this
    /// changes, so a jump does not play a fragment of where the film was.
    generation: AtomicU64,
    volume: AtomicU32,
    running: AtomicBool,
    closed: AtomicBool,
    /// What the device turned out to want. Not known until it is open, and the
    /// window holds this object from before that — so they are read rather
    /// than fixed, and the harmless defaults below are what a film with no
    /// soundtrack keeps for ever.
    rate: AtomicU32,
    channels: AtomicU32,
}

impl Sound {
    /// A sound with no device behind it yet, which is what a film with no
    /// soundtrack keeps and what a machine with no working output falls back
    /// to.
    fn new() -> Sound {
        Sound {
            ring: Mutex::new(VecDeque::new()),
            played: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            volume: AtomicU32::new(1.0f32.to_bits()),
            running: AtomicBool::new(true),
            closed: AtomicBool::new(false),
            rate: AtomicU32::new(48_000),
            channels: AtomicU32::new(2),
        }
    }

    fn rate(&self) -> u32 {
        self.rate.load(Ordering::Relaxed).max(1)
    }

    fn channels(&self) -> u16 {
        self.channels.load(Ordering::Relaxed).clamp(1, 8) as u16
    }

    /// What the device turned out to want, once it is open.
    fn settle(&self, rate: u32, channels: u16) {
        self.rate.store(rate.max(1), Ordering::Relaxed);
        self.channels
            .store(u32::from(channels.max(1)), Ordering::Relaxed);
    }

    fn seconds(&self) -> f64 {
        self.played.load(Ordering::Relaxed) as f64 / f64::from(self.rate())
    }

    /// How much sound is waiting to be played, in seconds.
    fn held(&self) -> f64 {
        let ring = self.ring.lock().unwrap_or_else(|it| it.into_inner());
        ring.len() as f64 / f64::from(self.channels()) / f64::from(self.rate())
    }

    fn push(&self, samples: &[f32]) {
        let mut ring = self.ring.lock().unwrap_or_else(|it| it.into_inner());
        ring.extend(samples.iter().copied());
    }

    fn reset(&self) {
        let mut ring = self.ring.lock().unwrap_or_else(|it| it.into_inner());
        ring.clear();
        self.played.store(0, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Release);
    }

    fn run(&self, running: bool) {
        self.running.store(running, Ordering::Release);
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// How loud, as a multiplier on the samples.
    ///
    /// Squared, because loudness is not linear in amplitude: half way along a
    /// linear control is about three quarters as loud as the end of it, which
    /// makes the top quarter of every volume control do nothing. This is the
    /// cheap curve everything uses and it is close enough to right.
    fn set_volume(&self, volume: f32) {
        let volume = if volume.is_finite() {
            volume.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.volume
            .store((volume * volume).to_bits(), Ordering::Relaxed);
    }
}

/// What rodio pulls from: the ring, one sample at a time.
///
/// It takes a block at a time under the lock and hands the block out
/// unlocked, because rodio asks for one sample per call and a lock per sample
/// on the audio thread is how a player crackles.
struct Bridge {
    sound: Arc<Sound>,
    block: Vec<f32>,
    at: usize,
    generation: u64,
    emitted: u64,
}

/// How many samples the bridge takes at once. About three milliseconds of
/// stereo at forty-eight kilohertz — small enough that a seek throws away
/// nothing anybody could hear, large enough that the lock is taken a few
/// hundred times a second rather than a hundred thousand.
const BLOCK: usize = 256;

impl Iterator for Bridge {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.sound.closed.load(Ordering::Acquire) {
            return None;
        }
        let generation = self.sound.generation.load(Ordering::Acquire);
        if generation != self.generation {
            self.generation = generation;
            self.block.clear();
            self.at = 0;
            self.emitted = 0;
        }
        if self.at >= self.block.len() {
            self.block.clear();
            self.at = 0;
            if self.sound.running.load(Ordering::Acquire) {
                let mut ring = self.sound.ring.lock().unwrap_or_else(|it| it.into_inner());
                let channels = usize::from(self.sound.channels());
                // Whole frames only, or the channels swap over on the next
                // block and the film plays with its ears crossed.
                let take = (ring.len() / channels * channels).min(BLOCK);
                self.block.extend(ring.drain(..take));
            }
        }
        // Nothing to hand over: silence, and the clock does not move. That is
        // the whole of what buffering looks like from here — the picture waits
        // because the sound waits, and the two never come apart.
        let Some(sample) = self.block.get(self.at).copied() else {
            return Some(0.0);
        };
        self.at += 1;
        self.emitted += 1;
        if self
            .emitted
            .is_multiple_of(u64::from(self.sound.channels()))
        {
            self.sound.played.fetch_add(1, Ordering::Relaxed);
        }
        Some(sample * f32::from_bits(self.sound.volume.load(Ordering::Relaxed)))
    }
}

impl rodio::Source for Bridge {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> rodio::ChannelCount {
        std::num::NonZero::new(self.sound.channels()).unwrap_or(std::num::NonZero::<u16>::MIN)
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        std::num::NonZero::new(self.sound.rate()).unwrap_or(std::num::NonZero::<u32>::MIN)
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

// ---- the worker ----------------------------------------------------------

/// Set up the library once, however many films are opened.
///
/// Public because the poster reader opens films too, on threads of its own,
/// and `ffmpeg::init` is not something two parts of one program may each do.
pub fn start_ffmpeg() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if ffmpeg::init().is_ok() {
            // Errors only. A film with a damaged packet in it prints a line
            // per packet at the default level, and this application's stderr
            // is where somebody looks for the reason a controller does
            // nothing.
            ffmpeg::util::log::set_level(ffmpeg::util::log::Level::Error);
        }
    });
}

/// How wide a film really is against how tall.
///
/// **A film's pixels are not always square.** A disk stores a widescreen
/// picture in a 720-wide frame and writes the ratio beside it; drawing the
/// stored size is the tall, thin picture everybody has seen at least once.
/// The stream carries the ratio, and where it does not the parameters do —
/// asked in that order because a container that says one thing and a codec
/// that says another is a container that was remuxed, and the container is the
/// later word.
///
/// Nought where nothing can be worked out, which every caller reads as "the
/// stored size is the shown size".
pub fn shape_of(stream: &ffmpeg::Stream) -> f32 {
    let parameters = stream.parameters();
    unsafe {
        let coded = parameters.as_ptr();
        let (width, height) = ((*coded).width as f32, (*coded).height as f32);
        if width <= 0.0 || height <= 0.0 {
            return 0.0;
        }
        let mut ratio = (*stream.as_ptr()).sample_aspect_ratio;
        if ratio.num <= 0 || ratio.den <= 0 {
            ratio = (*coded).sample_aspect_ratio;
        }
        let sample = if ratio.num > 0 && ratio.den > 0 {
            ratio.num as f32 / ratio.den as f32
        } else {
            1.0
        };
        width * sample / height
    }
}

fn seconds_of(stamp: i64, base: ffmpeg::Rational) -> f64 {
    stamp as f64 * f64::from(base.numerator()) / f64::from(base.denominator()).max(1.0)
}

fn run(
    path: &Path,
    from: f64,
    shared: &Arc<Shared>,
    sound_state: &Arc<Sound>,
    clock: &Arc<Clock>,
    orders: &mpsc::Receiver<Order>,
) -> Result<(), String> {
    start_ffmpeg();

    let mut input = ffmpeg::format::input(path).map_err(|err| said(&err))?;

    let length = if input.duration() > 0 {
        input.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)
    } else {
        0.0
    };
    shared.length.store(length.to_bits(), Ordering::Relaxed);

    // Everything the streams have to say, taken in one pass and owned, because
    // reading a packet wants the input to itself and a `Stream` borrowed from
    // it would still be alive when it does.
    let (picture_at, picture_base, picture_parameters, shape) = {
        let picture = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or_else(|| String::from(crate::i18n::text("no-video-stream")))?;
        (
            picture.index(),
            picture.time_base(),
            picture.parameters(),
            shape_of(&picture),
        )
    };
    shared.aspect.store(shape.to_bits(), Ordering::Relaxed);
    let sound_found = input
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .map(|stream| (stream.index(), stream.time_base(), stream.parameters()));
    let sound_at = sound_found.as_ref().map(|(index, ..)| *index);
    let sound_base = sound_found.as_ref().map(|(_, base, _)| *base);

    // The subtitle tracks: the film's own first, then the files beside it.
    // In that order because a track inside the film is the one its maker
    // chose, and a file beside it is one somebody added.
    let mut inside: Vec<(usize, ffmpeg::Rational, Track)> = Vec::new();
    for stream in input.streams() {
        if stream.parameters().medium() != ffmpeg::media::Type::Subtitle {
            continue;
        }
        let name = stream_name(&stream, inside.len());
        let forced = stream
            .disposition()
            .contains(ffmpeg::format::stream::Disposition::FORCED);
        inside.push((
            stream.index(),
            stream.time_base(),
            Track {
                name,
                from_a_file: false,
                forced,
            },
        ));
    }
    let files = subtitles::beside(path);
    let mut tracks: Vec<Track> = inside.iter().map(|(_, _, track)| track.clone()).collect();
    for file in &files {
        tracks.push(Track {
            name: subtitles::name_of(path, file),
            from_a_file: true,
            forced: false,
        });
    }
    // Nobody has said which track they want yet, so the film and the folder
    // are asked. See [`worth_showing`].
    {
        let mut chosen = shared.chosen.lock().unwrap_or_else(|it| it.into_inner());
        if chosen.is_none() {
            *chosen = worth_showing(&tracks);
        }
    }
    *shared.tracks.lock().unwrap_or_else(|it| it.into_inner()) = tracks;

    // The picture's decoder, through the hardware where there is any.
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(picture_parameters)
        .map_err(|err| said(&err))?
        .decoder();
    unsafe {
        let context = decoder.as_mut_ptr();
        // Let the library work out how many threads to use. Frame threading
        // is what makes one decoder keep up with 4K at all.
        (*context).thread_count = 0;
        (*context).thread_type = ffmpeg::ffi::FF_THREAD_FRAME | ffmpeg::ffi::FF_THREAD_SLICE;
    }
    let hardware = unsafe { attach_hardware(&mut decoder) };
    let mut picture_decoder = decoder.video().map_err(|err| said(&err))?;
    *shared
        .decoded_by
        .lock()
        .unwrap_or_else(|it| it.into_inner()) = match &hardware {
        Some(named) => named.clone(),
        None => picture_decoder
            .codec()
            .map(|codec| codec.name().to_string())
            .unwrap_or_else(|| String::from("software")),
    };
    shared
        .width
        .store(picture_decoder.width(), Ordering::Relaxed);
    shared
        .height
        .store(picture_decoder.height(), Ordering::Relaxed);

    // The soundtrack, its resampler and the device it goes to. All three or
    // none: a film whose device will not open plays silently rather than not
    // at all, and keeps time by the monotonic clock instead.
    let mut sound = sound_found
        .map(|(_, _, parameters)| parameters)
        .and_then(|parameters| Sounding::open(parameters, sound_state));
    let playing = shared.playing.load(Ordering::Acquire);
    if sound.is_some() {
        shared.has_sound.store(true, Ordering::Release);
        clock.keep_time_by(Arc::clone(sound_state), playing);
    } else {
        clock.run(playing);
    }

    let mut captions = Captioning::new(inside, files);
    captions.choose(
        *shared.chosen.lock().unwrap_or_else(|it| it.into_inner()),
        &mut input,
        shared,
    );

    // Where the clock counts from is the first thing really played, not
    // nought: a film whose first frame is stamped half a second in would
    // otherwise be half a second ahead of itself for its whole length.
    let mut base_set = false;
    let mut aiming: Option<f64> = None;
    if from > 0.05 {
        aiming = seek(&mut input, from, &mut picture_decoder, &mut sound, shared);
    }

    let mut packet = ffmpeg::codec::packet::Packet::empty();
    let mut converting = Converting::default();
    let mut at_the_end = false;

    loop {
        // What the window has asked for.
        loop {
            match orders.try_recv() {
                Ok(Order::Stop) | Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                Ok(Order::Seek(to)) => {
                    at_the_end = false;
                    base_set = false;
                    aiming = seek(&mut input, to, &mut picture_decoder, &mut sound, shared);
                    shared.ended.store(false, Ordering::Release);
                }
                Ok(Order::Subtitles(track)) => captions.choose(track, &mut input, shared),
                Ok(Order::AddSubtitles(file)) => captions.add(file),
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }

        let playing = shared.playing.load(Ordering::Acquire);
        if sound.is_some() {
            sound_state.run(playing && !shared.ended.load(Ordering::Acquire));
        }

        if at_the_end {
            // Everything decoded has been handed over; the film is over once
            // the last of it has been shown and heard.
            let frames = shared
                .frames
                .lock()
                .unwrap_or_else(|it| it.into_inner())
                .len();
            let held = if sound.is_some() {
                sound_state.held()
            } else {
                0.0
            };
            if frames == 0 && held < 0.05 {
                shared.ended.store(true, Ordering::Release);
            }
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }

        // Whether there is room for more. Both queues are asked, and either
        // one being hungry is reason enough to read: a file whose picture and
        // sound are a long way apart fills one of them while the other starves,
        // and stopping on the full one is how such a film stutters for ever.
        let frames_held = shared
            .frames
            .lock()
            .unwrap_or_else(|it| it.into_inner())
            .len();
        let sound_held = if sound.is_some() {
            sound_state.held()
        } else {
            0.0
        };
        let picture_full = frames_held >= FRAMES_AHEAD;
        let sound_full = sound.is_none() || sound_held >= SOUND_AHEAD;
        if (picture_full && sound_full) || sound_held > SOUND_MOST {
            std::thread::sleep(Duration::from_millis(4));
            continue;
        }

        match packet.read(&mut input) {
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => {
                // Flush what the decoders are still holding, then wait for it
                // to be shown.
                let _ = picture_decoder.send_eof();
                take_frames(
                    &mut picture_decoder,
                    picture_base,
                    &mut aiming,
                    &mut base_set,
                    clock,
                    shared,
                    &sound,
                    &mut converting,
                );
                at_the_end = true;
                continue;
            }
            Err(_) => {
                // A damaged packet is not the end of a film. Skip it.
                continue;
            }
        }

        let stream = packet.stream();
        if stream == picture_at {
            if picture_decoder.send_packet(&packet).is_ok() {
                take_frames(
                    &mut picture_decoder,
                    picture_base,
                    &mut aiming,
                    &mut base_set,
                    clock,
                    shared,
                    &sound,
                    &mut converting,
                );
            }
        } else if Some(stream) == sound_at {
            if let (Some(sounding), Some(base)) = (sound.as_mut(), sound_base) {
                sounding.take(&packet, base, aiming, &mut base_set, clock, sound_state);
            }
        } else {
            captions.take(&packet, stream, shared);
        }
    }
}

/// Turning a frame the shader cannot read into one it can.
///
/// Two arrangements are drawn directly — three planes and two — and they cover
/// every ordinary film. What is left is ten-bit video, the rarer chroma
/// samplings, and whatever a hardware decoder hands back that is not NV12;
/// those are converted here, once per frame, and the cost is the reason the
/// two common ones are not.
#[derive(Default)]
struct Converting {
    scaler: Option<(Pixel, u32, u32, ffmpeg::software::scaling::Context)>,
}

impl Converting {
    fn drawable(&mut self, frame: ffmpeg::frame::Video) -> Option<ffmpeg::frame::Video> {
        if crate::film::draws(frame.format()) {
            return Some(frame);
        }
        // **libswscale aborts the whole process** — not an error, an
        // `abort()` — when asked for a context whose input format was never
        // worked out, which is what a damaged file's first frame can look
        // like. There is no catching it and no test that survives it, so it is
        // refused here.
        let (width, height) = (frame.width(), frame.height());
        if frame.format() == Pixel::None || width == 0 || height == 0 {
            return None;
        }

        let same = self
            .scaler
            .as_ref()
            .is_some_and(|(format, was_wide, was_tall, _)| {
                *format == frame.format() && *was_wide == width && *was_tall == height
            });
        if !same {
            let made = ffmpeg::software::scaling::Context::get(
                frame.format(),
                width,
                height,
                Pixel::YUV420P,
                width,
                height,
                ffmpeg::software::scaling::Flags::BILINEAR,
            )
            .ok()?;
            self.scaler = Some((frame.format(), width, height, made));
        }
        let (_, _, _, scaler) = self.scaler.as_mut()?;
        let mut out = ffmpeg::frame::Video::empty();
        scaler.run(&frame, &mut out).ok()?;
        // The timestamps and the colour live on the frame rather than in the
        // pixels, and a conversion has none of them until they are carried
        // across. Without this every converted frame is stamped nought and the
        // film plays all at once.
        unsafe {
            if ffmpeg::ffi::av_frame_copy_props(out.as_mut_ptr(), frame.as_ptr()) < 0 {
                return None;
            }
        }
        Some(out)
    }
}

/// Everything about the picture that comes out of one `send_packet`.
#[allow(clippy::too_many_arguments)]
fn take_frames(
    decoder: &mut ffmpeg::decoder::Video,
    base: ffmpeg::Rational,
    aiming: &mut Option<f64>,
    base_set: &mut bool,
    clock: &Arc<Clock>,
    shared: &Arc<Shared>,
    sound: &Option<Sounding>,
    converting: &mut Converting,
) {
    let mut frame = ffmpeg::frame::Video::empty();
    while decoder.receive_frame(&mut frame).is_ok() {
        let stamp = frame.timestamp().or_else(|| frame.pts()).unwrap_or(0);
        let at = seconds_of(stamp, base);

        // Coming out of a seek: the frames between the keyframe and the point
        // asked for are decoded and thrown away, so the film really starts
        // where somebody pointed rather than several seconds before it.
        if let Some(target) = *aiming {
            if at + 0.001 < target && at > target - SEEK_PATIENCE {
                frame = ffmpeg::frame::Video::empty();
                continue;
            }
            *aiming = None;
            shared.seeking.store(false, Ordering::Release);
        }

        // The frame that comes back from a hardware decoder lives on the
        // graphics card; bring it into memory, where the texture is written
        // from. It arrives as NV12 or P010 — two planes rather than three —
        // which `film.rs` draws directly.
        let here = if frame.format() == Pixel::VAAPI {
            match to_memory(&frame) {
                Some(here) => here,
                None => {
                    frame = ffmpeg::frame::Video::empty();
                    continue;
                }
            }
        } else {
            std::mem::replace(&mut frame, ffmpeg::frame::Video::empty())
        };
        // And whatever arrangement it turned out to be, into one the shader
        // reads.
        let Some(ready) = converting.drawable(here) else {
            frame = ffmpeg::frame::Video::empty();
            continue;
        };

        shared.width.store(ready.width(), Ordering::Relaxed);
        shared.height.store(ready.height(), Ordering::Relaxed);
        // A film with no soundtrack has nothing else to set the clock from, so
        // the first frame after a seek does it.
        if !*base_set && sound.is_none() {
            *base_set = true;
            clock.set(at);
        }
        shared.ready.store(true, Ordering::Release);
        shared
            .frames
            .lock()
            .unwrap_or_else(|it| it.into_inner())
            .push_back(Shown { at, frame: ready });
        frame = ffmpeg::frame::Video::empty();
    }
}

/// Put the film at a point: the container's own seek, then everything that was
/// decoded from where it used to be is thrown away.
fn seek(
    input: &mut ffmpeg::format::context::Input,
    to: f64,
    decoder: &mut ffmpeg::decoder::Video,
    sound: &mut Option<Sounding>,
    shared: &Arc<Shared>,
) -> Option<f64> {
    // The ring itself is emptied by `Player::seek_to`, in the instant the
    // press is answered, so that what was already decoded stops being heard
    // before this thread has even noticed the order.
    let stamp = (to * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
    shared.seeking.store(true, Ordering::Release);
    // `..stamp` asks for the keyframe at or before the point, which is the
    // only kind of seek a compressed film really has. Landing exactly is the
    // decoding that follows.
    let landed = input.seek(stamp, ..stamp).is_ok();
    decoder.flush();
    shared
        .frames
        .lock()
        .unwrap_or_else(|it| it.into_inner())
        .clear();
    if let Some(sounding) = sound.as_mut() {
        sounding.decoder.flush();
    }
    if !landed {
        shared.seeking.store(false, Ordering::Release);
        return None;
    }
    Some(to)
}

fn said(err: &ffmpeg::Error) -> String {
    match err {
        ffmpeg::Error::InvalidData => String::from(crate::i18n::text("file-cannot-play")),
        _ => crate::message!("film-cannot-be-played", "why" => err.to_string()),
    }
}

/// What a subtitle stream is called on a menu.
///
/// Its title if it has one, then its language, then its number — in that
/// order, because a maker who wrote a title wrote it to be read and a
/// three-letter language code is better than "Track 2".
fn stream_name(stream: &ffmpeg::Stream, number: usize) -> String {
    let tags = stream.metadata();
    let title = tags.get("title").map(str::trim).filter(|it| !it.is_empty());
    let language = tags
        .get("language")
        .map(str::trim)
        .filter(|it| !it.is_empty() && *it != "und");
    match (title, language) {
        (Some(title), Some(language)) => format!("{title} ({language})"),
        (Some(title), None) => title.to_string(),
        (None, Some(language)) => language.to_string(),
        (None, None) => crate::message!("track-number", "number" => number + 1),
    }
}

// ---- the soundtrack's own half -------------------------------------------

/// The audio decoder, its resampler, the ring it fills and the device that
/// empties it. Kept together because a film either has all four or has none.
struct Sounding {
    decoder: ffmpeg::decoder::Audio,
    resampler: ffmpeg::software::resampling::Context,
    channels: u16,
    /// The device. Never touched again, and never dropped until the film is —
    /// dropping it stops the stream.
    _device: rodio::MixerDeviceSink,
}

impl Sounding {
    fn open(parameters: ffmpeg::codec::Parameters, sound: &Arc<Sound>) -> Option<Sounding> {
        let device = rodio::DeviceSinkBuilder::from_default_device()
            .and_then(|builder| builder.open_stream())
            .ok()?;
        let rate = device.config().sample_rate().get();
        // Two at most. A film's own layout is downmixed to what is asked for
        // here, and asking for eight channels on a machine whose output is a
        // pair of speakers is how a five-channel film loses its dialogue.
        let channels = device.config().channel_count().get().clamp(1, 2);

        let decoder = ffmpeg::codec::context::Context::from_parameters(parameters)
            .ok()?
            .decoder()
            .audio()
            .ok()?;
        let layout = if channels == 1 {
            ffmpeg::ChannelLayout::MONO
        } else {
            ffmpeg::ChannelLayout::STEREO
        };
        // A film's own layout, whatever it is, down to what the device has.
        // The library does a proper downmix, which is the difference between
        // five channels becoming two and five channels becoming a mess.
        let resampler = ffmpeg::software::resampling::Context::get(
            decoder.format(),
            decoder.channel_layout(),
            decoder.rate(),
            ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
            layout,
            rate,
        )
        .ok()?;

        sound.settle(rate, channels);
        device.mixer().add(Bridge {
            sound: Arc::clone(sound),
            block: Vec::with_capacity(BLOCK),
            at: 0,
            generation: sound.generation.load(Ordering::Acquire),
            emitted: 0,
        });
        Some(Sounding {
            decoder,
            resampler,
            channels,
            _device: device,
        })
    }

    /// Decode one packet of sound and put what comes out on the ring.
    fn take(
        &mut self,
        packet: &ffmpeg::codec::packet::Packet,
        base: ffmpeg::Rational,
        aiming: Option<f64>,
        base_set: &mut bool,
        clock: &Arc<Clock>,
        sound: &Arc<Sound>,
    ) {
        if self.decoder.send_packet(packet).is_err() {
            return;
        }
        let mut frame = ffmpeg::frame::Audio::empty();
        while self.decoder.receive_frame(&mut frame).is_ok() {
            let stamp = frame.timestamp().or_else(|| frame.pts()).unwrap_or(0);
            let at = seconds_of(stamp, base);
            // Sound from before the point a seek asked for is thrown away, as
            // the picture from before it is.
            if aiming.is_some_and(|target| at + 0.05 < target) {
                frame = ffmpeg::frame::Audio::empty();
                continue;
            }
            // The clock counts from the first sample that is really played,
            // because the clock *is* the sound: setting it from the picture
            // would leave the two a frame apart after every jump.
            if !*base_set {
                *base_set = true;
                clock.set(at);
            }
            let mut out = ffmpeg::frame::Audio::empty();
            if self.resampler.run(&frame, &mut out).is_err() {
                frame = ffmpeg::frame::Audio::empty();
                continue;
            }
            self.hand_over(&out, sound);
            frame = ffmpeg::frame::Audio::empty();
        }
    }

    fn hand_over(&self, out: &ffmpeg::frame::Audio, sound: &Arc<Sound>) {
        if out.samples() == 0 {
            return;
        }
        // Packed float: one plane, the channels interleaved, which is exactly
        // what the device wants. The plane is allocated to a whole number of
        // blocks and is longer than the samples in it, so the count decides
        // how much is real.
        let wanted = out.samples() * usize::from(self.channels);
        let plane: &[f32] = out.plane(0);
        sound.push(&plane[..wanted.min(plane.len())]);
    }
}

// ---- the subtitles' own half ---------------------------------------------

/// Which subtitle track is being read, and the decoder for it if it is one of
/// the film's own.
struct Captioning {
    inside: Vec<(usize, ffmpeg::Rational, Track)>,
    files: Vec<PathBuf>,
    /// The stream being read and the decoder reading it. `None` for a track
    /// that is a file, or for no track at all.
    reading: Option<(usize, ffmpeg::Rational, ffmpeg::decoder::Subtitle)>,
}

impl Captioning {
    fn new(inside: Vec<(usize, ffmpeg::Rational, Track)>, files: Vec<PathBuf>) -> Captioning {
        Captioning {
            inside,
            files,
            reading: None,
        }
    }

    /// Take a file somebody chose, at the end of the list.
    ///
    /// The order has to match the one the window built its list in — the
    /// film's own tracks first, then the files — because a track is a number
    /// on both sides and nothing carries its name across.
    fn add(&mut self, file: PathBuf) {
        if !self.files.contains(&file) {
            self.files.push(file);
        }
    }

    /// Read a different track, or none.
    ///
    /// A file is read whole, here and now: subtitle files are a few tens of
    /// kilobytes and reading one takes less time than the frame it happens on.
    /// A stream inside the film cannot be — its cues only exist as the film is
    /// decoded — so choosing one clears what is there and fills in as it plays.
    fn choose(
        &mut self,
        track: Option<usize>,
        input: &mut ffmpeg::format::context::Input,
        shared: &Arc<Shared>,
    ) {
        self.reading = None;
        let mut cues = shared.cues.lock().unwrap_or_else(|it| it.into_inner());
        cues.clear();
        let Some(track) = track else {
            return;
        };
        if let Some((index, base, _)) = self.inside.get(track) {
            let Some(stream) = input.streams().find(|stream| stream.index() == *index) else {
                return;
            };
            if let Ok(decoder) =
                ffmpeg::codec::context::Context::from_parameters(stream.parameters())
                    .and_then(|context| context.decoder().subtitle())
            {
                self.reading = Some((*index, *base, decoder));
            }
            return;
        }
        if let Some(file) = self.files.get(track - self.inside.len()) {
            *cues = subtitles::read(file);
        }
    }

    /// One packet, if it belongs to the track being read.
    fn take(
        &mut self,
        packet: &ffmpeg::codec::packet::Packet,
        stream: usize,
        shared: &Arc<Shared>,
    ) {
        let Some((index, base, decoder)) = self.reading.as_mut() else {
            return;
        };
        if stream != *index {
            return;
        }
        let at = seconds_of(packet.pts().unwrap_or(0), *base);
        let mut subtitle = ffmpeg::Subtitle::new();
        if !decoder.decode(packet, &mut subtitle).unwrap_or(false) {
            return;
        }
        let from = at + f64::from(subtitle.start()) / 1000.0;
        let mut to = at + f64::from(subtitle.end()) / 1000.0;
        if subtitle.end() == 0 {
            // No end of its own: the packet's duration, and failing that a few
            // seconds, which is what a line takes to read.
            let held = packet.duration();
            to = from
                + if held > 0 {
                    seconds_of(held, *base)
                } else {
                    4.0
                };
        }
        let mut cues = shared.cues.lock().unwrap_or_else(|it| it.into_inner());
        for rect in subtitle.rects() {
            let raw = unsafe {
                match rect {
                    // Read here rather than through the crate's own accessor,
                    // which assumes the text is valid UTF-8 and is undefined
                    // behaviour where it is not — and a subtitle stream in
                    // somebody's local code page is exactly where it is not.
                    ffmpeg::codec::subtitle::Rect::Ass(ass) => c_text((*ass.as_ptr()).ass),
                    ffmpeg::codec::subtitle::Rect::Text(text) => c_text((*text.as_ptr()).text),
                    // A picture-based subtitle — a DVD or a Blu-ray — is a
                    // bitmap rather than words. Drawing one would want a
                    // second picture pass over the film, and there is no text
                    // in it to draw with the interface's own type.
                    _ => None,
                }
            };
            let Some(raw) = raw else {
                continue;
            };
            let text = subtitles::clean(&dialogue(&raw));
            if text.is_empty() {
                continue;
            }
            cues.push(Cue { from, to, text });
        }
        // Cues arrive as the film is decoded, which is in order — but a seek
        // backwards puts an older one after a newer one, and the search that
        // finds the line on screen takes the first match.
        cues.sort_by(|left, right| left.from.total_cmp(&right.from));
        // A film four hours long with a line a second is fourteen thousand
        // cues, which is nothing; a broken stream that produced one per packet
        // for ever is not, so there is a ceiling.
        if cues.len() > 40_000 {
            let extra = cues.len() - 40_000;
            cues.drain(..extra);
        }
    }
}

/// A C string, as text, without assuming it is valid UTF-8.
unsafe fn c_text(ptr: *const libc_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    Some(
        unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned(),
    )
}

#[allow(non_camel_case_types)]
type libc_char = std::os::raw::c_char;

/// The words out of a SubStation dialogue line.
///
/// The decoder hands over the line in the format the file uses, which is a
/// fixed number of comma-separated fields and then the text — and the text may
/// hold commas of its own, so it is what is *left* rather than a field. Two
/// shapes exist: the one the library produces, with eight fields before the
/// text, and the whole `Dialogue:` line out of an older file, with nine.
fn dialogue(line: &str) -> String {
    let trimmed = line.trim_start();
    let (body, fields) = match trimmed.strip_prefix("Dialogue:") {
        Some(rest) => (rest.trim_start(), 10),
        None => (trimmed, 9),
    };
    // Not a dialogue line at all — a plain text subtitle — so it is all text.
    if body.matches(',').count() < fields - 1 {
        return line.to_string();
    }
    body.splitn(fields, ',').last().unwrap_or(line).to_string()
}

// ---- the hardware --------------------------------------------------------

/// Point the decoder at this machine's video hardware, if it has any that will
/// take this film.
///
/// Returns what it is called, for the details pane, or `None` where the film
/// will be decoded by the processor — which is not a failure and is never
/// reported as one. A machine with no VA-API driver, a codec the card does not
/// implement, a film in a profile it does not implement: all three end here
/// the same way, and all three play.
///
/// `VIDEONSOLE_HWACCEL=off` turns it off, which is the first thing to try when
/// a particular film looks wrong — a driver that decodes a stream incorrectly
/// is a real and common thing, and this is how to tell that apart from the
/// file being damaged.
unsafe fn attach_hardware(decoder: &mut ffmpeg::codec::decoder::Decoder) -> Option<String> {
    use ffmpeg::ffi;

    let asked = std::env::var("VIDEONSOLE_HWACCEL").unwrap_or_default();
    if asked.eq_ignore_ascii_case("off") || asked.eq_ignore_ascii_case("no") {
        return None;
    }

    unsafe {
        let context = decoder.as_mut_ptr();
        // Before `avcodec_open2` the context has no codec of its own yet, so
        // the decoder for this stream's id is what is asked about.
        let codec = ffi::avcodec_find_decoder((*context).codec_id);
        if codec.is_null() {
            return None;
        }

        // Does this decoder have a hardware path that takes a device — which
        // is the kind that decodes on the card and hands the frame back — and
        // is that path VA-API?
        let mut supported = false;
        let mut number = 0;
        loop {
            let config = ffi::avcodec_get_hw_config(codec, number);
            if config.is_null() {
                break;
            }
            let methods = (*config).methods;
            if methods & ffi::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32 != 0
                && (*config).device_type == ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI
            {
                supported = true;
                break;
            }
            number += 1;
        }
        if !supported {
            return None;
        }

        let mut device = std::ptr::null_mut();
        let opened = ffi::av_hwdevice_ctx_create(
            &mut device,
            ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
            std::ptr::null(),
            std::ptr::null_mut(),
            0,
        );
        if opened < 0 || device.is_null() {
            return None;
        }
        // With a device on the context, the library's own format chooser picks
        // the hardware pixel format, so there is no callback to install.
        (*context).hw_device_ctx = ffi::av_buffer_ref(device);
        ffi::av_buffer_unref(&mut device);
        if (*context).hw_device_ctx.is_null() {
            return None;
        }
        Some(String::from("VA-API"))
    }
}

/// Bring a frame off the graphics card into memory.
///
/// A hardware decoder hands back a handle rather than pixels. The copy is what
/// makes the frame drawable, and it is the one cost of this path — the decode
/// itself is free, and on a large film it is by far the larger half.
fn to_memory(frame: &ffmpeg::frame::Video) -> Option<ffmpeg::frame::Video> {
    use ffmpeg::ffi;
    let mut here = ffmpeg::frame::Video::empty();
    unsafe {
        if ffi::av_hwframe_transfer_data(here.as_mut_ptr(), frame.as_ptr(), 0) < 0 {
            return None;
        }
        // The timestamps and the colour are on the hardware frame, and the
        // copy has none of them until they are carried across. Without this
        // every frame arrives at time nought and the film plays at once.
        if ffi::av_frame_copy_props(here.as_mut_ptr(), frame.as_ptr()) < 0 {
            return None;
        }
    }
    Some(here)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(name: &str, from_a_file: bool, forced: bool) -> Track {
        Track {
            name: String::from(name),
            from_a_file,
            forced,
        }
    }

    /// A forced track is the one the film cannot be watched without.
    #[test]
    fn a_forced_track_is_shown_without_being_asked_for() {
        let tracks = vec![
            track("English", false, false),
            track("Signs", false, true),
            track("en", true, false),
        ];
        assert_eq!(worth_showing(&tracks), Some(1));
    }

    /// And a file somebody put beside the film, which nobody does by accident.
    #[test]
    fn a_file_beside_the_film_is_shown_without_being_asked_for() {
        let tracks = vec![track("English", false, false), track("en", true, false)];
        assert_eq!(worth_showing(&tracks), Some(1));
    }

    /// The one this is really guarding against: half the films anybody owns
    /// carry an English track inside them, and turning subtitles on for every
    /// one of them is what people go looking for a setting to stop.
    #[test]
    fn an_ordinary_track_inside_the_film_is_left_off() {
        let tracks = vec![
            track("English", false, false),
            track("Polish", false, false),
        ];
        assert_eq!(worth_showing(&tracks), None);
        assert_eq!(worth_showing(&[]), None);
    }

    #[test]
    fn a_stamp_becomes_seconds_in_the_stream_s_own_units() {
        let base = ffmpeg::Rational::new(1, 1000);
        assert!((seconds_of(1500, base) - 1.5).abs() < 1e-9);
        let base = ffmpeg::Rational::new(1001, 30000);
        assert!((seconds_of(30000, base) - 1001.0).abs() < 1e-6);
    }

    /// The one that eats a subtitle's own commas if it is written with a fixed
    /// field count.
    #[test]
    fn a_dialogue_line_keeps_the_commas_in_its_text() {
        assert_eq!(
            dialogue("0,0,Default,,0,0,0,,Well, no, not really"),
            "Well, no, not really"
        );
        assert_eq!(
            dialogue("Dialogue: 0,0:00:01.50,0:00:03.00,Default,,0,0,0,,Hello, there"),
            "Hello, there"
        );
    }

    #[test]
    fn a_plain_text_subtitle_is_all_text() {
        assert_eq!(dialogue("Just some words"), "Just some words");
    }

    /// Silence must not move the clock: the picture waits with the sound
    /// rather than running away from it.
    #[test]
    fn an_empty_ring_hands_over_silence_and_does_not_count_it() {
        let sound = Arc::new(Sound::new());
        let mut bridge = Bridge {
            sound: Arc::clone(&sound),
            block: Vec::new(),
            at: 0,
            generation: 0,
            emitted: 0,
        };
        for _ in 0..64 {
            assert_eq!(bridge.next(), Some(0.0));
        }
        assert_eq!(sound.seconds(), 0.0);
    }

    #[test]
    fn samples_handed_over_are_what_the_clock_counts() {
        let sound = Arc::new(Sound::new());
        sound.push(&vec![0.5; 200]);
        let mut bridge = Bridge {
            sound: Arc::clone(&sound),
            block: Vec::new(),
            at: 0,
            generation: 0,
            emitted: 0,
        };
        for _ in 0..100 {
            bridge.next();
        }
        // A hundred samples of two channels is fifty frames.
        assert_eq!(sound.played.load(Ordering::Relaxed), 50);
    }

    /// A seek throws away what the device was about to play, or a jump is
    /// heard as a fragment of where the film used to be.
    #[test]
    fn a_seek_drops_what_the_bridge_was_holding() {
        let sound = Arc::new(Sound::new());
        sound.push(&vec![0.5; 512]);
        let mut bridge = Bridge {
            sound: Arc::clone(&sound),
            block: Vec::new(),
            at: 0,
            generation: 0,
            emitted: 0,
        };
        assert_eq!(
            bridge.next(),
            Some(0.5),
            "at full volume, the sample as it was stored"
        );
        sound.reset();
        assert_eq!(bridge.next(), Some(0.0), "nothing left to play");
        assert_eq!(sound.played.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn volume_is_squared_and_mute_is_silence() {
        let sound = Sound::new();
        sound.set_volume(0.5);
        assert!((f32::from_bits(sound.volume.load(Ordering::Relaxed)) - 0.25).abs() < 1e-6);
        sound.set_volume(0.0);
        assert_eq!(f32::from_bits(sound.volume.load(Ordering::Relaxed)), 0.0);
        sound.set_volume(f32::NAN);
        assert_eq!(f32::from_bits(sound.volume.load(Ordering::Relaxed)), 0.0);
    }

    /// The clock with no sound behind it is the wall clock, and it stops.
    #[test]
    fn a_silent_film_keeps_time_by_the_wall_and_a_pause_stops_it() {
        let clock = Clock::new(10.0);
        clock.run(false);
        let at = clock.at();
        std::thread::sleep(Duration::from_millis(20));
        assert!(
            (clock.at() - at).abs() < 0.005,
            "a paused clock moved from {at} to {}",
            clock.at()
        );
        clock.run(true);
        std::thread::sleep(Duration::from_millis(20));
        assert!(clock.at() > at, "and starts again");
    }

    /// The one the nested harness found: a device that takes samples faster
    /// than it plays them must not carry the film along with it.
    #[test]
    fn a_sound_card_that_runs_fast_does_not_take_the_film_with_it() {
        let sound = Arc::new(Sound::new());
        let clock = Clock::new(0.0);
        clock.keep_time_by(Arc::clone(&sound), true);

        // Ten seconds of samples handed over in no time at all, which is
        // exactly what ALSA's `null` device does.
        sound
            .played
            .store(u64::from(sound.rate()) * 10, Ordering::Relaxed);
        let at = clock.at();
        assert!(
            at <= SOUND_MAY_LEAD + 0.05,
            "the film ran away with the device, to {at}"
        );

        // And the clock still only ever goes forwards.
        std::thread::sleep(Duration::from_millis(30));
        assert!(clock.at() >= at, "it went backwards");
    }

    /// The guard must not slow an honest device down: a card that has taken
    /// less than real time's worth is what the clock reads, unchanged.
    #[test]
    fn an_honest_sound_card_is_what_the_clock_reads() {
        let sound = Arc::new(Sound::new());
        let clock = Clock::new(4.0);
        clock.keep_time_by(Arc::clone(&sound), true);
        // A tenth of a second of samples, well under any wall time that has
        // passed plus the slack.
        sound
            .played
            .store(u64::from(sound.rate()) / 10, Ordering::Relaxed);
        let at = clock.at();
        assert!(
            (at - 4.1).abs() < 0.02,
            "the sound card said 0.1s past 4.0 and the clock said {at}"
        );
    }

    #[test]
    fn a_seek_puts_the_clock_where_it_was_asked_for() {
        let clock = Clock::new(0.0);
        clock.set(120.0);
        assert!((clock.at() - 120.0).abs() < 0.01);
    }
}
