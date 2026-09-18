//! Putting the browser and the player on the screen.
//!
//! Everything here except the film itself is `lxb-render` answering for the
//! material — the glass of a card, the light that travels between them, the
//! marks, the words and their sizes. Not one colour, radius or duration is
//! named in this file.
//!
//! The film is the exception, and the return value is where it goes: a
//! rectangle handed back to `main.rs` for `film.rs` to draw over the frame
//! once this one is composed, and a second one for the subtitle under it. See
//! the note at the top of `film.rs`.

use lxb_render::{Align, Fit, Ui};
use lxb_toolkit::{
    control,
    material::{light, Surface},
    metrics::Metric,
    palette::Role,
    settings::IconStyle,
    typography::Text,
};

use crate::film::Hole;

use crate::facts;
use crate::legend::{self, hint, Button, Hint};
use crate::library::Kind;
use crate::poster::Posters;
use crate::view::{Geometry, Mode, View, CARD, GROOVE, PLAY, SCROLL_BAR, STAGE, VOLUME};

/// Everything `main.rs` has to draw after the toolkit has composed the frame.
#[derive(Default)]
pub struct Over {
    pub film: Option<crate::film::Placement>,
    pub caption: Option<crate::subtitles::Caption>,
    /// The panels standing over the film, which the film is not drawn under.
    /// Collected as each one is drawn, because the only place a panel's
    /// rectangle is certainly right is where it was asked for.
    pub holes: Vec<Hole>,
    /// How much of the window is the film's own black rather than the shell's
    /// wallpaper, and how much of it that black covers. It stops short of the
    /// band the button hints are in — the toolkit fills that one, because
    /// anything it drew under this pass's black would be under it for good.
    pub blackout: f32,
    pub curtain: Hole,
    /// How much of the film the panels standing over it show through — the
    /// same reading of [`crate::view::View::on_black`] that stained them, so
    /// the picture arrives behind the glass on the frame the accent leaves it.
    pub behind: f32,
    /// Where the groove ended up, so a click on it can be turned back into a
    /// place in the film. Worked out while drawing because that is the only
    /// place it is known, and a second copy of the sum would be a groove that
    /// seeked somewhere other than where it was pressed.
    pub groove: [f32; 4],
}

/// Fill a band with the film's own black.
///
/// **Why this is not one call.** `lxb-render` draws glass, light and words,
/// and has no way to be asked for a plain opaque rectangle of a colour it did
/// not choose: `card` is glass, `rule` takes a colour from the palette, and
/// `chip` is the only one that takes a tint — and rounds it to a capsule. A
/// capsule far wider than the window is a rectangle inside it, which is the
/// first thing here.
///
/// The second is the stack. Everything the page drew is faded toward the
/// ground when a menu opens over it, and the ground is the shell's wallpaper —
/// so one black rectangle at four tenths is the wallpaper coming back through
/// a film's letterbox. Six of them are not.
fn blackout(ui: &mut Ui, rect: [f32; 4], amount: f32) {
    // Grown sideways, so the capsule's rounded ends fall outside the window
    // and what is left inside it is a rectangle. And grown *downwards only*:
    // the page is pushed back as a menu opens over it, which moves this band
    // up and would leave a sliver of wallpaper along the bottom of the window
    // — while a band grown upwards would be painted over the transport, whose
    // own panel abuts this one exactly.
    let wide = rect[2].max(1.0);
    let tall = rect[3].max(1.0);
    let stadium = [
        rect[0] - wide,
        rect[1],
        rect[2] + wide * 2.0,
        rect[3] + tall * 2.0,
    ];
    let tint = [0.0, 0.0, 0.0, amount.clamp(0.0, 1.0)];
    for _ in 0..6 {
        ui.chip(stadium, tint);
    }
}

/// Take the words of the page underneath out from under a panel.
///
/// **Every quad of a layer is drawn before every word of it**, so a name
/// written by the page below is drawn *over* whatever is put on top of it
/// afterwards — the title of a folder straight through the head bar of the
/// player, on every frame of a film opening out of it. `Ui::recede_behind` is
/// the only door on to the cut the toolkit makes for its own panels: at full
/// strength and with no dimming it is that cut and nothing else.
///
/// Only wanted while a page is changing. At every other moment there is one
/// page on the screen and nothing of another underneath it.
fn cut_under(ui: &mut Ui, view: &View, panel: [f32; 4]) {
    if view.crossing().is_some() {
        ui.recede_behind(panel, 1.0, 1.0);
    }
}

/// How deeply a panel is stained once it is standing on a film.
///
/// The toolkit's own panel is stained [`light::SIDEBAR_STAIN`], which is the
/// depth that lets the shell's wallpaper come through a menu as colour. On a
/// film there is no wallpaper to let through and the colour is the picture's,
/// so the stain goes nearly the whole way: what is left of the refraction is
/// the bevel at the panel's edge, and the film behind it arrives from
/// `film.rs` rather than from the page.
const ON_BLACK_STAIN: f32 = 0.88;

