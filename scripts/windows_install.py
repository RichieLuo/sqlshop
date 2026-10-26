#!/usr/bin/env python3
"""Install a verified Shop package for this user, with a stable latest-build launcher."""

import argparse
from contextlib import contextmanager
import ctypes
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time

SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))
from windows_desktop import run_json as desktop_json

APPLICATIONS = {
    "sqlshop": ("SQLShop", "SQL scripts and SQLite databases", {
        "SQL": ("SQL script", [".sql"]),
        "SQLite": ("SQLite database", [".sqlite", ".sqlite3", ".db", ".db3"]),
    }),
    "linkshop": ("LinkShop", "Postman collections and environments", {
        "PostmanJSON": ("Postman collection or environment", [".json"]),
    }),
    "rpshop": ("RPShop", "RPShop projects and supported native RP documents", {
        "Project": ("RPShop project", [".rpshop"]),
        "Native": ("Native RP project", [".rp"]),
        "Library": ("Native RP widget library", [".rplib"]),
        "JSONProject": ("RPShop JSON project", [".json"]),
    }),
}


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def atomic_write(path, content):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(prefix=".shop-update-", dir=path.parent, delete=False) as stream:
            temporary = Path(stream.name)
            stream.write(content)
        for attempt in range(10):
            try:
                os.replace(temporary, path)
                return
            except OSError:
                if attempt == 9:
                    raise
                time.sleep(0.1)
    finally:
        if temporary and temporary.exists():
            temporary.unlink()


def load_package(directory):
    directory = Path(directory).resolve()
    metadata = json.loads((directory / "package.json").read_text(encoding="utf-8"))
    name = metadata.get("name")
    if name not in APPLICATIONS or metadata.get("product") != APPLICATIONS[name][0]:
        raise ValueError("Not a supported Shop application package")
    if not metadata.get("target", "").endswith("-pc-windows-msvc"):
        raise ValueError("Only a Windows MSVC package can be installed")
    for key, checksum_key in (("executable", "executable_sha256"), ("launcher", "launcher_sha256")):
        filename = metadata.get(key, "")
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*\.exe", filename):
            raise ValueError(f"Invalid {key} filename")
        if sha256(directory / filename) != metadata.get(checksum_key):
            raise ValueError(f"Package {key} SHA-256 mismatch")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9.+-]*", metadata.get("version", "")):
        raise ValueError("Invalid package version")
    return metadata


class SystemInfo(ctypes.Structure):
    # SYSTEM_INFO's initial union is represented by its two WORD fields.
    _fields_ = [
        ("wProcessorArchitecture", ctypes.c_uint16), ("wReserved", ctypes.c_uint16),
        ("dwPageSize", ctypes.c_uint32), ("lpMinimumApplicationAddress", ctypes.c_void_p),
        ("lpMaximumApplicationAddress", ctypes.c_void_p), ("dwActiveProcessorMask", ctypes.c_size_t),
        ("dwNumberOfProcessors", ctypes.c_uint32), ("dwProcessorType", ctypes.c_uint32),
        ("dwAllocationGranularity", ctypes.c_uint32), ("wProcessorLevel", ctypes.c_uint16),
        ("wProcessorRevision", ctypes.c_uint16),
    ]


def native_windows_target():
    if os.name != "nt":
        return None
    info = SystemInfo()
    get_native_system_info = ctypes.WinDLL("kernel32", use_last_error=True).GetNativeSystemInfo
    get_native_system_info.argtypes = [ctypes.POINTER(SystemInfo)]
    get_native_system_info.restype = None
    get_native_system_info(ctypes.byref(info))
    # Query the OS rather than environment-backed platform.machine(), including under WOW64.
    architecture = {9: "x86_64", 12: "aarch64"}.get(info.wProcessorArchitecture)
    return f"{architecture}-pc-windows-msvc" if architecture else None


def install_home(product, install_base=None):
    if install_base is not None:
        return Path(install_base).resolve() / product
    local = os.environ.get("LOCALAPPDATA")
    if not local:
        raise OSError("LOCALAPPDATA is required for a per-user installation")
    return Path(local) / "Programs" / "Shop" / product


def installation_state(home):
    path = Path(home) / "install-state.json"
    if not path.is_file():
        return None
    return json.loads(path.read_text(encoding="utf-8"))


@contextmanager
def installation_lock(home):
    home.mkdir(parents=True, exist_ok=True)
    with (home / ".install.lock").open("a+b") as stream:
        stream.seek(0)
        stream.write(b"\0")
        stream.flush()
        stream.seek(0)
        if os.name == "nt":
            import msvcrt
            msvcrt.locking(stream.fileno(), msvcrt.LK_LOCK, 1)
        try:
            yield
        finally:
            if os.name == "nt":
                stream.seek(0)
                msvcrt.locking(stream.fileno(), msvcrt.LK_UNLCK, 1)


