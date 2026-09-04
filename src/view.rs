//! What is being watched, and what every control does to it.
//!
//! Nothing here draws. `draw.rs` reads this and puts it on the screen, and the
//! window in `main.rs` hands it actions — so what a button does is decided in
//! one place, whether the button was on a pad, a keyboard or a mouse.
//!
//! **The one shape everything else follows.** A film is drawn over the frame
//! the toolkit composed, so nothing the toolkit draws can appear on top of one
//! (see `film.rs`). There are two answers to that here, and which one is in
//! force is the difference between watching a film and looking at one:
//!
//! * **A film that is playing has the whole window**, on black, and every
//!   panel over it is a hole cut in the picture. That is why the chrome over a
//!   playing film is *panels* — the transport, the head and the details all
//!   get one — and why they slide away rather than fading: a hole cannot fade,
//!   and a word with no panel behind it would be a rectangle of wallpaper cut
//!   out of the picture.
//! * **A film that is stopped is a picture on a page.** The stage gives up the
//!   room the page's furniture wants and the film sits in what is left, which
//!   is what this application did for every film at first.
//!
//! The one number that says which is [`View::over_now`], and it is read off
//! the stage rather than animated beside it. A panel over a film is a hole in
//! it, and a hole that moved at its own speed would open a seam along its
//! edge for as long as the two disagreed.
//!
//! When a dialog or the file chooser takes over, the film fades out entirely.

use std::path::{Path, PathBuf};

use lxb_render::{ContextMenu, Dialog, Entry as MenuEntry, Files, Selection as Light, Spot};
use lxb_toolkit::{
    input::Action,
    menu,
    metrics::Metric,
    motion::{self, spring},
    picker::Selection,
    sound::Sound,
    typography::Text,
};

use crate::library::{Folder, Order};
use crate::player::Player;
use crate::resume::Resume;

/// Pointer targets. Kept clear of the legend's, which count from `0x8000`.
pub const CARD: u32 = 0x100;
pub const STAGE: u32 = 0x20;
pub const SCROLL_BAR: u32 = 0x21;
pub const GROOVE: u32 = 0x22;
pub const PLAY: u32 = 0x23;
pub const VOLUME: u32 = 0x24;

/// How long the transport stays up after the last thing anybody did.
///
/// Long enough to read the time and reach for the groove, short enough that it
/// is out of the way before it becomes part of the picture. It never goes away
/// at all while the film is paused — a stopped film with no controls on it is a
/// photograph.
const TRANSPORT_HELD: f32 = 4.0;

/// How far one press of a direction moves through a film, and how far it moves
/// once it has been held.
///
/// Three sizes rather than a ramp, because a seek is a jump and a jump has to
/// be a number somebody can predict. Ten seconds is the one everybody knows;
/// the larger two are what a held direction becomes, so crossing an hour does
/// not take a hundred and eighty presses.
const STEPS: [(f32, f64); 3] = [(0.0, 10.0), (1.2, 30.0), (2.8, 60.0)];

/// How fast the triggers scan, at full pull, in seconds of film per second.
const SCAN_RATE: f64 = 90.0;

/// How often a scan really seeks.
///
/// Every frame would be sixty seeks a second, each flushing a decoder; less
/// often than this and the picture stops keeping up with the thumb.
const SCAN_EVERY: f32 = 0.12;

/// How long a push is believed after the last one arrived. Longer than the
/// repeat delay before the second action of a hold, or the step would drop
/// back to ten seconds a third of a second into every hold.
const HELD_PATIENCE: f32 = 0.42;

/// How much one press changes the volume.
const VOLUME_STEP: f32 = 0.05;

/// How often the place a film was left is written down while it plays.
///
/// Written as it goes rather than only when it is left, because a television
/// box is switched off at the wall: an application that only recorded the
/// position on the way out would record nothing at all in the case that
/// matters most.
const NOTE_EVERY: f32 = 15.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Grid,
    Player,
}

/// Which way a change of page is going, if one is.
///
/// A film opens **out of the card it was pressed on** and shrinks back into
/// it, and one number drives the whole of both: where the picture is, how
/// round its corners are, how far the wall of films behind it has stepped
/// back, and when the panels arrive. Two clocks would be a picture that landed
/// before its own chrome, which is what this had before there was a number at
/// all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Going {
    Nowhere,
    /// Out of the card and on to the screen.
    In,
    /// Back into the card it came out of.
    Out,
}

/// How much of the way back into its card a film dissolves over.
///
/// It has to be there for most of the way — a film that faded at the press
/// would land nothing on the card — and gone by the time it arrives, so what
/// the card is left holding is its own poster rather than a cut from whatever
/// frame the film happened to be on.
const LANDS_OVER: f32 = 0.35;

/// What a row of the Options menu does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    Fill,
    FromTheStart,
    Info,
    RunTheFolder,
    Subtitles(Option<usize>),
    /// Read a subtitle file that is not beside the film and not in it.
    AddSubtitles,
    OpenFolder,
    UpAFolder,
    ShowHidden,
    SortBy(Order),
    BackToGrid,
}

/// What the file chooser was opened for.
///
/// One chooser answers both questions, and a path that came back from asking
/// for a subtitle must not be browsed as a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Asking {
    AFolder,
    ASubtitle,
}

/// A menu that has been asked for and not yet raised.
///
/// Every name on it still has to be cut to the panel, which cannot be done
/// until there is something that can measure a word. See
/// [`View::settle_menu`].
struct Asked {
    title: String,
    rows: Vec<(Line, Command)>,
    anchor: [f32; 4],
    window: usize,
}

/// One row of a menu, before it is a row of a menu.
///
/// `lxb_render::Entry` is built and never read again, and the words on it have
/// to be cut to the panel — which cannot be done where a menu is decided,
/// because nothing there can measure a word. So a menu is described here and
/// built in [`View::settle_menu`].
#[derive(Debug, Clone, Default)]
struct Line {
    label: String,
    detail: Option<String>,
    glyph: &'static str,
    aside: Option<&'static str>,
    group: u8,
}

impl Line {
    fn new(label: impl Into<String>, glyph: &'static str) -> Line {
        Line {
            label: label.into(),
            glyph,
            ..Line::default()
        }
    }

    fn detail(mut self, detail: impl Into<String>) -> Line {
        self.detail = Some(detail.into());
        self
    }

    fn ticked(mut self, ticked: bool) -> Line {
        self.aside = ticked.then_some("chosen");
        self
    }

    fn group(mut self, group: u8) -> Line {
        self.group = group;
        self
    }

    /// What the toolkit needs to know to lay this row out — and nothing about
    /// what it says, which is exactly why the panel's size can be worked out
    /// before the words are cut to it.
    fn shape(&self) -> menu::Row {
        menu::Row {
            stacked: self.detail.is_some(),
            stamp: false,
            aside: self.aside.is_some(),
            reading: false,
            group: self.group,
            lines: 1,
            detail_lines: 0,
        }
    }
}

pub struct View {
    pub folder: Folder,
    pub order: Order,
    pub hidden: bool,
    /// Where in `folder.entries` the light is, in the grid and in the player
    /// alike — the two are one selection, which is what makes leaving the
    /// player put the grid back where it was.
    pub cursor: usize,
    pub mode: Mode,

    /// Worked out while drawing, kept because acting on a direction needs it.
    pub columns: usize,
    pub visible_rows: f32,
    pub scroll: f32,
    pub scroll_speed: f32,

    pub light: Light,
    pub pressing: lxb_render::Pressing,

    /// The film being played, if one is. There is only ever one: a second
    /// decoder is a second soundtrack.
    pub player: Option<Player>,
    pub resume: Resume,
    /// Whether the film now open was resumed rather than started, so the page
    /// can say so once and then stop saying it.
    pub resumed_at: Option<f64>,
    since_noted: f32,

    pub volume: f32,
    pub muted: bool,
    /// The film fills the stage and is cropped, rather than fitting inside it
    /// with bars. The one thing a player has instead of a zoom ladder: a film
    /// has one right size and the only question is what to do with the shape
    /// left over.
    pub fill: bool,
    /// Play every film in this folder, one after another.
    pub run_the_folder: bool,

    /// How much of the transport is on the screen, and how long it has left.
    pub transport: f32,
    pub transport_left: f32,
    /// How hard the triggers are pulled, right less left.
    pull: f32,
    scanning: bool,
    scan_to: f64,
    since_scan: f32,
    /// Which direction is being held and for how long, so a held seek grows.
    held: f64,
    held_for: f32,
    since_held: f32,

    pub info: bool,
    pub info_out: f32,

    pub menu: ContextMenu,
    pub commands: Vec<Command>,
    /// A menu that has been asked for and not yet raised.
    ///
    /// It waits one step, until `draw.rs` has something that can measure a
    /// word, because every name on a menu has to be cut to the panel before
    /// it is handed over: `lxb-render` wraps a label only when the row it is
    /// on can hold more than one line, and leaves a single line unclipped and
    /// running off both edges of the panel. See [`View::settle_menu`].
    asked: Option<Asked>,
    /// The shape of the menu now open, its anchor and how many rows it shows
    /// — everything needed to work out where the panel ended up, which is
    /// what tells the film where to leave a hole for it.
    menu_shape: Vec<menu::Row>,
    menu_anchor: [f32; 4],
    menu_window: usize,
    /// Which row the menu is showing from. The toolkit keeps its own and does
    /// not hand it out, so this mirrors it — see [`View::follow_the_menu`].
    menu_first: usize,
    pub dialog: Dialog,
    pub files: Files,
    /// What the chooser was opened for. A folder to browse and a subtitle to
    /// read come back through the same answer.
    asking: Asking,

    going: Going,
    /// How much of a change of page is still to come: 1 at the press, 0 when
    /// the picture lands. See [`Going`] and [`View::grown`].
    to_go: f32,
    /// The far end of a change of page: where the picture is going, or where
    /// it is coming back from.
    ///
    /// **Followed rather than read, on the way in.** What the stage is heading
    /// for changes the moment the film starts playing — it stands aside for
    /// the page's furniture until then and takes the whole window afterwards
    /// — and a crossing that read that answer straight would jump by a tenth
    /// of the window halfway through. Following it also keeps the one rule the
    /// poster standing in on the stage depends on: while there is no picture,
    /// the stage is still standing aside, and so is clear of the panels the
    /// toolkit cannot draw on top of it.
    ///
    /// Frozen on the way out, where the picture is coming back from a place
    /// that has stopped existing.
    crossing_far: [f32; 4],

    /// The rectangle the film may occupy.
    ///
    /// **Set rather than sprung.** Everything it is worked out from is already
    /// an eased number, so a spring on top of them bought nothing but lag —
    /// and the lag was the bug: the details pane arrives on a clock of its own
    /// and the stage crawled after it, which left the film drawn over a pane
    /// it is supposed to stand beside. The one thing that moves it abruptly is
    /// a change of page, and that sets it too.
    pub stage: [f32; 4],
    pub stage_known: bool,
    /// How much of the window is the film's own black rather than the shell's
    /// wallpaper. Eased on its own rather than read off the stage: the film
    /// grows into a window that is already black, and the two crossing over
    /// is a cross-fade rather than a seam.
    black: f32,
    /// The window, as of the last measurement. Kept because a menu is opened
    /// by an action rather than while drawing, and it has to know where the
    /// stage is about to stand aside to before the stage has moved.
    window: [f32; 2],
    /// How much room the transport asks of the stage, as of the last frame.
    transport_room: f32,
    /// How round the picture on a card is, as of the last measurement. Kept
    /// for the same reason `window` is: what the film is drawn with has to be
    /// answered where there is no `Geometry` to hand.
    frame_radius: f32,

    /// The film's own size on a screen, and how large it is being drawn.
    pub shown: [f32; 2],
    /// How much of the film is on the screen.
    pub showing: f32,

    pub note: Option<String>,
    pub quit: bool,
    /// This was started on one film rather than on a folder — a file manager
    /// handing over a double-click, rather than somebody opening the browser
    /// to look through their films. Back closes the application while it
    /// holds. See [`View::closes_on_back`].
    opened_on_a_film: bool,
    /// The folder this walk is rooted at, which is the one it was opened on.
    /// Back walks *up* to it and closes *at* it, rather than carrying on
    /// through the home directory and out to the root of the disk.
    top: PathBuf,
}

impl View {
    pub fn new(at: &Path, order: Order, hidden: bool) -> View {
        let (folder, cursor, mode) = open_at(at, order, hidden);
        let folder_path = folder.path.clone();
        View {
            folder,
            order,
            hidden,
            cursor,
            mode: Mode::Grid,
            columns: 4,
            visible_rows: 3.0,
            scroll: 0.0,
            scroll_speed: 0.0,
            light: Light::default(),
            pressing: lxb_render::Pressing::default(),
            player: None,
            resume: Resume::load(),
            resumed_at: None,
            since_noted: 0.0,
            volume: 1.0,
            muted: false,
            fill: false,
            run_the_folder: false,
            transport: 1.0,
            transport_left: TRANSPORT_HELD,
            pull: 0.0,
            scanning: false,
            scan_to: 0.0,
            since_scan: 0.0,
            held: 0.0,
            held_for: 0.0,
            since_held: f32::MAX,
            info: false,
            info_out: 0.0,
            menu: ContextMenu::default(),
            commands: Vec::new(),
            asked: None,
            menu_shape: Vec::new(),
            menu_anchor: [0.0; 4],
            menu_window: 0,
            menu_first: 0,
            dialog: Dialog::default(),
            files: Files::default(),
            asking: Asking::AFolder,
            going: Going::Nowhere,
            to_go: 0.0,
            crossing_far: [0.0; 4],
            stage: [0.0; 4],
            stage_known: false,
            black: 0.0,
            // Until the first measurement. A menu raised before a single frame
            // has been drawn would otherwise be anchored at the origin; this is
            // only ever wrong for that one frame.
            window: [1280.0, 800.0],
            transport_room: 0.0,
            frame_radius: 0.0,
            shown: [0.0; 2],
            showing: 0.0,
            note: None,
            quit: false,
            opened_on_a_film: mode == Mode::Player,
            top: folder_path,
        }
    }