/// The material every panel in the player is cut from.
///
/// **Why this is not `Ui::pane`.** The toolkit's pane is lit by the accent at
/// its head and at its foot, rimmed in it, and stained thinly enough to
/// refract whatever the page drew underneath — which, on the shell's own
/// wallpaper, is the accent a third time. That is the right material for a
/// panel on a page and the wrong one for a panel standing on a film: the film
/// is the picture, and a bar of the shell's colour laid across it reads as
/// something pasted on top rather than a pane over it.
///
/// So `on_black` — [`View::on_black`], nought for a film that is a picture on
/// the page and one for a film that has taken the window — takes the accent
/// out of the material: the stain deepens until the wallpaper behind it is
/// gone, and what stands in for that much of the glass instead is the film,
/// blurred, laid in by `film.rs` inside the hole this panel asks for. The
/// glass, the gloss and the bevel are the toolkit's own [`Surface::Sidebar`]
/// throughout, because those are the material and the accent was only ever the
/// light on it.
///
/// **One quad, crossfaded, rather than two panels dissolving.** Pausing a film
/// hands the window back to the page over the whole of `motion::duration::PANEL`
/// and the panels stay on the screen the whole way, so the two materials are
/// one stain moving between two numbers. Two glass quads at half strength each
/// would be two bevels, two rims and two sets of corners for a third of a
/// second, every time somebody pressed pause.
fn panel(ui: &mut Ui, rect: [f32; 4], on_black: f32) {
    let on_black = on_black.clamp(0.0, 1.0);
    let stain = light::SIDEBAR_STAIN + (ON_BLACK_STAIN - light::SIDEBAR_STAIN) * on_black;
    ui.card(rect, Surface::Sidebar, Role::Glass, stain);
    // The rim, in ink rather than in accent. It is drawn *inside* the
    // rectangle, which is what lets the hole below be the rectangle itself.
    ui.control_out(rect, ui.m(Metric::CardRadius), 1.0);
}

/// The hole a panel asks the film to leave for it.
///
/// The panel's own rectangle exactly: [`panel`] draws nothing outside it, and
/// the edge the glass is feathered over is the same three quarters of a pixel
/// the film's shader feathers a hole over, so the two meet without a hairline
/// of either.
fn hole_for(ui: &Ui, rect: [f32; 4]) -> Hole {
    Hole {
        rect,
        radius: ui.m(Metric::CardRadius),
    }
}

/// What the buttons do, on each of the two pages.
///
/// Asked for by the same state that answers the press, so the row cannot come
/// to name something that is not there.
pub fn hints(view: &View) -> Vec<Hint> {
    match view.mode {
        Mode::Grid => {
            // Back walks out of the walk and closes at the top of it, so the
            // word changes with it rather than naming something that does not
            // happen.
            let mut hints = vec![
                hint(crate::i18n::text("options"), Button::Options),
                if view.closes_on_back() {
                    hint(crate::i18n::text("close"), Button::Back)
                } else {
                    hint(crate::i18n::text("back"), Button::Back)
                },
            ];
            if view.folder.films() > 0 {
                hints.insert(0, hint(crate::i18n::text("play-the-folder"), Button::Start));
            }
            if view.current().is_some() {
                hints.insert(
                    0,
                    hint(
                        if view.current().is_some_and(|entry| entry.is_folder()) {
                            crate::i18n::text("open")
                        } else {
                            crate::i18n::text("play")
                        },
                        Button::Accept,
                    ),
                );
            }
            hints
        }
        Mode::Player => {
            let playing = view
                .player
                .as_ref()
                .is_some_and(|player| player.playing() && !player.ended());
            vec![
                // What the button really does, which is not the same word for
                // the whole of a film: a legend saying Pause over a stopped
                // film is naming something that does not happen.
                hint(
                    if playing {
                        crate::i18n::text("pause")
                    } else {
                        crate::i18n::text("play")
                    },
                    Button::Accept,
                ),
                hint(crate::i18n::text("play-the-folder"), Button::Start),
                hint(crate::i18n::text("options"), Button::Options),
                if view.closes_on_back() {
                    hint(crate::i18n::text("close"), Button::Back)
                } else {
                    hint(crate::i18n::text("back"), Button::Back)
                },
            ]
        }
    }
}