def registry_entries(metadata, launcher, payload):
    """Describe registration only: no extension default value or UserChoice is written."""
    name, product = metadata["name"], metadata["product"]
    _, description, file_types = APPLICATIONS[name]
    launcher, payload = str(launcher), str(payload)
    command = f'"{launcher}" --open "%1"'
    icon = f'"{payload}",0'
    capabilities = f"Software\\Shop\\{product}\\Capabilities"
    entries = [
        (capabilities, "ApplicationName", product, 1),
        (capabilities, "ApplicationDescription", description, 1),
        (capabilities, "ApplicationIcon", icon, 1),
        ("Software\\RegisteredApplications", product, capabilities, 1),
        (f"Software\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{name}.exe", "", launcher, 1),
        (f"Software\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{name}.exe", "Path", str(Path(launcher).parent), 1),
    ]
    application = f"Software\\Classes\\Applications\\{product}.exe"
    entries += [(application, "FriendlyAppName", product, 1),
                (application + "\\DefaultIcon", "", icon, 1),
                (application + "\\shell\\open\\command", "", command, 1)]
    for kind, (friendly_name, extensions) in file_types.items():
        progid = f"Shop.{product}.{kind}"
        key = f"Software\\Classes\\{progid}"
        entries += [(key, "", friendly_name, 1), (key, "FriendlyTypeName", friendly_name, 1),
                    (key + "\\DefaultIcon", "", icon, 1),
                    (key + "\\shell\\open\\command", "", command, 1)]
        for extension in extensions:
            entries += [(f"Software\\Classes\\{extension}\\OpenWithProgids", progid, b"", 0),
                        (application + "\\SupportedTypes", extension, "", 1),
                        (capabilities + "\\FileAssociations", extension, progid, 1)]
    return entries


def write_registry(entries):
    import winreg
    for path, name, value, kind in entries:
        with winreg.CreateKeyEx(winreg.HKEY_CURRENT_USER, path, 0, winreg.KEY_SET_VALUE) as key:
            winreg.SetValueEx(key, name, 0, kind, value)


def create_shortcut(product, launcher, payload):
    roaming = os.environ.get("APPDATA")
    if not roaming:
        raise OSError("APPDATA is required for a Start menu shortcut")
    shortcuts = Path(roaming) / "Microsoft" / "Windows" / "Start Menu" / "Programs" / "Shop"
    shortcuts.mkdir(parents=True, exist_ok=True)
    script_directory = Path(__file__).resolve().parent
    helper = script_directory / "windows" / "create-shortcut.ps1"
    if not helper.is_file():
        helper = script_directory / "create-shortcut.ps1"
    shortcut = shortcuts / f"{product}.lnk"
    subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", str(helper),
                    "-ShortcutPath", str(shortcut), "-TargetPath", str(launcher), "-IconPath", str(payload),
                    "-WorkingDirectory", str(launcher.parent), "-Description", f"Open the latest packaged {product}"], check=True)
    return shortcut


def notify_shell():
    import ctypes
    ctypes.windll.shell32.SHChangeNotify(0x08000000, 0, None, None)


def package_fingerprint(directory):
    digest = hashlib.sha256()
    for path in sorted(Path(directory).rglob("*")):
        if path.is_symlink():
            raise ValueError("A package must not contain symbolic links")
        if path.is_file():
            digest.update(path.relative_to(directory).as_posix().encode("utf-8") + b"\0")
            digest.update(bytes.fromhex(sha256(path)))
    return digest.hexdigest()