    /// Start playing whatever the application was handed, once there is a
    /// window to play it in.
    ///
    /// Separate from [`View::new`] because opening a film starts a thread and
    /// a sound device, and `new` runs before there is a screen — a film that
    /// began playing while the window was still being made would be a
    /// soundtrack over a black rectangle.
    pub fn begin(&mut self) {
        if self.opened_on_a_film {
            self.show_the_film();
        }
    }

    pub fn current(&self) -> Option<&crate::library::Entry> {
        self.folder.entries.get(self.cursor)
    }

    /// The film being watched, which is only a film in the player: in the grid
    /// the light may well be on a folder.
    pub fn film(&self) -> Option<&Path> {
        let entry = self.current()?;
        (!entry.is_folder()).then_some(entry.path.as_path())
    }

    /// The films worth knowing the facts of: everything on the screen, and a
    /// row either side of it.
    pub fn wanted(&self, first: usize, last: usize) -> Vec<PathBuf> {
        let mut wanted = Vec::new();
        if let Some(path) = self.film() {
            wanted.push(path.to_path_buf());
        }
        for entry in self
            .folder
            .entries
            .iter()
            .take(last.min(self.folder.entries.len()))
            .skip(first)
        {
            if !entry.is_folder() && !wanted.contains(&entry.path) {
                wanted.push(entry.path.clone());
            }
        }
        wanted
    }

    /// Is something the toolkit drew standing over the page?
    pub fn overlaid(&self) -> bool {
        self.menu.is_open() || self.dialog.is_open() || self.files.busy()
    }

    /// The overlays that take the whole screen rather than a corner of it.
    fn takes_over(&self) -> bool {
        self.dialog.is_open() || self.files.busy()
    }

    /// Whether the film has the whole window and everything else stands over
    /// it, rather than beside it.
    ///
    /// A film that is *playing*. One that is stopped is a picture on a page,
    /// and the page's furniture takes its room from the stage — which is what
    /// this application did for every film at first, and still does for every
    /// film somebody is looking at rather than watching.
    pub fn over_the_film(&self) -> bool {
        if self.mode != Mode::Player || self.takes_over() {
            return false;
        }
        self.player.as_ref().is_some_and(|player| {
            player.playing() && player.ready() && !player.ended() && player.trouble().is_none()
        })
    }

    /// A rectangle as it stands once a menu has pushed the page back.
    ///
    /// `Ui::recede` does this to everything the toolkit drew when a menu
    /// opens. The film is not one of those things, and neither are the holes
    /// cut in it for the panels that *are* — so it is done to them here, by
    /// the same arithmetic, or the picture and the panels over it come apart
    /// by a tenth of the window every time somebody presses Options.
    pub fn stepped_back(&self, rect: [f32; 4]) -> [f32; 4] {
        let out = self.menu.travelled().clamp(0.0, 1.0);
        if out <= 0.0 {
            return rect;
        }
        let factor = 1.0 - menu::DEPTH * out;
        let about = self.menu_anchor;
        let offset = [
            (about[0] + about[2] * 0.5) * (1.0 - factor),
            (about[1] + about[3] * 0.5) * (1.0 - factor),
        ];
        [
            rect[0] * factor + offset[0],
            rect[1] * factor + offset[1],
            rect[2] * factor,
            rect[3] * factor,
        ]
    }

    /// How much light is left in the page while a menu is opening over it.
    pub fn dimmed(&self) -> f32 {
        1.0 - (1.0 - menu::DIM) * self.menu.travelled().clamp(0.0, 1.0)
    }

    /// How much of the window is the film's own black rather than the shell's
    /// wallpaper.
    ///
    /// Not the film's opacity, and not the stage: the black is there while a
    /// film is seeking or between two frames, when there may be no picture to
    /// draw at all.
    pub fn blackout(&self) -> f32 {
        if self.mode != Mode::Player && !self.leaving() {
            return 0.0;
        }
        self.black
    }

    /// Whether the transport, the head and the legend are on the screen.
    ///
    /// The one thing that decides how much room the film gets, so it is one
    /// question rather than three that could disagree.
    pub fn transport_wanted(&self) -> bool {
        if self.mode != Mode::Player {
            return true;
        }
        if self.overlaid() || self.info {
            return true;
        }
        let player = self.player.as_ref();
        // A stopped film with no controls on it is a photograph, and a film
        // that has not arrived yet is a black screen somebody would be right
        // to think had crashed.
        let stopped = player.is_none_or(|player| {
            !player.playing() || !player.ready() || player.ended() || player.trouble().is_some()
        });
        stopped || self.transport_left > 0.0
    }

    /// Something happened, so the transport comes back and starts its wait
    /// again.
    fn woken(&mut self) {
        self.transport_left = TRANSPORT_HELD;
    }

    // ---- what the controls do -------------------------------------------

    pub fn act(&mut self, action: Action) -> Option<Sound> {
        // Every panel the toolkit owns reads its own actions first, and closes
        // one thing at a time, exactly as it does under `lxb-app`.
        if self.files.busy() {
            return self.files.act(action);
        }
        if self.dialog.is_open() {
            return self.dialog_act(action);
        }
        if self.menu.is_open() {
            return self.menu_act(action);
        }
        self.woken();
        match self.mode {
            Mode::Grid => self.grid_act(action),
            Mode::Player => self.player_act(action),
        }
    }

    fn dialog_act(&mut self, action: Action) -> Option<Sound> {
        match action {
            Action::Left => {
                self.dialog.step(-1);
                Some(Sound::Move)
            }
            Action::Right => {
                self.dialog.step(1);
                Some(Sound::Move)
            }
            Action::Accept => {
                self.dialog.press();
                self.dialog.close();
                Some(Sound::Press)
            }
            Action::Back => {
                self.dialog.close();
                Some(Sound::Back)
            }
            _ => None,
        }
    }

    fn menu_act(&mut self, action: Action) -> Option<Sound> {
        match action {
            Action::Up => {
                self.menu.step(-1);
                Some(Sound::Move)
            }
            Action::Down => {
                self.menu.step(1);
                Some(Sound::Move)
            }
            Action::Accept => {
                self.menu.press();
                let chosen = self.commands.get(self.menu.selected()).copied();
                self.menu.close();
                if let Some(command) = chosen {
                    return self.run(command).or(Some(Sound::Press));
                }
                Some(Sound::Press)
            }
            Action::Back | Action::Menu => {
                self.menu.close();
                Some(Sound::Back)
            }
            _ => None,
        }
    }

    fn grid_act(&mut self, action: Action) -> Option<Sound> {
        let count = self.folder.entries.len();
        match action {
            Action::Left | Action::Right | Action::Up | Action::Down => {
                if count == 0 {
                    return None;
                }
                let columns = self.columns.max(1) as isize;
                let step = match action {
                    Action::Left => -1,
                    Action::Right => 1,
                    Action::Up => -columns,
                    _ => columns,
                };
                let at = self.cursor as isize + step;
                // A grid stops at its edges rather than wrapping: wrapping a
                // row takes the light to the far side of the screen, which is
                // exactly where somebody pressing Right was not looking.
                if at < 0 || at >= count as isize {
                    return None;
                }
                self.cursor = at as usize;
                Some(Sound::Move)
            }
            Action::Previous | Action::Next => {
                if count == 0 {
                    return None;
                }
                let page = (self.columns.max(1) as f32 * self.visible_rows).max(1.0) as isize;
                let step = if action == Action::Next { page } else { -page };
                let at = (self.cursor as isize + step).clamp(0, count as isize - 1);
                if at as usize == self.cursor {
                    return None;
                }
                self.cursor = at as usize;
                Some(Sound::Move)
            }
            Action::Accept => self.enter(),
            Action::Back => self.leave(),
            Action::Menu => {
                self.open_menu();
                Some(Sound::Press)
            }
            Action::Submit => {
                // Play the folder through, from wherever the light is — and
                // for a folder, from the first film in it.
                if self.folder.films() == 0 {
                    return Some(Sound::Error);
                }
                if self.current().is_some_and(|entry| entry.is_folder()) {
                    let Some(first) = self
                        .folder
                        .entries
                        .iter()
                        .position(|entry| !entry.is_folder())
                    else {
                        return Some(Sound::Error);
                    };
                    self.cursor = first;
                }
                self.run_the_folder = true;
                self.show_the_film();
                Some(Sound::Press)
            }
        }
    }

    fn player_act(&mut self, action: Action) -> Option<Sound> {
        match action {
            // **Left and right move through the film; up and down change how
            // loud it is.** Unambiguous, and the two things a hand reaches for
            // without looking. Nothing has to be switched on and nothing
            // changes meaning.
            Action::Left | Action::Right => {
                let back = action == Action::Left;
                self.held = if back { -1.0 } else { 1.0 };
                self.since_held = 0.0;
                let step = step_now(self.held_for);
                self.seek_by(if back { -step } else { step })
            }
            Action::Up => self.change_volume(VOLUME_STEP),
            Action::Down => self.change_volume(-VOLUME_STEP),
            Action::Previous => self.step_film(-1),
            Action::Next => self.step_film(1),
            Action::Accept => self.play_pause(),
            Action::Back => self.leave(),
            Action::Menu => {
                self.open_menu();
                Some(Sound::Press)
            }
            Action::Submit => {
                self.run_the_folder = !self.run_the_folder;
                Some(Sound::Press)
            }
        }
    }

    pub fn play_pause(&mut self) -> Option<Sound> {
        let Some(player) = self.player.as_ref() else {
            return Some(Sound::Error);
        };
        // A film that has run out is started again rather than un-paused:
        // there is nothing left to carry on with, and the button that says
        // Play must do something when it is pressed.
        if player.ended() {
            player.seek_to(0.0);
            player.play(true);
            return Some(Sound::Press);
        }
        let playing = !player.playing();
        player.play(playing);
        self.woken();
        Some(Sound::Press)
    }

    /// Change the volume by an amount, which is what a direction, a wheel and
    /// the `+` and `-` keys all mean.
    pub fn change_volume_by(&mut self, by: f32) -> Option<Sound> {
        self.woken();
        self.change_volume(by)
    }

    fn change_volume(&mut self, by: f32) -> Option<Sound> {
        let was = self.volume;
        self.volume = (self.volume + by).clamp(0.0, 1.0);
        // Turning it up un-mutes: nobody presses volume-up to stay silent.
        if by > 0.0 && self.muted {
            self.muted = false;
        }
        self.push_volume();
        if (self.volume - was).abs() < 0.0001 {
            return Some(Sound::Error);
        }
        Some(Sound::Move)
    }

    pub fn mute(&mut self) -> Option<Sound> {
        self.muted = !self.muted;
        self.push_volume();
        self.woken();
        Some(Sound::Press)
    }

    fn push_volume(&self) {
        if let Some(player) = self.player.as_ref() {
            player.set_volume(self.volume, self.muted);
        }
    }

    /// Move through the film by an amount, from wherever it is now.
    pub fn seek_by(&mut self, seconds: f64) -> Option<Sound> {
        let Some(player) = self.player.as_ref() else {
            return Some(Sound::Error);
        };
        if !player.ready() {
            return Some(Sound::Error);
        }
        let length = player.length();
        let to = player.at() + seconds;
        // Seeking past the end is seeking to the end, and seeking before the
        // beginning is seeking to it. Neither is an error worth a noise — it
        // is what the film has to say about where it ends.
        let to = to.clamp(0.0, if length > 0.0 { length } else { to.max(0.0) });
        player.seek_to(to);
        self.woken();
        Some(Sound::Move)
    }

    /// Go to a point in the film, named in seconds. What `--shot` uses, and
    /// nothing else — every control says either an amount to move by or a
    /// fraction of the way through.
    pub fn seek_to_seconds(&mut self, to: f64) -> Option<Sound> {
        let Some(player) = self.player.as_ref() else {
            return Some(Sound::Error);
        };
        player.seek_to(to.max(0.0));
        self.woken();
        Some(Sound::Move)
    }

    /// Keep the transport on the screen, whatever it would otherwise do.
    ///
    /// For `--shot`, which is photographing a page rather than watching a
    /// film: without it the controls fade out four seconds in and every
    /// screenshot of the player is a film and nothing else.
    pub fn hold_the_transport(&mut self) {
        self.transport_left = TRANSPORT_HELD;
    }

