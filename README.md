# ada-unlock

A UEFI boot entry for headless Debian servers with multiple RTX 4090s or L40s.
It runs before Linux, unlocks every applicable GPU, optionally selects the
64 GiB BAR1 size, and then chainloads the normal Debian bootloader.

This is a focused refactor of the RTX 4090 transaction in
[gpu-unlock-ada](https://github.com/JustAResearcher/gpu-unlock-ada) and its UEFI
port in [nv-uefi-unlocker](https://github.com/physicsdolphin/nv-uefi-unlocker).
It is not a general installer and it does not try to protect you from a wrong
config.

## What it does

For every applicable NVIDIA display function:

1. Open the GPU's `EFI_PCI_IO_PROTOCOL`, force D0, enable MEMORY/BUS_MASTER.
2. Verify the card is cold (`WPR2` down) and not already unlocked.
3. Read the card's own VBIOS, extract FWSEC-FRTS, and provision WPR2.
4. Fire the Booter with the selected V67 payload.
5. Verify the payload marker and permissions, write `SS0=0x88888888` / `SS1=0x8`.
6. Restore the stock signature and perform one PCIe FLR.
7. Verify the selectors survived the FLR. L40 re-fuses part of them on FLR,
   so the app rewrites `SS0`/`SS1` after the FLR and requires
   `FEAT_READOUT=0`. For the bar1 payload, verify the `CYA/CFG/XVE` construct
   survived, then select the 64 GiB ReBAR size if configured.
8. Move to the next GPU. Finally, `LoadImage`/`StartImage` the configured
   Debian bootloader from the same ESP.

The app never changes the BAR address, the bridge windows or the kernel command
line. Linux with `pci=realloc` owns bus resource assignment.

## Supported

| Device | Payload | BAR1 |
|---|---|---|
| `10de:2684` RTX 4090 | tensor or tensor-bar1 | 64 GiB on the bar1 payload |
| `10de:2685` RTX 4090 D | tensor only | not applicable |
| `10de:26b5` L40 | tensor only | already 64 GiB stock |

Other NVIDIA display devices are enumerated and skipped. The 4090 path has the
longest hardware evidence upstream. The L40 result is in
[docs/HARDWARE-L40.md](docs/HARDWARE-L40.md): it unlocks the FP32-accumulate
tensor paths, but L40 has three extra OTP speed-select fuses that keep
FP16-accumulate, FP8->FP16 and INT8 at half rate. That is not a full L40S and
must not be attempted by writing `FUSE_*`/OTP registers. The app targets
headless servers: it does not use or restore a GPU console/GOP. Keep the GPUs
off the UEFI console; use the BMC/SOL or onboard video.

First unlock after a power-on is the supported case. A warm reboot can leave
`WPR2` enabled; the app then logs `claim: WPR2 is enabled` and skips the card.
Power-cycle the machine for a clean retry.

## Build

Prerequisites on the build host:

- Rust stable with the `x86_64-unknown-uefi` target (`rustup target add ...`)
- Python 3, `zstd`, network access for the pinned NVIDIA inputs
- About 1 GiB of disk for the input cache and generated assets

```sh
make efi        # UEFI app only
make assets     # download/verify NVIDIA 610.43.03, generate ESP assets
make dist       # both, plus a verified staging tree in dist/
make check      # PE32+ EFI structure + asset hashes
```

`make assets` produces:

```
booter_load_ad102.bin
gsp_boot_ad102.bin
gsp_boot_desc_ad102.bin
fwimage_ad102.bin
fwsignature_ad102.bin
v67-ad102-four-plm.bin
v67-ad102-bar1-64g.bin
```

NVIDIA firmware and VBIOS blobs are not in git. They are regenerated from the
pinned public 610.43.03 source archive and `nvidia-utils` package, exactly as in
the upstream projects.

## Install

Copy the **contents** of `dist/` to the root of the ESP. The result is:

```
\EFI\ada-unlock\ada-unlock.efi
\ada-unlock.ini
\booter_load_ad102.bin
\gsp_boot_ad102.bin
\gsp_boot_desc_ad102.bin
\fwimage_ad102.bin
\fwsignature_ad102.bin
\v67-ad102-four-plm.bin
\v67-ad102-bar1-64g.bin
```

Edit `\ada-unlock.ini`, then add a firmware boot entry:

```sh
sudo efibootmgr -c -d /dev/nvme0n1 -p 1 \
  -L "Ada Unlock" -l '\EFI\ada-unlock\ada-unlock.efi'
```

Set the ESP disk/partition for your machine. Put the entry first if the server
should unlock on every boot:

```sh
sudo efibootmgr -o XXXX,YYYY
```

`chainload` must point at the bootloader that normally starts Debian, for
example `\EFI\debian\grubx64.efi` or `\EFI\systemd\systemd-bootx64.efi`.

## Config

`\ada-unlock.ini` is flat `key = value`:

```ini
payload = tensor-bar1
bar1_rebar64 = true
chainload = \EFI\debian\grubx64.efi
# targets = 0000:01:00.0,0000:02:00.0
log = \ada-unlock.log
```

- `payload = tensor`: SS0/SS1 compute unlock only.
- `payload = tensor-bar1`: compute unlock plus the `CYA=0x30` / `CFG=0x8000000a`
  64 GiB BAR1 construct.
- `bar1_rebar64 = true`: after the FLR, write the standard ReBAR BAR1 size
  control to the 64 GiB entry and leave it. Requires `payload = tensor-bar1`
  and Linux `pci=realloc`.
- `bar1_rebar64 = false`: after the FLR, restore the BAR1 size control to the
  value captured before the payload. The payload's 64 GiB capability stays in
  the ReBAR CAP, but Linux is not asked to allocate a 64 GiB resource. This is
  the usable bar1 state on a 24 GB card and the safe default while testing.
- `targets`: omit or `all` for every applicable GPU; otherwise a comma-separated
  list of full PCI addresses.
- `log`: file on the ESP. Every line also goes to UEFI ConOut, which is what
  the BMC's SOL sees.

Unknown keys and invalid values are fatal: the app logs the line and returns to
the firmware instead of guessing.

## BAR1 and `pci=realloc`

With `bar1_rebar64 = true`, the app changes only the ReBAR **size control**.
The BAR1 address stays where firmware put it. That is intentional: the 64 GiB
aperture must be re-assigned by Linux, not by the UEFI app.

Add `pci=realloc` to the kernel command line used by the chainloaded bootloader:

```sh
# /etc/default/grub
GRUB_CMDLINE_LINUX_DEFAULT="quiet pci=realloc"
sudo update-grub
```

The kernel then sees BAR1's 64 GiB size and rebuilds the bridge windows. The
server topology must have enough MMIO space for the windows; the app does not
work around a too-small root port window. Eight-card 4090 topologies may need
the bridge-window layout work described in the upstream
[bar1 report](https://github.com/84634E1A607A/gpu-unlock-4090-48g-64g-bar1/blob/main/docs/bar1-64g-report.md).

If `bar1_rebar64 = false`, the payload still opens the 64 GiB hardware
capability, but the app restores the pre-payload size control after the FLR.
Linux keeps the original BAR1 assignment and the NVIDIA driver loads; the
64 GiB entry remains visible in the ReBAR CAP for later experiments.

## Verify

On the SOL console and in `\ada-unlock.log`, a successful card ends with:

```
TX: card complete
HANDOFF: SS0=0x88888888 SS1=0x8 FEAT_PLM=0xffffffff ...
HANDOFF-bar1: CYA=0x30 CFG=0x8000000a XVE_PLM=0xffffffff
```

After Linux boots:

```sh
nvidia-smi --query-gpu=index,pci.bus_id,memory.total --format=csv
sudo lspci -vv -s 01:00.0 | grep -A8 -i 'resizable bar'
cat /sys/bus/pci/devices/0000:01:00.0/resource1
```

With the 64 GiB selection and a successful `pci=realloc`, the BAR1 resource
size is `68719476736` and `nvidia-smi -q` reports `BAR1 Total: 65536 MiB`.
The compute unlock is verified by the `SS0/SS1` readback and an FP16/BF16
tensor-throughput comparison; a loaded driver alone is not proof.

## Hardware status

Verified on 2026-11-06 on a single 24 GB RTX 4090 and on eight 48 GB RTX 4090s:

- single 24 GB card: tensor unlock (SM rates all zero = full), bar1 capability
  raised to 64 GiB, and `bar1_rebar64=true` + `pci=realloc` produced a usable
  64 GiB BAR1 with the driver healthy;
- eight 48 GB cards on dual EPYC: all eight unlocked in one boot, all eight BAR1
  resources 64 GiB, `nvidia-smi` BAR1 Total 65536 MiB each, and
  `nvidia-smi topo -p2p r` reports `OK` for every pair. The modified driver uses
  the standard BAR1 P2P path because BAR1 (64 GiB) is larger than VRAM (48 GiB);
  no "method 3" lines appear.

See [docs/HARDWARE-24G.md](docs/HARDWARE-24G.md) and
[docs/HARDWARE-27-8X.md](docs/HARDWARE-27-8X.md). No workload was run; the
eight-card result is a driver/property check.

## Bring-up

Do not start with eight cards and the bar1 payload. Use the staged sequence in
[docs/BRINGUP.md](docs/BRINGUP.md): tensor-only on one card, then all cards,
then the bar1 construct without ReBAR, then the 64 GiB selection.

## Credits

This repository is a new, GPL-2.0-only refactor. The transaction and payload
lineage is:

- [JustAResearcher/gpu-unlock-ada](https://github.com/JustAResearcher/gpu-unlock-ada):
  AD102 transaction, firmware pins, FWSEC/Booter/V67 work, RTX 4090 evidence.
- [physicsdolphin/nv-uefi-unlocker](https://github.com/physicsdolphin/nv-uefi-unlocker):
  UEFI `PciIo`, DMA, portal and transaction code that this app's core is
  refactored from. GPL-2.0-only.
- [84634E1A607A/gpu-unlock-4090-48g-64g-bar1](https://github.com/84634E1A607A/gpu-unlock-4090-48g-64g-bar1):
  64 GiB BAR1 payload construct and eight-card field report.
- [TOTORONG/AD103_win_unlocker](https://huggingface.co/TOTORONG/AD103_win_unlocker):
  original AD102 four-write payload.
- [Zagitta/gpu-unlock](https://github.com/Zagitta/gpu-unlock) and
  [NVIDIA/open-gpu-kernel-modules](https://github.com/NVIDIA/open-gpu-kernel-modules):
  the original unlock technique and the pinned firmware sources.

The full provenance is in [CREDITS.md](CREDITS.md). License: GPL-2.0-only.
NVIDIA firmware remains NVIDIA property and is regenerated locally, not
distributed here.
