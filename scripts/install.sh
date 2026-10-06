#!/usr/bin/env bash
set -euo pipefail

xuan_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
xuan_prefix=${1:-"$HOME/.local"}
xuan_binary="$xuan_root/bin/xuan"
if [[ ! -f "$xuan_binary" ]]; then
    xuan_binary="$xuan_root/target/release/xuan"
fi
if [[ ! -f "$xuan_binary" ]]; then
    printf 'Build first with cargo build --release, or extract a release archive.\n' >&2
    exit 1
fi
install -Dm755 "$xuan_binary" "$xuan_prefix/bin/xuan"
# Plugins that come with Xuan go to <prefix>/lib/xuan/plugins, where Xuan
# looks for them next to bin/xuan. The folder belongs to Xuan: it is
# replaced, so a plugin removed from a release does not linger. Plugins the
# user installs live in the configuration folder and are left alone.
xuan_bundled="$xuan_prefix/lib/xuan/plugins"
if [[ -d "$xuan_root/lib/xuan/plugins" ]]; then
    rm -rf -- "$xuan_bundled"
    install -d "$xuan_bundled"
    cp -R --preserve=mode "$xuan_root/lib/xuan/plugins/." "$xuan_bundled/"
elif [[ -f "$xuan_root/plugins/mcp-server/target/release/xuan-mcp-server" ]]; then
    # A source checkout with the plugin built (cargo build --release in
    # plugins/mcp-server).
    rm -rf -- "$xuan_bundled"
    install -Dm644 "$xuan_root/plugins/mcp-server/plugin.toml" "$xuan_bundled/mcp-server/plugin.toml"
    install -Dm755 "$xuan_root/plugins/mcp-server/target/release/xuan-mcp-server" \
        "$xuan_bundled/mcp-server/target/release/xuan-mcp-server"
fi
if [[ -d "$xuan_root/share" ]]; then
    install -d "$xuan_prefix/share"
    cp -R --preserve=mode "$xuan_root/share/." "$xuan_prefix/share/"
else
    for xuan_icon in "$xuan_root"/assets/icons/hicolor/*/apps/me.silverl.xuan.png; do
        install -Dm644 "$xuan_icon" "$xuan_prefix/share/icons/${xuan_icon#"$xuan_root/assets/icons/"}"
    done
    install -Dm644 "$xuan_root/packaging/me.silverl.xuan.desktop" "$xuan_prefix/share/applications/me.silverl.xuan.desktop"
    install -Dm644 "$xuan_root/packaging/me.silverl.xuan.xml" "$xuan_prefix/share/mime/packages/me.silverl.xuan.xml"
    install -Dm644 "$xuan_root/LICENSE" "$xuan_prefix/share/licenses/xuan/LICENSE"
    install -Dm644 "$xuan_root/THIRD_PARTY.md" "$xuan_prefix/share/licenses/xuan/THIRD_PARTY.md"
    install -Dm644 "$xuan_root/licenses/rawler-LGPL-2.1.txt" "$xuan_prefix/share/licenses/xuan/rawler-LGPL-2.1.txt"
    install -Dm644 "$xuan_root/licenses/tabler-icons-MIT.txt" "$xuan_prefix/share/licenses/xuan/tabler-icons-MIT.txt"
    install -Dm644 "$xuan_root/licenses/seccompiler-BSD-3-Clause.txt" "$xuan_prefix/share/licenses/xuan/seccompiler-BSD-3-Clause.txt"
    install -Dm644 "$xuan_root/licenses/kurbo-MIT.txt" "$xuan_prefix/share/licenses/xuan/kurbo-MIT.txt"
    install -Dm644 "$xuan_root/assets/fonts/Inter-LICENSE.txt" "$xuan_prefix/share/licenses/xuan/Inter-LICENSE.txt"
    install -Dm644 "$xuan_root/assets/fonts/DroidSansFallback-LICENSE.txt" "$xuan_prefix/share/licenses/xuan/DroidSansFallback-LICENSE.txt"
    for xuan_license in LICENSE-MIT LICENSE-APACHE; do
        install -Dm644 "$xuan_root/vendor/egui-winit/$xuan_license" "$xuan_prefix/share/licenses/xuan/egui-winit/$xuan_license"
    done
fi
# Remove the previous logo so desktops cannot select it as a scalable fallback.
rm -f -- "$xuan_prefix/share/icons/hicolor/scalable/apps/me.silverl.xuan.svg"
if command -v gtk-update-icon-cache >/dev/null; then
    gtk-update-icon-cache -f -t "$xuan_prefix/share/icons/hicolor"
fi
if command -v update-desktop-database >/dev/null; then
    update-desktop-database "$xuan_prefix/share/applications"
fi
if command -v update-mime-database >/dev/null; then
    update-mime-database "$xuan_prefix/share/mime"
fi
printf 'Installed xuan to %s. Ensure %s/bin is in PATH.\n' "$xuan_prefix" "$xuan_prefix"