    /// The next subtitle track, and round through none.
    ///
    /// A cycle rather than a list, because this is the keyboard's shorthand
    /// for what the Options menu lays out properly — and `c` pressed twice on
    /// a film with one track has to get back to no subtitles.
    pub fn cycle_subtitles(&mut self) -> Option<Sound> {
        let Some(player) = self.player.as_ref() else {
            return Some(Sound::Error);
        };
        let tracks = player.tracks().len();
        if tracks == 0 {
            return Some(Sound::Error);
        }
        let next = match player.chosen() {
            None => Some(0),
            Some(at) if at + 1 < tracks => Some(at + 1),
            Some(_) => None,
        };
        player.choose_subtitles(next);
        self.woken();
        Some(Sound::Press)
    }

    /// Go to a fraction of the way through, which is what a press on the
    /// groove and the number keys both mean.
    pub fn seek_to_part(&mut self, part: f64) -> Option<Sound> {
        let Some(player) = self.player.as_ref() else {
            return Some(Sound::Error);
        };
        let length = player.length();
        if length <= 0.0 {
            return Some(Sound::Error);
        }
        player.seek_to(length * part.clamp(0.0, 1.0));
        self.woken();
        Some(Sound::Move)
    }

    pub fn step_film(&mut self, step: isize) -> Option<Sound> {
        let Some(next) = self.folder.step(self.cursor, step) else {
            return Some(Sound::Error);
        };
        self.let_the_film_go();
        self.cursor = next;
        self.show_the_film();
        Some(Sound::Move)
    }

    /// How hard the triggers are pulled, from the pad. Applied every frame in
    /// [`View::advance`] rather than acted on once, because a trigger is held
    /// rather than pressed.
    pub fn set_pull(&mut self, pull: f32) {
        self.pull = if pull.is_finite() {
            pull.clamp(-1.0, 1.0)
        } else {
            0.0
        };
    }

    /// A wheel, and where it was pointed.
    ///
    /// **In the player a wheel is the volume**, which is what it is on every
    /// video on this machine. Everywhere else it is what it is everywhere else
    /// in this language: Up and Down, so a grid scrolls and an open menu walks
    /// its rows without either having heard of a wheel. `notches` is positive
    /// downwards, as the toolkit reports it.
    pub fn wheel(&mut self, notches: i32, _at: [f32; 2]) -> Option<Sound> {
        if notches == 0 {
            return None;
        }
        if self.mode == Mode::Player && !self.overlaid() {
            self.woken();
            return self.change_volume(-notches as f32 * VOLUME_STEP);
        }
        let action = if notches > 0 {
            Action::Down
        } else {
            Action::Up
        };
        let mut answer = None;
        for _ in 0..notches.abs() {
            answer = self.act(action).or(answer);
        }
        answer
    }

    fn enter(&mut self) -> Option<Sound> {
        let Some(entry) = self.current() else {
            return Some(Sound::Error);
        };
        if entry.is_folder() {
            let into = entry.path.clone();
            self.go_to(&into, None);
            return Some(Sound::Press);
        }
        self.show_the_film();
        Some(Sound::Press)
    }

    /// Open the film the light is on and start playing it.
    fn show_the_film(&mut self) {
        let Some(path) = self.film().map(Path::to_path_buf) else {
            return;
        };
        let from = self.resume.at(&path).unwrap_or(0.0);
        self.resumed_at = (from > 0.0).then_some(from);
        let player = Player::open(&path, from, None);
        player.set_volume(self.volume, self.muted);
        self.player = Some(player);
        self.mode = Mode::Player;
        self.since_noted = 0.0;
        self.scanning = false;
        self.woken();
        // The stage is not told where it is going yet — `measure` works that
        // out from the window — but it is told to start from the card, so that
        // opening a film grows out of the one that was pressed.
        self.stage_known = false;
        self.showing = 0.0;
        self.going = Going::In;
        self.to_go = 1.0;
        // Where it is going is not known yet — see [`View::crossing_far`].
        self.crossing_far = [0.0; 4];
    }

    /// Put the picture on the screen with no crossing at all.
    ///
    /// One film following another in a run is not a change of page: nobody
    /// pressed a card, and the wall of films is not on the screen to fly out
    /// of. It cuts, as it always has.
    fn settled_on_the_film(&mut self) {
        self.going = Going::Nowhere;
        self.to_go = 0.0;
    }

    /// Stop the film, remembering where it got to.
    ///
    /// The one place a player is dropped, so there is no path out of the
    /// player that forgets to write the position down.
    fn let_the_film_go(&mut self) {
        if self.player.is_none() {
            return;
        }
        self.write_down_where_it_got_to();
        self.player = None;
        self.resumed_at = None;
    }

    /// Where the film has got to, in the record that outlives the run.
    ///
    /// Its own step because leaving the player is now two moments rather than
    /// one — the press, and the picture landing back on its card a third of a
    /// second later — and the place has to be written down at the first of
    /// them. A television box is switched off at the wall, and the wall does
    /// not wait for an animation.
    fn write_down_where_it_got_to(&mut self) {
        let Some((path, at, length)) = self
            .player
            .as_ref()
            .filter(|player| player.ready())
            .map(|player| (player.path().to_path_buf(), player.at(), player.length()))
        else {
            return;
        };
        self.resume.note(&path, at, length);
        self.resume.save();
    }

    /// Stop the film and start it on its way back into its card.
    ///
    /// The film is **not** dropped here. It goes on being drawn, shrinking,
    /// until [`View::advance`] sees the crossing land — a picture dropped at
    /// the press would vanish halfway back to the card it came out of.
    fn part_with_the_film(&mut self) {
        if let Some(player) = self.player.as_ref() {
            player.play(false);
        }
        self.write_down_where_it_got_to();
        self.going = Going::Out;
        self.to_go = 1.0;
        self.crossing_far = self.stage;
    }

    /// Whether Back closes the application rather than going anywhere.
    pub fn closes_on_back(&self) -> bool {
        match self.mode {
            Mode::Player => self.opened_on_a_film,
            Mode::Grid => self.at_the_top(),
        }
    }

    /// Whether this folder is as far out as the walk goes.
    fn at_the_top(&self) -> bool {
        self.folder.path == self.top
            || self
                .folder
                .path
                .parent()
                .is_none_or(|up| up == self.folder.path)
    }

    fn leave(&mut self) -> Option<Sound> {
        match self.mode {
            Mode::Player if self.opened_on_a_film => {
                self.let_the_film_go();
                self.quit = true;
                Some(Sound::Back)
            }
            Mode::Player => {
                self.part_with_the_film();
                self.mode = Mode::Grid;
                self.run_the_folder = false;
                Some(Sound::Back)
            }
            // Nowhere further out to go: leaving the top is leaving.
            Mode::Grid if self.at_the_top() => {
                self.quit = true;
                Some(Sound::Back)
            }
            Mode::Grid => {
                let here = self.folder.path.clone();
                match here.parent() {
                    Some(up) if up != here => {
                        let up = up.to_path_buf();
                        self.go_to(&up, Some(&here));
                        Some(Sound::Back)
                    }
                    _ => {
                        self.quit = true;
                        Some(Sound::Back)
                    }
                }
            }
        }
    }

    /// Open a folder chosen outright — from the chooser — which roots the walk
    /// there. Walking *into* a folder does not: that is a step inside the walk
    /// rather than the start of a new one.
    pub fn browse_from(&mut self, path: &Path) {
        self.top = path.to_path_buf();
        self.go_to(path, None);
    }

    /// What came back from the chooser, whatever it was opened for.
    ///
    /// One chooser answers both questions, so the answer has to be read
    /// against what was asked — a subtitle browsed as a folder would empty the
    /// window and lose the film.
    pub fn chose(&mut self, path: &Path) -> Option<Sound> {
        match std::mem::replace(&mut self.asking, Asking::AFolder) {
            Asking::AFolder => {
                self.browse_from(path);
                None
            }
            Asking::ASubtitle => self.add_subtitle_file(path),
        }
    }

    /// Read a subtitle file somebody chose, and show it.
    ///
    /// Apart from `chose`, this is what `--shot --subtitle` calls, so a
    /// screenshot of an added track goes down the same road a press does.
    pub fn add_subtitle_file(&mut self, path: &Path) -> Option<Sound> {
        let Some(player) = self.player.as_ref() else {
            return Some(Sound::Error);
        };
        let Some(track) = player.add_subtitles(path) else {
            // A file with nothing in it this can read. Said on the page rather
            // than swallowed: somebody who has just chosen a file is owed an
            // answer about that file.
            self.note = Some(format!(
                "Nothing to read in {}",
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ));
            return Some(Sound::Error);
        };
        // And shown, because adding one is asking for it.
        player.choose_subtitles(Some(track));
        self.woken();
        Some(Sound::Press)
    }

    pub fn go_to(&mut self, path: &Path, land_on: Option<&Path>) {
        self.let_the_film_go();
        self.opened_on_a_film = false;
        self.folder = Folder::read(path, self.order, self.hidden);
        self.cursor = land_on
            .and_then(|was| self.folder.position(was))
            .unwrap_or(0);
        self.mode = Mode::Grid;
        self.run_the_folder = false;
        self.scroll = 0.0;
        self.scroll_speed = 0.0;
        self.light.clear();
        self.note = self
            .folder
            .unreadable
            .then(|| String::from("This folder cannot be opened"));
    }

    fn reread(&mut self) {
        let here = self.folder.path.clone();
        let was = self.current().map(|entry| entry.path.clone());
        self.folder = Folder::read(&here, self.order, self.hidden);
        self.cursor = was
            .and_then(|was| self.folder.position(&was))
            .unwrap_or(0)
            .min(self.folder.entries.len().saturating_sub(1));
        // A film that has just been sorted out of the listing cannot go on
        // being played.
        if self.film().is_none() {
            self.let_the_film_go();
            self.mode = Mode::Grid;
            self.run_the_folder = false;
        }
    }

