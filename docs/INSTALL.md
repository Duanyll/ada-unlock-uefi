# Install on a headless Debian server

This assumes you have console access through the BMC (SOL) and you know which
disk/partition holds the ESP. The app is unsigned; Secure Boot must be off.

## 1. Firmware settings

- Boot mode: UEFI (CSM off)
- Secure Boot: off
- Above 4G Decoding: on
- Resizable BAR: on (or Auto)
- Serial console redirection / SOL: on, 115200 8N1 unless your BMC says otherwise
- Keep a normal Debian boot entry in the boot menu
- GPU Option ROM (OpROM): off, and primary display: onboard/BMC video. Some
  server boards run the GPU OpROM during POST; that initializes the GPU before
  the app, so `WPR2` is already up and the app cannot claim a cold card.
- Do not use a GPU as the UEFI console; the app logs through ConOut, and a GPU
  console driver would touch the card during the transaction

The app runs before Linux. It does not need a GPU console or a display attached.

## 2. Build

On a build host (Linux or macOS with Rust, Python 3 and `zstd`):

```sh
rustup target add x86_64-unknown-uefi
make dist
```

`dist/` contains the EFI app, the seven generated assets, `ada-unlock.ini.example`
and this file.

## 3. Stage the ESP

Mount the ESP on the server (example `/dev/nvme0n1p1`):

```sh
sudo mkdir -p /mnt/esp
sudo mount /dev/nvme0n1p1 /mnt/esp
sudo cp -a dist/. /mnt/esp/
sudo cp /mnt/esp/ada-unlock.ini.example /mnt/esp/ada-unlock.ini
sudo "$EDITOR" /mnt/esp/ada-unlock.ini
```

Start with the tensor-only payload for the first server test:

```ini
payload = tensor
bar1_rebar64 = false
chainload = \EFI\debian\grubx64.efi
log = \ada-unlock.log
```

For L40, use `payload = tensor` only; its BAR1 is already 64 GiB at
power-on and the bar1 payload is not applicable. Also install the post-driver
helper from `l40-fix/` and enable its service; it re-applies the selectors if a
reset leaves them partial and never writes fuse/OTP registers.

After the tensor path is verified, switch to `payload = tensor-bar1` with
`bar1_rebar64 = false` first. That opens the 64 GiB capability but restores
the original BAR1 size, so the driver stays usable. Only set
`bar1_rebar64 = true` (with `pci=realloc`) when you actually want Linux to
assign the 64 GiB aperture.

## 4. Add the boot entry

Find the ESP disk and partition number with `lsblk -o NAME,PARTTYPENAME,MOUNTPOINTS`.
Then:

```sh
sudo efibootmgr -c -d /dev/nvme0n1 -p 1 \
  -L "Ada Unlock" -l '\EFI\ada-unlock\ada-unlock.efi'
sudo efibootmgr -v
```

If the server should unlock on every boot, put `Ada Unlock` first in `BootOrder`:

```sh
sudo efibootmgr -o XXXX,YYYY,ZZZZ
```

Keep the stock Debian entry in the list. You will need it if the ESP config is
wrong or you want to roll back.

## 5. Kernel command line for BAR1

Only for the 64 GiB path. Edit `/etc/default/grub` on the Debian install:

```sh
GRUB_CMDLINE_LINUX_DEFAULT="quiet pci=realloc"
sudo update-grub
```

The app does not edit GRUB or pass kernel options. The chainloaded bootloader
must already carry `pci=realloc`.

## 6. First boot

Power the machine off completely (AC off, wait a few seconds), then on. A warm
reboot may leave `WPR2` enabled, in which case the app logs
`claim: WPR2 is enabled` and skips the card.

Watch SOL or the attached video. Some firmware does not redirect UEFI ConOut
to SOL; the full log is in `\ada-unlock.log`. A successful card ends with:

```
ada-unlock 0.1.0
assets: booter=... payload=64000
target: RTX 4090 10de:2684 at 0000:01:00.0
TX: card complete
summary: 1 unlocked, 0 already, 0 failed, 1 present
chainload: \EFI\debian\grubx64.efi
```

Then let Debian boot and check the GPU as described in
[BRINGUP.md](BRINGUP.md).

## 7. Rollback

- Remove `Ada Unlock` from `BootOrder` or select the stock Debian entry.
- Remove `pci=realloc` from the kernel command line if you no longer want the
  64 GiB reallocation.
- A full power-off restores the card to its stock firmware state; the unlock is
  not persistent across a power cycle.
