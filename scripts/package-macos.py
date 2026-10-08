#!/usr/bin/env python3
"""Build an unsigned Xuan.app for the host's macOS architecture, in a DMG."""

import os
import platform
import plistlib
import runpy
import shutil
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
IDENTIFIER = "me.silverl.xuan"
# Matches MACOSX_DEPLOYMENT_TARGET in .github/workflows/macos.yml.
MINIMUM_SYSTEM = "11.0"
ICON_SIZES = (16, 32, 128, 256, 512)
LICENSES = (
    "LICENSE",
    "licenses/rawler-LGPL-2.1.txt",
    "licenses/heic-rs-MIT.txt",
    "licenses/seccompiler-BSD-3-Clause.txt",
    "licenses/kurbo-MIT.txt",
    "licenses/Hack-LICENSE.txt",
    "licenses/tabler-icons-MIT.txt",
    "assets/fonts/Inter-LICENSE.txt",
    "assets/fonts/DroidSansFallback-LICENSE.txt",
)
PROJECT_TYPE = f"{IDENTIFIER}.project"
PSB_TYPE = f"{IDENTIFIER}.psb"


def architecture():
    """The name used in package names: arm64 or x86_64."""
    return {"arm64": "arm64", "aarch64": "arm64", "x86_64": "x86_64"}[
        platform.machine()
    ]


def info_plist(version):
    """Info.plist: the bundle, `.xuan` projects (Xuan owns them) and the images and
    Photoshop files it imports (as an alternative to the user's default app), like the
    MIME types in packaging/me.silverl.xuan.xml and the .desktop file."""

    def document(name, types, rank):
        return {
            "CFBundleTypeName": name,
            "CFBundleTypeRole": "Editor",
            "LSHandlerRank": rank,
            "LSItemContentTypes": types,
        }

    return {
        "CFBundleDevelopmentRegion": "en",
        "CFBundleDisplayName": "Xuan",
        "CFBundleExecutable": "xuan",
        "CFBundleIconFile": "xuan",
        "CFBundleIdentifier": IDENTIFIER,
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": "Xuan",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": version,
        "LSApplicationCategoryType": "public.app-category.graphics-design",
        "LSMinimumSystemVersion": MINIMUM_SYSTEM,
        "NSHighResolutionCapable": True,
        "NSPrincipalClass": "NSApplication",
        "NSSupportsAutomaticGraphicsSwitching": True,
        "CFBundleDocumentTypes": [
            document("Xuan Project", [PROJECT_TYPE], "Owner"),
            document(
                "Photoshop Image", ["com.adobe.photoshop-image", PSB_TYPE], "Alternate"
            ),
            document(
                "Image",
                [
                    "public.png",
                    "public.jpeg",
                    "public.tiff",
                    "org.webmproject.webp",
                    "com.microsoft.bmp",
                    "com.compuserve.gif",
                    "public.heic",
                    "public.heif",
                    # NEF, NRW, CR2, CR3, CRW, RAF and ARW all conform to it.
                    "public.camera-raw-image",
                ],
                "Alternate",
            ),
        ],
        "UTExportedTypeDeclarations": [
            {
                "UTTypeIdentifier": PROJECT_TYPE,
                "UTTypeDescription": "Xuan Project",
                "UTTypeIconFile": "xuan",
                # A ZIP archive inside, but not conforming to public.zip-archive,
                # so Archive Utility does not offer to unpack it.
                "UTTypeConformsTo": ["public.data", "public.composite-content"],
                "UTTypeTagSpecification": {
                    "public.filename-extension": ["xuan"],
                    "public.mime-type": ["application/x-xuan-project"],
                },
            }
        ],
        "UTImportedTypeDeclarations": [
            {
                "UTTypeIdentifier": PSB_TYPE,
                "UTTypeDescription": "Photoshop Large Document",
                "UTTypeConformsTo": ["public.image", "public.data"],
                "UTTypeTagSpecification": {
                    "public.filename-extension": ["psb"],
                    "public.mime-type": ["image/x-photoshop-large"],
                },
            }
        ],
    }


