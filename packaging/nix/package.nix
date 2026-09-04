{
  lib,
  rustPlatform,
  pkg-config,
  makeWrapper,
  wayland,
  libxkbcommon,
  libGL,
  vulkan-loader,
  # What decodes a film, and what plays its soundtrack. Both linked outright
  # rather than opened by name, so these are the two in the list that the usual
  # RPATH machinery does find for itself — they are in `buildInputs` below and
  # not in `openedAtRuntime`.
  ffmpeg,
  alsa-lib,
  # The design language, as a derivation. It is a *build* dependency and not a
  # runtime one: `lxb-render` is a Rust path dependency, so cargo compiles those
  # sources into this binary and nothing of the toolkit is referenced once it
  # is built. It is here rather than in nixpkgs because that is where it is —
  # the flake at the root of this checkout is what supplies it.
  lxb-toolkit,
  src ? ../..,
}:

let
  sourceRoot = toString src;
  cleanSrc = lib.cleanSourceWith {
    inherit src;
    filter =
      path: type:
      let
        relative = lib.removePrefix "${sourceRoot}/" (toString path);
      in
      !(
        relative == ".git"
        || lib.hasPrefix ".git/" relative
        || relative == "target"
        || lib.hasPrefix "target/" relative
        || relative == "packaging/out"
        || lib.hasPrefix "packaging/out/" relative
        || relative == "result"
        || lib.hasPrefix "result-" relative
      );
  };
  version = lib.removeSuffix "\n" (builtins.readFile ../../VERSION);

  # Opened by name at run time rather than linked, so nothing that reads the
  # executable can find them and the usual RPATH machinery never sees them
  # either. wayland is in the list twice over — it is linked as well — and it
  # costs nothing to say so once here.
  openedAtRuntime = [
    wayland
    libxkbcommon
    libGL
    vulkan-loader
  ];
in
rustPlatform.buildRustPackage {
  pname = "videonsole";
  inherit version;
  src = cleanSrc;

  cargoLock.lockFile = "${cleanSrc}/Cargo.lock";

  strictDeps = true;
  nativeBuildInputs = [ pkg-config makeWrapper ];
  buildInputs = openedAtRuntime ++ [ ffmpeg alsa-lib ];

  # Cargo.toml names the toolkit's crates at /usr/share, which is where every
  # other distribution here puts them and is nowhere at all under Nix. This is
  # the one line that makes the FHS assumption a store path; the lock file is
  # untouched by it, because a path dependency carries no source there.
  postPatch = ''
    substituteInPlace Cargo.toml \
      --replace-fail "/usr/share/lxb-toolkit/crates" \
                     "${lxb-toolkit}/share/lxb-toolkit/crates"
  '';

  # cargoInstallHook would install the binary and nothing else — no desktop
  # entry, no icon, no AppStream data — and a viewer that does not appear in
  # the menu is one nobody opens, and that no file manager will offer to open a
  # picture with. install.sh is what every other package
  # definition here uses, and using it means the Nix build cannot quietly ship
  # a different set of files than the .deb does.
  installPhase = ''
    runHook preInstall

    # install.sh reads the release directory of a target dir; buildRustPackage
    # builds under a target triple, so point it at the parent of that.
    targetDir="target"
    if [ ! -d "target/release" ]; then
      targetDir="$(dirname "$(dirname "$(readlink -f target/*/release)")")"
    fi

    bash packaging/install.sh \
      --destdir "$out" \
      --prefix "" \
      --target-dir "$targetDir"

    install -Dm0644 LICENSE "$out/share/licenses/videonsole/LICENSE"
    install -Dm0644 README.md "$out/share/doc/videonsole/README.md"

    runHook postInstall
  '';

  postFixup = ''
    wrapProgram "$out/bin/videonsole" \
      --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath openedAtRuntime}"
  '';

  meta = {
    description = "A film browser and player in the LineXinBar design language, shown as Videos";
    homepage = "https://github.com/Petexy/videonsole";
    license = lib.licenses.gpl3Only;
    platforms = lib.platforms.linux;
    mainProgram = "videonsole";
  };
}