pub fn draw(
    view: &mut View,
    ui: &mut Ui,
    geometry: &Geometry,
    posters: &Posters,
    pad: bool,
    icons: IconStyle,
) -> Over {
    let mut over = Over::default();

    // **The wall of films is drawn under a film that is growing out of it or
    // shrinking back into it**, stepping back and fading out as it goes: the
    // gesture a page makes when a menu opens over it, at the size of a whole
    // change of page. `Ui::recede` and `Ui::recede_behind` are the only doors
    // on to it, and both act on everything drawn so far — which is why this
    // happens *between* the two pages rather than inside either.
    let crossing = view.crossing();
    if view.mode == Mode::Grid || crossing.is_some() {
        grid(view, ui, geometry, posters, icons);
    }
    if let Some(grown) = crossing.filter(|grown| *grown > 0.0) {
        // About the card, so that the one thing which does not move while the
        // wall steps back is the card the picture came out of.
        ui.recede(grown, view.crossing_about(geometry));
        // Faded to nothing rather than to the `menu::DIM` a page keeps behind
        // a panel: what stands over this one is not a panel but the whole of
        // the next page, and a wall left at four tenths would still be there
        // when the animation ended.
        //
        // **And the words cut out from under the picture**, which is the
        // second half of what this call is for. Every quad of a layer is drawn
        // before every word of it, so a name on a card is drawn over anything
        // the same page put on top of it however late — and the picture
        // growing out of the card is exactly that. The stage is the panel it
        // is cut out from under.
        ui.recede_behind(view.stage, 1.0 - grown, view.stage_solidity());
    }

    if view.mode == Mode::Player {
        player(view, ui, geometry, posters, icons, &mut over);
    } else if view.leaving() {
        // The picture is still on the screen on its way back into its card,
        // and the page it belonged to is not. This is everything `player`
        // would have said about it, less the panels that have gone.
        over.film = Some(view.placement_now());
    }
    if view.mode == Mode::Player || view.leaving() {
        over.blackout = view.blackout();
        over.curtain = view.letterbox(geometry);
        over.behind = view.on_black();
    }

    // The legend goes with the transport: a film that has taken the whole
    // screen has nothing on it at all, and a row of hints would be the only
    // thing there that was not the film. It travels with the transport but is
    // never *inside* it — see `Geometry::legend_band`.
    let showing = if view.mode == Mode::Player {
        view.transport
    } else {
        1.0
    };
    if showing > 0.01 {
        // The band the hints sit in is the one thing a playing film gives
        // room to, so it is the film's own black rather than the wallpaper —
        // and the black has to be the toolkit's here, because what is drawn
        // over a film in this application is drawn *under* the film's pass.
        //
        // It arrives and leaves **with the picture**, not with the state: a
        // strip of black across the foot of a window whose film is still the
        // size of a card belongs to nothing on the screen.
        let letterbox = view.on_black();
        if letterbox > 0.001 {
            blackout(ui, view.hints_band(geometry), letterbox);
        }
        let hints = hints(view);
        let line = ui.line(Text::Caption);
        let (middle, away) = if view.mode == Mode::Player {
            (
                geometry.legend_at(),
                geometry.transport_away(view.transport),
            )
        } else {
            (geometry.legend_at(), 0.0)
        };
        let middle = middle + away;
        let right = geometry.window[0] - geometry.margin;
        let left = legend::row(ui, right, middle, &hints, pad, icons);

        if let Some(note) = view.note.clone() {
            let width = (left - geometry.margin * 2.0).max(0.0);
            ui.label(
                [geometry.margin, middle - line * 0.5, width, line],
                Text::Caption,
                &note,
                Role::TextSoft,
                Align::Left,
            );
        }
    }

    // Last, and over everything: the panels the toolkit owns. Each is drawn in
    // its own layer and refracts what is already beneath it, which is why they
    // cannot be drawn before the page they are over.
    ui.context_menu(&mut view.menu);
    ui.dialog(&mut view.dialog);
    view.files.draw(ui);

    // The menu is the one panel whose rectangle the toolkit works out rather
    // than this file, so its hole is asked for rather than collected. It is
    // added last because it is drawn last: everything already in the list has
    // stepped back behind it.
    if view.mode == Mode::Player || view.leaving() {
        for hole in over.holes.iter_mut() {
            let stepped = view.stepped_back(hole.rect);
            hole.radius *= if hole.rect[2] > 0.0 {
                stepped[2] / hole.rect[2]
            } else {
                1.0
            };
            hole.rect = stepped;
        }
        over.holes.extend(view.menu_hole());
    }

    over
}

// ---- the folder ---------------------------------------------------------

fn grid(view: &mut View, ui: &mut Ui, geometry: &Geometry, posters: &Posters, icons: IconStyle) {
    // Not while the player's own head bar is standing in the same band — a
    // film opening out of a card is drawn under one, and two marks and two
    // names in one bar is not a change of page, it is both pages at once. The
    // words behind a panel are cut away by `cut_under`; a mark is a quad, and
    // glass refracts what is behind it rather than hiding it.
    if view.mode == Mode::Grid {
        let title = view
            .folder
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("/")
            .to_string();
        head(
            ui,
            0.0,
            [
                geometry.margin,
                0.0,
                (geometry.window[0] - geometry.margin * 2.0).max(1.0),
                geometry.head,
            ],
            "category-video",
            &title,
            &count(view),
            icons,
        );
    }

    let viewport = geometry.viewport();
    let step = geometry.cell[1] + geometry.gap;
    let lifted = view.scroll * step;

    if view.folder.entries.is_empty() {
        let line = ui.line(Text::Body);
        ui.label(
            [
                viewport[0],
                viewport[1] + viewport[3] * 0.5 - line * 0.5,
                viewport[2],
                line,
            ],
            Text::Body,
            if view.folder.unreadable {
                crate::i18n::text("this-folder-cannot-be-opened")
            } else {
                crate::i18n::text("no-films-here")
            },
            Role::TextSoft,
            Align::Centre,
        );
        return;
    }

    // Everything the grid draws is cut to the viewport afterwards, so a row
    // half over the edge is drawn in half rather than left out or spilled over
    // the head.
    let from = ui.written();

    // The light goes down before the cards it is behind, and travels rather
    // than appearing — which is the whole of what makes a wall of films feel
    // like one thing being moved over.
    let mut lit = geometry.card(view.cursor);
    lit[1] -= lifted;
    let dt = ui.dt();
    let strength = match view.pressing.through() {
        Some(through) => 1.0 - through * 0.35,
        None => 1.0,
    };
    let at = view.light.glide(lit, dt);
    // `Ui::selection` is shaped for a row — its radius is half the height,
    // which on a card is an ellipse. A card is lit to its own corner instead,
    // which is the same material at the same role.
    let radius = ui.m(Metric::CardRadius);
    ui.lit(at, radius, control::LIT_ROLE, at[2], strength);

    let (first, last) = shown_range(view, geometry);
    for index in first..last.min(view.folder.entries.len()) {
        let mut rect = geometry.card(index);
        rect[1] -= lifted;
        // Nothing is drawn for a row that is nowhere near the window; a folder
        // of ten thousand would otherwise be ten thousand cards a frame.
        if rect[1] > viewport[1] + viewport[3] || rect[1] + rect[3] < viewport[1] {
            continue;
        }
        card(view, ui, geometry, posters, index, rect, icons);
    }

    let to = ui.written();
    ui.cut_between(from, to, viewport);

    // The ends of the listing dissolve into what is behind the page rather
    // than stopping at a line. How much really continues past each end is what
    // decides how much of a fade there is.
    let rows = geometry.rows(view.folder.entries.len()) as f32;
    let most = (rows - geometry.visible_rows()).max(0.0);
    let band = geometry.cell[1] * 0.5;
    ui.soft_edges(
        viewport,
        band,
        (view.scroll / 0.6).clamp(0.0, 1.0),
        ((most - view.scroll) / 0.6).clamp(0.0, 1.0),
    );

    // A bar is for a hand holding a pointer. A pad has the light, which
    // already says where in the listing it is, and a bar it could not reach
    // would be a control on the screen that no button touches.
    if most > 0.0 {
        let width = ui.scroll_bar_width();
        let track = [
            viewport[0] + viewport[2] - width,
            viewport[1],
            width,
            viewport[3],
        ];
        let thumb = ui.scroll_bar(
            track,
            view.scroll / most.max(0.0001),
            (geometry.visible_rows() / rows).clamp(0.0, 1.0),
            false,
        );
        ui.spot(SCROLL_BAR, thumb);
    }
}

