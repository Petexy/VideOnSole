//! Videonsole — a film browser and player for LineXinBar, driven by a
//! controller.
//!
//!     videonsole                        # the videos folder
//!     videonsole ~/Films                # that folder
//!     videonsole ~/Films/holiday.mkv    # that film
//!     videonsole --shot page.png        # one settled frame, with no display
//!
//! It is an ordinary Wayland application — no shell protocol, nothing private
//! — and it runs under GNOME or Plasma as readily as under LineXinBar.
//!
//! **It draws its own window**, which most applications built on the toolkit
//! do not need to do. The reason is in `film.rs`: the film is drawn at its own
//! resolution in a pass of this application's own, over the frame `lxb-render`
//! composed, and the subtitle in a second pass over that. Everything else on
//! the screen — the glass, the cards, the light that travels between them, the
//! transport, the menu, the chooser, the marks and the words — is the toolkit
//! answering for the material.
//!
//! Six threads: this one, which draws; one per film, which decodes it; two
//! reading films for their poster frames; and the two the sound device and the
//! controller library keep for themselves.

mod draw;
mod facts;
mod film;
mod i18n;
mod legend;
mod library;
mod pad;
mod player;
mod poster;
mod resume;
mod subtitles;
mod view;

use std::path::PathBuf;
use std::sync::Arc;

use lxb_input::Controls;
use lxb_render::{Spot, Ui, WallpaperClock};
use lxb_sound::Sounds;
use lxb_toolkit::{
    accent::Accent,
    input::{Action, Key, Wheel},
    settings::ShellTheme,
    sound::Sound,
};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::ModifiersState,
    platform::wayland::WindowAttributesExtWayland,
    window::{CursorIcon, Window, WindowId},
};

use film::Films;
use library::Order;
use poster::Posters;
use subtitles::Captions;
use view::{Command, Geometry, Mode, View};

/// The stable id, which agrees with the desktop entry, the executable name and
/// `StartupWMClass`. See the toolkit's docs/application-development.md.
///
/// It is **not** what the application is called to a person. The visible name
/// is for people and may be anything; this one is for matching a launched
/// process to the window that appeared, and it has to agree in five places.
const APP_ID: &str = "videonsole";

