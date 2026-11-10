"""Writes the licence texts of every crate built into Unkludge's two programs for one target, to ship next to them.

Usage: python packaging/third_party_notices.py <target triple> <output file>

The crates are what `cargo tree` says the programs are built with for that target: no build or dev dependencies,
which don't end up in the programs. Each crate's licence files come from its own source folder. A crate without any
gets the standard text of a licence it offers, from packaging/licenses, with its authors as the copyright holders.
GPL-3.0 is Unkludge's own licence, in LICENSE. A crate with neither makes the script fail, to be looked at.
"""

import json
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

PROGRAMS = ("unkludge-app", "unkludge-cli")
LICENCE_FILE_STARTS = ("license", "licence", "copying", "notice", "unlicense", "copyright")
STANDARD = Path(__file__).parent / "licenses"


def cargo(*args):
    return subprocess.run(["cargo", *args], check=True, capture_output=True).stdout.decode("utf-8")


def built_crates(target):
    """Name and version of every crate the programs are built with, theirs included."""
    tree = cargo(
        "tree", "--locked", "--edges", "normal", "--target", target, "--prefix", "none", "--format", "{p}",
        *[arg for program in PROGRAMS for arg in ("-p", program)],
    )
    crates = set()
    for line in tree.splitlines():
        match = re.match(r"(\S+) v(\S+)", line)
        if match:
            crates.add((match.group(1), match.group(2)))
    return crates


def licence_files(package):
    folder = Path(package["manifest_path"]).parent
    paths = set()
    if package.get("license_file"):
        paths.add(folder / package["license_file"])
    for entry in folder.iterdir():
        if entry.is_file() and entry.name.lower().startswith(LICENCE_FILE_STARTS):
            paths.add(entry)
    texts = []
    for path in sorted(paths):
        text = path.read_text(encoding="utf-8", errors="replace").replace("\r\n", "\n").strip()
        if text:
            texts.append(text)
    return texts


def standard_text(package):
    """A licence the crate offers that there's a standard text for, or None."""
    offered = re.split(r"\s+OR\s+|/", package.get("license") or "")
    authors = ", ".join(package.get("authors") or []) or f"the {package['name']} authors"
    for licence in (name.strip("() ") for name in offered):
        if licence == "GPL-3.0-only" or licence == "GPL-3.0-or-later":
            return f"Used under {licence}, Unkludge's own licence. Its text is in LICENSE, next to this file."
        if licence == "MIT":
            body = (STANDARD / "MIT.txt").read_text(encoding="utf-8").strip()
            return f"Used under MIT.\n\nCopyright (c) {authors}\n\n{body}"
        if licence == "Apache-2.0":
            return "Used under Apache-2.0. Its text is under \"Apache License 2.0\" at the end of this file."
    return None


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    target, output = sys.argv[1], Path(sys.argv[2])
    meta = json.loads(cargo("metadata", "--format-version", "1", "--locked", "--filter-platform", target))
    packages = {(package["name"], package["version"]): package for package in meta["packages"]}
    ours = {package["name"] for package in meta["packages"] if package["id"] in meta["workspace_members"]}

    by_texts = defaultdict(list)
    failed = []
    for key in sorted(built_crates(target)):
        if key[0] in ours:
            continue
        package = packages[key]
        label = f"{package['name']} {package['version']} ({package.get('license') or 'no licence given'})"
        texts = licence_files(package)
        if not texts:
            standard = standard_text(package)
            if standard is None:
                failed.append(label)
                continue
            texts = [f"No licence file comes with this crate. Source: {package.get('repository') or 'crates.io'}.\n\n{standard}"]
        by_texts[tuple(texts)].append(label)

    lines = [
        "Third-party software in Unkludge",
        "",
        f"Unkludge's programs for {target} are built with the crates below. Each group lists crates that ship",
        "the same licence text, followed by that text.",
    ]
    for texts, labels in sorted(by_texts.items(), key=lambda item: item[1][0]):
        lines += ["", "=" * 78, ""] + labels + ["", "-" * 78]
        for text in texts:
            lines += ["", text]
    lines += ["", "=" * 78, "", "Apache License 2.0", "", (STANDARD / "Apache-2.0.txt").read_text(encoding="utf-8").strip()]
    output.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")

    count = sum(len(labels) for labels in by_texts.values())
    print(f"{output}: {count} crates, {len(by_texts)} licence groups")
    if failed:
        print("No licence file and no standard text for:\n  " + "\n  ".join(failed))
        sys.exit(1)


if __name__ == "__main__":
    main()