/// Which cards are near enough the window to be worth drawing, and therefore
/// which films are worth opening for their poster.
///
/// One answer, read by the drawing and by what asks the readers for pictures —
/// two would be a grid that drew a card it had never asked for a picture of.
pub fn shown_range(view: &View, geometry: &Geometry) -> (usize, usize) {
    let columns = geometry.columns.max(1);
    let first = (view.scroll.floor() as usize).saturating_sub(1) * columns;
    let last = ((view.scroll + geometry.visible_rows()).ceil() as usize + 1) * columns;
    (first, last.min(view.folder.entries.len()))
}

fn count(view: &View) -> String {
    let films = view.folder.films();
    let folders = view.folder.entries.len() - films;
    // Counts go to the catalog as numbers, never as text: the form of the noun
    // is the language's decision and it makes it by looking at the number.
    match (films, folders) {
        (0, 0) => String::new(),
        (0, folders) => crate::message!("count-folders", "count" => folders),
        (films, 0) => crate::message!("count-films", "count" => films),
        (films, folders) => format!(
            "{}  ·  {}",
            crate::message!("count-films", "count" => films),
            crate::message!("count-folders", "count" => folders)
        ),
    }
}

fn card(
    view: &View,
    ui: &mut Ui,
    geometry: &Geometry,
    posters: &Posters,
    index: usize,
    rect: [f32; 4],
    icons: IconStyle,
) {
    let entry = &view.folder.entries[index];
    let lit = index == view.cursor;
    let radius = ui.m(Metric::CardRadius);
    let press = view.pressing.state(lit);

    ui.spot(CARD + index as u32, rect);
    // **Only the chosen card wears the control.** A wall of films in which
    // every one of them is a button is a wall with nothing picked out: the
    // glass, the rim and the press read as decoration on all of them at once
    // and as selection on none. So the rest are the picture and its name, and
    // the one being looked at is the only thing on the page that looks like
    // something to press.
    let sunk = if lit {
        ui.control(rect, radius, press, rect[2], 1.0)
    } else {
        rect
    };

    let inset = ui.s(10.0);
    let line = ui.line(Text::Caption);
    // Asked for rather than worked out here: a film opening grows out of this
    // rectangle, and the two answers have to be one answer. See
    // `Geometry::frame_in`.
    let picture = geometry.frame_in(sunk);

    match entry.kind {
        Kind::Folder => {
            // The tile a frame fills, with the mark on it. Not the control —
            // that belongs to whichever card is chosen — but a folder with
            // nothing behind it is a small mark adrift in a row of solid
            // rectangles, and the row stops reading as a row.
            ui.card(
                picture,
                Surface::Panel,
                Role::Glass,
                if lit { 0.5 } else { 0.35 },
            );
            let mark = ui.s(64.0).min(picture[3]);
            ui.icon_tinted(
                [
                    picture[0] + picture[2] * 0.5 - mark * 0.5,
                    picture[1] + picture[3] * 0.5 - mark * 0.5,
                    mark,
                    mark,
                ],
                "file-folder",
                icons,
                Role::Text,
                if lit { 1.0 } else { 0.8 },
            );
        }
        Kind::Film => {
            // A poster is a file on a disk by the time it reaches here — see
            // `poster.rs` — so it goes through the toolkit's own atlas and is
            // covered, rounded, cut and faded by the same code that draws
            // every other picture in this design language.
            let drawn = posters.poster(&entry.path).is_some_and(|poster| {
                ui.picture(picture, geometry.frame_radius(), poster, Fit::Cover, 1.0)
            });
            if !drawn {
                // Not opened yet, or a film no frame could be got out of.
                // Something has to hold the card's shape either way, or the
                // grid flickers as it fills in.
                ui.card(picture, Surface::Panel, Role::Glass, 0.5);
                let mark = ui.s(52.0).min(picture[3]);
                ui.icon_tinted(
                    [
                        picture[0] + picture[2] * 0.5 - mark * 0.5,
                        picture[1] + picture[3] * 0.5 - mark * 0.5,
                        mark,
                        mark,
                    ],
                    "category-video",
                    icons,
                    Role::TextSoft,
                    if lit { 0.9 } else { 0.65 },
                );
            }

            // How long it runs, in the corner of the frame. The one fact
            // that decides whether somebody presses a film tonight, and the
            // one a name never carries.
            if let Some(length) = posters
                .facts(&entry.path)
                .map(|facts| facts.length)
                .filter(|length| *length > 0.0)
            {
                let said = facts::length(length);
                let caption = ui.line(Text::Caption);
                let pad = ui.s(6.0);
                let width = ui.measure(Text::Caption, &said) + pad * 2.0;
                let chip = [
                    picture[0] + picture[2] - width - pad,
                    picture[1] + picture[3] - caption - pad * 2.0 - pad,
                    width,
                    caption + pad,
                ];
                ui.chip(chip, [0.0, 0.0, 0.0, 0.45]);
                ui.label(
                    [chip[0], chip[1] + pad * 0.5, chip[2], caption],
                    Text::Caption,
                    &said,
                    Role::Text,
                    Align::Centre,
                );
            }

            // And a mark on a film somebody has already started, which is the
            // other thing a wall of names cannot say.
            if view.resume.at(&entry.path).is_some() {
                let bar = ui.s(3.0);
                ui.lit(
                    [
                        picture[0],
                        picture[1] + picture[3] - bar,
                        picture[2] * 0.34,
                        bar,
                    ],
                    bar * 0.5,
                    Role::Accent,
                    picture[2],
                    1.0,
                );
            }
        }
    }

    let room = (sunk[2] - inset * 2.0).max(1.0);
    let name = fit_text(ui, Text::Caption, &entry.name, room);
    ui.label(
        [
            sunk[0] + inset,
            sunk[1] + sunk[3] - inset - line,
            room,
            line,
        ],
        Text::Caption,
        &name,
        if lit { Role::Text } else { Role::TextSoft },
        Align::Left,
    );
}