/// What the window is drawn into. `Bgra8UnormSrgb` is what a Wayland surface
/// wants; the film's own pass is built for whichever this is, so that a film
/// and the page under it are blended in the same space.
const SURFACE: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let result = if arguments.iter().any(|one| one == "--version") {
        println!("videonsole {}", env!("CARGO_PKG_VERSION"));
        Ok(())
    } else if arguments.iter().any(|one| one == "--help" || one == "-h") {
        println!("{HELP}");
        Ok(())
    } else if arguments.iter().any(|one| one == "--controllers") {
        controllers();
        Ok(())
    } else if let Some(at) = arguments.iter().position(|one| one == "--shot") {
        match arguments.get(at + 1) {
            Some(path) => shot(path, &arguments),
            None => Err(String::from("--shot needs a file to write")),
        }
    } else {
        window(&arguments)
    };
    if let Err(message) = result {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

/// Where to open, from the arguments or from the user's own videos.
fn opening(arguments: &[String]) -> PathBuf {
    arguments
        .iter()
        .find(|one| !one.starts_with('-'))
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .unwrap_or_else(library::default_folder)
}

fn window(arguments: &[String]) -> Result<(), String> {
    let theme = ShellTheme::load();
    // Read once, before any thread of this process starts, because reading it
    // takes it out of the environment. A window opening in front of a
    // wallpaper already on screen draws the second that screen is showing
    // rather than starting the animation again in front of the user.
    let wallpaper =
        WallpaperClock::from_environment(theme.accent.name).unwrap_or_else(WallpaperClock::local);

    let event_loop = EventLoop::new().map_err(|err| err.to_string())?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut application = Application::new(opening(arguments), theme, wallpaper);
    event_loop
        .run_app(&mut application)
        .map_err(|err| err.to_string())
}

struct Application {
    instance: wgpu::Instance,
    window: Option<Arc<Window>>,
    surface: Option<wgpu::Surface<'static>>,
    ui: Option<Ui>,
    films: Option<Films>,
    captions: Option<Captions>,
    posters: Posters,

    view: View,
    theme: ShellTheme,
    /// The palette as the renderer wants it. Built once from the setting: the
    /// shell publishes no accent-change protocol to ordinary applications, so
    /// this is a snapshot taken at startup rather than something to follow.
    accent: Accent,
    geometry: Geometry,
    /// Where the groove was drawn last frame, so a click on it can be turned
    /// back into a place in the film.
    groove: [f32; 4],
    /// Which film is on the card now, so the last frame of the one before it
    /// is not left on the screen when another is opened.
    showing: Option<PathBuf>,

    opened: std::time::Instant,
    last: std::time::Instant,
    /// The wallpaper's own clock, which is the one thing here that may have
    /// started before this process did.
    wallpaper: WallpaperClock,

    controls: Controls,
    /// Only the triggers, and only because scanning wants an amount rather
    /// than an event. Everything else about every controller is `controls`.
    pad: pad::Pad,
    sounds: Sounds,
    pointer: [f32; 2],
    wheel: Wheel,
    shift: bool,
    hand: bool,
    /// Which control the legend should picture.
    pad_in_hand: bool,
    /// `VIDEONSOLE_DEBUG_ACTIONS` in the environment: every action this is
    /// driven by, on stderr. It is the only way to tell a control that is not
    /// reaching the application from one that is reaching it and doing
    /// nothing, and those two have completely different causes.
    say_actions: bool,
    said_pull: f32,
}

impl Application {
    fn new(at: PathBuf, theme: ShellTheme, wallpaper: WallpaperClock) -> Application {
        let controls = Controls::new();
        if let Some(trouble) = controls.trouble() {
            // Said once and never again: a controller is an enhancement, not a
            // startup requirement, and this is the one line somebody with a
            // dead pad will go looking for.
            eprintln!("no controller input: {trouble}");
        }
        let pad = pad::Pad::new();
        if let Some(trouble) = pad.trouble() {
            eprintln!("no trigger scanning: {trouble}");
        }
        if controls.pads() == 0 {
            eprintln!(
                "videonsole: no controller found; the keyboard and mouse still work.\n\
                 videonsole: run `videonsole --controllers` to see what was looked at."
            );
        }
        let pad_in_hand = controls.pads() > 0;
        Application {
            instance: lxb_render::instance(),
            window: None,
            surface: None,
            ui: None,
            films: None,
            captions: None,
            posters: Posters::new(),
            view: View::new(&at, Order::default(), false),
            accent: Accent::new(theme.accent.name).unwrap_or_else(Accent::default_accent),
            theme,
            geometry: Geometry::of([1280.0, 800.0], |value| value, 12.0),
            groove: [0.0; 4],
            showing: None,
            opened: std::time::Instant::now(),
            last: std::time::Instant::now(),
            wallpaper,
            controls,
            pad,
            sounds: Sounds::new(),
            pointer: [0.0; 2],
            wheel: Wheel::default(),
            shift: false,
            hand: false,
            pad_in_hand,
            say_actions: std::env::var_os("VIDEONSOLE_DEBUG_ACTIONS").is_some(),
            said_pull: 0.0,
        }
    }

    fn now(&self) -> std::time::Duration {
        self.opened.elapsed()
    }

    /// Do one action and answer it.
    ///
    /// The single place a sound is played, so that a move made with a
    /// direction and the same move made with a click cannot sound different.
    fn act(&mut self, action: Action) {
        let answer = self.view.act(action);
        self.answer(answer);
    }

    fn answer(&mut self, sound: Option<Sound>) {
        if let Some(sound) = sound {
            self.sounds.play(sound);
        }
    }

    fn spot(&self) -> Spot {
        let [x, y] = self.pointer;
        self.ui
            .as_ref()
            .map(|ui| ui.at(x, y))
            .unwrap_or(Spot::Nothing)
    }

    /// A click, wherever it landed.
    fn press_at(&mut self, spot: Spot, right: bool) {
        if self.view.files.busy() {
            let answer = self.view.files.press_at(spot, right);
            self.answer(answer);
            return;
        }
        if right {
            self.act(Action::Menu);
            return;
        }
        if self.view.menu.is_open() {
            // A click on a row of the menu is a press of that row; anywhere
            // else dismisses it, which is how every panel on every desktop
            // closes.
            match spot {
                Spot::MenuRow { .. } => self.act(Action::Accept),
                _ => {
                    self.view.menu.close();
                    self.sounds.play(Sound::Back);
                }
            }
            return;
        }
        if self.view.dialog.is_open() {
            if matches!(spot, Spot::DialogButton(_)) {
                self.act(Action::Accept);
            }
            return;
        }
        // The legend is a row of controls: a click on a pair is a press of the
        // button it pictures, which is how a mouse reaches a Back that is only
        // ever drawn there.
        if let Spot::Control(id) = spot {
            let hints = draw::hints(&self.view);
            if let Some(button) = legend::pressed(id, &hints, self.pad_in_hand) {
                self.act(button.action());
                return;
            }
        }
        let pointer = self.pointer;
        let groove = self.groove;
        let answer = self.view.press_at(spot, pointer, groove);
        if answer.is_some() {
            self.view.pressing.press();
        }
        self.answer(answer);
    }

    /// The letters and digits this application binds for itself.
    ///
    /// Deliberately clear of `Action::of_letter`'s `wasd`, `hjkl` and `y`,
    /// which stay what they are everywhere else in this language: moving.
    fn letter(&mut self, letter: char) -> bool {
        // A digit is a tenth of the way through, which is the one shorthand
        // every video on this machine has.
        if let Some(digit) = letter.to_digit(10) {
            if self.view.mode == Mode::Player {
                let answer = self.view.seek_to_part(f64::from(digit) / 10.0);
                self.answer(answer);
                return true;
            }
            return false;
        }
        let answer = match letter {
            '+' | '=' => self.view.change_volume_by(0.05),
            '-' | '_' => self.view.change_volume_by(-0.05),
            'm' | 'M' => self.view.mute(),
            'f' | 'F' => self.view.command(Command::Fill),
            'b' | 'B' => self.view.command(Command::FromTheStart),
            'r' | 'R' => self.view.command(Command::RunTheFolder),
            'c' | 'C' => self.view.cycle_subtitles(),
            'i' | 'I' => self.view.command(Command::Info),
            'n' | 'N' => self.view.step_film(1),
            'p' | 'P' => self.view.step_film(-1),
            'g' | 'G' => self.view.command(Command::BackToGrid),
            'o' | 'O' => self.view.command(Command::OpenFolder),
            _ => return false,
        };
        self.answer(answer);
        true
    }
}

impl ApplicationHandler for Application {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(crate::i18n::text("app-title"))
            // An application on LineXinBar is maximised and pinned to the
            // display it launched on; this size is for every other desktop.
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0))
            .with_name(APP_ID, APP_ID);
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                eprintln!("no window: {err}");
                event_loop.exit();
                return;
            }
        };
        let surface = match self.instance.create_surface(window.clone()) {
            Ok(surface) => surface,
            Err(err) => {
                eprintln!("no surface: {err}");
                event_loop.exit();
                return;
            }
        };
        let size = window.inner_size();
        let ui = match pollster::block_on(Ui::new(
            &self.instance,
            Some(&surface),
            SURFACE,
            size.width,
            size.height,
        )) {
            Ok(ui) => ui,
            Err(message) => {
                eprintln!("{message}");
                event_loop.exit();
                return;
            }
        };

        configure(&surface, &ui, size.width, size.height);
        self.films = Some(Films::new(&ui.device, SURFACE));
        self.captions = Some(Captions::new(&ui.device, &ui.queue, SURFACE));
        self.window = Some(window);
        self.surface = Some(surface);
        self.ui = Some(ui);
        self.opened = std::time::Instant::now();
        self.last = self.opened;
        self.wallpaper.restart();
        // Only now: opening a film starts a thread and a sound device, and a
        // soundtrack that began before there was a window to draw it in would
        // be a film playing over a black rectangle.
        self.view.begin();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => {
                self.view.closing();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let (Some(surface), Some(ui)) = (self.surface.as_ref(), self.ui.as_ref()) {
                    configure(surface, ui, size.width, size.height);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = [position.x as f32, position.y as f32];
                self.pad_in_hand = false;
                let spot = self.spot();
                if self.view.files.busy() {
                    self.view.files.point_at(spot);
                } else {
                    self.view.point_at(spot);
                }
                let hand = spot.pressable();
                if hand != self.hand {
                    self.hand = hand;
                    window.set_cursor(if hand {
                        CursorIcon::Pointer
                    } else {
                        CursorIcon::Default
                    });
                }
            }
            WindowEvent::CursorLeft { .. } => self.wheel.reset(),
            WindowEvent::MouseInput { state, button, .. } => {
                // On the press rather than the release, as every other control
                // in this language fires.
                if state == ElementState::Released {
                    return;
                }
                self.pad_in_hand = false;
                let spot = self.spot();
                self.press_at(spot, button == MouseButton::Right);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let notches = match delta {
                    MouseScrollDelta::LineDelta(_, lines) => self.wheel.notches(-lines),
                    MouseScrollDelta::PixelDelta(at) => self.wheel.distance(-at.y as f32),
                };
                self.pad_in_hand = false;
                let pointer = self.pointer;
                let answer = self.view.wheel(notches, pointer);
                self.answer(answer);
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.shift = modifiers.state().contains(ModifiersState::SHIFT);
            }
            // A window that has lost focus has had every control let go of.
            WindowEvent::Focused(false) => self.controls.release(),
            WindowEvent::KeyboardInput { event, .. } => {
                // The platform's own repeat is dropped: the pace a held
                // direction moves at belongs to the interface, and `lxb-input`
                // invents the same middle for an arrow key as for a D-pad.
                if event.repeat {
                    return;
                }
                let down = event.state == ElementState::Pressed;
                let key = lxb_input::key_of(&event, self.shift);
                if down {
                    self.pad_in_hand = false;
                }
                // Everything a keyboard means to the chooser, in one call.
                if down && self.view.files.busy() {
                    self.view.files.key(key, event.text.as_deref());
                    return;
                }
                let Some(key) = key else {
                    return;
                };
                if let Key::Letter(letter) = key {
                    if down && !self.letter(letter) {
                        if let Some(action) = Action::of_letter(letter) {
                            self.act(action);
                        }
                    }
                    return;
                }
                let now = self.now();
                if let Some(action) = self.controls.key(key, down, now) {
                    self.act(action);
                }
            }
            WindowEvent::RedrawRequested => self.frame(event_loop, &window),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}