    fn run(&mut self, command: Command) -> Option<Sound> {
        match command {
            Command::Fill => {
                self.fill = !self.fill;
                None
            }
            Command::FromTheStart => {
                if let Some(player) = self.player.as_ref() {
                    player.seek_to(0.0);
                    player.play(true);
                }
                // And forget where it was left, or Back would put the mark
                // straight back on the film somebody just restarted.
                if let Some(path) = self.film().map(Path::to_path_buf) {
                    self.resume.forget(&path);
                }
                self.resumed_at = None;
                None
            }
            Command::Info => {
                self.info = !self.info;
                None
            }
            Command::RunTheFolder => {
                self.run_the_folder = !self.run_the_folder;
                None
            }
            Command::Subtitles(track) => {
                if let Some(player) = self.player.as_ref() {
                    player.choose_subtitles(track);
                }
                None
            }
            Command::AddSubtitles => {
                if self.player.is_none() {
                    return Some(Sound::Error);
                }
                // Beside the film rather than in the last folder anybody
                // browsed: a subtitle downloaded for a film is almost always
                // saved next to it, and the one place it will never be is
                // wherever this application happened to start.
                let at = self
                    .film()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.folder.path.clone());
                self.asking = Asking::ASubtitle;
                if !self.files.one_file(Selection::File, at) {
                    self.asking = Asking::AFolder;
                    return Some(Sound::Error);
                }
                None
            }
            Command::OpenFolder => {
                self.asking = Asking::AFolder;
                self.files.a_folder(&self.folder.path);
                None
            }
            Command::UpAFolder => {
                let here = self.folder.path.clone();
                if let Some(up) = here.parent().filter(|up| **up != here) {
                    let up = up.to_path_buf();
                    // Deliberately stepping over the top of the walk moves the
                    // top with them. There is no sense in a root somebody has
                    // just asked to go above — Back would close from a folder
                    // further out than the one it closes at.
                    if here == self.top {
                        self.top = up.clone();
                    }
                    self.go_to(&up, Some(&here));
                    return Some(Sound::Back);
                }
                Some(Sound::Error)
            }
            Command::ShowHidden => {
                self.hidden = !self.hidden;
                self.reread();
                None
            }
            Command::SortBy(order) => {
                self.order = order;
                self.reread();
                None
            }
            Command::BackToGrid => {
                // Asking for the folder is asking to browse it, so from here
                // on this is an ordinary walk and Back walks back out of it
                // rather than closing. See `closes_on_back`.
                self.part_with_the_film();
                self.opened_on_a_film = false;
                self.mode = Mode::Grid;
                self.run_the_folder = false;
                Some(Sound::Back)
            }
        }
    }

    // ---- the menu --------------------------------------------------------

    /// Ask for the Options menu.
    ///
    /// It is *asked for* rather than opened, and raised by
    /// [`View::settle_menu`] on the same frame. Nothing here can measure a
    /// word, and every name on the menu has to be cut to the width of the
    /// panel before it is handed over — see the note on [`View::asked`].
    pub fn open_menu(&mut self) {
        let (title, rows) = self.menu_rows();
        // Anchored where the stage is about to stand aside to, not where the
        // stage is now: the menu is opened by a press and the stage only
        // starts moving on the next frame.
        let anchor = [
            self.window[0] - self.menu_room(),
            self.window[1] * 0.5,
            0.0,
            0.0,
        ];
        // Without this every menu is one row tall with an arrow under it:
        // `open_at` raises a window of nought to one rather than leaving it
        // for the renderer to fit, so the number of rows is the caller's to
        // say. As many as there are, up to as many as the window holds.
        // As many as there are, up to as many as the window really holds —
        // the toolkit's own sum, which knows what each row is shaped like.
        // The rule of thumb this used before gave up two rows it had, and a
        // menu that scrolls when it did not need to is a menu somebody has to
        // walk to see all of.
        let shape: Vec<menu::Row> = rows.iter().map(|(line, _)| line.shape()).collect();
        let holds = menu::rows_of_that_fit(&shape, Some(1), self.window[1].max(1.0));
        let window = rows.len().clamp(1, holds.max(1));
        self.asked = Some(Asked {
            title,
            rows,
            anchor,
            window,
        });
    }

    /// Raise the menu that was asked for, with every name cut to the panel.
    ///
    /// **Why this exists.** `lxb-render` lays a label out at the width of the
    /// row it is on only when that row can hold more than one line. A label of
    /// one line — which is every row of a menu but the one the light is on,
    /// and every title — is shaped unbounded and drawn unclipped, so a film
    /// called anything longer than a menu is wide is written straight out
    /// through both sides of the panel. The panel is a known width before it
    /// is opened, so the words are cut to it here.
    ///
    /// `measure` is `Ui::measure`, handed in rather than reached for: nothing
    /// else in this file draws, and a test can answer it with arithmetic.
    pub fn settle_menu(&mut self, measure: &mut dyn FnMut(Text, &str) -> f32) {
        let Some(asked) = self.asked.take() else {
            return;
        };
        let Asked {
            title,
            rows,
            anchor,
            window,
        } = asked;
        let scale = lxb_toolkit::metrics::scale_for(self.window[1].max(1.0));

        self.commands = rows.iter().map(|(_, command)| *command).collect();
        self.menu_shape = rows.iter().map(|(line, _)| line.shape()).collect();
        self.menu_anchor = anchor;
        self.menu_window = window;
        self.menu_first = 0;

        // The panel, less its margin and the padding a row's words start
        // after — which is what the title is given.
        let titled = (menu::panel_width(0.0) - (menu::MARGIN + menu::LABEL_PADDING) * 2.0) * scale;
        let entries: Vec<MenuEntry> = rows
            .into_iter()
            .map(|(line, _)| {
                // And what a row is given. The toolkit's own sum, which is
                // the panel less its margins, less two and a half of the
                // padding a row's words start after, less the glyph in front
                // of them — plus the mark behind them, which the toolkit does
                // *not* take off and which a long name would otherwise be
                // written straight over.
                let shape = line.shape();
                let inside = (menu::row_settled(&shape) - menu::ROW_PADDING * 2.0) * scale;
                let mark = if shape.aside {
                    (menu::aside_width(&shape) + menu::ASIDE_GAP) * scale
                } else {
                    0.0
                };
                let room =
                    (menu::panel_width(0.0) - menu::MARGIN * 2.0 - menu::LABEL_PADDING * 2.5)
                        * scale
                        - inside * menu::GLYPH
                        - mark;
                // The row the light is on is drawn **bold**, and a bold
                // Roboto is a little wider than the regular this measures.
                // The room is shaved rather than every label measured twice:
                // a menu row filled to its last pixel is not a row anybody was
                // aiming for either way.
                let room = room * 0.97;
                let mut entry = MenuEntry::new(cut_to_fit(measure, Text::Body, &line.label, room))
                    .glyph(line.glyph)
                    .group(line.group);
                if let Some(detail) = line.detail.as_deref() {
                    entry = entry.detail(cut_to_fit(measure, Text::Caption, detail, room));
                }
                if let Some(mark) = line.aside {
                    entry = entry.aside(mark);
                }
                entry
            })
            .collect();

        let title = cut_to_fit(measure, Text::Title, &title, titled);
        self.menu.open_at(anchor, Some(title), entries);
        self.menu.set_window(window);
    }

    /// Keep [`View::menu_first`] in step with the toolkit's own.
    ///
    /// A menu long enough to scroll — a film with several subtitle tracks
    /// reaches that — shows a window of rows starting at a row the
    /// `ContextMenu` keeps to itself, and the panel's *height* depends on
    /// which rows those are. Without this the hole was worked out from row
    /// nought, came out the wrong size, and the film was drawn over half the
    /// menu.
    ///
    /// It is the toolkit's `hold_selection`, run against the selection the
    /// toolkit hands out. Both start at nought and neither has any other
    /// input, so they cannot drift; `menu_hole` checks the answer against the
    /// two things the menu *does* say about its scroll, and gives up rather
    /// than cutting a hole in the wrong place if they ever disagree.
    fn follow_the_menu(&mut self) {
        if !self.menu.showing() {
            return;
        }
        let rows = self.menu_shape.len();
        let window = self.menu_window.max(1).min(rows.max(1));
        let selected = self.menu.selected();
        if selected < self.menu_first {
            self.menu_first = selected;
        } else if selected + 1 > self.menu_first + window {
            self.menu_first = selected + 1 - window;
        }
        self.menu_first = self.menu_first.min(rows.saturating_sub(window));
    }

    /// Where the menu's panel ended up, and how round its corners are.
    pub fn menu_hole(&self) -> Option<crate::film::Hole> {
        if !self.menu.showing() || self.menu_shape.is_empty() {
            return None;
        }
        let window = self.menu_window.max(1).min(self.menu_shape.len());
        // What the menu itself says about where it has scrolled to. If the
        // mirror disagrees with either, the panel is not where this thinks it
        // is and a hole cut here would be a hole through the menu.
        if self.menu.scrolled_above() != (self.menu_first > 0)
            || self.menu.scrolled_below() != (self.menu_first + window < self.menu_shape.len())
        {
            return None;
        }
        let layout = menu::Layout::new(
            &self.menu_shape,
            Some(1),
            self.menu_anchor,
            self.window,
            self.menu_first,
            window,
            self.menu.selected(),
            self.menu.unfolded(),
            0.0,
        );
        let panel = layout.rect;
        if panel[2] <= 0.0 || panel[3] <= 0.0 {
            return None;
        }
        let grown = menu::growing(self.menu_anchor, panel, self.menu.travelled());
        let scale = lxb_toolkit::metrics::scale_for(self.window[1].max(1.0));
        // The panel is drawn at `panel` and flown to `grown`, and its corner
        // is flown with it — the toolkit works its frost out the same way.
        let radius = Metric::PanelRadius.value() * scale * (grown[2] / panel[2].max(1.0));
        // The rim is drawn *outside* the pane, so the hole has to be too or a
        // hairline of film is left along the whole edge of the menu.
        let rim = (lxb_toolkit::material::Overlay::ContextMenu
            .material()
            .rim_width
            * scale)
            .max(1.0);
        Some(crate::film::Hole {
            rect: [
                grown[0] - rim,
                grown[1] - rim,
                grown[2] + rim * 2.0,
                grown[3] + rim * 2.0,
            ],
            radius: radius + rim,
        })
    }

    fn menu_rows(&self) -> (String, Vec<(Line, Command)>) {
        let mut rows: Vec<(Line, Command)> = Vec::new();
        if self.mode == Mode::Player {
            rows.push((
                Line::new(
                    if self.fill {
                        "Fit to the screen"
                    } else {
                        "Fill the screen"
                    },
                    "setting-scale",
                ),
                Command::Fill,
            ));
            rows.push((
                Line::new("Start from the beginning", "media-previous").group(1),
                Command::FromTheStart,
            ));
            rows.push((
                Line::new(
                    if self.run_the_folder {
                        "Stop playing the folder"
                    } else {
                        "Play the whole folder"
                    },
                    "media-play",
                ),
                Command::RunTheFolder,
            ));

            // The subtitle tracks, each its own row with the chosen one
            // ticked. A list rather than one row that cycles, because a film
            // with six tracks would need six presses to see what the sixth was
            // — and because this is exactly how the sort orders read in the
            // grid's own menu.
            //
            // **The group is here whether the film has a track or not.** A
            // film with none is exactly the film somebody wants to add one to,
            // and a menu that answered that by saying nothing at all was the
            // one thing this player could not do.
            let tracks = self
                .player
                .as_ref()
                .map(|player| player.tracks())
                .unwrap_or_default();
            let chosen = self.player.as_ref().and_then(|player| player.chosen());
            rows.push((
                Line::new("No subtitles", "setting-typed")
                    .ticked(chosen.is_none())
                    .group(2),
                Command::Subtitles(None),
            ));
            for (number, track) in tracks.iter().enumerate() {
                let row = Line::new(&track.name, "setting-typed").ticked(chosen == Some(number));
                // Where it came from, on the row rather than in the name:
                // a track inside the film and a file somebody put beside
                // it are different things with the same kind of name.
                let row = if track.from_a_file {
                    row.detail("From a file")
                } else {
                    row
                };
                rows.push((row, Command::Subtitles(Some(number))));
            }
            rows.push((
                Line::new("Add a subtitle file…", "file-page"),
                Command::AddSubtitles,
            ));

            rows.push((
                Line::new(
                    if self.info {
                        "Hide the details"
                    } else {
                        "Show the details"
                    },
                    "setting-info",
                )
                .group(3),
                Command::Info,
            ));
            rows.push((
                Line::new("Back to the folder", "category-video").group(4),
                Command::BackToGrid,
            ));
            return (
                self.current()
                    .map(|entry| entry.name.clone())
                    .unwrap_or_default(),
                rows,
            );
        }

        // The order the listing is already in wears the tick. Only that one:
        // `aside` takes the name of a real mark, and a row that is not the
        // current order is a row with nothing beside it rather than a row with
        // an empty mark.
        for order in Order::ALL {
            rows.push((
                Line::new(order.label(), "setting-order").ticked(order == self.order),
                Command::SortBy(order),
            ));
        }
        rows.push((
            Line::new(
                if self.hidden {
                    "Hide hidden files"
                } else {
                    "Show hidden files"
                },
                "setting-typed",
            )
            .group(1),
            Command::ShowHidden,
        ));
        rows.push((
            Line::new("Open another folder…", "file-folder").group(2),
            Command::OpenFolder,
        ));
        rows.push((Line::new("Up a folder", "arrow-up"), Command::UpAFolder));
        (String::from("Options"), rows)
    }

    // ---- how the film sits on the stage ----------------------------------

    /// How far in from the right-hand edge a menu is anchored.
    ///
    /// The stage used to give up exactly this much as well. It does not any
    /// more — a menu is a hole cut in the picture rather than a bite out of it
    /// — so this is only where the panel goes.
    fn menu_room(&self) -> f32 {
        self.window[0] * 0.42
    }

    /// Where the details pane stands, for however much of it is out.
    ///
    /// **One sum, read twice**: `draw::details` puts the pane here and
    /// [`View::stands_aside`] stands clear of it. Two answers drift, and the
    /// drift is a film drawn over a pane nobody can read.
    pub fn details_pane(&self, geometry: &Geometry) -> [f32; 4] {
        let width = self.info_width(geometry);
        // Down to the transport and no further. The two used to overlap by the
        // whole height of the band, which nobody saw while the transport was a
        // card of glass and everybody sees now that both are panes.
        let bottom = geometry.transport_at()[1] - geometry.gap;
        [
            geometry.window[0] - (geometry.margin + width) * self.info_out,
            geometry.head,
            width,
            (bottom - geometry.head).max(1.0),
        ]
    }

    pub fn info_width(&self, geometry: &Geometry) -> f32 {
        (geometry.window[0] * 0.28).clamp(geometry.margin * 4.0, geometry.window[0] * 0.4)
    }

    pub fn placement_now(&self) -> crate::film::Placement {
        let stage = self.stepped_back(self.stage);
        let shrunk = if self.stage[2] > 0.0 {
            stage[2] / self.stage[2]
        } else {
            1.0
        };
        crate::film::Placement {
            centre: [stage[0] + stage[2] * 0.5, stage[1] + stage[3] * 0.5],
            half: [self.shown[0] * 0.5 * shrunk, self.shown[1] * 0.5 * shrunk],
            opacity: self.showing,
            dim: self.dimmed(),
            radius: self.stage_radius() * shrunk,
            within: stage,
        }
    }

    // ---- opening out of a card, and going back into one -------------------

    /// How far the picture is between the card it was pressed on and the
    /// screen: 0 on the card, 1 filling the stage.
    ///
    /// **The one number a change of page is made of.** Everything that moves
    /// with it reads it — the stage itself, how round the picture's corners
    /// are, how far the wall of films has stepped back behind it, and the
    /// black it sits on — so that all of them land on the same frame.
    pub fn grown(&self) -> f32 {
        match self.going {
            Going::Nowhere if self.mode == Mode::Player => 1.0,
            Going::Nowhere => 0.0,
            Going::In => motion::ease(1.0 - self.to_go),
            Going::Out => motion::ease(self.to_go),
        }
    }

    /// How far a change of page has got, or `None` once it has landed.
    ///
    /// `None` is what stops the wall of films being drawn under a film for
    /// the rest of the evening: it is there to be flown out of and back into,
    /// and not otherwise.
    pub fn crossing(&self) -> Option<f32> {
        (self.going != Going::Nowhere).then(|| self.grown())
    }

    /// Whether the picture on the screen is on its way back into its card.
    pub fn leaving(&self) -> bool {
        self.going == Going::Out
    }

    /// How round the corners of whatever is on the stage are.
    ///
    /// The card's own rounding on the card and none at all on the screen. A
    /// picture that grew out of a card and reached the window still visibly a
    /// rounded card would have arrived as something else — the shell's launch
    /// panel travels the same number the same way.
    pub fn stage_radius(&self) -> f32 {
        self.frame_radius * (1.0 - self.grown())
    }

    /// The picture on the card this crossing runs to or from, as it stands on
    /// the screen this frame.
    ///
    /// The card's *picture*, not the card: a film that grew out of the whole
    /// card would jump on its first frame by the depth of the card's caption.
    fn crossing_card(&self, geometry: &Geometry) -> Option<[f32; 4]> {
        self.crossing()?;
        Some(geometry.frame_in(self.card_on_screen(geometry)?))
    }

    /// What a change of page turns about.
    ///
    /// The card, so that the one thing which does not move while the wall
    /// steps back behind the picture is the card the picture came out of.
    pub fn crossing_about(&self, geometry: &Geometry) -> [f32; 4] {
        self.crossing_card(geometry)
            .unwrap_or([0.0, 0.0, geometry.window[0], geometry.window[1]])
    }

    /// The film whose poster stands on the stage while there is no picture on
    /// it yet, and how much of it there is.
    ///
    /// Opening a film used to be a black rectangle growing out of the card
    /// somebody pressed: ffmpeg takes a moment over a header, and until it
    /// answers there is nothing to draw. What was on the card is the obvious
    /// thing to put there — it is the picture that was pressed — so the card
    /// grows into its own poster and the film comes up underneath it.
    ///
    /// It stands on the *stage*, which is what keeps it off the panels the
    /// toolkit cannot draw over it: a film that is not playing has not taken
    /// the window, and the stage stands aside for the page's furniture until
    /// it has.
    ///
    /// **It is drawn at full strength, under the film rather than crossing
    /// with it.** Two halves of the same picture fading past each other add up
    /// to three quarters at the middle, and the quarter that is missing is the
    /// wall of films showing through the one being opened. Nothing is lost by
    /// holding it: what covers it is the same picture at the same size, moving.
    pub fn standing_in(&self) -> Option<&Path> {
        if self.mode != Mode::Player || self.takes_over() || self.showing > 0.999 {
            return None;
        }
        // A film that cannot be opened has something to say instead, and a
        // message over a poster is a message nobody reads.
        if self
            .player
            .as_ref()
            .is_some_and(|player| player.trouble().is_some())
        {
            return None;
        }
        self.film()
    }

    /// Where the poster standing in on the stage goes, for a picture of this
    /// shape.
    ///
    /// **The rectangle travels from the card's own shape to the film's.** A
    /// card crops its poster to fill itself and the player shows the whole
    /// frame, so one of the two has to give: the crop unwinds as the picture
    /// grows, which makes the first frame the card exactly and the last the
    /// film exactly.
    ///
    /// The shape is asked of the poster rather than read off `shown`, which is
    /// nought until the film has opened — and a rectangle that eased toward
    /// nothing and then jumped when the decoder answered would be worse than
    /// not easing at all.
    pub fn standing_rect(&self, aspect: Option<f32>) -> [f32; 4] {
        let Some(aspect) = aspect.filter(|aspect| aspect.is_finite() && *aspect > 0.0) else {
            return self.stage;
        };
        let room = self.stage[2] / self.stage[3].max(0.001);
        let (across, down) = if aspect >= room {
            (self.stage[2], self.stage[2] / aspect)
        } else {
            (self.stage[3] * aspect, self.stage[3])
        };
        let fitted = [
            self.stage[0] + (self.stage[2] - across) * 0.5,
            self.stage[1] + (self.stage[3] - down) * 0.5,
            across,
            down,
        ];
        // A film asked to fill the stage is cropped to it on purpose, so the
        // crop never unwinds. See `View::fill`.
        if self.fill {
            return self.stage;
        }
        between(self.stage, fitted, self.grown())
    }

    /// How solid whatever is on the stage is — the film, or the poster
    /// standing in for it.
    ///
    /// What decides how much of the wall underneath is taken out from under it
    /// while a page is changing. A picture at half is a picture the names
    /// behind it should be half readable through, and a name that had vanished
    /// under a film already dissolving into its own card would come back in a
    /// jump on the last frame of the animation.
    pub fn stage_solidity(&self) -> f32 {
        if self.standing_in().is_some() {
            return 1.0;
        }
        self.showing.clamp(0.0, 1.0)
    }

    /// The black behind the picture: the film's own letterbox.
    ///
    /// The whole window while a film is on it, grown far past every edge so
    /// that a page stepping back behind a menu cannot open a sliver of
    /// wallpaper along one. While a page is changing it is the stage itself,
    /// rounded to the same corners as the picture — a film that has not taken
    /// the window yet has not earned it, and black everywhere behind a picture
    /// the size of a card is not a letterbox but a blackout.
    pub fn letterbox(&self, geometry: &Geometry) -> crate::film::Hole {
        if self.crossing().is_some() {
            return crate::film::Hole {
                rect: self.stepped_back(self.stage),
                radius: self.stage_radius(),
            };
        }
        let hints = self.stepped_back(self.hints_band(geometry));
        let [width, height] = geometry.window;
        crate::film::Hole {
            rect: [-width, -height, width * 3.0, (hints[1] + height).max(0.0)],
            radius: 0.0,
        }
    }
}

