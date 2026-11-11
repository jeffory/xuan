#!/usr/bin/env python3
"""Build a portable Windows x86_64 ZIP and matching rebuildable sources."""

import os
import platform
import runpy
import shutil
import subprocess
import sys
import tempfile
import tomllib
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TARGET = "x86_64-pc-windows-msvc"


def main():
    if sys.platform != "win32" or platform.machine().lower() not in ("amd64", "x86_64"):
        raise SystemExit(
            "Windows packaging requires a native x86_64 Windows host with MSVC."
        )
    version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))[
        "package"
    ]["version"]
    name = f"xuan-{version}-windows-x86_64"
    destination = ROOT / "dist"
    destination.mkdir(exist_ok=True)

    env = os.environ.copy()
    # An explicit target keeps the static CRT setting out of host build scripts
    # and proc macros. The portable executable needs no VC++ redistributable.
    env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    env["RUSTFLAGS"] = (
        f"{env.get('RUSTFLAGS', '')} -C target-feature=+crt-static".strip()
    )
    target_directory = ROOT / "target"
    subprocess.run(
        [
            "cargo",
            "build",
            "--release",
            "--locked",
            "--target",
            TARGET,
            "--target-dir",
            str(target_directory),
        ],
        cwd=ROOT,
        env=env,
        check=True,
    )

    # The MCP server plugin ships in the package. It is its own Cargo
    # workspace, so it gets its own target directory.
    plugin_source = ROOT / "plugins/mcp-server"
    plugin_target_directory = plugin_source / "target"
    subprocess.run(
        [
            "cargo",
            "build",
            "--release",
            "--locked",
            "--target",
            TARGET,
            "--target-dir",
            str(plugin_target_directory),
        ],
        cwd=plugin_source,
        env=env,
        check=True,
    )

    with tempfile.TemporaryDirectory(prefix=".windows-", dir=destination) as directory:
        stage = Path(directory) / name
        stage.mkdir()
        shutil.copy2(target_directory / TARGET / "release/xuan.exe", stage / "xuan.exe")
        # Bundled plugins sit in `plugins` next to xuan.exe, where Xuan looks
        # for them. The manifest's command, target/release/xuan-mcp-server,
        # starts the .exe: Windows adds the extension.
        plugin = stage / "plugins/mcp-server"
        (plugin / "target/release").mkdir(parents=True)
        shutil.copy2(plugin_source / "plugin.toml", plugin / "plugin.toml")
        shutil.copy2(
            plugin_target_directory / TARGET / "release/xuan-mcp-server.exe",
            plugin / "target/release/xuan-mcp-server.exe",
        )
        # The Python plugin SDK sits in sdk\python next to xuan.exe, beside
        # the bundled plugins; Xuan puts it on every plugin's PYTHONPATH.
        sdk = stage / "sdk/python"
        sdk.mkdir(parents=True)
        shutil.copy2(ROOT / "sdk/python/xuan_plugin.py", sdk / "xuan_plugin.py")
        licenses = stage / "share/licenses/xuan"
        licenses.mkdir(parents=True)
        for filename in (
            "LICENSE",
            "licenses/rawler-LGPL-2.1.txt",
            "licenses/heic-rs-MIT.txt",
            "licenses/seccompiler-BSD-3-Clause.txt",
            "licenses/kurbo-MIT.txt",
            "licenses/libwebp-BSD-3-Clause.txt",
            "licenses/Hack-LICENSE.txt",
            "licenses/tabler-icons-MIT.txt",
            "assets/fonts/Inter-LICENSE.txt",
            "assets/fonts/DroidSansFallback-LICENSE.txt",
        ):
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
                str(stage / "share/doc/xuan"),
                version,
            ],
            check=True,
        )
        package = destination / f"{name}.zip"
        with zipfile.ZipFile(package, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for path in sorted(stage.rglob("*")):
                if path.is_file():
                    archive.write(path, path.relative_to(stage.parent).as_posix())

    source_packager = runpy.run_path(str(ROOT / "scripts/package-source.py"))
    source_packager["write_checksum"](package)
    source_packager["main"]()
    print(f"Created {package}")


if __name__ == "__main__":
    main()