impl Application {
    fn frame(&mut self, event_loop: &ActiveEventLoop, window: &Arc<Window>) {
        let now = std::time::Instant::now();
        // An application stopped while it is hidden must not treat the time it
        // was away as one frame; clamp it, as the toolkit's own spring does.
        let dt = (now - self.last).as_secs_f32().clamp(0.0, 0.1);
        self.last = now;

        let size = window.inner_size();
        let elapsed = self.wallpaper.elapsed_secs();
        self.posters.settle();
        let (Some(surface), Some(ui), Some(films), Some(captions)) = (
            self.surface.as_ref(),
            self.ui.as_mut(),
            self.films.as_mut(),
            self.captions.as_mut(),
        ) else {
            return;
        };

        ui.begin(
            size.width as f32,
            size.height as f32,
            elapsed,
            &self.accent,
            self.theme.wallpaper,
            self.theme.icons,
        );

        self.geometry = Geometry::of(
            [size.width as f32, size.height as f32],
            |value| ui.s(value),
            ui.m(lxb_toolkit::metrics::Metric::Gap),
        );
        // Measure, act, move, draw — in that order, so that a press this frame
        // is answered against the film as it is now rather than as it was
        // before the last animation settled.
        self.view.measure(&self.geometry);

        let actions = self.controls.poll(self.opened.elapsed());
        if !actions.is_empty() {
            self.pad_in_hand = self.controls.pads() > 0;
        }
        // And the one thing a list of actions cannot carry: how far.
        self.pad.settle();
        let pull = self.pad.pull();
        if pull != 0.0 {
            self.pad_in_hand = true;
        }
        self.view.set_pull(pull);
        if self.say_actions && (pull - self.said_pull).abs() > 0.05 {
            self.said_pull = pull;
            eprintln!("videonsole: triggers {pull:+.2}");
        }
        for action in actions {
            if self.say_actions {
                eprintln!("videonsole: {action:?}");
            }
            // The fields are reached one at a time rather than through
            // `answer`, because `ui` is borrowed for the whole of this frame
            // and a method taking all of `self` would want it back.
            if let Some(sound) = self.view.act(action) {
                self.sounds.play(sound);
            }
        }

        // A folder or a subtitle chosen through the chooser, whether it
        // answered here or in the desktop's own panel.
        if let Some(chosen) = self.view.files.answered() {
            if let Some(path) = chosen.into_iter().next() {
                if let Some(sound) = self.view.chose(&path) {
                    self.sounds.play(sound);
                }
            }
        }
        self.view.files.hand(self.pad_in_hand);
        if self.view.quit {
            self.view.closing();
            event_loop.exit();
            return;
        }

        self.view.advance(dt, &self.geometry);

        // The films on the screen, and a row either side, are worth opening
        // for their poster and their length.
        let (first, last) = draw::shown_range(&self.view, &self.geometry);
        self.posters.want(&self.view.wanted(first, last));

        // Another film: the one before it goes off the card rather than
        // hanging on the screen behind the new one's first frame.
        let playing = self
            .view
            .player
            .as_ref()
            .map(|player| player.path().to_path_buf());
        if playing != self.showing {
            self.showing = playing;
            films.clear();
        }
        // And the frame whose time has come, if one has.
        if let Some(frame) = self
            .view
            .player
            .as_ref()
            .and_then(|player| player.frame_due())
        {
            films.show(&ui.device, &ui.queue, &frame);
        }

        // A menu asked for this frame is raised here, where there is
        // something that can measure a word: every name on it has to be cut
        // to the panel before it is handed over. See `View::settle_menu`.
        self.view
            .settle_menu(&mut |text, string| ui.measure(text, string));

        let over = draw::draw(
            &mut self.view,
            ui,
            &self.geometry,
            &self.posters,
            self.pad_in_hand,
            self.theme.icons,
        );
        self.groove = over.groove;
        let screen = [size.width as f32, size.height as f32];
        captions.settle(&ui.device, &ui.queue, screen, over.caption.as_ref());

        use wgpu::CurrentSurfaceTexture as Acquired;
        match surface.get_current_texture() {
            Acquired::Success(frame) | Acquired::Suboptimal(frame) => {
                let target = frame
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                if let Err(message) = ui.end(&target) {
                    eprintln!("{message}");
                }
                // And then the film over the frame the toolkit composed, and
                // the subtitle over the film. In that order and no other:
                // each is drawn on top of what came before it.
                let mut encoder =
                    ui.device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("film"),
                        });
                films.draw(
                    &ui.queue,
                    &mut encoder,
                    &target,
                    screen,
                    film::Showing {
                        placement: over.film,
                        holes: &over.holes,
                        blackout: over.blackout,
                        curtain: over.curtain,
                        behind: over.behind,
                    },
                );
                captions.draw(&mut encoder, &target);
                ui.queue.submit(Some(encoder.finish()));
                ui.queue.present(frame);
            }
            Acquired::Outdated | Acquired::Lost => configure(surface, ui, size.width, size.height),
            // Occluded, timed out, or refused: skip the frame rather than draw
            // one nobody will see.
            _ => {}
        }
    }
}

