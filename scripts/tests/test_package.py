"""Packaging contract tests; no compiler, network, or real application data needed."""

import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

SPEC = importlib.util.spec_from_file_location("shop_package", Path(__file__).resolve().parents[1] / "package.py")
packaging = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(packaging)


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="shop package tests ")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "docs").mkdir()
        (self.root / "docs" / "portable-readme.txt").write_text("启动说明\n", encoding="utf-8")
        self.package = {"id": "shop", "name": "testshop", "version": "1.2.3",
                        "manifest_path": str(self.root / "Cargo.toml"),
                        "metadata": {"shop": {"product-name": "TestShop"}}}
        self.target_directory = self.root / "custom target"
        vendor = self.root / "vendor" / "patched-driver"
        vendor.mkdir(parents=True)
        (vendor / "LICENSE").write_text("patched driver license", encoding="utf-8")
        self.metadata = {
            "packages": [self.package, {"id": "driver", "name": "driver", "version": "2.0",
                                         "source": None, "license": "MIT", "manifest_path": str(vendor / "Cargo.toml")}],
            "resolve": {"nodes": [{"id": "shop"}, {"id": "driver"}]},
            "target_directory": str(self.target_directory),
        }

    def executable(self, target):
        extension = ".exe" if "windows" in target else ""
        path = self.target_directory / target / "release" / f"testshop{extension}"
        path.parent.mkdir(parents=True)
        path.write_bytes(b"test executable")
        path.chmod(0o755)
        return path

    def build_package(self, target, **kwargs):
        def launcher(path, _target):
            Path(path).write_bytes(b"test launcher")
        with patch.object(packaging, "cargo_json", return_value=self.metadata), \
             patch.object(packaging, "build_windows_launcher", side_effect=launcher), \
             patch.object(packaging, "refresh_install") as refresh:
            refresh.return_value = None
            return packaging.package_project(self.root, target, **kwargs)

    def test_windows_archive_checksum_custom_target_and_vendored_license(self):
        target = "x86_64-pc-windows-msvc"
        self.executable(target)
        archive = self.build_package(target, skip_build=True)
        self.assertEqual(archive.name, "TestShop-1.2.3-windows-x64.zip")
        checksum = archive.with_name(archive.name + ".sha256").read_text()
        self.assertEqual(checksum, f"{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n")
        with zipfile.ZipFile(archive) as contents:
            prefix = "TestShop-1.2.3-windows-x64/"
            self.assertEqual(contents.read(prefix + "testshop.exe"), b"test executable")
            self.assertIn(prefix + "licenses/driver-2.0/LICENSE", contents.namelist())
            self.assertIn("driver", contents.read(prefix + "THIRD-PARTY-NOTICES.tsv").decode("utf-8-sig"))
            self.assertEqual(json.loads(contents.read(prefix + "package.json"))["target"], target)
            self.assertIn("启动说明", contents.read(prefix + "使用说明.txt").decode("utf-8-sig"))
            self.assertEqual(contents.read(prefix + "shop-launcher.exe"), b"test launcher")
            self.assertIn(prefix + "install.ps1", contents.namelist())
            self.assertIn(prefix + "windows_desktop.py", contents.namelist())
            self.assertIn(prefix + "launch-desktop.ps1", contents.namelist())
        with self.assertRaises(FileExistsError):
            self.build_package(target, skip_build=True)

    def test_unix_archive_retains_executable_permission(self):
        for target, platform in [("aarch64-apple-darwin", "macos-arm64"), ("x86_64-unknown-linux-gnu", "linux-x64")]:
            with self.subTest(target=target):
                self.executable(target)
                archive = self.build_package(target, skip_build=True)
                with tarfile.open(archive) as contents:
                    self.assertTrue(contents.getmember(f"TestShop-1.2.3-{platform}/testshop").mode & 0o111)

    def test_tag_mismatch_rejected_before_build(self):
        with patch.object(packaging.subprocess, "run") as run:
            with self.assertRaisesRegex(ValueError, "does not match"):
                self.build_package("x86_64-pc-windows-msvc", expected_tag="v9.0.0")
            run.assert_not_called()

    def test_failed_build_leaves_no_package(self):
        with patch.object(packaging.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "cargo")):
            with self.assertRaises(subprocess.CalledProcessError):
                self.build_package("x86_64-pc-windows-msvc")
        self.assertFalse((self.root / "dist").exists())

    def test_windows_build_uses_static_runtime_with_encoded_flags(self):
        target = "x86_64-pc-windows-msvc"
        self.executable(target)
        with patch.dict(packaging.os.environ, {"CARGO_ENCODED_RUSTFLAGS": "-C\x1fdebuginfo=0"}):
            with patch.object(packaging.subprocess, "run") as run:
                self.build_package(target)
                self.assertIn("\x1f-C\x1ftarget-feature=+crt-static", run.call_args.kwargs["env"]["CARGO_ENCODED_RUSTFLAGS"])

    def test_missing_binary_and_unsafe_product_do_not_create_output(self):
        with self.assertRaises(FileNotFoundError):
            self.build_package("x86_64-pc-windows-msvc", skip_build=True)
        self.package["metadata"]["shop"]["product-name"] = "../outside"
        with self.assertRaises(ValueError):
            self.build_package("x86_64-pc-windows-msvc", skip_build=True)
        self.assertFalse((self.root / "dist").exists())

    def test_empty_encoded_flags_do_not_introduce_an_empty_rustc_argument(self):
        target = "x86_64-pc-windows-msvc"
        self.executable(target)
        with patch.dict(packaging.os.environ, {"CARGO_ENCODED_RUSTFLAGS": ""}):
            with patch.object(packaging.subprocess, "run") as run:
                self.build_package(target)
                self.assertEqual(run.call_args.kwargs["env"]["CARGO_ENCODED_RUSTFLAGS"],
                                 "-C\x1ftarget-feature=+crt-static")

    def test_failed_build_never_refreshes_an_installed_pointer(self):
        pointer = self.root / "installed" / "current.path"
        pointer.parent.mkdir()
        pointer.write_text("previous immutable payload.exe\n", encoding="utf-8")
        before = pointer.read_bytes()
        with patch.object(packaging, "cargo_json", return_value=self.metadata), \
             patch.object(packaging, "refresh_install", side_effect=lambda *args: pointer.write_text("incorrect update")) as refresh, \
             patch.object(packaging.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "cargo")):
            with self.assertRaises(subprocess.CalledProcessError):
                packaging.package_project(self.root, "x86_64-pc-windows-msvc", install=True)
            refresh.assert_not_called()
        self.assertEqual(pointer.read_bytes(), before)

    def test_skip_install_and_automatic_modes_reach_the_post_package_hook(self):
        for index, mode in enumerate((None, False, True)):
            target = "x86_64-pc-windows-msvc"
            if index == 0:
                self.executable(target)
            def launcher(path, _target):
                Path(path).write_bytes(b"test launcher")
            with patch.object(packaging, "cargo_json", return_value=self.metadata), \
                 patch.object(packaging, "build_windows_launcher", side_effect=launcher), \
                 patch.object(packaging, "refresh_install", return_value=None) as refresh:
                packaging.package_project(self.root, target, self.root / f"output-{index}", True, install=mode)
                self.assertEqual(refresh.call_args.args[2], mode)


if __name__ == "__main__":
    unittest.main()