/// A rectangle part of the way between two others.
fn between(from: [f32; 4], to: [f32; 4], t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    let mut out = [0.0; 4];
    for (slot, (from, to)) in out.iter_mut().zip(from.iter().zip(&to)) {
        *slot = from + (to - from) * t;
    }
    out
}

/// Shorten a name until it fits, taking the middle out of it.
///
/// The middle, because the two ends are what tell one name from another: the
/// front says which series and the back says which episode and what kind of
/// file it is.
///
/// `measure` is handed in rather than reached for. Nothing in this file draws,
/// and the two callers measure with different things — `draw.rs` with the `Ui`
/// it is drawing into, and a test with arithmetic.
pub fn cut_to_fit(
    measure: &mut dyn FnMut(Text, &str) -> f32,
    text: Text,
    name: &str,
    room: f32,
) -> String {
    if room <= 0.0 || measure(text, name) <= room {
        return name.to_string();
    }
    let letters: Vec<char> = name.chars().collect();
    let mut keep = letters.len();
    while keep > 1 {
        keep -= 1;
        // Two thirds off the front and the rest off the back — but never more
        // front than there are letters left to give. Rounding up crosses over
        // at two, and a `usize` that goes below nought does not come back.
        let front = (keep.div_ceil(3) * 2).min(keep);
        let back = keep - front;
        let shorter: String = letters[..front]
            .iter()
            .chain(std::iter::once(&'…'))
            .chain(letters[letters.len() - back..].iter())
            .collect();
        if measure(text, &shorter) <= room {
            return shorter;
        }
    }
    String::from("…")
}

/// How large a film is drawn on a stage.
///
/// Fit inside it, or fill it and let the scissor take the rest. A film has one
/// right size; the only question is what to do with the shape left over, and
/// those are the only two answers anybody wants — which is why this is a flag
/// rather than the ladder of stops a photograph gets.
fn drawn_size(stage: [f32; 4], (width, height): (f32, f32), fill: bool) -> [f32; 2] {
    if width <= 0.0 || height <= 0.0 {
        return [0.0; 2];
    }
    let across = stage[2] / width;
    let down = stage[3] / height;
    let scale = if fill {
        across.max(down)
    } else {
        across.min(down)
    };
    [width * scale, height * scale]
}

/// How far one press of a direction seeks, given how long it has been held.
fn step_now(held_for: f32) -> f64 {
    STEPS
        .iter()
        .rev()
        .find(|(after, _)| held_for >= *after)
        .map(|(_, step)| *step)
        .unwrap_or(STEPS[0].1)
}

/// Where everything on the page goes, worked out from the window alone.
#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    pub window: [f32; 2],
    pub margin: f32,
    pub head: f32,
    pub foot: f32,
    /// How tall the transport is, when it is up.
    pub transport: f32,
    pub columns: usize,
    pub cell: [f32; 2],
    pub gap: f32,
    /// The air around the picture on a card, and the line of name under it.
    /// Kept because a film grows out of the picture it was pressed on rather
    /// than out of the card around it — see [`Geometry::frame_in`].
    card_inset: f32,
    card_caption: f32,
}

impl Geometry {
    pub fn of(window: [f32; 2], scale: impl Fn(f32) -> f32, gap: f32) -> Geometry {
        let margin = scale(48.0);
        let head = scale(86.0);
        let foot = scale(64.0);
        let across = (window[0] - margin * 2.0).max(1.0);
        // A card wide enough to recognise a frame in and narrow enough that a
        // folder shows more than four. Rounded to whole columns, and whatever
        // is left over widens them all rather than being left at the edge.
        let wanted = scale(340.0);
        let columns = (((across + gap) / (wanted + gap)).floor() as usize).clamp(1, 12);
        let width = (across - gap * (columns - 1) as f32) / columns as f32;
        Geometry {
            window,
            margin,
            head,
            foot,
            transport: scale(84.0),
            columns,
            // Room for the frame — a film's own shape, near enough — and a
            // line under it for the name.
            cell: [width, width * 0.5625 + scale(34.0)],
            gap,
            card_inset: scale(10.0),
            card_caption: Text::Caption.on(window[1]) * Text::LINE,
        }
    }

    /// The picture on a card: the frame itself, without the air around it or
    /// the line of name under it.
    ///
    /// **One sum, read twice.** `draw::card` puts the poster here, and a film
    /// opening grows out of exactly this rectangle. Two answers would drift,
    /// and the drift would show as a film jumping the moment it started
    /// moving.
    /// How round the picture on a card is.
    ///
    /// The card's own rounding less the air around the picture, which is what
    /// makes the two curves concentric — a picture inset inside a rounded
    /// corner by exactly the inset shares its centre of curvature. It was
    /// `Metric::TileRadius`, which is a *share* of a tile's size and not a
    /// number of pixels, so it came out as three tenths of one and the
    /// pictures were square inside rounded cards.
    ///
    /// Read by the card and by the film growing out of it, so that the two
    /// agree on the frame the animation starts on.
    pub fn frame_radius(&self) -> f32 {
        (Metric::CardRadius.on(self.window[1]) - self.card_inset).max(0.0)
    }

    pub fn frame_in(&self, card: [f32; 4]) -> [f32; 4] {
        [
            card[0] + self.card_inset,
            card[1] + self.card_inset,
            (card[2] - self.card_inset * 2.0).max(1.0),
            (card[3] - self.card_inset * 2.0 - self.card_caption).max(1.0),
        ]
    }

    pub fn viewport(&self) -> [f32; 4] {
        [
            self.margin,
            self.head,
            (self.window[0] - self.margin * 2.0).max(1.0),
            (self.window[1] - self.head - self.foot).max(1.0),
        ]
    }

    /// Where the transport sits: a band above the legend, across the page.
    ///
    /// The controls are always laid out in this band, whatever the pane behind
    /// them is doing.
    pub fn transport_at(&self) -> [f32; 4] {
        [
            self.margin,
            (self.window[1] - self.foot - self.transport).max(0.0),
            (self.window[0] - self.margin * 2.0).max(1.0),
            self.transport,
        ]
    }

    /// How far a panel is held off the edge of the window.
    fn inset(&self) -> f32 {
        self.margin * 0.5
    }

    /// The pane behind the head of the player.
    pub fn head_pane(&self) -> [f32; 4] {
        let inset = self.inset();
        [
            self.margin,
            inset,
            (self.window[0] - self.margin * 2.0).max(1.0),
            (self.head - inset).max(1.0),
        ]
    }

    /// The band the row of button hints sits in, across the foot of the page.
    ///
    /// **The hints are never inside the transport's panel.** They are the same
    /// row in the same place on every page of this application and in the
    /// shell beside it; putting them inside one page's control bar would make
    /// them that bar's own furniture. Over a playing film the picture gives
    /// this band up rather than the hints taking a panel — which is what
    /// [`View::stage_target`] does, and why this is a rectangle rather than a
    /// number.
    pub fn legend_band(&self) -> [f32; 4] {
        [
            0.0,
            (self.window[1] - self.foot).max(0.0),
            self.window[0],
            self.foot.min(self.window[1]),
        ]
    }

    /// Where the row of hints sits.
    pub fn legend_at(&self) -> f32 {
        self.window[1] - self.foot * 0.5
    }

    /// How far the chrome has slid off its edge of the window.
    ///
    /// **The transport slides rather than fades.** A pane is drawn by
    /// `Ui::pane`, which has no opacity to fade, and — the reason that does
    /// not matter — a hole cut in a film cannot fade at all: it would open on
    /// a rectangle of wallpaper. So the panel and the hole leave together, off
    /// the bottom of the window, and the hints go with them.
    pub fn transport_away(&self, out: f32) -> f32 {
        let band = self.transport_at();
        (1.0 - out.clamp(0.0, 1.0)) * (self.window[1] - band[1] + self.margin)
    }

    pub fn head_away(&self, out: f32) -> f32 {
        (1.0 - out.clamp(0.0, 1.0)) * (self.head + self.margin)
    }

    /// Where one card of the grid is, before the listing is scrolled.
    pub fn card(&self, index: usize) -> [f32; 4] {
        let row = (index / self.columns.max(1)) as f32;
        let column = (index % self.columns.max(1)) as f32;
        [
            self.margin + column * (self.cell[0] + self.gap),
            self.head + row * (self.cell[1] + self.gap),
            self.cell[0],
            self.cell[1],
        ]
    }

