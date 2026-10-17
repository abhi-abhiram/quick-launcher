#!/usr/bin/env python3
"""Run every check a PR must pass: the Rust workspace's and v0's.

Run it before pushing. The CI workflow runs the same commands on every
push and PR.
"""

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

CHECKS = [
    ["cargo", "fmt", "--all", "--check"],
    ["cargo", "clippy", "--all-targets", "--", "-D", "warnings"],
    ["cargo", "test"],
    # v0 keeps passing until the switch to v1 deletes it.
    [sys.executable, "-m", "unittest", "discover", "-s", "tests"],
    ["node", "--test", "tests/test_model.mjs"],
]


def main():
    failed = []
    for argv in CHECKS:
        print(f"==> {' '.join(argv)}", flush=True)
        try:
            code = subprocess.run(argv, cwd=ROOT).returncode
        except FileNotFoundError:
            print(f"{argv[0]} is not installed", file=sys.stderr)
            code = 127
        if code != 0:
            failed.append(argv)
    if failed:
        print("\nFailed:", file=sys.stderr)
        for argv in failed:
            print(f"  {' '.join(argv)}", file=sys.stderr)
        return 1
    print("\nAll checks passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
