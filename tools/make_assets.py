#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Fetch pinned NVIDIA 610.43.03 inputs and generate the ESP asset set.

Outputs, all verified against the manifests in tools/firmware:
  booter_load_ad102.bin        BTL1 BooterLoad bundle
  gsp_boot_ad102.bin           GSP bootloader image
  gsp_boot_desc_ad102.bin      GSP boot descriptor
  fwimage_ad102.bin            .fwimage section of gsp_ga10x.bin
  fwsignature_ad102.bin        .fwsignature_ad10x section
  v67-ad102-four-plm.bin       tensor-only payload
  v67-ad102-bar1-64g.bin       tensor + 64 GiB BAR1 payload

Nothing is downloaded at build time by the UEFI crate; run this once and
copy the output next to the EFI binary on the ESP.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
import tarfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
FW = HERE / "firmware"
MANIFEST = json.loads((FW / "artifacts-610.43.03.json").read_text())
ADA = json.loads((FW / "ada-610.43.03.json").read_text())
SOURCE_DIR = "NVIDIA-kernel-module-source-610.43.03"

OUTPUTS = [
    "booter_load_ad102.bin",
    "gsp_boot_ad102.bin",
    "gsp_boot_desc_ad102.bin",
    "fwimage_ad102.bin",
    "fwsignature_ad102.bin",
    "v67-ad102-four-plm.bin",
    "v67-ad102-bar1-64g.bin",
]


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def run(script: str, *args: str) -> None:
    cmd = [sys.executable, str(FW / script), *args]
    print("+", " ".join(cmd))
    subprocess.run(cmd, check=True)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--cache", type=Path, default=Path.home() / ".cache/ada-unlock-uefi",
                    help="download/cache directory")
    ap.add_argument("--out", type=Path, default=Path("assets"),
                    help="asset output directory")
    args = ap.parse_args()
    cache = args.cache.resolve()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)

    # 1. Pinned downloads (source archive + nvidia-utils package).
    run("fetch_artifacts.py", str(cache))
    source_archive = cache / "NVIDIA-kernel-module-source-610.43.03.tar.xz"
    package = cache / "nvidia-utils-610.43.03-1-x86_64_v3.pkg.tar.zst"

    # 2. Extract the kernel-module source once.
    source_root = cache / SOURCE_DIR
    if not source_root.is_dir():
        print(f"extracting {source_archive.name}")
        with tarfile.open(source_archive, "r:xz") as tar:
            try:
                tar.extractall(cache, filter="data")
            except TypeError:  # Python < 3.12
                tar.extractall(cache)
    if not (source_root / "src/nvidia/generated").is_dir():
        raise SystemExit("source archive layout is not the pinned NVIDIA tree")

    # 3. Booter + GSP boot assets from the generated BINDATA sources.
    run("extract_boot_assets.py", str(source_root), str(out), "--chip", "ad102")

    # 4. gsp_ga10x.bin and the two payloads from the pinned nvidia-utils package.
    run("generate_v67.py", str(package), str(out))
    (out / "v67-ad102-610.43.03-four-plm.bin").replace(out / "v67-ad102-four-plm.bin")
    (out / "v67-ad102-610.43.03-bar1-64g.bin").replace(out / "v67-ad102-bar1-64g.bin")

    # 5. The Rust loader wants the GSP ELF sections separately. The offsets
    # and sizes are pinned in ada-610.43.03.json and the whole-file SHA-256
    # is checked first, so these are not a heuristic ELF parse.
    gsp = out / "gsp_ga10x.bin"
    if sha256(gsp) != ADA["shared_gsp"]["sha256"]:
        raise SystemExit("gsp_ga10x.bin SHA-256 mismatch")
    raw = gsp.read_bytes()
    fw = ADA["shared_gsp"]
    image = raw[fw["fwimage_offset"]:fw["fwimage_offset"] + fw["fwimage_size"]]
    sig = raw[fw["ada_signature_offset"]:fw["ada_signature_offset"] + fw["ada_signature_size"]]
    if len(image) != fw["fwimage_size"] or len(sig) != fw["ada_signature_size"]:
        raise SystemExit("GSP section slice outside the file")
    (out / "fwimage_ad102.bin").write_bytes(image)
    (out / "fwsignature_ad102.bin").write_bytes(sig)

    # 6. Record exactly what was produced.
    manifest = {}
    for name in OUTPUTS:
        p = out / name
        if not p.is_file():
            raise SystemExit(f"missing generated asset: {name}")
        manifest[name] = {"size": p.stat().st_size, "sha256": sha256(p)}
        print(f"{name:32} {p.stat().st_size:>10}  {manifest[name]['sha256']}")
    (out / "assets.json").write_text(json.dumps(manifest, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except subprocess.CalledProcessError as e:
        raise SystemExit(f"asset command failed: {e}")