def install_package(directory, repository=None, install_base=None, integrate=True, automatic=False):
    directory = Path(directory).resolve()
    metadata = load_package(directory)
    product = metadata["product"]
    if integrate and metadata["target"] != native_windows_target():
        raise ValueError("Install only a package matching this Windows machine's architecture")
    home = install_home(product, install_base)
    fingerprint = package_fingerprint(directory)
    with installation_lock(home):
        previous = installation_state(home)
        if automatic and previous and previous.get("packaged_at", "") > metadata.get("packaged_at", ""):
            return previous
        versions = home / "versions"
        versions.mkdir(exist_ok=True)
        version = versions / f"{metadata['version']}-{metadata['executable_sha256'][:12]}-{fingerprint[:12]}"
        if not version.exists():
            # Copy privately; the old, possibly running payload is never touched.
            with tempfile.TemporaryDirectory(prefix=".preparing-", dir=versions) as temporary:
                prepared = Path(temporary) / "payload"
                shutil.copytree(directory, prepared)
                if package_fingerprint(prepared) != fingerprint:
                    raise OSError("Installed package copy verification failed")
                os.replace(prepared, version)
        elif package_fingerprint(version) != fingerprint:
            raise OSError("An existing immutable payload was modified; refusing to reuse it")
        payload = version / metadata["executable"]
        launcher = home / f"{product}.exe"
        launcher_source = directory / metadata["launcher"]
        if not launcher.exists() or sha256(launcher) != metadata["launcher_sha256"]:
            atomic_write(launcher, launcher_source.read_bytes())
        if integrate:
            write_registry(registry_entries(metadata, launcher, payload))
            shortcut = create_shortcut(product, launcher, payload)
        else:
            shortcut = None
        # Publish only after the new payload and shell registration are ready.
        atomic_write(home / "current.path", (str(payload) + "\n").encode("utf-8"))
        state = {
            "name": metadata["name"], "product": product, "version": metadata["version"],
            "target": metadata["target"], "executable_sha256": metadata["executable_sha256"],
            "launcher": str(launcher), "payload": str(payload), "package_directory": str(directory),
            "package_fingerprint": fingerprint, "packaged_at": metadata.get("packaged_at", ""),
            "source_repository": str(Path(repository).resolve()) if repository else None,
            "installed_at": datetime.now(timezone.utc).isoformat(),
            "start_menu_shortcut": str(shortcut) if shortcut else None,
        }
        atomic_write(home / "install-state.json", (json.dumps(state, ensure_ascii=False, indent=2) + "\n").encode("utf-8"))
        if integrate:
            notify_shell()
        return state


def refresh_install(directory, repository, mode=None):
    if mode is False:
        return None
    if os.name != "nt" or os.environ.get("CI", "").lower() in ("true", "1") or os.environ.get("GITHUB_ACTIONS") == "true":
        if mode:
            raise ValueError("Installation is available only on a local native Windows build")
        return None
    # Only the real desktop worker may inspect the user's installation state.
    return desktop_json([sys.executable, str(Path(__file__).resolve()), "--desktop-worker",
                         "--package-directory", str(Path(directory).resolve()),
                         "--repository", str(Path(repository).resolve()),
                         "--refresh-mode", "install" if mode else "automatic"])


def refresh_install_here(directory, repository, mode=None):
    if mode is False:
        return None
    if os.name != "nt" or os.environ.get("CI", "").lower() in ("true", "1") or os.environ.get("GITHUB_ACTIONS") == "true":
        if mode:
            raise ValueError("Installation is available only on a local native Windows build")
        return None
    summary = json.loads((Path(directory) / "package.json").read_text(encoding="utf-8"))
    if summary.get("name") not in APPLICATIONS:
        if mode:
            raise ValueError("This package has no registered Shop installation profile")
        return None
    metadata = load_package(directory)
    if metadata["target"] != native_windows_target():
        if mode:
            raise ValueError("Cannot install a cross-compiled package")
        return None
    state = installation_state(install_home(metadata["product"]))
    registered_repository = state.get("source_repository") if state else None
    if not mode and (not registered_repository or os.path.normcase(registered_repository) != os.path.normcase(str(Path(repository).resolve()))):
        return None
    return install_package(directory, repository, automatic=mode is None)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package-directory", required=True, type=Path)
    parser.add_argument("--repository", type=Path)
    parser.add_argument("--open-default-apps", action="store_true")
    parser.add_argument("--desktop-worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--refresh-mode", choices=("automatic", "install"), help=argparse.SUPPRESS)
    arguments = parser.parse_args()
    repository = arguments.repository
    if repository is None and Path(__file__).parent.name == "scripts":
        repository = Path(__file__).resolve().parent.parent
    try:
        if os.name == "nt" and not arguments.desktop_worker:
            command = [sys.executable, str(Path(__file__).resolve()), "--desktop-worker",
                       "--package-directory", str(arguments.package_directory.resolve())]
            if repository:
                command.extend(["--repository", str(repository.resolve())])
            if arguments.open_default_apps:
                command.append("--open-default-apps")
            state = desktop_json(command)
        elif arguments.refresh_mode:
            state = refresh_install_here(arguments.package_directory, repository,
                                         True if arguments.refresh_mode == "install" else None)
        else:
            state = install_package(arguments.package_directory, repository)
            if arguments.open_default_apps:
                os.startfile(f"ms-settings:defaultapps?registeredAppUser={state['product']}")
        print(json.dumps(state, ensure_ascii=True, indent=2))
    except subprocess.CalledProcessError as error:
        # The worker's normal stderr was already forwarded by the broker.
        parser.exit(error.returncode)
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f"Installation failed: {error}\n")


if __name__ == "__main__":
    main()