/// Shorten a name to fit, out of the middle.
///
/// **Nothing clips a word.** Every quad of a layer is drawn before any of that
/// layer's words, so a label wider than the rectangle it was given is not cut
/// off — it is written straight over whatever is beside it, which on a grid is
/// the next card's name. So anything that might not fit has to be cut before
/// it is asked for.
///
/// Out of the middle, because both ends of a file name carry something and the
/// middle rarely does: a folder of `Series.S01E04.1080p.WEB-DL.mkv` differs
/// from its neighbours in two digits near the front and says what it is at the
/// back, and cutting either end alone would leave a column of identical
/// labels.
fn fit_text(ui: &mut Ui, text: Text, name: &str, room: f32) -> String {
    crate::view::cut_to_fit(
        &mut |text, string| ui.measure(text, string),
        text,
        name,
        room,
    )
}

// ---- one film -----------------------------------------------------------

fn player(
    view: &mut View,
    ui: &mut Ui,
    geometry: &Geometry,
    posters: &Posters,
    icons: IconStyle,
    over: &mut Over,
) {
    let Some(entry) = view.current().cloned() else {
        return;
    };
    if view.transport > 0.01 {
        // The head travels off the top and the transport off the bottom. They
        // slide rather than fade because a hole cut in a film cannot fade: it
        // would open on a rectangle of wallpaper. See `film.rs`.
        let away = geometry.head_away(view.transport);
        let pane = geometry.head_pane();
        let pane = [pane[0], pane[1] - away, pane[2], pane[3]];
        panel(ui, pane, view.on_black());
        cut_under(ui, view, pane);
        over.holes.push(hole_for(ui, pane));
        head(
            ui,
            ui.m(Metric::PanelPadding),
            pane,
            "category-video",
            &entry.name,
            &position(view),
            icons,
        );
    }

    // The stage is a target a pointer can land on: a press on the film is a
    // press of the same button the legend names.
    ui.spot(STAGE, view.stage);

    if view.info_out > 0.01 {
        over.holes
            .push(details(view, ui, geometry, posters, &entry));
    }

    // What was on the card, while there is nothing else to put on the stage.
    // A film takes a moment over its header, and until this that moment was a
    // black rectangle growing out of the card somebody had just pressed.
    //
    // `Fit::Cover` because that is how the card holds it, and this rectangle
    // starts as the card's own — the picture has to be the same picture on
    // the frame the animation begins on.
    let stood_in = view
        .standing_in()
        .and_then(|film| posters.poster(film))
        .is_some_and(|poster| {
            let poster = poster.to_path_buf();
            let rect = view.standing_rect(ui.picture_aspect(&poster));
            ui.picture(rect, view.stage_radius(), &poster, Fit::Cover, 1.0)
        });

    // Nothing else at all is drawn where the film goes. It is not that the
    // stage is empty — it is that whatever were drawn there would be under the
    // film, and therefore never seen.
    let trouble = view.player.as_ref().and_then(|player| player.trouble());
    let ready = view.player.as_ref().is_some_and(|player| player.ready());
    let line = ui.line(Text::Body);
    let middle = [
        view.stage[0],
        view.stage[1] + view.stage[3] * 0.5 - line * 0.5,
        view.stage[2],
        line,
    ];
    if let Some(trouble) = trouble {
        ui.label(middle, Text::Body, &trouble, Role::TextSoft, Align::Centre);
    } else if !ready {
        // Said only when there is nothing to look at. A film's own poster
        // growing out of its card says the same thing better, and a word over
        // it would be a caption on the animation.
        if !stood_in {
            ui.label(
                middle,
                Text::Body,
                crate::i18n::text("opening"),
                Role::TextSoft,
                Align::Centre,
            );
        }
    } else {
        over.film = Some(view.placement_now());
        // The subtitle sits inside the film's own rectangle, so when the film
        // steps aside for the transport the words step aside with it rather
        // than ending up behind the controls.
        if let Some(text) = view.player.as_ref().and_then(|player| player.caption()) {
            let mut picture = [
                view.stage[0] + (view.stage[2] - view.shown[0]).max(0.0) * 0.5,
                view.stage[1] + (view.stage[3] - view.shown[1]).max(0.0) * 0.5,
                view.shown[0].min(view.stage[2]),
                view.shown[1].min(view.stage[3]),
            ];
            // Over a playing film the transport is drawn on top of the picture
            // rather than beside it, so the words are lifted off the bottom of
            // the picture by however much of it the panel has taken. A
            // subtitle behind the groove is a subtitle nobody read.
            if view.transport > 0.01 {
                let band = geometry.transport_at();
                let away = geometry.transport_away(view.transport);
                let taken = (picture[1] + picture[3]) - (band[1] + away) + geometry.gap;
                if taken > 0.0 {
                    picture[3] = (picture[3] - taken).max(picture[3] * 0.5);
                }
            }
            over.caption = Some(crate::subtitles::Caption {
                text,
                within: picture,
                // See the opacity below.
                // Held to the film's own height rather than the window's: a
                // subtitle on a film in a corner of the screen has to be a
                // subtitle on *that* picture.
                size: (picture[3] * 0.052).clamp(ui.s(15.0), ui.s(44.0)),
                // Out of the way of a menu. The subtitle is drawn in a pass
                // of its own after the film's, so it is over *everything* —
                // including a panel that is supposed to be over it. It steps
                // out as the menu comes in rather than being cut off by it,
                // which is the one thing a pass drawn last can do politely.
                opacity: view.showing * (1.0 - view.menu.travelled().clamp(0.0, 1.0)),
            });
        }
    }

    if view.transport > 0.01 {
        let away = geometry.transport_away(view.transport);
        let band = geometry.transport_at();
        let pane = [band[0], band[1] + away, band[2], band[3]];
        panel(ui, pane, view.on_black());
        cut_under(ui, view, pane);
        over.holes.push(hole_for(ui, pane));
        over.groove = transport(view, ui, geometry, away, icons);
    }
}

