#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Verify the generated ESP assets before they are copied to a server."""

import hashlib
import json
import sys
from pathlib import Path

# Pinned values from the upstream manifests. fwimage/fwsignature hashes are
# recorded by make_assets.py after the whole gsp_ga10x.bin hash is checked.
PINNED = {
    "booter_load_ad102.bin": (57672, "6ee05b20e1dc244c08c052f36ef28469b8d09f4955d1fe33e48a179e11a7fd49"),
    "gsp_boot_ad102.bin": (36864, "45d6335a8185d6ff13e8d1484cea73f68d3d13aa1c19dc080ed1c3482dd5bac0"),
    "gsp_boot_desc_ad102.bin": (84, "83f8f3f1f5f9496e0ea8fc7103c2beb64d0b6de2f5ffe725f13514de5a95fa42"),
    "v67-ad102-four-plm.bin": (64000, "44c36b2d6b3424e53e18f3e55db85c833655de071ce66ce42749588a4a7dc8cb"),
    "v67-ad102-bar1-64g.bin": (64000, "cf3566d0887d3d31e8cb3611bf0a1b3fafcd9584ea7a73306f75f8d0eafb1284"),
    "fwimage_ad102.bin": (84226048, None),
    "fwsignature_ad102.bin": (4096, None),
}

def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()

def main(root: Path) -> int:
    manifest_path = root / "assets.json"
    if not manifest_path.is_file():
        raise SystemExit(f"{root}/assets.json missing; run: make assets")
    manifest = json.loads(manifest_path.read_text())
    problems = []
    for name, (size, pin) in PINNED.items():
        p = root / name
        if not p.is_file():
            problems.append(f"missing {name}")
            continue
        if p.stat().st_size != size:
            problems.append(f"{name}: size {p.stat().st_size}, want {size}")
        got = sha256(p)
        want = pin or manifest.get(name, {}).get("sha256")
        if want is None:
            problems.append(f"{name}: no pinned hash")
        elif got != want:
            problems.append(f"{name}: sha256 {got}, want {want}")
    if problems:
        for p in problems:
            print("FAIL:", p)
        return 1
    print(f"assets: {len(PINNED)} files verified in {root}")
    return 0

if __name__ == "__main__":
    raise SystemExit(main(Path(sys.argv[1])))
