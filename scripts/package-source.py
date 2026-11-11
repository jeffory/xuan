#!/usr/bin/env python3
"""Package matching, rebuildable sources for every supported platform."""

import hashlib
import json
import os
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def write_checksum(package):
    with package.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    package.with_name(package.name + ".sha256").write_text(
        f"{digest}  {package.name}\n", encoding="utf-8", newline="\n"
    )


def main():
    version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))[
        "package"
    ]["version"]
    name = f"xuan-{version}-source"
    destination = ROOT / "dist"
    destination.mkdir(exist_ok=True)
    host = (
        subprocess.check_output(["rustc", "-vV"], text=True)
        .split("host: ")[1]
        .splitlines()[0]
    )
    metadata_args = [
        "cargo",
        "metadata",
        "--format-version",
        "1",
        "--filter-platform",
        host,
    ]
    metadata = json.loads(
        subprocess.check_output([*metadata_args, "--locked"], cwd=ROOT)
    )
    rawler = next(p for p in metadata["packages"] if p["name"] == "rawler")

    with tempfile.TemporaryDirectory(prefix=".source-", dir=destination) as directory:
        source = Path(directory) / name
        source.mkdir()
        ignore = shutil.ignore_patterns("*.env", "__pycache__", "*.pyc", ".git")
        for folder in (
            "src",
            "assets",
            "vendor",
            "licenses",
            "scripts",
            "packaging",
            "docs",
            ".github",
        ):
            shutil.copytree(ROOT / folder, source / folder, ignore=ignore)
        # The MCP server plugin ships in the binary packages; it is its own
        # Cargo workspace with its own lock file, built on the plugin SDK.
        plugin_ignore = shutil.ignore_patterns(
            "*.env", "__pycache__", "*.pyc", ".git", "target"
        )
        for folder in ("plugins/mcp-server", "sdk/xuan-plugin"):
            shutil.copytree(ROOT / folder, source / folder, ignore=plugin_ignore)
        # So does the Python plugin SDK, for Python plugins.
        (source / "sdk/python").mkdir(parents=True)
        shutil.copy2(ROOT / "sdk/python/xuan_plugin.py", source / "sdk/python")
        for filename in (
            "build.rs",
            "Cargo.toml",
            "Cargo.lock",
            "LICENSE",
            "README.md",
            "THIRD_PARTY.md",
        ):
            shutil.copy2(ROOT / filename, source / filename)
        shutil.copytree(
            Path(rawler["manifest_path"]).parent,
            source / "vendor/rawler",
            dirs_exist_ok=True,
            ignore=ignore,
        )
        # The tarball has no .git; build.rs reads the commit from this file instead.
        commit = os.environ.get("XUAN_BUILD_COMMIT", "").strip()
        if not commit:
            try:
                commit = subprocess.check_output(
                    ["git", "rev-parse", "HEAD"],
                    cwd=ROOT,
                    text=True,
                    stderr=subprocess.DEVNULL,
                ).strip()
            except (OSError, subprocess.CalledProcessError):
                commit = ""
        if commit:
            (source / ".xuan-build-commit").write_text(
                commit[:7] + "\n", encoding="utf-8", newline="\n"
            )
        manifest = source / "Cargo.toml"
        content = manifest.read_text(encoding="utf-8")
        if "rawler" not in tomllib.loads(content).get("patch", {}).get("crates-io", {}):
            content = content.replace(
                "[patch.crates-io]\n",
                '[patch.crates-io]\nrawler = { path = "vendor/rawler" }\n',
                1,
            )
            manifest.write_text(content, encoding="utf-8", newline="\n")
        # Resolve the path patch so recipients can rebuild with --locked.
        subprocess.run(
            [*metadata_args, "--offline", "--manifest-path", str(manifest)],
            cwd=ROOT,
            stdout=subprocess.DEVNULL,
            check=True,
        )
        package = destination / f"{name}.tar.gz"
        with tarfile.open(package, "w:gz") as archive:
            archive.add(source, arcname=name)
    write_checksum(package)
    print(f"Created {package}")


if __name__ == "__main__":
    main()