fn configure(surface: &wgpu::Surface<'_>, ui: &Ui, width: u32, height: u32) {
    surface.configure(
        &ui.device,
        &wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: SURFACE,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        },
    );
}

/// One settled frame, to a PNG, with no display at all.
///
/// The same page functions, the same renderer and the same film and subtitle
/// passes as the window — which is what makes it worth looking at, and is how
/// an interface in this language is checked without a screen.
fn shot(path: &str, arguments: &[String]) -> Result<(), String> {
    let named = |flag: &str| -> Option<String> {
        arguments
            .iter()
            .position(|one| one == flag)
            .and_then(|at| arguments.get(at + 1))
            .cloned()
    };
    let number = |flag: &str, fallback: u32| -> u32 {
        named(flag)
            .and_then(|value| value.parse().ok())
            .unwrap_or(fallback)
    };
    let width = number("--width", 1600);
    let height = number("--height", 900);

    let theme = ShellTheme::load();
    let accent = Accent::new(theme.accent.name).unwrap_or_else(Accent::default_accent);
    // An sRGB target rather than the toolkit's own float one, so that what
    // comes back is already the bytes a PNG wants.
    const SHOT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
    let instance = lxb_render::instance();
    let mut ui = pollster::block_on(Ui::new(&instance, None, SHOT, width, height))?;
    let mut films = Films::new(&ui.device, SHOT);
    let mut captions = Captions::new(&ui.device, &ui.queue, SHOT);
    let mut posters = Posters::new();

    let at = arguments
        .iter()
        .skip(1)
        .find(|one| !one.starts_with('-') && *one != path)
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .unwrap_or_else(library::default_folder);
    let mut view = View::new(&at, Order::default(), false);
    if let Some(row) = named("--row").and_then(|value| value.parse::<usize>().ok()) {
        view.cursor = row.min(view.folder.entries.len().saturating_sub(1));
    }
    // Whatever it was handed. `View::new` opens no film — nothing decodes
    // before there is somewhere to draw it — so this is the same call the
    // window makes once its surface exists.
    view.begin();
    // A picture of a page in motion rather than at rest: settle everything
    // first, then press, then count exactly the frames that were asked for.
    // Nothing else can photograph an animation — the loop below is built to
    // wait until nothing is moving.
    let after = named("--after").and_then(|value| value.parse::<f32>().ok());
    // With `--after` the *last* press waits for the middle of the loop, where
    // the page it acts on is really there: a poster that has not arrived is a
    // card with nothing in it to grow out of, and a film that is not up cannot
    // shrink back into one. Everything before that last press is done here.
    let then = named("--then").unwrap_or_else(|| {
        String::from(if arguments.iter().any(|one| one == "--back") {
            "back"
        } else {
            "play"
        })
    });
    if after.is_none() || then != "play" {
        press_the_page(&mut view, arguments);
    }
    let mut pressed = after.is_none().then_some(0);
    if arguments.iter().any(|one| one == "--details") {
        let _ = view.command(Command::Info);
    }
    // Everything asked for on the command line is applied to the view *after*
    // a frame has been measured, exactly as a real press is.
    let to = named("--at").and_then(|value| value.parse::<f64>().ok());
    // A film opened by name is playing, which is the one state `--play`
    // cannot photograph. Stopping it is how the page beside a film — the
    // wallpaper, the stage standing aside — is photographed at all.
    let stopping = arguments.iter().any(|one| one == "--pause");
    let mut menu_wanted =
        arguments.iter().any(|one| one == "--menu") && named("--then").as_deref() != Some("menu");
    let mut asked = false;

    let texture = ui.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("shot"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SHOT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target = texture.create_view(&wgpu::TextureViewDescriptor::default());

    // Drawn until the films have been opened and every animation has settled,
    // so what is photographed is the page at rest rather than on its way
    // there. Six seconds of frames is far longer than any of it takes; the
    // loop leaves as soon as nothing is moving.
    let mut over = draw::Over::default();
    for frame in 0..360 {
        let dt = 1.0 / 60.0;
        posters.settle();
        ui.begin(
            width as f32,
            height as f32,
            6.0,
            &accent,
            theme.wallpaper,
            theme.icons,
        );
        let geometry = Geometry::of(
            [width as f32, height as f32],
            |value| ui.s(value),
            ui.m(lxb_toolkit::metrics::Metric::Gap),
        );
        view.measure(&geometry);
        // Once the film is really open, not before: a seek asked of a player
        // that has not read its own header does nothing at all.
        let open = view
            .player
            .as_ref()
            .is_none_or(|player| player.ready() || player.trouble().is_some());
        if !asked && open {
            asked = true;
            if let Some(to) = to {
                let _ = view.seek_to_seconds(to);
            }
            if stopping {
                if let Some(player) = view.player.as_ref() {
                    player.play(false);
                }
            }
            // The same call the chooser's answer makes, so a screenshot of an
            // added track goes down the road a press does.
            if let Some(file) = named("--subtitle") {
                let _ = view.add_subtitle_file(std::path::Path::new(&file));
            }
            if menu_wanted {
                view.open_menu();
                menu_wanted = false;
            }
        }
        // A photograph of a player is a photograph of its controls; without
        // this the transport fades out four seconds in and the shot is a film
        // and nothing else, whatever it was asked for.
        view.hold_the_transport();
        view.settle_menu(&mut |text, string| ui.measure(text, string));
        view.advance(dt, &geometry);
        let (first, last) = draw::shown_range(&view, &geometry);
        posters.want(&view.wanted(first, last));
        if let Some(shown) = view.player.as_ref().and_then(|player| player.frame_due()) {
            films.show(&ui.device, &ui.queue, &shown);
        }
        over = draw::draw(&mut view, &mut ui, &geometry, &posters, true, theme.icons);
        captions.settle(
            &ui.device,
            &ui.queue,
            [width as f32, height as f32],
            over.caption.as_ref(),
        );

        // `Ui::end` takes the scene as it draws it, so it is called exactly
        // once an iteration and the last one is the frame that is kept.
        ui.end(&target)?;

        let settled = asked
            && view
                .wanted(first, last)
                .iter()
                .all(|path| posters.settled(path))
            && view.crossing().is_none()
            && view.player.as_ref().is_none_or(|player| !player.seeking());
        if let Some(after) = after {
            match pressed {
                // The film has to be really up before it is left: a picture
                // that was not on the screen cannot shrink back into its card.
                None if settled && frame > 30 => {
                    match then.as_str() {
                        "back" => {
                            let _ = view.act(Action::Back);
                        }
                        "details" => {
                            let _ = view.command(Command::Info);
                        }
                        "menu" => view.open_menu(),
                        _ => press_the_page(&mut view, arguments),
                    }
                    pressed = Some(frame);
                }
                Some(at) if frame >= at + (after * 60.0).round().max(0.0) as usize => break,
                _ => {}
            }
            continue;
        }
        if frame > 90 && settled {
            break;
        }
    }

    let mut encoder = ui
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("shot"),
        });
    films.draw(
        &ui.queue,
        &mut encoder,
        &target,
        [width as f32, height as f32],
        film::Showing {
            placement: over.film,
            holes: &over.holes,
            blackout: over.blackout,
            curtain: over.curtain,
            behind: over.behind,
        },
    );
    captions.draw(&mut encoder, &target);
    ui.queue.submit(Some(encoder.finish()));

    let pixels = read_back(&ui.device, &ui.queue, &texture, width, height)?;
    view.closing();
    write_png(path, &pixels, width, height)
}

