# macOS port

Xuan ships for Linux and Windows. This page tracks the work to add macOS (Apple Silicon first) as a
third platform. It replaces four tracking issues; tick items off as they land.

The engine already ports cleanly: decoders are pure Rust, shaders use only baseline WebGPU features,
fonts are bundled, and the `cfg(unix)` code (process groups, file modes, atomic saves) suits macOS.
A cross-compiled `cargo clippy --target aarch64-apple-darwin` of `develop` built every dependency
and the library. The work is in four parts.

## 1. Build, start and test on a Mac

- [x] Enable wgpu's `metal` backend for macOS. eframe turns wgpu's defaults off, so without it a Mac
  build finds no adapter at startup.
- [x] `TabletInput::new` (`src/app/tablet/mod.rs`) returns `None` where there is no native pen
  backend, instead of failing to compile.
- [x] `.github/workflows/macos.yml`: formatting, Clippy, tests and goldens, the GPU checks on Metal,
  the native clipboard test, a `--demo --screenshot` smoke test and the MCP server plugin, on an
  Apple Silicon runner. It uploads the unsigned executable and the screenshot as the
  `xuan-macos-arm64-unsigned` artifact.
- [x] `scripts/fetch-raw-fixtures.sh` falls back to `shasum -a 256` where GNU `sha256sum` is missing.

## 2. Mac keyboard and window conventions

- [ ] **Command shortcuts.** Commands keep one primary modifier, written `Ctrl` in the
  configuration and the docs. On macOS it is ⌘ Command (the Control key also works), and menus,
  tool tips and Settings show Mac symbols such as ⇧⌘S. Ctrl+click actions are ⌘-click.
- [ ] **Safe quit.** winit's default app menu quits with `terminate:` and never asks the app, so
  ⌘Q, Quit in the Dock and logging out would skip the unsaved-changes prompt. Xuan now answers
  `applicationShouldTerminate:`: it quits at once when nothing is unsaved, and otherwise cancels
  and shows the prompt.
- [ ] **Native title bar.** The drawn macOS-style title bar is retired on every platform. On macOS the
  Compact style keeps the system's own window buttons over a full-size content view, with Xuan's
  menus beside them; System keeps a standard title bar. Configuration files that say
  `title_bar = "macos"` load as Compact.
- [ ] A native menu bar at the top of the screen, generated from the command registry (for example
  with `muda`), with About, Settings… (⌘,) and the Window menu where Mac users expect them.

## 3. App bundle and distribution (unsigned for now)

- [ ] `scripts/package-macos.py`: `Xuan.app` with an `Info.plist` declaring `.xuan` and the import
  types (from `packaging/me.silverl.xuan.xml`), an `.icns` from the existing PNG icons, and a DMG.
- [ ] Open files from Finder: double-click, Open With and drops on the Dock icon arrive as an
  `odoc` Apple Event that winit does not forward. Install a handler in
  `applicationWillFinishLaunching`.
- [ ] Bundled plugins and the Python SDK under `Contents/Resources` (`bundled_dir_for` in
  `src/plugins/mod.rs`), with `current_exe()` canonicalized. Finder starts apps with a minimal
  `PATH`, so plugin interpreters from Homebrew are not found.
- [ ] `check-packages.py --macos` and the release workflow (`xuan-*-x86_64` download pattern,
  checksums, uploads).
- [ ] Later: Developer ID signing and notarization (hardened runtime, no App Sandbox, which would
  stop plugins from running).

## 4. Platform features and docs

- [ ] Pen pressure, tilt and eraser: a `tablet/macos.rs` backend reading `NSEvent` tablet events.
- [ ] System theme (`src/app/system_theme.rs`): read `AppleInterfaceStyle` and `AppleAccentColor`
  from the user defaults. `NSApp.effectiveAppearance` is main-thread only, and the theme watcher
  polls from its own thread.
- [ ] Decide the configuration folder: `~/.config/xuan` today, `~/Library/Application Support/xuan`
  by Mac convention. Moving it after a release needs a migration.
- [ ] Hide dot-prefixed system font families (".SF NS") in the font list.
- [ ] Plugin network blocking stays Linux-only, as on Windows; `sandbox-exec` is deprecated.
- [ ] User docs: README, USAGE (install, configuration path, shortcuts), PLUGINS, MCP, THIRD_PARTY,
  and GENERATIVE.md, which says macOS is not a release target.
