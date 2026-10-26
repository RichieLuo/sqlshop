"""Optional real launcher smoke test: set SHOP_TEST_LAUNCHER_EXE to a compiled launcher."""

import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import time
import unittest


@unittest.skipUnless(os.name == "nt" and os.environ.get("SHOP_TEST_LAUNCHER_EXE"),
                     "set SHOP_TEST_LAUNCHER_EXE on Windows for the real launcher smoke test")
class WindowsLauncherTests(unittest.TestCase):
    def test_new_launch_uses_current_payload_and_preserves_windows_arguments(self):
        with tempfile.TemporaryDirectory(prefix="Shop launcher 用户 ") as temporary:
            root = Path(temporary)
            source = root / "probe.rs"
            source.write_text('''#![windows_subsystem = "windows"]
use std::{env, fs, os::windows::ffi::OsStrExt};
fn main() {
    let values = std::iter::once(env::current_dir().unwrap().into_os_string())
        .chain(env::args_os().skip(1));
    let mut record = Vec::new();
    for value in values {
        let words: Vec<u16> = value.encode_wide().collect();
        record.extend_from_slice(&(words.len() as u32).to_le_bytes());
        for word in words { record.extend_from_slice(&word.to_le_bytes()); }
    }
    fs::write(env::var_os("SHOP_LAUNCHER_RECORD").unwrap(), record).unwrap();
}
''', encoding="utf-8")
            probe = root / "probe.exe"
            subprocess.run(["rustc", "--edition", "2024", "--crate-name", "shop_launcher_probe",
                            "-C", "target-feature=+crt-static", str(source), "-o", str(probe)], check=True)
            home = root / "Programs 用户" / "SQLShop"
            home.mkdir(parents=True)
            launcher = home / "SQLShop.exe"
            shutil.copyfile(os.environ["SHOP_TEST_LAUNCHER_EXE"], launcher)
            arguments = ["--open", str(root / '文件 with spaces "quotes" & $ ;.sql'),
                         "trailing slash\\", "", "emoji 😀", "line\nbreak"]
            for version in ("1 first", "2 最新"):
                payload = home / "versions" / version / "sqlshop.exe"
                payload.parent.mkdir(parents=True)
                shutil.copyfile(probe, payload)
                (home / "current.path").write_text(str(payload) + "\n", encoding="utf-8")
                record = root / f"{version}.bin"
                environment = os.environ.copy()
                environment["SHOP_LAUNCHER_RECORD"] = str(record)
                subprocess.run([str(launcher), *arguments], env=environment, check=True, timeout=10)
                deadline = time.monotonic() + 10
                while not record.is_file() and time.monotonic() < deadline:
                    time.sleep(0.05)
                data = record.read_bytes()
                values = []
                while data:
                    length = struct.unpack("<I", data[:4])[0] * 2
                    values.append(data[4:4 + length].decode("utf-16-le", errors="surrogatepass"))
                    data = data[4 + length:]
                self.assertEqual(Path(values[0]), payload.parent)
                self.assertEqual(values[1:], arguments)


if __name__ == "__main__":
    unittest.main()