/// The transport: what is playing, how far in, and how loud.
///
/// Returns where the groove ended up, so a press on it can be turned back into
/// a place in the film.
fn transport(
    view: &View,
    ui: &mut Ui,
    geometry: &Geometry,
    away: f32,
    icons: IconStyle,
) -> [f32; 4] {
    // The band, not the pane behind it: the pane grows down to take the legend
    // in while the film is under it, and controls that followed it would drift
    // down the screen as a film started playing.
    let band = geometry.transport_at();
    let rect = [band[0], band[1] + away, band[2], band[3]];
    // Nothing on it fades. What comes and goes is the panel behind it, and it
    // goes by sliding — taking the words with it, because a hole cut in a
    // film cannot fade and the panel is one.

    let player = view.player.as_ref();
    let at = player.map(|player| player.at()).unwrap_or(0.0);
    let length = player.map(|player| player.length()).unwrap_or(0.0);
    let playing = player.is_some_and(|player| player.playing() && !player.ended());

    let padding = ui.m(Metric::PanelPadding);
    let middle = rect[1] + rect[3] * 0.5;
    let mark = ui.s(30.0);
    let caption = ui.line(Text::Caption);

    // Play and pause, at the left-hand end where a hand looks for it.
    let button = [
        rect[0] + padding,
        middle - mark * 0.5 - ui.s(6.0),
        mark + ui.s(12.0),
        mark + ui.s(12.0),
    ];
    ui.spot(PLAY, button);
    ui.icon_tinted(
        [button[0] + ui.s(6.0), button[1] + ui.s(6.0), mark, mark],
        if playing { "media-pause" } else { "media-play" },
        icons,
        Role::Text,
        1.0,
    );

    // How loud, at the other end. A mark rather than a slider: the directions
    // already change the volume, and a slider a pad cannot reach would be a
    // control on the screen that no button touches.
    let volume_at = [
        rect[0] + rect[2] - padding - mark - ui.s(12.0),
        button[1],
        mark + ui.s(12.0),
        button[3],
    ];
    ui.spot(VOLUME, volume_at);
    ui.icon_tinted(
        [
            volume_at[0] + ui.s(6.0),
            volume_at[1] + ui.s(6.0),
            mark,
            mark,
        ],
        if view.muted || view.volume <= 0.001 {
            "volume-muted"
        } else {
            "volume"
        },
        icons,
        Role::Text,
        if view.muted { 0.55 } else { 1.0 },
    );

    // The two clocks, laid 1.0 from the widest the film will ever make them so
    // that nothing under them moves as it plays. See `facts::clock`.
    let widest = ui.measure(Text::Caption, &facts::clock(length, length));
    let elapsed = [
        button[0] + button[2] + ui.s(10.0),
        middle - caption * 0.5,
        widest,
        caption,
    ];
    ui.label(
        elapsed,
        Text::Caption,
        &facts::clock(at, length),
        Role::Text,
        Align::Left,
    );
    let left = [
        volume_at[0] - ui.s(10.0) - widest,
        middle - caption * 0.5,
        widest,
        caption,
    ];
    if length > 0.0 {
        ui.label(
            left,
            Text::Caption,
            &facts::clock(length, length),
            Role::TextSoft,
            Align::Right,
        );
    }

    // The groove.
    let height = ui.s(6.0);
    let groove = [
        elapsed[0] + widest + ui.s(14.0),
        middle - height * 0.5,
        (left[0] - ui.s(14.0) - (elapsed[0] + widest + ui.s(14.0))).max(1.0),
        height,
    ];
    ui.spot(GROOVE, [groove[0], rect[1], groove[2], rect[3]]);
    ui.card(groove, Surface::Control, Role::Glass, 0.8);
    let part = if length > 0.0 {
        (at / length).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    if part > 0.0 {
        ui.lit(
            [groove[0], groove[1], groove[2] * part, groove[3]],
            height * 0.5,
            Role::Accent,
            groove[2],
            1.0,
        );
    }
    // Where the film is, as a bead on the groove — the one thing on the
    // transport that says *this is the thing you are moving*.
    let bead = ui.s(13.0);
    ui.lit(
        [
            groove[0] + groove[2] * part - bead * 0.5,
            middle - bead * 0.5,
            bead,
            bead,
        ],
        bead * 0.5,
        Role::Accent,
        bead,
        1.0,
    );

    // What a film is doing that the groove cannot say: waiting, running the
    // folder through, or resumed where somebody left it.
    let said = saying(view);
    if !said.is_empty() {
        ui.label(
            [
                groove[0],
                rect[1] + rect[3] - caption - ui.s(4.0),
                groove[2],
                caption,
            ],
            Text::Caption,
            &said,
            Role::TextSoft,
            Align::Centre,
        );
    }
    groove
}

/// The line under the groove, or nothing.
fn saying(view: &View) -> String {
    let Some(player) = view.player.as_ref() else {
        return String::new();
    };
    if player.ended() {
        return String::from(crate::i18n::text("the-end"));
    }
    if player.seeking() {
        return String::new();
    }
    let mut said = Vec::new();
    if let Some(from) = view.resumed_at {
        said.push(crate::message!(
            "carried-on-from",
            "time" => facts::clock(from, player.length())
        ));
    }
    if view.run_the_folder {
        said.push(String::from(crate::i18n::text("playing-the-folder")));
    }
    if view.muted {
        said.push(String::from(crate::i18n::text("muted")));
    } else if view.volume < 0.999 {
        said.push(crate::message!(
            "volume-per-cent",
            "per-cent" => format!("{:.0}", view.volume * 100.0)
        ));
    }
    said.join("  ·  ")
}

fn position(view: &View) -> String {
    let total = view.folder.films();
    let before = view
        .folder
        .entries
        .iter()
        .take(view.cursor)
        .filter(|entry| !entry.is_folder())
        .count();
    if total == 0 {
        return String::new();
    }
    crate::message!("place-in-folder", "place" => before + 1, "total" => total)
}

/// The pane of details, down the right-hand side.
///
/// A pane rather than a strip over the film, for the reason the whole
/// application is shaped around: it is drawn by the toolkit and the film is
/// drawn over that, so a panel on top of one would be a panel underneath it.
/// The stage has already given up exactly this much room.
fn details(
    view: &View,
    ui: &mut Ui,
    geometry: &Geometry,
    posters: &Posters,
    entry: &crate::library::Entry,
) -> Hole {
    // **Slid in from the edge rather than faded up.** Over a playing film the
    // pane is a hole cut in the picture, and a hole cannot fade — it would
    // open on a rectangle of wallpaper and then dissolve into one. Beside a
    // stopped one it is the stage that has to stand clear of this rectangle at
    // every moment of the animation, so it is asked for rather than worked out
    // here and the two cannot disagree: see `View::details_pane`.
    let rect = view.details_pane(geometry);
    panel(ui, rect, view.on_black());

    let padding = ui.m(Metric::PanelPadding);
    let mut at = rect[1] + padding;
    let inner = (rect[2] - padding * 2.0).max(1.0);
    let left = rect[0] + padding;

    let say = |ui: &mut Ui, at: &mut f32, label: &str, value: &str| {
        if value.is_empty() {
            return;
        }
        let caption = ui.line(Text::Caption);
        let body = ui.line(Text::Body);
        ui.label(
            [left, *at, inner, caption],
            Text::Caption,
            label,
            Role::TextSoft,
            Align::Left,
        );
        *at += caption;
        ui.label(
            [left, *at, inner, body],
            Text::Body,
            value,
            Role::Text,
            Align::Left,
        );
        *at += body + ui.s(14.0);
    };

    let name = fit_text(ui, Text::Body, &entry.name, inner);
    say(ui, &mut at, crate::i18n::text("name"), &name);

    // What the player knows first, and what was read off the file before it —
    // so the pane says something the moment it is opened rather than filling
    // in a field at a time.
    let player = view.player.as_ref();
    let size = player
        .and_then(|player| player.shown_size())
        .or_else(|| {
            posters
                .facts(&entry.path)
                .map(|facts| (facts.width, facts.height))
        })
        .filter(|(width, height)| *width > 0 && *height > 0)
        .map(|(width, height)| facts::picture(width, height))
        .unwrap_or_default();
    say(ui, &mut at, crate::i18n::text("picture"), &size);

    let length = player
        .map(|player| player.length())
        .filter(|length| *length > 0.0)
        .or_else(|| posters.facts(&entry.path).map(|facts| facts.length))
        .filter(|length| *length > 0.0)
        .map(facts::length)
        .unwrap_or_default();
    say(ui, &mut at, crate::i18n::text("length"), &length);
    say(
        ui,
        &mut at,
        crate::i18n::text("on-disk"),
        &facts::size(entry.bytes),
    );

    // Which is worth saying because it is the answer to "why is this film
    // stuttering" and to "why does this film look wrong", and there is nowhere
    // else on the machine to find it out.
    let decoded = player.map(|player| player.decoded_by()).unwrap_or_default();
    say(ui, &mut at, crate::i18n::text("decoded-by"), &decoded);
    let sound = match player {
        Some(player) if player.has_sound() => crate::i18n::text("yes"),
        Some(_) => crate::i18n::text("none"),
        None => "",
    };
    say(ui, &mut at, crate::i18n::text("sound"), sound);

    if let Some(changed) = entry.changed {
        say(
            ui,
            &mut at,
            crate::i18n::text("written"),
            &facts::when(changed),
        );
    }
    let folder = entry
        .path
        .parent()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let folder = cut_from_the_front(ui, &folder, inner);
    say(ui, &mut at, crate::i18n::text("folder"), &folder);

    hole_for(ui, rect)
}

// ---- the head of the page ------------------------------------------------

/// The mark, the name and the count, laid out inside `within`.
///
/// **`within` is the box, not the page.** On a folder it is the head band and
/// the padding is nought, which is what a page's head has always been. In the
/// player it is the panel behind them, and everything is held off its edges by
/// the same padding the transport's own controls are — a mark against the
/// rounded corner of a panel reads as a mark falling out of it, and a count
/// flush with the far edge reads as a count that has been cut.
///
/// Centred on the box rather than at fifty-five hundredths of it: that
/// fraction is an optical lift for a band whose top edge is the top of the
/// window and which has nothing under it. A panel has two edges and the eye
/// measures both.
fn head(
    ui: &mut Ui,
    padding: f32,
    within: [f32; 4],
    mark: &str,
    title: &str,
    aside: &str,
    icons: IconStyle,
) {
    let line = ui.line(Text::Title);
    let glyph = ui.s(34.0);
    let middle = if padding > 0.0 {
        within[1] + within[3] * 0.5
    } else {
        within[1] + within[3] * 0.55
    };
    let left = within[0] + padding;
    let right = within[0] + within[2] - padding;
    ui.icon_tinted(
        [left, middle - glyph * 0.5, glyph, glyph],
        mark,
        icons,
        // A mark on a panel is drawn in the same ink as the words beside it,
        // as every mark on the transport's own panel is. The accent is for a
        // mark on the page itself, where it is the one coloured thing.
        if padding > 0.0 {
            Role::Text
        } else {
            Role::Accent
        },
        1.0,
    );

    let caption = ui.line(Text::Caption);
    let width = ui.measure(Text::Caption, aside);
    if !aside.is_empty() {
        ui.label(
            [right - width, middle - caption * 0.5, width, caption],
            Text::Caption,
            aside,
            Role::TextSoft,
            Align::Right,
        );
    }

    let from = left + glyph + ui.s(14.0);
    let room = (right - width - ui.s(24.0) - from).max(1.0);
    // Cut before it is drawn, or a long file name is written straight over the
    // count in the far corner. See `fit_text`.
    let title = fit_text(ui, Text::Title, title, room);
    ui.label(
        [from, middle - line * 0.5, room, line],
        Text::Title,
        &title,
        Role::Text,
        Align::Left,
    );
}

/// Shorten a path to fit, taking it off the front.
///
/// The front, because the end of a path is the part that says which folder
/// this is; the shell's own chooser cuts one the same way. A word that is
/// drawn wider than the pane it is in is not clipped by the renderer — every
/// quad is drawn before every word — so anything that might not fit has to be
/// cut before it is asked for.
fn cut_from_the_front(ui: &mut Ui, path: &str, width: f32) -> String {
    if ui.measure(Text::Body, path) <= width {
        return path.to_string();
    }
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    for skip in 1..parts.len() {
        let shorter = format!("…/{}", parts[skip..].join("/"));
        if ui.measure(Text::Body, &shorter) <= width {
            return shorter;
        }
    }
    // Even the last name is too wide: take characters off it until it is not.
    let last = parts.last().copied().unwrap_or(path);
    let mut chars: Vec<char> = last.chars().collect();
    while !chars.is_empty() {
        let shorter: String = std::iter::once('…').chain(chars.iter().copied()).collect();
        if ui.measure(Text::Body, &shorter) <= width {
            return shorter;
        }
        chars.remove(0);
    }
    String::from("…")
}