/// `--play`: press the film the light is on, which is what somebody looking at
/// a folder would do.
fn press_the_page(view: &mut View, arguments: &[String]) {
    if !arguments.iter().any(|one| one == "--play") || view.mode != Mode::Grid {
        return;
    }
    if view.current().is_some_and(|entry| entry.is_folder()) {
        if let Some(first) = view
            .folder
            .entries
            .iter()
            .position(|entry| !entry.is_folder())
        {
            view.cursor = first;
        }
    }
    let _ = view.act(Action::Accept);
}

/// Copy the drawn texture back and hand over its bytes.
fn read_back(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    // A copy out of a texture is written in rows padded to 256 bytes.
    let row = (width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));

    let slice = buffer.slice(..);
    let (send, receive) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = send.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .map_err(|err| err.to_string())?;
    receive
        .recv()
        .map_err(|err| err.to_string())?
        .map_err(|err| err.to_string())?;

    let mapped = slice.get_mapped_range().map_err(|err| err.to_string())?;
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        let from = (y * row) as usize;
        pixels.extend_from_slice(&mapped[from..from + (width * 4) as usize]);
    }
    drop(mapped);
    buffer.unmap();
    Ok(pixels)
}

fn write_png(path: &str, pixels: &[u8], width: u32, height: u32) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|err| err.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .map_err(|err| err.to_string())?
        .write_image_data(pixels)
        .map_err(|err| err.to_string())
}