def write_icon(destination):
    """xuan.icns from the PNG icons, each size at 1x and 2x."""
    icons = ROOT / "assets/icons/hicolor"
    with tempfile.TemporaryDirectory() as directory:
        iconset = Path(directory) / "xuan.iconset"
        iconset.mkdir()
        for size in ICON_SIZES:
            for scale, suffix in ((1, ""), (2, "@2x")):
                pixels = size * scale
                shutil.copy2(
                    icons / f"{pixels}x{pixels}/apps/{IDENTIFIER}.png",
                    iconset / f"icon_{size}x{size}{suffix}.png",
                )
        subprocess.run(
            ["iconutil", "--convert", "icns", "--output", destination, iconset],
            check=True,
        )


def build(manifest_directory, target_directory):
    subprocess.run(
        [
            "cargo",
            "build",
            "--release",
            "--locked",
            "--target-dir",
            str(target_directory),
        ],
        cwd=manifest_directory,
        check=True,
    )


def main():
    if sys.platform != "darwin":
        raise SystemExit("macOS packaging requires a macOS host with Xcode's tools.")
    os.umask(0o022)
    version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))[
        "package"
    ]["version"]
    name = f"xuan-{version}-macos-{architecture()}"
    destination = ROOT / "dist"
    destination.mkdir(exist_ok=True)

    target_directory = ROOT / "target"
    build(ROOT, target_directory)
    # The MCP server plugin ships in the package. It is its own Cargo
    # workspace, so it gets its own target directory.
    plugin_source = ROOT / "plugins/mcp-server"
    plugin_target_directory = plugin_source / "target"
    build(plugin_source, plugin_target_directory)

    with tempfile.TemporaryDirectory(prefix=".macos-", dir=destination) as directory:
        stage = Path(directory) / name
        app = stage / "Xuan.app"
        contents = app / "Contents"
        resources = contents / "Resources"
        (contents / "MacOS").mkdir(parents=True)
        resources.mkdir()
        with (contents / "Info.plist").open("wb") as stream:
            plistlib.dump(info_plist(version), stream)
        (contents / "PkgInfo").write_text("APPL????", encoding="ascii")
        shutil.copy2(target_directory / "release/xuan", contents / "MacOS/xuan")
        write_icon(resources / "xuan.icns")
        # Bundled plugins and the Python SDK sit in Resources, where Xuan
        # looks for them (src/plugins/mod.rs, bundled_dir_for).
        plugin = resources / "plugins/mcp-server"
        (plugin / "target/release").mkdir(parents=True)
        shutil.copy2(plugin_source / "plugin.toml", plugin / "plugin.toml")
        plugin_executable = plugin / "target/release/xuan-mcp-server"
        shutil.copy2(
            plugin_target_directory / "release/xuan-mcp-server", plugin_executable
        )
        sdk = resources / "sdk/python"
        sdk.mkdir(parents=True)
        shutil.copy2(ROOT / "sdk/python/xuan_plugin.py", sdk / "xuan_plugin.py")
        # The same layout as the Linux packages' share folder, so the
        # documents' relative links to the licenses keep working.
        licenses = resources / "share/licenses/xuan"
        licenses.mkdir(parents=True)
        for filename in LICENSES:
            shutil.copy2(ROOT / filename, licenses / Path(filename).name)
        (licenses / "egui-winit").mkdir()
        for filename in ("LICENSE-MIT", "LICENSE-APACHE"):
            shutil.copy2(
                ROOT / "vendor/egui-winit" / filename,
                licenses / "egui-winit" / filename,
            )
        subprocess.run(
            [
                sys.executable,
                str(ROOT / "scripts/package-docs.py"),
                str(resources / "share/doc/xuan"),
                version,
            ],
            check=True,
        )
        # Not a Developer ID signature: an ad-hoc one seals the bundle, which
        # Apple Silicon requires to run it. Nested code is signed first.
        for code in (plugin_executable, app):
            subprocess.run(
                ["codesign", "--force", "--sign", "-", "--timestamp=none", code],
                check=True,
            )
        (stage / "Applications").symlink_to("/Applications")
        package = destination / f"{name}.dmg"
        package.unlink(missing_ok=True)
        subprocess.run(
            [
                "hdiutil",
                "create",
                "-volname",
                f"Xuan {version}",
                "-srcfolder",
                stage,
                "-fs",
                "HFS+",
                "-format",
                "UDZO",
                package,
            ],
            check=True,
        )

    source_packager = runpy.run_path(str(ROOT / "scripts/package-source.py"))
    source_packager["write_checksum"](package)
    source_packager["main"]()
    print(f"Created {package}")


if __name__ == "__main__":
    main()
