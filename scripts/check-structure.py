#!/usr/bin/env python3
"""Keep first-party source and test modules small enough to review and maintain."""

import argparse
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXTENSIONS = {".rs", ".py", ".ps1", ".js", ".mjs", ".css"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--max-lines", type=int, default=1200)
    parser.add_argument("--max-bytes", type=int, default=128 * 1024)
    parser.add_argument("--top", type=int, default=10)
    arguments = parser.parse_args()
    entries = []
    for directory in ("src", "tests", "scripts", "examples"):
        for path in (ROOT / directory).rglob("*"):
            if path.suffix in EXTENSIONS and path.is_file():
                content = path.read_bytes()
                entries.append((len(content.splitlines()), len(content), path.relative_to(ROOT)))
    entries.sort(key=lambda item: (-item[0], str(item[2])))
    failures = [entry for entry in entries if entry[0] > arguments.max_lines or entry[1] > arguments.max_bytes]
    for lines, size, path in entries[:arguments.top]:
        print(f"{lines:5} lines  {size:7} bytes  {path.as_posix()}")
    if failures:
        print("\nSplit these modules along responsibilities before adding more code:")
        for lines, size, path in failures:
            print(f"  {path.as_posix()}: {lines} lines, {size} bytes")
        return 1
    print(f"Checked {len(entries)} files; no module exceeds {arguments.max_lines} lines or {arguments.max_bytes} bytes.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
