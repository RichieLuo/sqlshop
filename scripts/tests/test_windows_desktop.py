"""Explorer broker regressions; real roundtrips are opt-in and never install anything."""

import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("shop_windows_desktop_tests", SCRIPTS / "windows_desktop.py")
desktop = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(desktop)


class WindowsDesktopTests(unittest.TestCase):
    @unittest.skipUnless(os.name == "nt", "Windows desktop path validation")
    def test_worker_rejects_a_package_private_start_menu_handle(self):
        private = r"\\?\C:\Users\adm\AppData\Local\Packages\Example\LocalCache\Roaming\Microsoft\Windows\Start Menu\Programs"
        with patch.object(desktop, "final_path", return_value=private):
            with self.assertRaisesRegex(OSError, "redirected Start menu"):
                desktop.verify_desktop()


@unittest.skipUnless(os.name == "nt" and os.environ.get("SHOP_TEST_DESKTOP_BROKER") == "1",
                     "set SHOP_TEST_DESKTOP_BROKER=1 for real read-only Explorer roundtrips")
class RealExplorerBrokerTests(unittest.TestCase):
    def test_paths_working_directory_arguments_stderr_and_exit_status(self):
        target = SCRIPTS.parent / "target"
        target.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="desktop broker 中文 ", dir=target) as temporary:
            directory = Path(temporary)
            helper = directory / "windows_desktop.py"
            shutil.copyfile(SCRIPTS / "windows_desktop.py", helper)
            (directory / "windows").mkdir()
            shutil.copyfile(SCRIPTS / "windows" / "launch-desktop.ps1", directory / "windows" / "launch-desktop.ps1")
            spec = importlib.util.spec_from_file_location("desktop_broker_roundtrip", helper)
            broker = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(broker)
            probe = directory / "只读 probe.py"
            probe.write_text('''import json, os, sys
from pathlib import Path
import windows_desktop
print(json.dumps({"arguments": sys.argv[1:], "directory": os.getcwd(),
                  "programs_final_path": windows_desktop.verify_desktop()}, ensure_ascii=True))
''', encoding="utf-8")
            arguments = ["--open", "中文 filename with spaces.sql", "", 'literal "quote"', "ending\\"]
            previous = Path.cwd()
            try:
                os.chdir(directory)
                result = broker.run_json([sys.executable, str(probe), *arguments], timeout=20)
                self.assertEqual(result["arguments"], arguments)
                self.assertEqual(Path(result["directory"]), directory)
                self.assertNotIn("\\localcache\\", result["programs_final_path"].lower())
                expected = str(Path(os.environ["APPDATA"]) / "Microsoft/Windows/Start Menu/Programs")
                self.assertEqual(os.path.normcase(result["programs_final_path"].removeprefix("\\\\?\\")), os.path.normcase(expected))
                failure = directory / "失败 probe.py"
                failure.write_text("import sys\nprint('故意 stderr failure', file=sys.stderr)\nsys.exit(42)\n", encoding="utf-8")
                with patch.object(broker, "write_stderr") as stderr:
                    with self.assertRaises(subprocess.CalledProcessError) as error:
                        broker.run_json([sys.executable, str(failure)], timeout=20)
                self.assertEqual(error.exception.returncode, 42)
                self.assertEqual(stderr.call_args.args[0].decode("utf-8").replace("\r\n", "\n"), "故意 stderr failure\n")
            finally:
                os.chdir(previous)


if __name__ == "__main__":
    unittest.main()
