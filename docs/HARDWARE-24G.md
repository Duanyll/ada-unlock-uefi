# Hardware result: single 24 GB RTX 4090

Tested 2026-11-06 on the lab machine `villa-Super-Server-24`:

- Supermicro H12SSL, single `10de:2684` RTX 4090, 24564 MiB
- Debian 13 (trixie), kernel `6.12.94+deb13-amd64`
- NVIDIA open kernel module `590.48.01`
- Secure Boot disabled, BMC SOL on `ttyS1`/115200
- App build `d91e4e0a36849cea9610b5cd5e910048e85e7d038c0cf684726943d1ae8c7509`

The old `gpu-unlock` DKMS module, firmware package, initramfs copy and
`gpu_unlock.targets=` kernel option were removed before this test.

## Tensor payload

`payload=tensor`, `bar1_rebar64=false`.

App log:

```
claim: cold card, FEAT_PLM=0xffffff8f, WPR2_HI=0x0
FWSEC: WPR2 verified
booter: HALT cpuctl=0x00000010 mailbox0=0x67b00001
post-boot: FEAT_PLM=0xffffffff ... WPRCFG=0xfffff0ff WPR=0xffffffff
selector SS0: read 0x4042043, wrote 0x88888888, readback 0x88888888
selector SS1: read 0x3, wrote 0x8, readback 0x8
post-FLR: SS0=0x88888888 SS1=0x8
summary: 1 unlocked, 0 already, 0 failed, 1 present
chainload: loaded, starting
```

`tools/read_sm_rates.py` after Debian boot:

```
rates: imla0=0 fmla16=0 dp=0 fmla32=0 ffma=0 imla1..4=0
meaning: 0 = full rate
```

`nvidia-smi` was healthy and the driver loaded without NVRM errors.

## BAR1 payload

`payload=tensor-bar1`, `bar1_rebar64=false`.

Before the payload the video BIOS state was:

```
CYA=0x28 CFG=0x80000009
ReBAR BAR1: current 256MB, supported ... 16GB 32GB
```

After the payload and FLR:

```
booter: HALT ... mailbox0=0x67b00004
post-boot bar1: CYA=0x30 CFG=0x8000000a XVE_PLM=0xffffffff
post-FLR bar1: CYA=0x30 CFG=0x8000000a XVE_PLM=0xffffffff FEAT_PLM=0xffffffff
ReBAR post-FLR: CAP=0x001ffc00 possible=0x1ffc0 64G_supported=true ctrl=0x00001001 size_index=16
ReBAR: restored BAR1 size control to 0x00000801 (readback 0x00000801, size_index=8)
```

Linux then reported:

```
BAR 1: current size: 256MB, supported: 64MB ... 32GB 64GB
nvidia-smi: RTX 4090, 24564 MiB, driver 590.48.01, healthy
read_sm_rates: all zero (full rate)
```

So on this 24 GB card the payload raised the BAR1 capability to 64 GiB, and
restoring the pre-payload size control kept the NVIDIA driver usable. No 64 GiB
resource was assigned in this mode; that is the intended
`bar1_rebar64=false` state.

## 64 GiB BAR1 with `pci=realloc`

Then `bar1_rebar64=true` and `pci=realloc` were added to the Debian kernel
command line, followed by a cold boot. The app left the size control at 64 GiB:

```
ReBAR post-FLR: ... ctrl=0x00001001 size_index=16
ReBAR: BAR1 size control 0x00001001 -> 0x00001001 ... (64G selected=true)
```

The kernel first failed the normal pass, then the `pci=realloc` retry moved the
BAR and expanded the bridge window:

```
pci 0000:01:00.0: BAR 1 [mem size 0x1000000000 64bit pref]: can't assign; no space
pci 0000:01:00.0: BAR 1 ... failed to assign
pci_bus 0000:00: No. 2 try to assign unassigned res
pci 0000:00:01.1: bridge window [mem 0x28800000000-0x29fffffffff 64bit pref]: assigned
pci 0000:01:00.0: BAR 1 [mem 0x29000000000-0x29fffffffff 64bit pref]: assigned
```

Result:

```
lspci: BAR 1: current size: 64GB, supported: ... 32GB 64GB
/sys/.../resource1: 68719476736 bytes
nvidia-smi -q: BAR1 Total: 65536 MiB, Used: 2 MiB
nvidia-smi: RTX 4090, 24564 MiB, driver 590.48.01, healthy
read_sm_rates: all zero (full rate)
```

So a single 24 GB 4090 can carry a usable 64 GiB BAR1 aperture with
`pci=realloc`. The 24 GB framebuffer is unaffected; BAR1 is the aperture, not
VRAM.

### Why the upstream report says 32 GiB after FLR

The kernel module lives inside Linux, so after its FLR the Linux PCI core
restores the cached 32 GiB size control. The EFI app has no Linux PCI core; the
FLR leaves the hardware at the payload's 64 GiB setting. `bar1_rebar64=true`
is therefore the explicit "set current to 64 GiB after FLR" option, and
`bar1_rebar64=false` restores the pre-payload size captured before the
transaction. Both modes are now hardware-verified:

| Config | After FLR | Linux/driver |
|---|---|---|
| `bar1_rebar64=false` | restore pre-payload size (256 MB here) | driver loads without `pci=realloc`; CAP still advertises 64 GiB |
| `bar1_rebar64=true` + `pci=realloc` | 64 GiB | kernel assigns 64 GiB; driver loads; `nvidia-smi` BAR1 65536 MiB |

### What failed first

The first version of the app left the ReBAR size control alone when
`bar1_rebar64=false`. The payload itself leaves the control at 64 GiB, and
without Linux resource assignment the driver failed:

```
NVRM: This PCI I/O region assigned to your NVIDIA device is invalid:
NVRM: BAR1 is 0M @ 0x0 (PCI:0000:01:00.0)
nvidia 0000:01:00.0: probe with driver nvidia failed with error -1
```

The machine was recovered by a cold power cycle into the stock `debian` entry.
The app now snapshots the BAR1 control before the payload and restores it after
the FLR when `bar1_rebar64=false`; that is the version verified above.

## Deployment state left on the machine

- `Boot0000` Ada Unlock points at `\EFI\ada-unlock\ada-unlock.efi`.
- `Boot000C` stock `debian` (shimx64) remains in the boot menu.
- `BootOrder: 0000,000C,0004` (Ada Unlock first, stock second).
- `/boot/efi/ada-unlock.ini` is the `tensor-bar1`, `bar1_rebar64=true` config.
- `pci=realloc` is on the Debian kernel command line.
- `/boot/efi/ada-unlock.log` is the latest run.
- The current boot has a 64 GiB BAR1 resource and `nvidia-smi` BAR1 Total 65536 MiB.
- The old `gpu-unlock` packages are purged; kernel cmdline and initramfs are
  clean. `/root/ada-unlock-backup/20261106-212409` holds the pre-change config,
  initrd, package list and PCI/NVIDIA dumps.