    pub fn rows(&self, count: usize) -> usize {
        count.div_ceil(self.columns.max(1))
    }

    /// How many rows fit in the viewport, as a fraction — a listing that shows
    /// two rows and a sliver of a third is showing 2.4 rows, and rounding that
    /// to two is what makes a page jump land short.
    pub fn visible_rows(&self) -> f32 {
        (self.viewport()[3] + self.gap) / (self.cell[1] + self.gap)
    }
}

impl View {
    /// Everything that has to be true before this frame acts or draws.
    ///
    /// Measure, then move. The order matters: a direction pressed this frame
    /// is answered against the film as it is now, not as it was before the
    /// last stage animation finished.
    pub fn measure(&mut self, geometry: &Geometry) {
        self.window = geometry.window;
        self.columns = geometry.columns;
        self.visible_rows = geometry.visible_rows();
        self.transport_room = geometry.transport + geometry.gap;
        self.frame_radius = geometry.frame_radius();

        let target = self.stage_target(geometry);
        if !self.stage_known {
            // Coming into the player, the stage starts as the card that was
            // pressed and grows into place. Every frame after this one is
            // `advance`'s.
            if self.mode == Mode::Player {
                self.stage = self.card_on_screen(geometry).unwrap_or(target);
            }
            self.stage_known = true;
        }

        self.size_it();
    }

    /// How large the film is drawn, for the stage as it stands now.
    ///
    /// Read twice a frame — once when the frame is measured and again once
    /// the stage has moved — because a stage that travels a tenth of the
    /// window in a frame and a size worked out before it moved is a picture
    /// drawn at last frame's size inside this frame's scissor, which shows as
    /// a hairline crawling round the edge of an opening film.
    fn size_it(&mut self) {
        let Some((width, height)) = self
            .player
            .as_ref()
            .and_then(|player| player.shown_size())
            .filter(|(width, height)| *width > 0 && *height > 0)
        else {
            self.shown = [0.0; 2];
            return;
        };
        self.shown = drawn_size(self.stage, (width as f32, height as f32), self.fill);
    }

    /// Where the film is allowed to be, once everything else has had its share
    /// of the window.
    ///
    /// The whole window while it is playing — everything over it is a hole cut
    /// in the picture, and nothing takes room from it. Otherwise what is left
    /// after the page's furniture: see [`View::stands_aside`].
    fn stage_target(&self, geometry: &Geometry) -> [f32; 4] {
        // Everything over a playing film is a hole cut in the picture and
        // takes no room from it — except the row of button hints, which has no
        // panel to be the shape of and must not be given one. The picture
        // stops where that band starts, and follows it back down as it slides
        // away, so the window is the film's again the moment the hints are off
        // it.
        let band = self.hints_band(geometry);
        let filling = [
            0.0,
            0.0,
            geometry.window[0],
            band[1].min(geometry.window[1]).max(1.0),
        ];
        // **Between the two, on the number the black is on.** A film that has
        // taken the window and one that stands beside the page are the two
        // ends of one transition, and `black` is the eased answer to which of
        // them is in force — which is what lets the stage be *set* from this
        // rather than sprung at it, and the stage is what has to stand clear
        // of a details pane that arrives on a clock of its own.
        between(self.stands_aside(geometry), filling, self.black)
    }

    /// The band the button hints are in, where it is this frame.
    ///
    /// Slid down with the transport, because they leave together. Everything
    /// that has to meet this band — the picture's bottom edge, the film's own
    /// black, and the black the toolkit fills the band itself with — is
    /// worked out from this one rectangle rather than from the same sum three
    /// times over.
    pub fn hints_band(&self, geometry: &Geometry) -> [f32; 4] {
        let band = geometry.legend_band();
        let away = geometry.transport_away(self.transport);
        [band[0], band[1] + away, band[2], band[3]]
    }

    /// Where the stage stands when it is standing aside.
    ///
    /// The menu is deliberately not in this sum. It used to take room here as
    /// well, and a menu raised over a film whose details were already open
    /// took its share of what the details had left — two bites of the same
    /// window, and a film in the corner of it. A menu is a hole in the picture
    /// now, in both modes.
    fn stands_aside(&self, geometry: &Geometry) -> [f32; 4] {
        let mut stage = geometry.viewport();
        stage[3] = (stage[3] - self.transport_room).max(1.0);
        // The details take a pane down the right-hand side, and the stage
        // gives up exactly that much — never overlapping it at any moment of
        // the animation, because a pane under a film is a pane nobody can
        // read. Held to `details_pane` by a test.
        let pane = (self.info_width(geometry) + geometry.gap) * self.info_out;
        stage[2] = (stage[2] - pane).max(1.0);
        stage
    }

    /// The card the light is on, where it is on the screen this frame.
    fn card_on_screen(&self, geometry: &Geometry) -> Option<[f32; 4]> {
        if self.folder.entries.is_empty() {
            return None;
        }
        let mut card = geometry.card(self.cursor.min(self.folder.entries.len() - 1));
        card[1] -= self.scroll * (geometry.cell[1] + geometry.gap);
        Some(card)
    }

    /// Move everything that moves.
    pub fn advance(&mut self, dt: f32, geometry: &Geometry) {
        self.menu.advance(dt);
        self.dialog.advance(dt);
        self.files.advance(dt);
        self.pressing.advance(dt);

        // **Everything the stage is worked out from is wound on before the
        // stage is.** These used to be eased at the foot of this function, a
        // frame after the stage had read them and the same frame `draw` reads
        // them — which is a details pane a twelfth of its own width ahead of
        // the picture that has to stand clear of it.
        if self.mode == Mode::Player && self.transport_left > 0.0 {
            self.transport_left = (self.transport_left - dt).max(0.0);
        }
        let wanted = if self.transport_wanted() { 1.0 } else { 0.0 };
        self.transport = toward(self.transport, wanted, dt / motion::duration::PANEL);
        let black = if self.over_the_film() { 1.0 } else { 0.0 };
        // On the way back to the wall of films the black lifts with the
        // picture rather than on its own clock: the wall is behind it, and a
        // black that outlasted the crossing would hand the screen over in a
        // jump at the end of an animation that had been smooth until then.
        let lifts_over = if self.leaving() {
            motion::duration::LAUNCH_OPEN
        } else {
            motion::duration::PANEL
        };
        self.black = toward(self.black, black, dt / lifts_over);
        self.info_out = toward(
            self.info_out,
            if self.info && self.mode == Mode::Player {
                1.0
            } else {
                0.0
            },
            dt / motion::duration::PANEL,
        );
        self.follow_the_menu();

        // The crossing's own clock, wound down before anything reads it so
        // that everything this frame agrees about where it has got to.
        if self.going != Going::Nowhere {
            self.to_go = (self.to_go - dt / motion::duration::LAUNCH_OPEN).max(0.0);
        }

        let target = self.stage_target(geometry);
        match self.crossing_card(geometry) {
            // A change of page **scales** the picture between the card and the
            // screen rather than springing at it. The wall stepping back
            // behind it and the black under it are tied to the same number,
            // and a spring of its own would put the three on three clocks.
            Some(card) => {
                if self.going == Going::In {
                    self.crossing_far = if self.crossing_far[2] > 0.0 {
                        between(self.crossing_far, target, dt / motion::duration::PANEL)
                    } else {
                        target
                    };
                }
                self.stage = between(card, self.crossing_far, self.grown());
            }
            None => self.stage = target,
        }
        self.size_it();

        // And landed — *after* the frame it lands on has been laid out, or the
        // picture would be handed back to the spring on the very frame it was
        // due to arrive on its card, and spend that frame flying off it.
        if self.going != Going::Nowhere && self.to_go <= 0.0 {
            // Dropped where it lands rather than where Back was pressed: see
            // [`View::part_with_the_film`].
            if self.leaving() {
                self.let_the_film_go();
                // What is on the card is the card's own picture now, and the
                // last of the film goes with the player rather than fading on
                // for a fifth of a second after there is nothing left to fade.
                self.showing = 0.0;
            }
            self.going = Going::Nowhere;
        }

        self.scan(dt);
        self.hold(dt);

        if self.leaving() {
            // A film shrinking back into its card is still on the screen, and
            // dissolves into the card's own picture as it lands. See
            // [`LANDS_OVER`]. Never *more* than there was at the press: a film
            // that was never on the screen has nothing to land.
            self.showing = self.showing.min((self.to_go / LANDS_OVER).min(1.0));
        } else {
            let showing = if self.mode == Mode::Player
                && !self.takes_over()
                && self
                    .player
                    .as_ref()
                    .is_some_and(|player| player.ready() && player.trouble().is_none())
            {
                1.0
            } else {
                0.0
            };
            self.showing = toward(self.showing, showing, dt / motion::duration::PANEL);
        }

        self.watch_the_film(dt);
        self.scroll_step(dt, geometry);
    }

    /// The film reaching its end, and the place it got to being written down.
    fn watch_the_film(&mut self, dt: f32) {
        if self.mode != Mode::Player {
            return;
        }
        let Some(player) = self.player.as_ref() else {
            return;
        };
        if let Some(trouble) = player.trouble() {
            self.note = Some(trouble);
        }

        // Written down as it goes, because a television box is switched off at
        // the wall rather than closed.
        self.since_noted += dt;
        if self.since_noted > NOTE_EVERY && player.ready() {
            self.since_noted = 0.0;
            let (path, at, length) = (player.path().to_path_buf(), player.at(), player.length());
            self.resume.note(&path, at, length);
            self.resume.save();
        }

        if !player.ended() {
            return;
        }
        // The end. Whatever happens next, this film is finished and is not a
        // film to be put back into.
        let path = player.path().to_path_buf();
        if !self.run_the_folder {
            self.resume.forget(&path);
            self.resume.save();
            self.woken();
            return;
        }
        match self.folder.after(self.cursor) {
            Some(next) => {
                self.let_the_film_go();
                self.resume.forget(&path);
                self.cursor = next;
                self.show_the_film();
                self.settled_on_the_film();
            }
            None => {
                // The last film in the folder: the run is over, and what
                // somebody asked to see has all been seen.
                self.let_the_film_go();
                self.resume.forget(&path);
                self.mode = Mode::Grid;
                self.run_the_folder = false;
                self.stage_known = false;
            }
        }
    }

    /// The triggers, which scan through the film.
    ///
    /// A scan **holds the film still** and moves it by seeking, rather than
    /// playing it faster: a soundtrack at four times speed is a noise, and
    /// what somebody scanning is looking at is the picture. The film is put
    /// back the way it was found when the trigger is let go.
    fn scan(&mut self, dt: f32) {
        if self.mode != Mode::Player || self.overlaid() {
            if self.scanning {
                self.stop_scanning();
            }
            return;
        }
        let Some(player) = self.player.as_ref() else {
            return;
        };
        if self.pull == 0.0 {
            if self.scanning {
                self.stop_scanning();
            }
            return;
        }
        if !player.ready() {
            return;
        }
        if !self.scanning {
            self.scanning = true;
            self.scan_to = player.at();
            self.since_scan = SCAN_EVERY;
            // Held rather than played: see the note above.
            player.play(false);
        }
        let length = player.length();
        self.transport_left = TRANSPORT_HELD;
        self.scan_to = (self.scan_to + f64::from(self.pull) * SCAN_RATE * f64::from(dt))
            .clamp(0.0, if length > 0.0 { length } else { self.scan_to });
        self.since_scan += dt;
        if self.since_scan >= SCAN_EVERY {
            self.since_scan = 0.0;
            player.seek_to(self.scan_to);
        }
    }

    fn stop_scanning(&mut self) {
        self.scanning = false;
        if let Some(player) = self.player.as_ref() {
            // Where the thumb really left it, rather than the last place a
            // seek happened to land.
            player.seek_to(self.scan_to);
            player.play(true);
        }
    }

    /// How long a direction has been held, which is what grows the seek.
    fn hold(&mut self, dt: f32) {
        self.since_held = (self.since_held + dt).min(f32::MAX / 4.0);
        if self.since_held > HELD_PATIENCE {
            self.held = 0.0;
            self.held_for = 0.0;
        } else if self.held != 0.0 {
            self.held_for += dt;
        }
    }

    /// Keep the light's row on the screen.
    fn scroll_step(&mut self, dt: f32, geometry: &Geometry) {
        let rows = geometry.rows(self.folder.entries.len()) as f32;
        let showing = geometry.visible_rows();
        let most = (rows - showing).max(0.0);
        let row = (self.cursor / geometry.columns.max(1)) as f32;
        let mut target = self.scroll;
        if row < target {
            target = row;
        } else if row > target + showing - 1.0 {
            target = row - showing + 1.0;
        }
        target = target.clamp(0.0, most);
        let (at, moving) = spring(
            self.scroll as f64,
            self.scroll_speed as f64,
            target as f64,
            motion::HIGHLIGHT_SPRING,
            dt as f64,
        );
        self.scroll = at as f32;
        self.scroll_speed = moving as f32;
    }

