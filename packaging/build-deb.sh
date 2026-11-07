#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
# Build the internal-only ada-unlock-uefi binary package from a prepared dist/.
#
# This must run on Debian (or a Debian container) with dpkg-buildpackage and
# debhelper. The EFI application and NVIDIA firmware assets are built by
# `make dist`; this script never downloads or regenerates them.

set -eu

cd "$(dirname "$0")/.."

if ! command -v dpkg-buildpackage >/dev/null 2>&1; then
    echo "dpkg-buildpackage not found; run this on Debian" >&2
    exit 1
fi

for f in \
    dist/EFI/ada-unlock/ada-unlock.efi \
    dist/booter_load_ad102.bin \
    dist/gsp_boot_ad102.bin \
    dist/gsp_boot_desc_ad102.bin \
    dist/fwimage_ad102.bin \
    dist/fwsignature_ad102.bin \
    dist/v67-ad102-four-plm.bin \
    dist/v67-ad102-bar1-64g.bin \
    dist/assets.json
do
    if [ ! -f "$f" ]; then
        echo "missing $f; run 'make dist' on a build host first" >&2
        exit 1
    fi
done

# -b: binary package only (dist/ is intentionally not a source tarball)
# -uc -us: internal package, no signing keys
# -Zxz: compress the 84 MiB firmware asset
exec dpkg-buildpackage -b -uc -us -Zxz "$@"
