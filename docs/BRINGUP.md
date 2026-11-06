# Bring-up on the test server

The code is built and QEMU-tested, but the unlock and BAR1 paths have not run
on this repository's hardware yet. Use this order. Each step isolates one
variable.

## 0. QEMU smoke test (optional)

The app boots in OVMF, reads `\ada-unlock.ini`, writes `\ada-unlock.log`, sees
no GPU, and reports the missing chainload target. This checks the EFI entry,
file system and config parser without touching a GPU.

## 1. Tensor-only, one card

```ini
payload = tensor
bar1_rebar64 = false
targets = 0000:01:00.0
chainload = \EFI\debian\grubx64.efi
```

Cold power-on. Expected on SOL:

```
TX: begin
claim: cold card, FEAT_PLM=...
layout: frts=...
FWSEC: WPR2 verified
TX: WprMeta published
booter: HALT ...
post-boot: FEAT_PLM=0xffffffff ...
selector SS0: ... 0x88888888
selector SS1: ... 0x8
FLR: triggered
post-FLR: SS0=0x88888888 SS1=0x8
TX: card complete
summary: 1 unlocked, 0 already, 0 failed, 1 present
```

Then in Debian:

```sh
nvidia-smi
journalctl -k -b | grep -iE 'nvidia|bar1|xid'
```

Run the upstream FP16/BF16 tensor benchmark or any workload that exercises the
FP32-accumulate tensor path. A healthy `nvidia-smi` plus the selector readback
is necessary but not sufficient proof of the compute unlock.

## 2. Tensor-only, all cards

Remove `targets` (or set `targets = all`). Cold boot.

Expected summary:

```
summary: N unlocked, 0 already, 0 failed, N present
```

If one card fails, its log block names the phase. The remaining cards still run.

## 3. BAR1 construct, no ReBAR selection

```ini
payload = tensor-bar1
bar1_rebar64 = false
targets = 0000:01:00.0
```

Cold boot. Expected:

```
TX-bar1: pre-check CYA=0x28 CFG=0x80000009
post-boot bar1: CYA=0x30 CFG=0x8000000a XVE_PLM=0xffffffff
post-FLR bar1: CYA=0x30 CFG=0x8000000a XVE_PLM=0xffffffff FEAT_PLM=0xffffffff
ReBAR post-FLR: CAP=0x001ffc00 ... 64G_supported=true ... size_index=16
ReBAR: restored BAR1 size control to 0x00000801 (readback ... size_index=8)
```

The standard ReBAR capability now advertises 64 GiB, while the current BAR1
size is restored to what firmware/Linux had. The NVIDIA driver must load
normally; `lspci` shows the 64 GiB entry in the supported list.

## 4. BAR1 construct + 64 GiB ReBAR

```ini
payload = tensor-bar1
bar1_rebar64 = true
targets = 0000:01:00.0
```

Cold boot with `pci=realloc` in the Debian kernel command line. Expected:

```
ReBAR post-FLR: ... ctrl=... size_index=...
ReBAR: BAR1 size control ... -> ... (64G selected=true)
ReBAR after-select: ... 64G_supported=true ... size_index=16
```

After Linux:

```sh
cat /sys/bus/pci/devices/0000:01:00.0/resource1
# start end flags; size = end - start + 1
nvidia-smi -q -i 0 | grep -A3 -i 'BAR1'
lspci -vv -s 01:00.0 | grep -A10 -i 'resizable bar'
```

Expected BAR1 size `68719476736` and `nvidia-smi` `BAR1 Total: 65536 MiB`.
This was verified on a single 24 GB 4090: the first kernel pass reported
`can't assign; no space`, the `pci=realloc` retry assigned
`0x29000000000-0x29fffffffff`, and the driver loaded with BAR1 65536 MiB.
If Linux keeps the old size, check that `pci=realloc` is actually on the
running kernel (`cat /proc/cmdline`) and that the bridge windows have room.
The app does not reallocate the bus.

## 5. All cards

Set `targets = all` or omit it. Cold boot. Verify every card's BAR1 and driver
binding:

```sh
for d in /sys/bus/pci/devices/0000:*; do
  [ "$(cat $d/vendor 2>/dev/null)" = 0x10de ] || continue
  echo "$d $(cat $d/resource1 | cut -d' ' -f1) $(cat $d/resource1 | cut -d' ' -f2)"
done
nvidia-smi --query-gpu=index,pci.bus_id,memory.total --format=csv
```

## 6. Reboot

Test one ordinary reboot. If the app logs `WPR2 is enabled` or `already
unlocked`, the platform retained state; a full power cycle is the clean retry.
Record whether the 64 GiB selection and driver binding survive the reboot.
