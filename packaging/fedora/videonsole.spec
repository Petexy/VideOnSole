Name:           videonsole
Version:        0.9.0
Release:        1%{?dist}
Summary:        A film browser and player in the LineXinBar design language, shown as Videos

# This program, and the locked Rust dependency graph vendored into the source
# archive. Every crate offering a choice is taken under its permissive option:
# self_cell as Apache-2.0 rather than GPL-2.0-only, r-efi as MIT rather than
# LGPL-2.1-or-later. What is left after that choice is this list, and it is
# derived from the lock file rather than remembered.
License:        GPL-3.0-only AND Apache-2.0 AND MIT AND Apache-2.0 WITH LLVM-exception AND BSD-2-Clause AND BSD-3-Clause AND ISC AND MPL-2.0 AND Unicode-3.0 AND Unlicense AND Zlib AND 0BSD AND CDLA-Permissive-2.0
URL:            https://github.com/Petexy/videonsole
Source0:        videonsole-%{version}.tar.gz

ExclusiveArch:  x86_64 aarch64

# Cargo's release profile emits no DWARF, so find-debuginfo would produce an
# empty debugsourcefiles.list and rpmbuild would fail on it after the whole
# build. An archive submission wants real debuginfo instead: drop this, and
# with it the -Cdebuginfo=0 in %build that holds Fedora's own -Cdebuginfo=2 off,
# so the DWARF is built and packaged rather than built and binned.
%global debug_package %{nil}

BuildRequires:  cargo >= 1.90
BuildRequires:  rust >= 1.90
BuildRequires:  gcc
BuildRequires:  pkgconfig
BuildRequires:  desktop-file-utils
BuildRequires:  libappstream-glib
# The design language, as Rust sources. It is a build dependency and not a
# runtime one: `lxb-render` is a path dependency, so cargo compiles it into
# this binary and the finished program links no liblxb_*.so at all.
BuildRequires:  lxb-toolkit-devel >= 0.9.0
# What the program links outright, each asked for as a pkg-config name, which
# is what the Rust bindings look for: ALSA for the interface sounds, libudev
# for the game controllers and xkbcommon for the keyboard.
BuildRequires:  pkgconfig(alsa)
BuildRequires:  pkgconfig(libudev)
BuildRequires:  pkgconfig(xkbcommon)
# What decodes a film. Asked for as pkg-config names rather than as a package,
# because Fedora ships two ffmpegs that both provide them — the free build in
# the main repository and the whole one from RPM Fusion — and this builds and
# runs against either.
BuildRequires:  pkgconfig(libavcodec)
BuildRequires:  pkgconfig(libavformat)
BuildRequires:  pkgconfig(libavutil)
BuildRequires:  pkgconfig(libswresample)
BuildRequires:  pkgconfig(libswscale)
# bindgen's, which writes the FFmpeg bindings from those headers at build time.
BuildRequires:  clang

# Opened by name at run time rather than linked, so rpm's automatic dependency
# generator cannot see it in the ELF.
Requires:       libglvnd-egl
# Choosing another folder is put to whatever chooser this desktop runs, through
# the portal. Without one the toolkit draws its own, so this is not required.
Recommends:     xdg-desktop-portal
# With a Vulkan driver present this draws through it; without one it falls back
# to EGL, so the loader is worth having and is not required.
Suggests:       vulkan-loader
# What lets a film decode on the graphics card rather than the processor, which
# on a 4K film is the difference between playing and not. The libraries
# themselves are linked and so are found by rpm for themselves; a driver is
# not, and there is no one package that is the right one for every card.
Recommends:     libva
Suggests:       libva-intel-media-driver
Suggests:       mesa-va-drivers

%description
Shown as Videos. A film browser and player drawn in the LineXinBar design
language: the same colours, glass, motion and marks as the shell it was made
for, and driven from a controller, a keyboard and a pointer at once. It is an
ordinary Wayland application and runs under GNOME or Plasma as readily as under
that shell.

A folder is a wall of films, each card wearing a frame out of the film itself
and how long it runs; those frames are written to the same thumbnail cache
every other program on the machine shares. Playing one fills the screen with
the film — the controls come up when anything is pressed and go away four
seconds later, and the film grows to take the whole screen when they do, so
nothing is ever drawn over the picture except its subtitles. Left and right
move through the film, ten seconds a press and further while held; a pad's
triggers scan through it; up and down are the volume. Where you left off is
remembered and offered again next time, and a film watched to its end is
forgotten rather than remembered at its credits.

