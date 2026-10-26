#!/usr/bin/env python3
"""Build and package a Shop desktop application using only Python's standard library."""

import argparse
import csv
from datetime import datetime, timezone
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile

SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))
from windows_install import refresh_install


def build_windows_launcher(destination, target):
    arguments = ["rustc", "--edition", "2024", "--crate-name", "shop_launcher", "--target", target,
                 "-O", "-C", "strip=symbols", "-C", "target-feature=+crt-static",
                 str(SCRIPT_DIRECTORY / "windows" / "launcher.rs"), "-o", str(destination)]
    linker = os.environ.get(f"CARGO_TARGET_{target.replace('-', '_').upper()}_LINKER")
    if linker:
        arguments.extend(["-C", f"linker={linker}"])
    subprocess.run(arguments, check=True)


def cargo_json(root, *arguments):
    result = subprocess.run(
        ["cargo", *arguments], cwd=root, check=True, capture_output=True, text=True
    )
    return json.loads(result.stdout)


def native_target():
    result = subprocess.run(
        ["rustc", "-vV"], check=True, capture_output=True, text=True
    )
    return next(line.removeprefix("host: ") for line in result.stdout.splitlines()
                if line.startswith("host: "))


def platform_name(target):
    architecture = {"x86_64": "x64", "aarch64": "arm64"}.get(target.split("-")[0])
    if not architecture:
        raise ValueError(f"Unsupported package architecture: {target}")
    if target.endswith("-pc-windows-msvc"):
        return f"windows-{architecture}", ".zip", ".exe"
    if target.endswith("-apple-darwin"):
        return f"macos-{architecture}", ".tar.gz", ""
    if "-linux-" in target:
        return f"linux-{architecture}", ".tar.gz", ""
    raise ValueError(f"Unsupported package target: {target}")


def copy_notices(metadata, package, stage, root):
    """Include the resolved graph, including local patched crates and build tools."""
    resolved = {node["id"] for node in metadata["resolve"]["nodes"]}
    output = io.StringIO(newline="")
    writer = csv.writer(output, delimiter="\t", lineterminator="\n")
    writer.writerow(["Component", "Version", "License", "License files"])
    for dependency in sorted(metadata["packages"], key=lambda item: (item["name"], item["version"])):
        if dependency["id"] == package["id"] or dependency["id"] not in resolved:
            continue
        source = Path(dependency["manifest_path"]).parent
        files = {path for path in source.iterdir() if path.is_file()
                 and re.match(r"^(LICENSE|LICENCE|COPYING|COPYRIGHT|NOTICE)([.\-_].*)?$", path.name, re.I)}
        if dependency.get("license_file"):
            explicit = source / dependency["license_file"]
            if explicit.is_file():
                files.add(explicit)
        copied = []
        for path in sorted(files):
            destination = stage / "licenses" / f"{dependency['name']}-{dependency['version']}" / path.name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, destination)
            copied.append(destination.relative_to(stage).as_posix())
        writer.writerow([dependency["name"], dependency["version"], dependency.get("license") or "",
                         "; ".join(copied)])
    (stage / "THIRD-PARTY-NOTICES.tsv").write_text(output.getvalue(), encoding="utf-8-sig")
    # Fonts and other embedded assets can carry licenses outside Cargo's graph.
    assets = root / "assets"
    if assets.exists():
        for path in sorted(assets.rglob("*")):
            if path.is_file() and re.match(r"^(LICENSE|LICENCE|COPYING|COPYRIGHT|NOTICE|OFL)([.\-_].*)?$", path.name, re.I):
                destination = stage / "licenses" / "assets" / path.relative_to(assets)
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(path, destination)


def write_archive(stage, destination, extension, executable_name):
    if extension == ".zip":
        with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for path in sorted(stage.rglob("*")):
                if path.is_file():
                    # Cargo license files may have epoch timestamps; ZIP starts in 1980.
                    entry = zipfile.ZipInfo((Path(stage.name) / path.relative_to(stage)).as_posix())
                    entry.compress_type = zipfile.ZIP_DEFLATED
                    entry.external_attr = (path.stat().st_mode & 0xFFFF) << 16
                    archive.writestr(entry, path.read_bytes())
    else:
        def executable_mode(member):
            if member.name == f"{stage.name}/{executable_name}":
                member.mode |= 0o111
            return member

        with tarfile.open(destination, "w:gz") as archive:
            archive.add(stage, arcname=stage.name, filter=executable_mode)


