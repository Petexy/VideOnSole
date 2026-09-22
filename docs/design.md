# How Videonsole is built, and why

This is the long answer. [The README](../README.md) is the short one.

## It draws its own window

Most applications built on the toolkit do not need to. `Ui::picture` reads a
*file* into one 512-pixel cell of a shared atlas — exactly right for the card
of a grid, and a film is not a file that can be read once. Its frames arrive
twenty-four times a second at their own size, so the frame is composed by
`lxb-render` as everywhere else and the film is drawn over it in a pass of this
application's own.

**Everything else follows from that.** The film is drawn *over* the composed
frame, so nothing the toolkit draws can appear on top of one. There are two
answers to that here, and which one is in force is the difference between
watching a film and looking at one.

**A stopped film gets a stage** — a rectangle it may occupy — and everything
else is laid out outside it. The transport narrows the stage and the film steps
aside; the details pane takes its room the same way; a dialog or the file
chooser takes the screen and the film fades out. That is why the stage is
animated state rather than a rectangle worked out while drawing.

**A playing film is punched through.** It has the whole window, and every panel
over it is a *hole*: a rounded rectangle the film's pass leaves alone, so what
the toolkit drew there survives untouched. The panels are still the toolkit's
own material — nothing is hand-painted — and the film is still drawn last.

Two things fall out of that. **A panel is the shape of its hole**, so anything
standing over a playing film has to be on a panel: a hole cut around a loose
word would be a rectangle of wallpaper cut out of the picture. That is why the
button hints are not over the film at all — the film gives up their band
instead, and takes it back the moment they go. And **a hole cannot fade**,
because it would open on that same rectangle and then dissolve into it, so the
chrome **slides** off the edges of the window.

## The black is this application's own

`lxb-render` draws glass, light and words, and has no way to be asked for a
plain opaque rectangle of a colour it did not choose — so the letterbox around
a playing film is painted in the same pass the film is. With one exception, and
it is why `draw.rs` has a `blackout` function: the hints' band has to be black
*under* words the toolkit drew, and this pass runs after the toolkit. There it
is stacked out of the one call that takes a colour at all.

## A change of page is drawn twice over

Both pages are on the screen while a film is opening or leaving, and the
toolkit draws every quad of a layer before every word of it — so a name on a
card is drawn *over* whatever the same frame puts on top of it, however late.
`Ui::recede_behind` is the door on to both halves of the answer: it steps the
wall back and fades it out, and it takes the words out from under the picture
and the panels standing over them, in proportion to how solid each of those is.
It is the same call a menu makes over the page it opens on.

The subtitle is the single exception, and it is drawn in a pass of its own
after the film's — in `glyphon`, the crate `lxb-render` sets all of its own
type with, loading the toolkit's own face. A different pass, and the same
material.

## The colour is done on the graphics card

A decoder hands back planes of luma and chroma, not pixels; turning those into
red, green and blue is twelve megabytes of arithmetic per 4K frame that a
graphics card does for nothing while a processor doing it is a processor not
decoding the next frame. So the planes go up as they are and the shader
converts, from the coefficients the film itself declares.

What that does *not* do is high dynamic range — a BT.2020 film with a
perceptual-quantiser curve goes through the standard-range path and will look
flat.

The colour path is worth checking against the library itself rather than by
eye:

```sh
ffmpeg -ss 90 -i film.mkv -frames:v 1 -pix_fmt rgb24 -f rawvideo ref.rgb
```

and comparing a flat area with the same pixel of a `--shot`. On a film that
declares its colour space they agree to within a count or two. On one that
declares nothing they will not: this reads an untagged HD film as BT.709, which
is what every player does and what real HD content is, while `swscale`'s own
default for an untagged stream is BT.601.

## The triggers, and nothing else

`pad.rs` is the one place this goes past `lxb-input`, and only for the
triggers. An action is a thing that happened and a trigger is a quantity, and
there is no honest way to say "sixty percent" in a list of actions. A scan
holds the film still and moves it by seeking, rather than playing it faster — a
soundtrack at four times speed is a noise, and what somebody scanning is
looking at is the picture.

## The button hints are the one thing over a playing film

It is the same row in the same place on every page of this application and in
the shell beside it, so it is never folded into the control bar and never given
a panel of its own. The picture stops just above it instead — the only room
anything takes from a playing film, and it is given back the moment the hints
go.

## Where the pictures come from

`--demo` draws a made-up folder of films into `$XDG_CACHE_HOME/videonsole` and
opens it. Nothing of the user's is read or written, and every page above it is
the ordinary code path: the same directory walk, the same demuxer, the same
decoder, the same poster frame written into the same shared thumbnail cache. A
player photographed against a special case would be a picture of the special
case.

The films are drawn rather than filmed — scenes made out of gradients and
noise, panned across — and they are deterministic, so two shots taken a week
apart differ by what changed in the program and by nothing else. They are H.264
where the machine can write it and MPEG-4 part 2 where it cannot; see
`src/demo.rs`, which says why it is that way round.

The pictures in the README are taken at the **Indigo** accent, which is not any
particular machine's. The accent is the one setting a picture of the interface
cannot help stating, and shots taken on different days in different colours
would read as different programs. Regenerate them with a scratch settings file
rather than by changing anybody's desktop:

```sh
mkdir -p /tmp/lxb-shot/lxb
printf 'accent = "Indigo"\n' > /tmp/lxb-shot/lxb/shell.toml
export XDG_CONFIG_HOME=/tmp/lxb-shot

videonsole --demo --shot docs/folder.png  --width 1600 --height 900
videonsole --demo --shot docs/film.png    --width 1600 --height 900 --row 5 --play --at 6
videonsole --demo --shot docs/options.png --width 1600 --height 900 --row 2 --play --at 3 --menu
videonsole --demo --shot docs/details.png --width 1600 --height 900 --row 5 --play --pause --at 4 --details
```

`--pause` is the one to reach for when a shot comes out looking wrong: a film
named on the command line opens *playing*, which is the state where it has the
whole window and the chrome is holes cut in it. Stopping it is the only way to
photograph the page a film stands aside on.

![The details](details.png)