/// What this machine offers to be driven with, and what it does not.
///
/// Worth a flag of its own because a controller that does nothing has two
/// completely different causes — the application ignoring it, or there being
/// no gamepad on the machine to ignore — and from the outside they look
/// identical.
fn controllers() {
    let controls = Controls::new();
    let pad = pad::Pad::new();
    if let Some(trouble) = controls.trouble() {
        println!("No controller support at all: {trouble}");
        return;
    }

    let found = pad.found();
    if found.is_empty() {
        println!("No controllers found.");
        println!();
        println!("Nothing on this machine is presenting a gamepad. That is not");
        println!("always a fault: a controller whose driver is not in the kernel");
        println!("— a Steam Controller outside the session shell that drives it,");
        println!("for one — appears as a mouse and a keyboard and no gamepad, so");
        println!("there is nothing here for any program to read.");
        println!();
        println!("Look for one with:  ls /dev/input/js*");
        return;
    }

    println!(
        "{} controller{} found:",
        found.len(),
        if found.len() == 1 { "" } else { "s" }
    );
    for one in &found {
        println!();
        println!("  {}", one.name);
        println!(
            "    buttons   {}",
            if one.mapped {
                "named by this desktop's own mapping"
            } else {
                "guessed from the driver — the face buttons may be round the wrong way"
            }
        );
        println!(
            "    triggers  {}",
            if one.triggers {
                "yes — they scan through a film"
            } else {
                "none reported; the arrows still move through a film"
            }
        );
    }
}