def package_project(root, target, output=None, skip_build=False, expected_tag=None, install=None):
    root = Path(root).resolve()
    platform, extension, executable_extension = platform_name(target)
    if install and not executable_extension:
        raise ValueError("Start menu installation requires a native Windows package")
    manifest = cargo_json(root, "metadata", "--no-deps", "--locked", "--offline", "--format-version", "1")
    package = next(item for item in manifest["packages"]
                   if Path(item["manifest_path"]).resolve() == root / "Cargo.toml")
    settings = package["metadata"]["shop"]
    product, version = settings["product-name"], package["version"]
    if expected_tag and expected_tag != f"v{version}":
        raise ValueError(f"Release tag {expected_tag!r} does not match Cargo version v{version}")
    # These values become filesystem names, so reject path separators and traversal.
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", product):
        raise ValueError("product-name must be a plain filename")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9.+-]*", version):
        raise ValueError("Invalid package version")
    name = f"{product}-{version}-{platform}"
    output = Path(output).resolve() if output else root / "dist"
    stage = output / name
    archive_path = output / f"{name}{extension}"
    checksum_path = output / f"{archive_path.name}.sha256"
    for path in (stage, archive_path, checksum_path):
        if path.exists():
            raise FileExistsError(f"Package already exists: {path}; choose another output directory")
    if not skip_build:
        environment = os.environ.copy()
        if executable_extension:
            # Apply to Cargo's encoded flags too, which take precedence over RUSTFLAGS.
            if "CARGO_ENCODED_RUSTFLAGS" in environment:
                previous_flags = environment["CARGO_ENCODED_RUSTFLAGS"]
                environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(
                    part for part in (previous_flags, "-C", "target-feature=+crt-static") if part
                )
            else:
                environment["RUSTFLAGS"] = (environment.get("RUSTFLAGS", "") + " -C target-feature=+crt-static").strip()
        subprocess.run(["cargo", "build", "--locked", "--release", "--target", target],
                       cwd=root, env=environment, check=True)
    executable = Path(manifest["target_directory"]) / target / "release" / f"{package['name']}{executable_extension}"
    if not executable.is_file():
        raise FileNotFoundError(f"Release executable was not produced: {executable}")
    metadata = cargo_json(root, "metadata", "--locked", "--offline", "--format-version", "1", "--filter-platform", target)
    output.mkdir(parents=True, exist_ok=True)
    # Prepare privately and move finished artifacts only after every step succeeds.
    with tempfile.TemporaryDirectory(prefix=".shop-package-", dir=output) as temporary:
        temporary = Path(temporary)
        prepared = temporary / name
        prepared.mkdir()
        shutil.copy2(executable, prepared / executable.name)
        readme = (root / "docs" / "portable-readme.txt").read_text(encoding="utf-8-sig")
        header = f"{product} {version} · {platform}\n\n"
        (prepared / "使用说明.txt").write_text(header + readme, encoding="utf-8-sig")
        for filename in ("LICENSE", "LICENSE.md", "LICENSE.txt", "COPYING"):
            if (root / filename).is_file():
                shutil.copyfile(root / filename, prepared / filename)
        copy_notices(metadata, package, prepared, root)
        package_metadata = {
            "name": package["name"], "product": product, "version": version,
            "target": target, "executable": executable.name,
            "executable_sha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
            "packaged_at": datetime.now(timezone.utc).isoformat(),
        }
        if executable_extension:
            launcher = prepared / "shop-launcher.exe"
            build_windows_launcher(launcher, target)
            package_metadata.update(launcher=launcher.name, launcher_sha256=hashlib.sha256(launcher.read_bytes()).hexdigest())
            for source, filename in (("install.ps1", "install.ps1"), ("windows_install.py", "windows_install.py"),
                                     ("windows_desktop.py", "windows_desktop.py"),
                                     ("windows/create-shortcut.ps1", "create-shortcut.ps1"),
                                     ("windows/launch-desktop.ps1", "launch-desktop.ps1")):
                shutil.copyfile(SCRIPT_DIRECTORY / source, prepared / filename)
        (prepared / "package.json").write_text(json.dumps(package_metadata, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        temporary_archive = temporary / archive_path.name
        write_archive(prepared, temporary_archive, extension, executable.name)
        digest = hashlib.sha256(temporary_archive.read_bytes()).hexdigest()
        temporary_checksum = temporary / checksum_path.name
        temporary_checksum.write_text(f"{digest}  {archive_path.name}\n", encoding="ascii")
        shutil.move(str(prepared), str(stage))
        shutil.move(str(temporary_archive), str(archive_path))
        shutil.move(str(temporary_checksum), str(checksum_path))
    if executable_extension:
        try:
            installation = refresh_install(stage, root, install)
            if installation:
                print(f"Start menu launcher updated: {installation['launcher']}")
        except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
            raise RuntimeError(f"Package created at {archive_path}, but installation refresh failed: {error}") from error
    return archive_path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", help="Rust target triple; defaults to the installed host")
    parser.add_argument("--output-directory", type=Path)
    parser.add_argument("--skip-build", action="store_true", help="Package an existing release build")
    parser.add_argument("--expected-tag", help="Require a v<version> tag matching Cargo.toml")
    installation = parser.add_mutually_exclusive_group()
    installation.add_argument("--install", dest="install", action="store_const", const=True, default=None,
                              help="Install this native Windows package and register Start menu/Open With")
    installation.add_argument("--no-install", dest="install", action="store_const", const=False,
                              help="Build a package without updating an existing local installation")
    arguments = parser.parse_args()
    try:
        print(package_project(Path(__file__).resolve().parent.parent,
                              arguments.target or native_target(), arguments.output_directory,
                              arguments.skip_build, arguments.expected_tag, arguments.install))
    except (OSError, ValueError, KeyError, RuntimeError, StopIteration, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Package failed: {error}\n")


if __name__ == "__main__":
    main()
