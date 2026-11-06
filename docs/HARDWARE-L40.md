# Hardware result: four L40s, tensor issue-rate unlock

Tested 2026-11-06/07 on a four-GPU lab host:

- 4 x `10de:26b5` NVIDIA L40, AD102GL, 48 GB, stock 64 GiB BAR1
- Debian 13, kernel `6.12.85+deb13-amd64`
- NVIDIA driver `590.48.01` (unmodified, open kernel modules)
- 300 W board power limit; 2490 MHz max SM clock
- Secure Boot disabled
- GPU Option ROM disabled; primary display is onboard/BMC video
- GPU addresses: `03, 44, 8a, c3` (all function 0)
- VBIOS: `95.02.39.00.01` on `03/44/8a`, `95.02.5D.00.01` on `c3`
- No `pci=realloc`; firmware assigned 64 GiB BAR1 to every card

The UEFI app used here is build
`a00a5e29cc514521b15359fb1d19aa7c174e44fce8e4e6c1ae5c5018537c470b`.
It differs from the 4090 build only by the `26b5` admission, the post-FLR
selector rewrite and the effective-readout check.

## Result

Cold boot with `payload=tensor`, `bar1_rebar64=false`, `targets=all`:

```text
[000002] INFO  config: payload=Tensor bar1_rebar64=false targets=all ...
[000074] INFO  card 1: unlocked
[000137] INFO  card 2: unlocked
[000200] INFO  card 3: unlocked
[000263] INFO  card 4: unlocked
[000264] INFO  summary: 4 unlocked, 0 already, 0 failed, 4 present
```

Every card's `HANDOFF` line was:

```text
HANDOFF: SS0=0x88888888 SS1=0x8 FEAT_PLM=0xffffffff READOUT=0x00000000 WPR2=0x1ffffe00/0x0
```

The post-driver module saw all four full and was a no-op:

```text
l40_fix: 0000:c3:00.0 PLM=0xffffffff SS0 0x88888888 -> 0x88888888 SS1 0x8 -> 0x8 READOUT=0x0
l40_fix: 0000:8a:00.0 PLM=0xffffffff SS0 0x88888888 -> 0x88888888 SS1 0x8 -> 0x8 READOUT=0x0
l40_fix: 0000:44:00.0 PLM=0xffffffff SS0 0x88888888 -> 0x88888888 SS1 0x8 -> 0x8 READOUT=0x0
l40_fix: 0000:03:00.0 PLM=0xffffffff SS0 0x88888888 -> 0x88888888 SS1 0x8 -> 0x8 READOUT=0x0
```

`nvidia-smi` showed all four L40s, `GPU Recovery Action: None`, and `dmesg`
had no Xid. BAR1 stayed at 64 GiB; the app did not touch XVE/ReBAR.

## What was different from the 4090

1. The FLR re-fuses part of the selectors on L40. Immediately after the
   transaction's FLR:

   ```text
   post-FLR: FEAT_PLM=0xffffffff SS0=0x89989899 SS1=0x9
   ```

   `FEAT_PLM` stays open, so the app now rewrites `SS0=0x88888888` and
   `SS1=0x8` after the FLR and requires `FEAT_READOUT=0x0`. The initial
   write before FLR is not enough.

2. `FEAT_READOUT = 0` is necessary but not sufficient for the L40's
   full tensor rate; see the fuse section below.

## Post-driver module and Xid

The `l40-fix` module is a safety net for a reset that re-fuses the selectors
after the app has run. It maps BAR0, checks `FEAT_PLM == 0xffffffff`, and
writes only `FEAT_OVR_SM_SPEED_SELECT` / `_1`. It never writes `FUSE_*`,
`FUSECTRL`, `FUSE_EN_PROGRAM` or any OTP/fuse-programming register.

The first 4-card attempt loaded the module at 22.8 s, while NVIDIA was still
initializing the GPUs. It wrote 44 and 03 during that window and produced:

```text
NVRM: Xid (PCI:0000:44:00): 62, ...
NVRM: Xid (PCI:0000:44:00): 154, GPU recovery action changed ... GPU Reset Required
NVRM: Xid (PCI:0000:03:00): 62, ...
NVRM: Xid (PCI:0000:03:00): 154, GPU recovery action changed ... GPU Reset Required
```