Films decode through this machine's video hardware where it has any, and in
software where it does not.

%prep
%autosetup -n videonsole-%{version}

%build
export RUSTUP_TOOLCHAIN=stable
export CARGO_TARGET_DIR=target
# Fedora exports its own %%{build_rustflags} into RUSTFLAGS before this runs, and
# they carry -Cdebuginfo=2 -Cstrip=none. RUSTFLAGS is appended after the release
# profile's own flags and wins, so every crate in the graph was generating full
# DWARF — and with %%global debug_package %%{nil} above, no package was ever made
# of it. -Cdebuginfo=0 last is what turns that back off. It is worth about
# 274 MiB of resident memory on the final rustc here, measured: 1572 MiB with
# the DWARF, 1298 MiB without.
export RUSTFLAGS="${RUSTFLAGS:-} -Cdebuginfo=0"

# And Cargo takes its job count from the core count alone, knowing nothing about
# how much memory the machine has to hold that many rustc at once. wgpu and naga
# are in this graph and thin LTO with one codegen unit is what the release
# profile asks for, so the count has to answer to memory as well. The sister
# repository's shell was killed by the kernel's OOM killer twice on an 8 GiB
# Apple M1 for want of exactly this.
#
# Arithmetic rather than %%limit_build, the Fedora macro meant for this, which
# swallowed the remainder of the script it was used in on Fedora Asahi.
build_jobs="%{_smp_build_ncpus}"
build_room="$(awk '/^MemTotal:/ { n = int($2 / 1024 / 2048); print (n < 1 ? 1 : n) }' /proc/meminfo 2>/dev/null || true)"
if [ -n "$build_room" ] && [ "$build_room" -lt "$build_jobs" ]; then
    build_jobs="$build_room"
fi
echo "building with $build_jobs of %{_smp_build_ncpus} jobs, for the memory this machine has"
cargo build --offline --locked --release -j"$build_jobs"

%install
export CARGO_TARGET_DIR=target
./packaging/install.sh \
    --destdir %{buildroot} \
    --prefix %{_prefix} \
    --target-dir target

%check
export RUSTUP_TOOLCHAIN=stable
export CARGO_TARGET_DIR=target
# The same two as %%build. The dev profile asks for full DWARF and this phase
# builds the graph a second time to get it, with no package made of it either;
# a failing test still names its file and line, which the panic carries rather
# than DWARF.
export RUSTFLAGS="${RUSTFLAGS:-} -Cdebuginfo=0"
build_jobs="%{_smp_build_ncpus}"
build_room="$(awk '/^MemTotal:/ { n = int($2 / 1024 / 2048); print (n < 1 ? 1 : n) }' /proc/meminfo 2>/dev/null || true)"
if [ -n "$build_room" ] && [ "$build_room" -lt "$build_jobs" ]; then
    build_jobs="$build_room"
fi
cargo test --offline --locked -j"$build_jobs"
# The two files that are read by something other than this program. Both are
# installed by then, so what is checked is what ships rather than what is in
# the checkout.
desktop-file-validate %{buildroot}%{_datadir}/applications/videonsole.desktop
appstream-util validate-relax --nonet \
    %{buildroot}%{_metainfodir}/io.github.petexy.videonsole.metainfo.xml

%files
%license LICENSE
%doc README.md
%{_bindir}/videonsole
%{_datadir}/applications/videonsole.desktop
%{_datadir}/icons/hicolor/scalable/apps/videonsole.svg
%{_metainfodir}/io.github.petexy.videonsole.metainfo.xml

%changelog
* Mon Aug 31 2026 Piotr Lewandowski <piotr.petexy@gmail.com> - 0.9.0-1
- First packaged release. A folder of films and one film at a time, in the
  LineXinBar design language.
- The film is drawn at its own resolution in a pass of the application's own,
  over the frame the toolkit composed, with the colour done on the graphics
  card from the coefficients the film itself declares.
- Decodes through VA-API where the machine has it and in software where it
  does not.
- Driven from a controller, a keyboard and a pointer at once: the controls come
  and go and the film takes the whole screen when they are away, the triggers
  scan through a film, and where you left off is remembered.
- Requires lxb-toolkit 0.9.0 to build, the version LineXinBar, the toolkit,
  Imagonsole, DistriBumpy and CEDM all release under.
