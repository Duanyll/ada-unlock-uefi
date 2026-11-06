# Hardware result: eight 48 GB RTX 4090s

Tested 2026-11-06 on `villa-Super-Server-27`:

- Dual EPYC, eight `10de:2684` RTX 4090 48 GB, direct CPU PCIe root ports
- Debian 13, kernel `6.12.107+deb13-amd64`
- Modified NVIDIA open kernel module `615.71.09` (P2P build)
- GPU Option ROM disabled in the BIOS
- Secure Boot disabled, BMC SOL on `ttyS1`/115200
- App build `d91e4e0a36849cea9610b5cd5e910048e85e7d038c0cf684726943d1ae8c7509`

GPU addresses: `01, 25, 41, 61, 81, a1, c1, e1` (all function 0).

## Config

```ini
payload = tensor-bar1
bar1_rebar64 = true
chainload = \EFI\debian\grubx64.efi
log = \ada-unlock.log
```

`targets` omitted (all applicable GPUs). Kernel command line includes
`pci=realloc` and `console=ttyS1,115200n8`.

## App result

```
[000083] INFO  card 1: unlocked
[000147] INFO  card 2: unlocked
[000211] INFO  card 3: unlocked
[000275] INFO  card 4: unlocked
[000339] INFO  card 5: unlocked
[000403] INFO  card 6: unlocked
[000467] INFO  card 7: unlocked
[000531] INFO  card 8: unlocked
[000532] INFO  summary: 8 unlocked, 0 already, 0 failed, 8 present
[000534] INFO  chainload: loaded, starting
```

Each card's block contains the same shape: FWSEC WPR2 verified, Booter HALT
with mailbox marker `0x67b00004`, `CYA=0x30` / `CFG=0x8000000a` / XVE PLM open,
selectors `SS0=0x88888888` / `SS1=0x8` before and after FLR, and the ReBAR
control at size index 16.

## Linux result

All eight BAR1 resources are 64 GiB:

```text
/sys/bus/pci/devices/0000:01:00.0/resource1 68719476736
...
/sys/bus/pci/devices/0000:e1:00.0/resource1 68719476736
```

`lspci` for every card:

```text
BAR 1: current size: 64GB, supported: 64MB ... 32GB 64GB
```

`nvidia-smi -q` reports `BAR1 Total: 65536 MiB` on all eight cards.
`nvidia-smi` shows all eight 4090s (49140 MiB each), driver `615.71.09`,
with no NVRM probe failure and no Xid in `dmesg`.

Kernel assignments:

```text
pci 0000:61:00.0: BAR 1 [mem 0x11000000000-0x11fffffffff 64bit pref]: assigned
pci 0000:41:00.0: BAR 1 [mem 0x19000000000-0x19fffffffff 64bit pref]: assigned
pci 0000:01:00.0: BAR 1 [mem 0x29000000000-0x29fffffffff 64bit pref]: assigned
pci 0000:25:00.0: BAR 1 [mem 0x21000000000-0x21fffffffff 64bit pref]: assigned
pci 0000:e1:00.0: BAR 1 [mem 0x39000000000-0x39fffffffff 64bit pref]: assigned
pci 0000:c1:00.0: BAR 1 [mem 0x41000000000-0x41fffffffff 64bit pref]: assigned
pci 0000:81:00.0: BAR 1 [mem 0x51000000000-0x51fffffffff 64bit pref]: assigned
pci 0000:a1:00.0: BAR 1 [mem 0x49000000000-0x49fffffffff 64bit pref]: assigned
```

## P2P

`nvidia-smi topo -p2p r` reports `OK` for every GPU pair:

```text
     GPU0 GPU1 GPU2 GPU3 GPU4 GPU5 GPU6 GPU7
GPU0  X   OK   OK   OK   OK   OK   OK   OK
...
GPU7  OK  OK   OK   OK   OK   OK   OK   X
```

Because every BAR1 is 64 GiB and larger than the 48 GiB framebuffer, this is
the standard BAR1 P2P path. `dmesg` contains no "method 3" lines, which is
consistent with the modified driver only using method 3 when BAR1 is smaller
than VRAM. All eight cards use the same method, so the driver advertises P2P
between them. No workload was run; this is a driver/property check only.

## Deployment state

- `BootOrder: 0002,0000,0008,0010,0001,0003,0004,0005,0006`
- `Boot0002` Ada Unlock, `Boot0000` stock Debian
- `/boot/efi/ada-unlock.ini` is the all-GPU `tensor-bar1`,
  `bar1_rebar64=true` config
- `pci=realloc` and `console=ttyS1,115200n8` are in GRUB; SOL shows the kernel
  log
- `/boot/efi/ada-unlock.log` is the full eight-card run
- Backup: `/root/ada-unlock-backup/20261106-222436`
