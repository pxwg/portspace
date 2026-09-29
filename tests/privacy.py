#!/usr/bin/env python3
"""Reject personal home directories and private hostnames in source/release assets."""
import argparse
import re
import subprocess
import tarfile
from pathlib import Path

PATTERNS = [
    re.compile(rb"/(?:Users|home)/[A-Za-z0-9_.@-]+/"),
    re.compile(rb"[A-Za-z]:\\Users\\[A-Za-z0-9_.@-]+\\"),
    re.compile(rb"\b[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)*\.local\b"),
]


def check(label, data):
    if any(pattern.search(data) for pattern in PATTERNS):
        # Do not echo the matching private content into public CI logs.
        raise SystemExit(f"Privacy check failed: {label}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--history", action="store_true", help="Scan every reachable Git blob")
    parser.add_argument("paths", nargs="*", help="Additional binaries or .tar.gz archives")
    args = parser.parse_args()
    files = subprocess.check_output(["git", "ls-files", "-z"]).split(b"\0")
    for name in filter(None, files):
        path = Path(name.decode())
        check(str(path), path.read_bytes())
    if args.history:
        objects = subprocess.check_output(["git", "rev-list", "--objects", "--all"])
        for line in objects.splitlines():
            oid = line.split(b" ", 1)[0].decode()
            kind = subprocess.check_output(["git", "cat-file", "-t", oid]).strip()
            if kind == b"blob":
                check(f"historical blob {oid}", subprocess.check_output(["git", "cat-file", "blob", oid]))
    for name in args.paths:
        path = Path(name)
        if name.endswith(".tar.gz"):
            with tarfile.open(path, "r:gz") as archive:
                for member in archive.getmembers():
                    check(f"archive member name in {path.name}", member.name.encode())
                    if member.isfile():
                        check(f"{path.name}: {member.name}", archive.extractfile(member).read())
        else:
            check(path.name, path.read_bytes())
    print("PASS: privacy scan")


if __name__ == "__main__":
    main()
