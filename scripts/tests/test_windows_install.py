"""Check installer behavior in temporary directories without changing Windows registrations."""

import importlib.util
import json
import os
from pathlib import Path
import platform
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("shop_windows_install_tests", SCRIPTS / "windows_install.py")
installer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(installer)


class WindowsInstallTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="shop install ")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.base = self.root / "Programs 用户"

    def package(self, folder, payload=b"first payload", name="sqlshop", packaged_at="2026-10-26T01:00:00+00:00"):
        directory = self.root / folder
        directory.mkdir()
        executable = directory / f"{name}.exe"
        executable.write_bytes(payload)
        launcher = directory / "shop-launcher.exe"
        launcher.write_bytes(b"stable launcher")
        (directory / "LICENSE").write_text("License preserved", encoding="utf-8")
        metadata = {
            "name": name, "product": installer.APPLICATIONS[name][0], "version": "1.2.3",
            "target": "x86_64-pc-windows-msvc", "executable": executable.name,
            "executable_sha256": installer.sha256(executable), "launcher": launcher.name,
            "launcher_sha256": installer.sha256(launcher), "packaged_at": packaged_at,
        }
        (directory / "package.json").write_text(json.dumps(metadata), encoding="utf-8")
        return directory

    def install(self, directory, **arguments):
        return installer.install_package(directory, repository=self.root, install_base=self.base,
                                         integrate=False, **arguments)

    @unittest.skipUnless(os.name == "nt", "Windows architecture API regression")
    def test_native_architecture_works_when_processor_environment_is_missing(self):
        expected = installer.native_windows_target()
        self.assertIn(expected, ("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"))
        environment = {key: value for key, value in os.environ.items()
                       if key not in ("PROCESSOR_ARCHITECTURE", "PROCESSOR_ARCHITEW6432")}
        with patch.dict(os.environ, environment, clear=True), \
                patch.object(platform, "machine", return_value="") as machine:
            self.assertEqual(installer.native_windows_target(), expected)
        machine.assert_not_called()

    @unittest.skipUnless(os.name == "nt", "Windows architecture API regression")
    def test_native_architecture_uses_official_x64_and_arm64_values(self):
        for architecture, expected in ((9, "x86_64-pc-windows-msvc"),
                                       (12, "aarch64-pc-windows-msvc"), (0, None)):
            with self.subTest(architecture=architecture), \
                    patch.object(installer.ctypes, "WinDLL") as library:
                def fill_info(pointer):
                    info = installer.ctypes.cast(pointer, installer.ctypes.POINTER(installer.SystemInfo))
                    info.contents.wProcessorArchitecture = architecture
                library.return_value.GetNativeSystemInfo.side_effect = fill_info
                self.assertEqual(installer.native_windows_target(), expected)

    def test_update_keeps_running_payload_and_stable_entry_and_preserves_notices(self):
        first = self.install(self.package("first 包"))
        first_payload = Path(first["payload"])
        # A loaded executable can remain open throughout an installation update.
        with first_payload.open("rb") as running_payload:
            second = self.install(self.package("second 包", b"latest payload", packaged_at="2026-10-26T02:00:00+00:00"))
            self.assertEqual(running_payload.read(), b"first payload")
        self.assertEqual(first["launcher"], second["launcher"])
        self.assertNotEqual(first["payload"], second["payload"])
        self.assertEqual(Path(second["payload"]).read_bytes(), b"latest payload")
        self.assertEqual((first_payload.parent / "LICENSE").read_text(), "License preserved")
        self.assertEqual((Path(second["payload"]).parent / "LICENSE").read_text(), "License preserved")
        self.assertEqual((Path(second["launcher"]).parent / "current.path").read_text(encoding="utf-8").strip(), second["payload"])
        repeated = self.install(Path(second["package_directory"]))
        self.assertEqual(repeated["payload"], second["payload"])
        self.assertEqual(len(list((self.base / "SQLShop" / "versions").iterdir())), 2)

    def test_bad_package_does_not_publish_a_pointer(self):
        first = self.install(self.package("first"))
        pointer = self.base / "SQLShop" / "current.path"
        before = pointer.read_bytes()
        second = self.package("corrupt", b"second")
        (second / "sqlshop.exe").write_bytes(b"corrupted after packaging")
        with self.assertRaisesRegex(ValueError, "SHA-256"):
            self.install(second)
        self.assertEqual(pointer.read_bytes(), before)
        self.assertTrue(Path(first["payload"]).is_file())

    def test_shell_registration_always_targets_the_same_launcher_after_an_update(self):
        registrations = []
        shortcuts = []
        def shortcut(product, launcher, payload):
            shortcuts.append((product, launcher, payload))
            return self.root / f"{product}.lnk"
        with patch.object(installer, "native_windows_target", return_value="x86_64-pc-windows-msvc"), \
                patch.object(installer, "write_registry", side_effect=lambda entries: registrations.append(entries)), \
                patch.object(installer, "create_shortcut", side_effect=shortcut), \
                patch.object(installer, "notify_shell"):
            first = installer.install_package(self.package("first"), self.root, self.base)
            second = installer.install_package(self.package("second", b"same version new binary"), self.root, self.base)
        self.assertEqual(shortcuts[0][1], shortcuts[1][1])
        self.assertNotEqual(shortcuts[0][2], shortcuts[1][2])
        for entries in registrations:
            commands = [value for key, _, value, _ in entries if key.endswith("\\shell\\open\\command")]
            self.assertTrue(all(command == f'"{first["launcher"]}" --open "%1"' for command in commands))
        self.assertEqual(first["launcher"], second["launcher"])
        self.assertEqual(Path(first["payload"]).read_bytes(), b"first payload")

    def test_registration_failure_does_not_publish_a_new_pointer(self):
        first = self.install(self.package("first"))
        second = self.package("second", b"second")
        with patch.object(installer, "native_windows_target", return_value="x86_64-pc-windows-msvc"), \
                patch.object(installer, "write_registry", side_effect=OSError("test registration failure")), \
                patch.object(installer, "create_shortcut") as shortcut:
            with self.assertRaisesRegex(OSError, "test registration failure"):
                installer.install_package(second, self.root, self.base)
        shortcut.assert_not_called()
        pointer = self.base / "SQLShop" / "current.path"
        self.assertEqual(pointer.read_text(encoding="utf-8").strip(), first["payload"])

    def test_automatic_update_does_not_roll_back_to_an_older_completed_package(self):
        latest = self.install(self.package("latest", b"latest", packaged_at="2026-10-26T02:00:00+00:00"))
        result = self.install(self.package("old", b"old"), automatic=True)
        self.assertEqual(result["payload"], latest["payload"])

    def test_modified_immutable_payload_is_not_reused(self):
        package = self.package("first")
        state = self.install(package)
        Path(state["payload"]).write_bytes(b"modified installed binary")
        with self.assertRaisesRegex(OSError, "immutable payload"):
            self.install(package)

    def test_registrations_leave_user_choice_and_extension_defaults_untouched(self):
        expected = {"sqlshop": {".sql", ".sqlite", ".sqlite3", ".db", ".db3"},
                    "linkshop": {".json"}, "rpshop": {".rpshop", ".rp", ".rplib", ".json"}}
        for name, extensions in expected.items():
            with self.subTest(name=name):
                metadata = installer.load_package(self.package(name, name=name))
                launcher = self.base / metadata["product"] / f"{metadata['product']}.exe"
                entries = installer.registry_entries(metadata, launcher, launcher.parent / "versions" / "构建 一" / f"{name}.exe")
                self.assertFalse(any("userchoice" in key.lower() for key, _, _, _ in entries))
                self.assertFalse(any(key.lower() == f"software\\classes\\{ext}" and value_name == ""
                                     for ext in extensions for key, value_name, _, _ in entries))
                registered = [value_name for key, value_name, _, _ in entries if key == "Software\\RegisteredApplications"]
                self.assertEqual(registered, [metadata["product"]])
                actual = {value_name for key, value_name, _, _ in entries if key.endswith("\\FileAssociations")}
                self.assertEqual(actual, extensions)
                commands = [value for key, _, value, _ in entries if key.endswith("\\shell\\open\\command")]
                self.assertTrue(commands)
                self.assertTrue(all(command == f'"{launcher}" --open "%1"' for command in commands))

    def test_start_menu_helper_receives_paths_as_separate_unmodified_arguments(self):
        launcher = self.base / "SQLShop" / "SQLShop.exe"
        payload = launcher.parent / "versions" / "版本 1" / "sqlshop.exe"
        with patch.dict(os.environ, {"APPDATA": str(self.root / "用户 roaming")}), \
                patch.object(installer.subprocess, "run") as run:
            shortcut = installer.create_shortcut("SQLShop", launcher, payload)
        arguments = run.call_args.args[0]
        self.assertEqual(arguments[arguments.index("-TargetPath") + 1], str(launcher))
        self.assertEqual(arguments[arguments.index("-IconPath") + 1], str(payload))
        self.assertEqual(arguments[arguments.index("-ShortcutPath") + 1], str(shortcut))
        self.assertEqual(shortcut.name, "SQLShop.lnk")

    @unittest.skipUnless(os.name == "nt", "automatic installation is Windows only")
    def test_refresh_only_updates_this_repository_after_first_explicit_install(self):
        package = self.package("first")
        state = {"source_repository": str(self.root.resolve())}
        with patch.dict(os.environ, {"CI": "", "GITHUB_ACTIONS": "", "LOCALAPPDATA": str(self.root)}), \
                patch.object(installer, "native_windows_target", return_value="x86_64-pc-windows-msvc"), \
                patch.object(installer, "installation_state", return_value=state), \
                patch.object(installer, "install_package", return_value={"launcher": "stable"}) as install:
            installer.refresh_install_here(package, self.root)
            install.assert_called_once_with(package, self.root, automatic=True)
            install.reset_mock()
            installer.refresh_install_here(package, self.root / "another checkout")
            installer.refresh_install_here(package, self.root, False)
            with patch.dict(os.environ, {"CI": "true"}):
                installer.refresh_install_here(package, self.root)
            with patch.object(installer, "native_windows_target", return_value="aarch64-pc-windows-msvc"):
                installer.refresh_install_here(package, self.root)
            install.assert_not_called()
            installer.refresh_install_here(package, self.root / "another checkout", True)
            install.assert_called_once_with(package, self.root / "another checkout", automatic=False)

    @unittest.skipUnless(os.name == "nt", "desktop integration is Windows only")
    def test_refresh_reads_installation_state_only_inside_the_desktop_worker(self):
        expected = {"launcher": "real desktop launcher"}
        with patch.dict(os.environ, {"CI": "", "GITHUB_ACTIONS": ""}), \
                patch.object(installer, "installation_state", side_effect=AssertionError("virtualized parent read")), \
                patch.object(installer, "desktop_json", return_value=expected) as desktop:
            result = installer.refresh_install(self.root / "package", self.root)
        self.assertEqual(result, expected)
        command = desktop.call_args.args[0]
        self.assertIn("--desktop-worker", command)
        self.assertEqual(command[command.index("--refresh-mode") + 1], "automatic")


if __name__ == "__main__":
    unittest.main()