A later module reload after the driver was fully up fixed the selectors without
Xid. The systemd unit now waits for `nvidia-smi -L` to succeed, sleeps 3 s, and
then loads the module. The final cold boot had no Xid and all four selectors
were already full, so the module was a no-op.

## Fuse difference from the 4090

L40 has three extra speed-select fuses set to half rate:

| Fuse | L40 stock | 4090 stock | Affected modes |
|---|---:|---:|---|
| `FUSE_SS_FMLA16` | **1** | **0** | FP16->FP16, FP8->FP16 |
| `FUSE_SS_IMLA0` | **1** | **0** | INT8 |
| `FUSE_SS_IMLA1` | **1** | **0** | INT8 |
| `FUSE_SS_FMLA32` | 1 | 1 | FP16/BF16/FP8->FP32 |
| `FUSE_SS_IMLA2` | 1 | 1 | FP32-accumulate group |
| `FUSE_SS_IMLA4` | 1 | 1 | FP32-accumulate group |
| `FUSE_SS_IMLA3` | 0 | 0 | - |
| `FUSE_SS_FFMA` / `DP` | 0 | 0 | FFMA / DP |

The 4090's `FMLA16/IMLA0/IMLA1 = 0` is why its FP16->FP16 and INT8 rates are
already full; its `FMLA32/IMLA2/IMLA4 = 1` are the fields the `FEAT_OVR` block
overrides. L40 has both groups set. The current `SS0=0x88888888` / `SS1=0x8`
override covers the FP32-accumulate group, but not `FMLA16/IMLA0/IMLA1`.

`FUSE_*` is the raw OTP readout sensed into a shadow register; it is not a
software override. `FUSE_EN_PROGRAM=1` is the OTP programming enable and must
not be touched. `FUSE_SS_PLM=0xffffffff` is the fuse-block access mask, not a
promise that the fuses can be rewritten. `SM_ISSUE_RATE_MODIFIER=0xbadf1201`
was read with the driver unbound; it is a privilege/engine sentinel, not an
extra throttle. The FECS copies are `0xbadf5040` on both parts, and
`FEAT_OVR_ECC` is ECC-related. None of those three is a safe extra unlock path.

So this result is **L40 + FP32-accumulate tensor unlock**, not a full L40S.
The remaining half-rate paths are fused. L40S is presumably the same die with
`FMLA16/IMLA0/IMLA1 = 0`; that is a fuse/SKU difference, not something this
register transaction can change. No fuse/OTP write was attempted.

## Benchmark

The owner measured `c3` with `tclab` at the 300 W limit. Stock L40 versus the
current partial unlock:

| Mode | Stock flop/clk/SM | Stock TFLOPS | c3 flop/clk/SM | c3 TFLOPS | Ratio |
|---|---:|---:|---:|---:|---:|
| FP16 -> FP32 | 511 | 180.6 | 1025 | 264.4 | 1.46x |
| BF16 -> FP32 | 511 | 180.6 | 1023 | 291.1 | 1.61x |
| FP16 -> FP16 | 511 | 180.7 | 512 | 180.8 | 1.00x |
| TF32 | 255 | 90.3 | 512 | 159.9 | 1.77x |
| FP8 e4m3 -> FP32 | 1021 | 361.2 | 2040 | 543.4 | 1.50x |
| FP8 e4m3 -> FP16 | 1022 | 361.3 | 1023 | 361.3 | 1.00x |
| INT8 | 1023 | 361.5 | 1023 | 361.5 | 1.00x |
| FFMA (control) | 246 | 82.9 | 246 | 82.5 | 1.00x |

The issue-rate doubles on the FP32-accumulate paths, but the 300 W limit keeps
the delivered TFLOPS at 1.46-1.77x. The FP16-accumulate, INT8 and FFMA paths
stay at the L40's fused rate.

A four-card benchmark was not rerun in this pass: the lab host's existing `tclab`
binary needs `libcublasLt.so.13`, which is not installed on the host. The app
and module logs above are the four-card register-level proof; the owner's
container/tool can rerun the table.
