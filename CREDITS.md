# Credits and provenance

`ada-unlock` is a new GPL-2.0-only work. It refactors the RTX 4090 transaction
and UEFI plumbing from the projects below. The upstream copyright and license
notices apply to the derived code.

## Transaction core

`loader/src/tx.rs`, `loader/src/tx_fwsec.rs`, `loader/src/tx_booter.rs`,
`loader/src/tx_portal.rs` and `loader/src/pci.rs` are refactored from
**[physicsdolphin/nv-uefi-unlocker](https://github.com/physicsdolphin/nv-uefi-unlocker)**
(GPL-2.0-only), which in turn ports the Linux transaction from
**[JustAResearcher/gpu-unlock-ada](https://github.com/JustAResearcher/gpu-unlock-ada)**
(GPL-2.0-only). The upstream UEFI transaction was field-tested on Ada cards; this
repository removes its Windows display handoff, marker-file experiments and
diagnostic probes and keeps the AD102 path.

- `loader/src/tx.rs`: claim/cold-state checks, Falcon reset/DMA unblock, FLR,
  transaction sequence.
- `loader/src/tx_fwsec.rs`: VBIOS PROM parse, WPR layout, FWSEC ignition.
- `loader/src/tx_booter.rs`: Booter bundle, WprMeta, radix, SEC2 fire, selectors,
  verification.
- `loader/src/tx_portal.rs`: Falcon v4 register portal.
- `loader/src/pci.rs`: `EFI_PCI_IO_PROTOCOL` wrapper, DMA, bridge BME.

## BAR1 construct

The tensor + 64 GiB BAR1 payload and the register construct come from
**[84634E1A607A/gpu-unlock-4090-48g-64g-bar1](https://github.com/84634E1A607A/gpu-unlock-4090-48g-64g-bar1)**
(GPL-2.0-only), specifically `docs/bar1-64g-report.md` and
`firmware/generate_v67.py`:

- CYA `0x88724 = 0x30`
- CFG `0x88dcc = 0x8000000a`
- XVE PLM `0x88ff4 = 0xffffffff`
- FEAT PLM `0x823804 = 0xffffffff`
- mailbox marker `0x67b00004`

The payload generator is a modified copy of the upstream
`gpu-unlock-ada`/`gpu-unlock-bar1` script. The AD102 word table is adapted from
**[TOTORONG/AD103_win_unlocker](https://huggingface.co/TOTORONG/AD103_win_unlocker)**
via `gpu-unlock-ada`'s `firmware/generate_v67.py` provenance note
(revision `51df9c17ec89eb8486df52cc9079dde4bd1f9974`).

The ReBAR size-control write in `loader/src/bar1.rs` is new code implementing
the standard PCIe Resizable BAR capability.

## Firmware generation

`tools/firmware/extract_boot_assets.py`, `fetch_artifacts.py`,
`generate_v67.py`, `artifacts-610.43.03.json` and `ada-610.43.03.json` are
copied from the pinned 610.43.03 workflows of
**[gpu-unlock-bar1](https://github.com/84634E1A607A/gpu-unlock-4090-48g-64g-bar1)**
and **[gpu-unlock-ada](https://github.com/JustAResearcher/gpu-unlock-ada)**
(GPL-2.0-only).

They download and verify:

- `NVIDIA-kernel-module-source-610.43.03.tar.xz`
- `nvidia-utils-610.43.03-1-x86_64_v3.pkg.tar.zst`

No NVIDIA firmware, VBIOS or payload binary is distributed in this repository.
`make assets` regenerates them locally from those pinned inputs.

## Technique lineage

- **[Zagitta/gpu-unlock](https://github.com/Zagitta/gpu-unlock)**: original
  pre-driver unlock technique.
- **[NVIDIA/open-gpu-kernel-modules](https://github.com/NVIDIA/open-gpu-kernel-modules)**:
  the firmware sources and FWSEC/Booter formats the generators consume.
- The RTX 4090 evidence and transaction reports in
  **[gpu-unlock-ada](https://github.com/JustAResearcher/gpu-unlock-ada)** and
  **[gpu-unlock-bar1](https://github.com/84634E1A607A/gpu-unlock-4090-48g-64g-bar1)**.

## Licenses

- This repository: GPL-2.0-only, see [LICENSE](LICENSE).
- GPL-2.0 text: [licenses/GPL-2.0.txt](licenses/GPL-2.0.txt).
- NVIDIA MIT portions: [licenses/NVIDIA-MIT.txt](licenses/NVIDIA-MIT.txt).
- NVIDIA firmware and VBIOS remain NVIDIA property. They are not covered by this
  repository's GPL license and are not redistributed here.
