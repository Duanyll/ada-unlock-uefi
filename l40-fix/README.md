# l40-fix.ko

Post-driver re-apply of the L40 SM speed-select override. The UEFI app opens
`FEAT_OVR_PLM` and writes `SS0=0x88888888` / `SS1=0x8`; this module handles
the cases where a later GPU reset or an early write races with NVIDIA
initialization and leaves some selector fields at `0x9` (half rate).

It maps BAR0, checks `FEAT_PLM == 0xffffffff`, and writes only
`FEAT_OVR_SM_SPEED_SELECT` / `_1`. It never touches `FUSE_*`, `FUSECTRL`,
`FUSE_EN_PROGRAM` or any OTP/fuse-programming register.

Build on the target host:

```sh
make -C l40-fix
install -D -m 0644 l40-fix/l40_fix.ko /lib/modules/$(uname -r)/extra/l40_fix.ko
depmod -a
install -m 0644 l40-fix/l40-fix.service /etc/systemd/system/l40-fix.service
systemctl daemon-reload
systemctl enable l40-fix.service
```

The service waits for `nvidia-smi -L` to succeed and then sleeps 3 s before
loading the module. Loading it during the driver's own init caused Xid 62/154
on two L40s once; the delay removed that. The module is a no-op when all four
selectors are already `0x88888888`/`0x8` and `FEAT_READOUT=0`.