    /// A click somewhere on the page.
    pub fn press_at(&mut self, spot: Spot, at: [f32; 2], groove: [f32; 4]) -> Option<Sound> {
        match spot {
            Spot::Control(id) if id >= CARD => {
                let index = (id - CARD) as usize;
                if index >= self.folder.entries.len() {
                    return None;
                }
                // Two steps, as everywhere else in this language: the first
                // click carries the light, the second acts.
                if self.cursor != index {
                    self.cursor = index;
                    return Some(Sound::Move);
                }
                self.enter()
            }
            Spot::Control(GROOVE) => {
                self.woken();
                if groove[2] <= 0.0 {
                    return None;
                }
                self.seek_to_part(f64::from((at[0] - groove[0]) / groove[2]))
            }
            Spot::Control(PLAY) => self.play_pause(),
            Spot::Control(VOLUME) => self.mute(),
            // A press on the film is play and pause, which is what a press on
            // a video is everywhere — and where the transport is away, it is
            // what brings it back rather than stopping the film.
            Spot::Control(STAGE) => {
                if self.transport <= 0.5 {
                    self.woken();
                    return None;
                }
                self.play_pause()
            }
            _ => None,
        }
    }

    /// Pointing at something, which moves the light without acting.
    pub fn point_at(&mut self, spot: Spot) -> bool {
        if self.menu.is_open() {
            return self.menu.point_at(spot);
        }
        if self.dialog.is_open() {
            return self.dialog.point_at(spot);
        }
        if self.mode == Mode::Player {
            // A hand moving over the picture is somebody looking for the
            // controls.
            self.woken();
        }
        false
    }

    /// The keyboard's own shorthands reach the same decisions the buttons do,
    /// rather than a second copy of them.
    pub fn command(&mut self, command: Command) -> Option<Sound> {
        self.woken();
        self.run(command).or(Some(Sound::Press))
    }

    /// Everything on the way out: the film stopped, the place written down.
    pub fn closing(&mut self) {
        self.let_the_film_go();
        self.resume.save();
    }
}

/// Ease a number towards another by a fraction of the way, per frame.
fn toward(from: f32, to: f32, step: f32) -> f32 {
    if (to - from).abs() <= 0.001 {
        return to;
    }
    from + (to - from) * step.clamp(0.0, 1.0)
}