const HELP: &str = "\
usage: videonsole [FILE-OR-FOLDER]
       videonsole --shot FILE [FOLDER] [--width N] [--height N]
                  [--play] [--back] [--details] [--menu] [--row N]
                  [--at SECONDS] [--after SECONDS] [--then WHAT]

With no arguments it opens the videos folder. Given a folder it opens that;
given a film it plays that film, with its own folder behind it.

A pad             A keyboard
-----             ----------
D-pad / stick     arrows, wasd, hjkl   move; in a film, seek and the volume
A                 Enter, Space         play and pause; open a folder
B                 Escape               back
Y                 F10, Menu, right-click   the Options menu
Start             r                    play the whole folder
LB / RB           Shift-Tab / Tab      the film before or after
triggers          --                   scan through the film

                  + and -              the volume
                  m                    mute
                  0 to 9               a tenth of the way through
                  f                    fill the screen, or fit to it
                  b                    start from the beginning
                  c                    the next subtitle track
                  i                    the details
                  n / p                the film after / before
                  g                    back to the folder
                  o                    open another folder

In a film left and right move through it — ten seconds a press, and further
while the direction is held — and up and down change how loud it is. The
controls come up when anything is pressed and go away four seconds later, and
the film grows to fill the screen when they do.

Where you left off is remembered, and offered again the next time that film is
opened. A film watched to its end is forgotten rather than remembered at its
credits.

--shot writes one settled frame to a PNG with no display at all, through the
same renderer and the same film and subtitle passes the window uses.

VIDEONSOLE_HWACCEL=off  decode in software, whatever this machine can do
--controllers  list what this machine can be driven with
--version      print the version
--help         print this message";
