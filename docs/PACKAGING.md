# Debian package

`ada-unlock-uefi` is packaged for internal deployment as a Debian binary
package. The package contains locally generated NVIDIA firmware and must not be
uploaded to a public package archive or a public apt repository.

## Build

Build the ESP staging tree first, on a host with Rust, Python 3, `zstd` and
network access for the pinned NVIDIA inputs:

```sh
make dist
```

Then build the package on Debian (or a Debian container) with
`dpkg-buildpackage` and debhelper:

```sh
./packaging/build-deb.sh
```

The script refuses to build without a complete verified `dist/`, does not
download anything, and writes the package next to the source tree:

```text
../ada-unlock-uefi_0.1.0_amd64.deb
```

The EFI binary and firmware assets are not stored in git. `dist/` is a build
artifact; keep the produced `.deb` on internal storage only.

## What the package installs

The package itself installs into the root filesystem:

```text
/usr/lib/ada-unlock/esp/                    EFI application and firmware assets
/etc/default/ada-unlock                     operator configuration (conffile)
/usr/sbin/ada-unlockctl                     apply/remove/status helper
/usr/share/doc/ada-unlock-uefi/             documentation and NVIDIA license
```

`postinst` calls `ada-unlockctl apply`, which copies the payload to the mounted
ESP:

```text
\EFI\ada-unlock\ada-unlock.efi
\ada-unlock.ini
\booter_load_ad102.bin
\gsp_boot_ad102.bin
\gsp_boot_desc_ad102.bin
\fwimage_ad102.bin
\fwsignature_ad102.bin
\v67-ad102-four-plm.bin
\v67-ad102-bar1-64g.bin
\assets.json
```

It detects the ESP at `/boot/efi` or `/efi` and the Debian bootloader at
`\EFI\debian\grubx64.efi` (with fallbacks). The app's log stays at
`\ada-unlock.log` and is not deleted by package removal.

## Configuration

Edit the conffile and re-apply:

```sh
sudo editor /etc/default/ada-unlock
sudo ada-unlockctl apply
```

The default is the compute-only path, which is valid for both RTX 4090 and L40:

```sh
ADA_UNLOCK_PAYLOAD="tensor"
ADA_UNLOCK_BAR1_REBAR64="false"
```

For an RTX 4090 with the 64 GiB BAR1 construct:

```sh
ADA_UNLOCK_PAYLOAD="tensor-bar1"
ADA_UNLOCK_BAR1_REBAR64="true"
```

`ADA_UNLOCK_SET_BOOTORDER="true"` (the default) puts the UEFI application first
in `BootOrder`. The original order is saved and restored on removal. Set it to
`false` to create the entry without changing the order. Set
`ADA_UNLOCK_ADD_BOOT_ENTRY="false"` to omit the NVRAM entry entirely.

`ADA_UNLOCK_TARGETS` is `all` or a comma-separated list of PCI addresses.
`ADA_UNLOCK_CHAINLOAD` overrides bootloader autodetection. `ADA_UNLOCK_ESP`
overrides ESP autodetection.

## GRUB and UEFI integration

The app has to run **before** GRUB; it performs the PCIe transaction and then
chainloads the Debian bootloader. The package therefore creates a direct UEFI
boot entry rather than a GRUB menu entry: a GRUB entry that chainloaded the app
would return to the same GRUB menu after the app chainloaded GRUB.

GRUB is still used for the kernel command line. When the 64 GiB BAR1 path is
selected, `ada-unlockctl apply` writes one generated drop-in:

```text
/etc/default/grub.d/90-ada-unlock.cfg
```

It appends `pci=realloc` only if it is not already present in
`GRUB_CMDLINE_LINUX` or `GRUB_CMDLINE_LINUX_DEFAULT`, then runs `update-grub`.
Debian's `grub-mkconfig` sources this directory, so the option survives kernel
updates. The package never edits `/etc/default/grub` itself. On removal the
drop-in is deleted and `update-grub` runs again; a `pci=realloc` that was
already in the administrator's configuration is left untouched.

The package does not add a GRUB menu entry. The direct UEFI entry is visible in
`efibootmgr` as `Ada Unlock`; if that label is already used by an unrelated
entry, the package uses `Ada Unlock (apt)`.

## Install and remove

```sh
sudo apt install ./ada-unlock-uefi_0.1.0_amd64.deb
sudo ada-unlockctl status
sudo apt remove ada-unlock-uefi
sudo apt purge ada-unlock-uefi
```

`apt remove` deletes the package-managed UEFI entry, restores the saved
`BootOrder`, removes the GRUB drop-in and regenerates GRUB, and removes the ESP
files. The `/etc/default/ada-unlock` conffile remains for a later reinstall.
`apt purge` also removes the conffile and `/var/lib/ada-unlock`.

If files already existed on the ESP (for example from a manual installation),
the package copies them to `/var/lib/ada-unlock/backup` before overwriting
them, and restores them on removal. A pre-existing `Ada Unlock` UEFI entry that
matches the package's loader path is adopted rather than created, and is left in
place on removal.

## Requirements and limits

- Debian 13/trixie was used for validation. Debian's `grub-mkconfig` must read
  `/etc/default/grub.d/*.cfg`.
- Secure Boot must be off; the EFI application is unsigned.
- Disable the GPU Option ROM in firmware and select onboard/BMC video as the
  primary display. If POST initializes the GPU, `WPR2` is already up and the app
  cannot claim a cold card.
- The ESP must be mounted when installing or removing, normally at `/boot/efi`.
  Removal deliberately does not fail when it is unmounted; it warns and keeps
  the state so the files can be cleaned later.
- This package installs the UEFI app only. The L40 post-driver helper in
  `l40-fix/` remains a separate module and service.
- Package installation changes the next boot. It does not unlock the running
  system, and the app runs only on a cold boot.

## Validation performed

On Debian 13 with a single RTX 4090 and GRUB EFI:

- `apt install ./ada-unlock-uefi_0.1.0_amd64.deb` unpacked the package, created
  the ESP files, wrote a valid `ada-unlock.ini`, and created the `Ada Unlock`
  UEFI entry at the front of `BootOrder`.
- Reconfiguring to `tensor-bar1` + `bar1_rebar64=true` and running
  `ada-unlockctl apply` added exactly one effective `pci=realloc` to the normal
  GRUB Linux entries. Running it again did not duplicate the option.
- `apt remove` deleted the UEFI entry, restored the pre-install `BootOrder`,
  removed `/etc/default/grub.d/90-ada-unlock.cfg`, regenerated GRUB without the
  package-added `pci=realloc`, and restored the pre-existing ESP files byte for
  byte. The conffile remained.
- `apt purge` then removed the conffile and any state directory.
- A second installation over an existing matching `Ada Unlock` entry and manual
  ESP files adopted the entry instead of creating a new one. Purging left that
  pre-existing entry in place and restored the manual files byte for byte.
- The machine was not rebooted during this validation, and its original boot
  entry, `BootOrder`, ESP files, and GRUB configuration were restored after the
  test.
