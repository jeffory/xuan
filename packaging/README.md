# Xuan

A native image editor for layered compositions, photo retouching, and camera RAW development on Linux and Windows.

On Windows, extract the entire ZIP and double-click `xuan.exe`, or run `.\xuan.exe --demo` in PowerShell to explore a sample composition.

On Linux, launch Xuan from your application menu or run `xuan --demo`. In a portable Linux archive, run `bin/xuan` directly or use `scripts/install.sh` to install under `~/.local`.

Native Linux releases require glibc 2.35 or newer. The AppImage includes glibc and its loader for older systems.

For an AppImage, make the downloaded file executable with `chmod +x xuan-*.AppImage`, then run `./xuan-*.AppImage --demo`. If FUSE is unavailable, add `--appimage-extract-and-run` before `--demo`.

Every package includes the MCP Server plugin, which lets LLM clients such as Claude Code see and edit your open documents from this computer. It lives in the bundled plugins folder next to Xuan: `/usr/lib/xuan/plugins/` for the deb and rpm packages, `lib/xuan/plugins/` in the portable Linux archive (`scripts/install.sh` installs it to `~/.local/lib/xuan/plugins/`), `usr/lib/xuan/plugins/` inside the AppImage, and `plugins\` next to `xuan.exe` on Windows. It does not run until you allow it: open **Window → MCP Server** and press **Review Permissions…**. To use a build of your own instead, install it with **Plugins → Install from Folder or Zip…**; a plugin with the same id in your plugins folder replaces the bundled one, and asks for permission again.

- [Installation and editing](../docs/USAGE.md)
- [Keyboard shortcuts](../docs/SHORTCUTS.md)
- [RAW workflow](../docs/RAW.md)
- [Source download and rebuilding](SOURCES.md)
- [Third-party notices](../THIRD_PARTY.md)

Xuan is [MIT licensed](../LICENSE), with separately licensed dependencies. Original Compositor copyright © 2026 Wonder Assembly LLC.