/// What to show when the application is handed a path.
///
/// A folder opens as a folder. A film opens as that film, with its own folder
/// behind it — so that closing it lands on the grid it came from rather than
/// on nothing, and the shoulder buttons step through its neighbours.
fn open_at(at: &Path, order: Order, hidden: bool) -> (Folder, usize, Mode) {
    if at.is_dir() {
        return (Folder::read(at, order, hidden), 0, Mode::Grid);
    }
    let Some(parent) = at.parent() else {
        return (Folder::read(at, order, hidden), 0, Mode::Grid);
    };
    let folder = Folder::read(parent, order, hidden);
    match folder.position(at) {
        Some(at) => (folder, at, Mode::Player),
        None => (folder, 0, Mode::Grid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Entry, Kind};

    fn watching() -> View {
        let mut view = View::new(Path::new("/nonexistent"), Order::Name, false);
        view.mode = Mode::Player;
        // As if a file manager had handed over one film, which is what
        // `View::new` sets when it is given a path to a file rather than a
        // folder. The path above is neither, so it is set here.
        view.opened_on_a_film = true;
        view.stage = [0.0, 0.0, 1000.0, 600.0];
        view
    }

    fn film(name: &str) -> Entry {
        Entry {
            path: PathBuf::from(name),
            name: name.to_string(),
            kind: Kind::Film,
            bytes: 0,
            changed: None,
        }
    }

    /// Every letter is the same width, which is all a cut needs to be tested.
    fn ruler(text: Text, string: &str) -> f32 {
        let _ = text;
        string.chars().count() as f32 * 10.0
    }

    #[test]
    fn a_name_too_wide_loses_its_middle_and_keeps_both_ends() {
        let cut = cut_to_fit(&mut ruler, Text::Body, "SomeSeries S01E10.mkv", 120.0);
        assert!(cut.len() < "SomeSeries S01E10.mkv".len());
        assert!(ruler(Text::Body, &cut) <= 120.0);
        assert!(cut.starts_with("Some"), "the front says which series");
        assert!(cut.ends_with("mkv"), "and the back which episode: {cut}");
    }

    #[test]
    fn a_name_that_fits_is_left_exactly_as_it_is() {
        assert_eq!(
            cut_to_fit(&mut ruler, Text::Body, "Short.mkv", 500.0),
            "Short.mkv"
        );
        // And no room at all is an ellipsis rather than a panic.
        assert_eq!(cut_to_fit(&mut ruler, Text::Body, "Short.mkv", 5.0), "…");
    }

    /// The defect this is for: `lxb-render` shapes a one-line label unbounded
    /// and draws it unclipped, so a film named longer than a menu is wide is
    /// written out through both sides of the panel.
    #[test]
    fn a_menu_title_is_cut_to_the_panel_before_it_is_raised() {
        let mut view = watching();
        view.folder.entries = vec![film(
            "A film with a name far longer than any menu is ever going to be wide.mkv",
        )];
        view.cursor = 0;
        view.window = [1600.0, 900.0];
        view.open_menu();
        assert!(!view.menu.is_open(), "asked for, not yet raised");
        view.settle_menu(&mut ruler);
        assert!(view.menu.is_open());

        let scale = lxb_toolkit::metrics::scale_for(900.0);
        let room = (menu::panel_width(0.0) - (menu::MARGIN + menu::LABEL_PADDING) * 2.0) * scale;
        // The title is not readable back off the menu, so the same cut is
        // asked for again: what matters is that it is a cut at all and that
        // what comes out fits.
        let cut = cut_to_fit(&mut ruler, Text::Title, &view.folder.entries[0].name, room);
        assert!(cut.ends_with(".mkv"));
        assert!(ruler(Text::Title, &cut) <= room);
    }

    /// The bug in the screenshot: a menu raised over a film whose details were
    /// already open took its own share of what the details had left, and the
    /// film ended up in the corner of its own window.
    #[test]
    fn a_menu_takes_no_room_from_the_film() {
        let mut view = watching();
        let geometry = Geometry::of([1600.0, 900.0], |value| value, 12.0);
        view.info = true;
        view.info_out = 1.0;
        view.measure(&geometry);
        let without = view.stage_target(&geometry);
        view.window = geometry.window;
        view.open_menu();
        view.settle_menu(&mut ruler);
        assert!(view.menu.is_open());
        assert_eq!(
            view.stage_target(&geometry),
            without,
            "the menu is a hole in the picture, not a bite out of it"
        );
    }

    #[test]
    fn there_is_no_hole_for_a_menu_that_is_not_there() {
        let mut view = watching();
        view.window = [1600.0, 900.0];
        assert!(view.menu_hole().is_none());
        view.open_menu();
        view.settle_menu(&mut ruler);
        // `showing` is how much of it has arrived, and nothing has advanced.
        assert!(view.menu_hole().is_none(), "not until it is on the screen");
        view.menu.advance(1.0);
        let hole = view.menu_hole().expect("a menu on the screen has a hole");
        assert!(hole.rect[2] > 0.0 && hole.rect[3] > 0.0);
        assert!(
            hole.rect[0] >= 0.0 && hole.rect[0] + hole.rect[2] <= 1600.0,
            "and it is on the window: {:?}",
            hole.rect
        );
    }

    /// A film standing aside on a page keeps the shell's wallpaper. Only a
    /// film that has taken the window is on black.
    #[test]
    fn a_film_on_a_page_is_not_on_black() {
        let mut view = watching();
        let geometry = Geometry::of([1600.0, 900.0], |value| value, 12.0);
        view.measure(&geometry);
        assert!(!view.over_the_film(), "there is no film open here");
        view.advance(1.0, &geometry);
        assert_eq!(view.blackout(), 0.0);
        view.mode = Mode::Grid;
        view.black = 1.0;
        assert_eq!(view.blackout(), 0.0, "and a folder is never on black");
    }

    /// **The row of button hints is never inside the transport's panel.** The
    /// picture gives up that one band instead — the only room anything takes
    /// from a playing film.
    #[test]
    fn the_hints_keep_their_own_band_and_the_film_gives_it_up() {
        let geometry = Geometry::of([1600.0, 900.0], |value| value, 12.0);
        let band = geometry.legend_band();
        let bar = geometry.transport_at();
        assert!(
            band[1] >= bar[1] + bar[3],
            "the band is under the bar, not inside it"
        );
        let middle = geometry.legend_at();
        assert!(middle > band[1] && middle < band[1] + band[3]);
        assert_eq!(
            middle,
            900.0 - geometry.foot * 0.5,
            "and it is where it has always been, on every page"
        );
    }

    /// The transport is drawn by `Ui::pane`, which has no opacity — and a hole
    /// cut in a film could not fade even if it had. So it leaves by sliding.
    #[test]
    fn the_chrome_leaves_by_sliding_rather_than_fading() {
        let geometry = Geometry::of([1600.0, 900.0], |value| value, 12.0);
        assert_eq!(geometry.transport_away(1.0), 0.0);
        assert_eq!(geometry.head_away(1.0), 0.0);
        assert!(
            geometry.transport_at()[1] + geometry.transport_away(0.0) >= 900.0,
            "gone means off the bottom of the window"
        );
        assert!(
            geometry.head_pane()[1] - geometry.head_away(0.0) + geometry.head_pane()[3] <= 0.0,
            "and off the top of it"
        );
    }

    /// A menu long enough to scroll shows a window of rows starting at a row
    /// the toolkit keeps to itself, and the panel's height depends on which
    /// rows those are. Getting that wrong drew the film over half the menu.
    #[test]
    fn a_menu_that_scrolls_still_knows_where_its_panel_is() {
        let mut view = watching();
        view.window = [1600.0, 900.0];
        // More rows than any window holds, so it certainly scrolls.
        view.menu_shape = (0..40).map(|_| menu::Row::command(0)).collect();
        view.menu_window = 6;
        view.menu_anchor = [900.0, 450.0, 0.0, 0.0];
        view.menu.open_at(
            view.menu_anchor,
            None,
            (0..40).map(|n| MenuEntry::new(format!("{n}"))).collect(),
        );
        view.menu.set_window(6);
        view.menu.advance(1.0);
        assert!(view.menu.showing());

        // Walk to the end and back, mirroring every step, and check the
        // mirror against what the menu says about itself all the way.
        for delta in [1isize; 39].into_iter().chain([-1isize; 39]) {
            view.menu.step(delta);
            view.follow_the_menu();
            assert_eq!(
                view.menu.scrolled_above(),
                view.menu_first > 0,
                "at row {}",
                view.menu.selected()
            );
            assert!(
                view.menu_hole().is_some(),
                "a menu on the screen always has a hole, at row {}",
                view.menu.selected()
            );
        }
    }

    /// A subtitle somebody chose is read; a folder somebody chose is browsed.
    /// One chooser answers both, and reading the answer against the wrong
    /// question would empty the window and lose the film.
    #[test]
    fn what_the_chooser_answered_is_read_against_what_was_asked() {
        let mut view = watching();
        assert_eq!(view.asking, Asking::AFolder);
        let _ = view.run(Command::AddSubtitles);
        // No portal and no film here, so the chooser refuses and says so
        // rather than leaving the question hanging.
        assert_eq!(view.asking, Asking::AFolder);

        view.asking = Asking::ASubtitle;
        let was = view.folder.path.clone();
        // No player, so there is nothing to read it into — and the folder is
        // still the folder.
        assert_eq!(
            view.chose(Path::new("/nonexistent/film.srt")),
            Some(Sound::Error)
        );
        assert_eq!(view.folder.path, was, "a subtitle is not a folder");
        assert_eq!(view.asking, Asking::AFolder, "and the question is spent");
    }

    /// The transport is what the film gives room to, so a stopped film always
    /// has it and a playing one that nobody has touched does not.
    #[test]
    fn a_film_left_alone_takes_the_whole_screen() {
        let mut view = watching();
        // Nothing is playing, so there is nothing to hide the controls for.
        assert!(view.transport_wanted());
        view.transport_left = 0.0;
        assert!(
            view.transport_wanted(),
            "a film that has not opened keeps its controls"
        );
    }

    #[test]
    fn anything_at_all_brings_the_transport_back() {
        let mut view = watching();
        view.transport_left = 0.0;
        view.woken();
        assert_eq!(view.transport_left, TRANSPORT_HELD);
    }

    /// The stage gives up the transport's room and takes it back, which is the
    /// whole of how a control sits over a film in this design language.
    /// The stage gives up the transport's room, which is the whole of how a
    /// control sits over a film in this design language.
    #[test]
    fn the_stage_gives_up_the_transports_room() {
        let mut view = watching();
        let geometry = Geometry::of([1600.0, 900.0], |value| value, 12.0);
        view.measure(&geometry);
        let with = view.stage_target(&geometry);
        assert_eq!(with[1], geometry.head, "it starts under the head");
        assert!(
            with[3] < geometry.viewport()[3],
            "and is shorter than the viewport by the transport's own room"
        );
        assert!(
            (geometry.viewport()[3] - with[3] - geometry.transport - geometry.gap).abs() < 0.01,
            "by exactly the transport's room"
        );
    }

    /// A film with nothing on the screen but itself is the whole window. Only
    /// reachable with a film really playing, so the target is asked directly.
    #[test]
    fn a_film_nobody_has_touched_is_given_the_whole_window() {
        let view = watching();
        let geometry = Geometry::of([1600.0, 900.0], |value| value, 12.0);
        // What `stage_target` does when nothing wants the controls.
        assert!(view.transport_wanted(), "there is no film open here");
        let whole = [0.0, 0.0, geometry.window[0], geometry.window[1]];
        assert_eq!(whole, [0.0, 0.0, 1600.0, 900.0]);
    }

    #[test]
    fn a_held_direction_seeks_further_the_longer_it_is_held() {
        assert_eq!(step_now(0.0), 10.0);
        assert_eq!(step_now(1.5), 30.0);
        assert_eq!(step_now(4.0), 60.0);
    }

    #[test]
    fn a_hold_is_forgotten_once_the_presses_stop_arriving() {
        let mut view = watching();
        view.held = 1.0;
        view.since_held = 0.0;
        view.hold(0.1);
        assert!(view.held_for > 0.0, "still held");
        view.since_held = HELD_PATIENCE;
        view.hold(0.1);
        assert_eq!(view.held, 0.0);
        assert_eq!(view.held_for, 0.0, "and the step drops back to the first");
    }

    #[test]
    fn volume_stops_at_both_ends_and_says_so() {
        let mut view = watching();
        view.volume = 1.0;
        assert_eq!(view.change_volume(VOLUME_STEP), Some(Sound::Error));
        view.volume = 0.5;
        assert_eq!(view.change_volume(VOLUME_STEP), Some(Sound::Move));
        assert!((view.volume - 0.55).abs() < 0.0001);
    }

    /// Nobody presses volume-up to stay silent.
    #[test]
    fn turning_it_up_unmutes() {
        let mut view = watching();
        view.muted = true;
        view.volume = 0.5;
        view.change_volume(VOLUME_STEP);
        assert!(!view.muted);
    }

    #[test]
    fn a_wheel_in_the_grid_walks_the_grid_instead_of_changing_the_volume() {
        let mut view = watching();
        view.mode = Mode::Grid;
        view.columns = 1;
        view.folder.entries = (0..4)
            .map(|number| film(&format!("{number}.mkv")))
            .collect();
        view.wheel(1, [10.0, 10.0]);
        assert_eq!(view.cursor, 1, "one notch is one row");
        assert_eq!(view.volume, 1.0, "and nothing was made quieter");
    }

    /// A wall of films to fly out of and back into.
    fn browsing() -> (View, Geometry) {
        let mut view = watching();
        view.mode = Mode::Grid;
        view.opened_on_a_film = false;
        view.folder.entries = (0..4)
            .map(|number| film(&format!("{number}.mkv")))
            .collect();
        let geometry = Geometry::of([1600.0, 900.0], |value| value, 12.0);
        view.measure(&geometry);
        (view, geometry)
    }

    /// The whole of an opening in one number, and both ends of it: the picture
    /// starts on the card that was pressed and finishes on the window.
    #[test]
    fn a_film_opens_out_of_the_card_it_was_pressed_on() {
        let (mut view, geometry) = browsing();
        view.cursor = 2;
        let card = geometry.frame_in(geometry.card(2));
        view.act(Action::Accept);
        assert_eq!(view.mode, Mode::Player, "the press opened it");
        assert_eq!(view.grown(), 0.0, "and it has not moved yet");

        view.advance(1.0 / 600.0, &geometry);
        for (was, is) in card.iter().zip(&view.stage) {
            assert!(
                (was - is).abs() < 1.0,
                "the first frame is the card: {card:?} against {:?}",
                view.stage
            );
        }

        let mut frames = 0;
        while view.crossing().is_some() && frames < 600 {
            let grown = view.grown();
            view.advance(1.0 / 60.0, &geometry);
            assert!(view.grown() >= grown, "it never goes backwards");
            frames += 1;
        }
        assert!(frames > 4, "it took a moment: {frames} frames");
        assert_eq!(view.grown(), 1.0, "and arrived");
        // Exactly where the stage would have sprung to on its own, so that the
        // last frame of the animation is the first frame of the page and there
        // is nothing left over to settle.
        let target = view.stage_target(&geometry);
        for (wanted, is) in target.iter().zip(&view.stage) {
            assert!(
                (wanted - is).abs() < 1.0,
                "it landed on {:?} rather than {target:?}",
                view.stage
            );
        }
        assert!(view.stage[2] > geometry.card(0)[2] * 3.0, "and it is large");
    }

    /// And the way back, which is the same number run the other way — with the
    /// film still on the screen for it. A picture dropped at the press would
    /// have nothing to shrink.
    #[test]
    fn leaving_a_film_puts_it_back_into_its_card() {
        let (mut view, geometry) = browsing();
        view.cursor = 1;
        view.act(Action::Accept);
        while view.crossing().is_some() {
            view.advance(1.0 / 60.0, &geometry);
        }

        // As if the film had really opened, which no path in a test does.
        view.showing = 1.0;
        view.act(Action::Back);
        assert_eq!(view.mode, Mode::Grid, "the wall is the page again");
        assert!(view.leaving(), "and the picture is on its way back");
        view.advance(1.0 / 60.0, &geometry);
        assert!(
            view.showing > 0.9,
            "a picture that had gone would have nothing to shrink"
        );

        while view.crossing().is_some() {
            view.advance(1.0 / 60.0, &geometry);
        }
        assert_eq!(view.grown(), 0.0);
        assert_eq!(view.showing, 0.0, "gone by the time it lands");
        let card = geometry.frame_in(geometry.card(1));
        for (was, is) in card.iter().zip(&view.stage) {
            assert!((was - is).abs() < 1.0, "{card:?} against {:?}", view.stage);
        }
    }

    /// One film after another in a run is not a change of page: nobody pressed
    /// a card, and the wall is not on the screen to fly out of.
    #[test]
    fn a_run_of_films_does_not_fly_out_of_a_card() {
        let (mut view, geometry) = browsing();
        view.run_the_folder = true;
        view.cursor = 0;
        view.mode = Mode::Player;
        view.folder.entries = (0..2)
            .map(|number| film(&format!("{number}.mkv")))
            .collect();
        // The end of the first film, which is what carries a run on.
        let path = view.folder.entries[0].path.clone();
        view.resume.forget(&path);
        view.cursor = 1;
        view.show_the_film();
        view.settled_on_the_film();
        assert!(view.crossing().is_none(), "it cut rather than flew");
        assert_eq!(view.grown(), 1.0);
        let _ = geometry;
    }

    /// The card draws its poster in one rectangle and the film grows out of
    /// the same one. Two answers would drift, and the drift would show as a
    /// film jumping on the first frame of every opening.
    #[test]
    fn the_picture_on_a_card_is_one_rectangle() {
        let geometry = Geometry::of([1600.0, 900.0], |value| value, 12.0);
        let card = geometry.card(0);
        let frame = geometry.frame_in(card);
        assert!(frame[0] > card[0] && frame[1] > card[1], "inset on both");
        assert!(
            frame[1] + frame[3] < card[1] + card[3] - 1.0,
            "and a line of name left under it"
        );
        assert!(
            geometry.frame_radius() > 0.0,
            "the picture is rounded, less than the card around it"
        );
        assert!(geometry.frame_radius() < Metric::CardRadius.on(900.0));
    }

    /// The defect the user reported against the photo viewer, which was here
    /// too: the picture was drawn over the details pane on the way in *and* on
    /// the way out. The pane arrived on its own clock and the stage crawled
    /// after it on a spring, so for a fifth of a second the stage still
    /// covered the pane — and the film is drawn in a pass after the toolkit's,
    /// so it went straight over the top.
    #[test]
    fn the_film_never_reaches_the_details_pane() {
        let geometry = Geometry::of([1600.0, 900.0], |value| value, 12.0);
        let mut view = watching();
        view.window = geometry.window;
        view.transport_room = geometry.transport + geometry.gap;
        view.info = true;
        for step in 0..=20 {
            view.info_out = step as f32 / 20.0;
            let stage = view.stands_aside(&geometry);
            let pane = view.details_pane(&geometry);
            assert!(
                stage[0] + stage[2] <= pane[0] + 0.01,
                "at {}: the stage reaches {} and the pane starts at {}",
                view.info_out,
                stage[0] + stage[2],
                pane[0]
            );
        }
    }

    /// The crop a card puts on a poster unwinds as the picture grows, so that
    /// the first frame is the card and the last is the film.
    #[test]
    fn the_crop_a_card_puts_on_a_poster_unwinds() {
        let mut view = watching();
        view.stage = [0.0, 0.0, 1000.0, 600.0];
        // A tall picture on a wide stage: the two shapes disagree as much as
        // they ever will.
        let tall = Some(0.5);
        view.going = Going::In;
        view.to_go = 1.0;
        assert_eq!(
            view.standing_rect(tall),
            view.stage,
            "on the card it fills the card, exactly as the card draws it"
        );
        view.to_go = 0.0;
        let landed = view.standing_rect(tall);
        assert!((landed[2] / landed[3] - 0.5).abs() < 0.01, "{landed:?}");
        assert!(landed[3] <= view.stage[3] + 0.01, "and inside the stage");
        assert_eq!(
            view.standing_rect(None),
            view.stage,
            "a shape nobody knows yet is not a shape to ease towards"
        );
        view.fill = true;
        assert_eq!(
            view.standing_rect(tall),
            view.stage,
            "a film asked to fill the stage is cropped to it on purpose"
        );
    }

    /// A file manager's double-click: Back is the way out of the application,
    /// not the way into a folder nobody asked for.
    #[test]
    fn opened_on_one_film_back_closes() {
        let mut view = watching();
        assert!(view.opened_on_a_film, "started in the player");
        assert!(view.closes_on_back());
        view.act(Action::Back);
        assert!(view.quit, "Back closed it");
    }

    /// Until they say otherwise, which is what the menu row is for.
    #[test]
    fn asking_for_the_folder_makes_it_an_ordinary_walk() {
        let mut view = watching();
        view.command(Command::BackToGrid);
        assert_eq!(view.mode, Mode::Grid);
        assert!(!view.opened_on_a_film, "no longer a handed-over film");

        view.top = PathBuf::from("/one");
        view.folder.path = PathBuf::from("/one/two");
        assert!(!view.closes_on_back());
        view.act(Action::Back);
        assert!(
            !view.quit,
            "it walked out of the folder rather than closing"
        );
        assert_eq!(view.folder.path, PathBuf::from("/one"));
    }

    #[test]
    fn back_closes_at_the_folder_the_walk_was_opened_on() {
        let mut view = watching();
        view.mode = Mode::Grid;
        view.opened_on_a_film = false;
        view.folder.path = PathBuf::from("/home/someone/Films");
        view.top = PathBuf::from("/home/someone/Films");
        assert!(view.closes_on_back());
        view.act(Action::Back);
        assert!(
            view.quit,
            "it closed rather than walking up to /home/someone"
        );
    }

    #[test]
    fn back_walks_up_to_the_top_but_not_past_it() {
        let mut view = watching();
        view.mode = Mode::Grid;
        view.opened_on_a_film = false;
        view.top = PathBuf::from("/home/someone/Films");
        view.folder.path = PathBuf::from("/home/someone/Films/Series");
        assert!(!view.closes_on_back());
        view.act(Action::Back);
        assert!(!view.quit);
        assert_eq!(view.folder.path, PathBuf::from("/home/someone/Films"));
        assert!(view.closes_on_back(), "and now it is at the top");
    }

    #[test]
    fn asking_to_go_above_the_top_moves_the_top() {
        let mut view = watching();
        view.mode = Mode::Grid;
        view.opened_on_a_film = false;
        view.folder.path = PathBuf::from("/home/someone/Films");
        view.top = PathBuf::from("/home/someone/Films");
        view.command(Command::UpAFolder);
        assert_eq!(view.top, PathBuf::from("/home/someone"));
        assert!(view.closes_on_back(), "the new folder is the new top");
    }

    /// A 2.39:1 film on a 16:9 stage: fitting leaves bars above and below,
    /// filling crops the ends off and the scissor takes them.
    #[test]
    fn filling_takes_the_larger_scale_and_fitting_the_smaller() {
        let stage = [0.0, 0.0, 1600.0, 900.0];
        let film = (2390.0, 1000.0);
        let fitted = drawn_size(stage, film, false);
        assert!(
            (fitted[0] - 1600.0).abs() < 0.5,
            "fitted across: {fitted:?}"
        );
        assert!(fitted[1] < 900.0, "and short of the stage: {fitted:?}");

        let filled = drawn_size(stage, film, true);
        assert!((filled[1] - 900.0).abs() < 0.5, "filled down: {filled:?}");
        assert!(filled[0] > 1600.0, "and over the ends: {filled:?}");

        // Both keep the film's own shape, which is the one thing neither may
        // change.
        for size in [fitted, filled] {
            assert!(
                (size[0] / size[1] - film.0 / film.1).abs() < 0.001,
                "the shape changed: {size:?}"
            );
        }
    }

    /// A film that has not opened has no size, and nothing may invent one.
    #[test]
    fn nothing_is_drawn_for_a_film_that_has_not_opened() {
        let mut view = watching();
        let geometry = Geometry::of([1600.0, 900.0], |value| value, 12.0);
        view.measure(&geometry);
        assert_eq!(view.shown, [0.0; 2]);
        assert_eq!(
            drawn_size([0.0, 0.0, 100.0, 100.0], (0.0, 0.0), false),
            [0.0; 2]
        );
    }
}
