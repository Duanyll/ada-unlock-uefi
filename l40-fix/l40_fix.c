// SPDX-License-Identifier: GPL-2.0-only
// Re-apply the L40 SM speed-select override after the NVIDIA driver has
// initialized the GPU. The UEFI app opens FEAT_PLM and writes the selectors;
// the driver's own GPU init re-fuses some fields, but FEAT_PLM stays open.
#include <linux/module.h>
#include <linux/pci.h>
#include <linux/io.h>

#define NV_VENDOR 0x10de
#define L40_DEVICE 0x26b5
#define FEAT_PLM 0x823804
#define FEAT_READOUT 0x823818
#define FEAT_SS0 0x82381c
#define FEAT_SS1 0x823820
#define SS0_UNLOCK 0x88888888u
#define SS1_UNLOCK 0x8u

static int __init l40_fix_init(void)
{
	struct pci_dev *pdev = NULL;
	int found = 0;

	while ((pdev = pci_get_device(NV_VENDOR, L40_DEVICE, pdev))) {
		resource_size_t start = pci_resource_start(pdev, 0);
		resource_size_t len = pci_resource_len(pdev, 0);
		void __iomem *bar;
		u32 plm, old0, old1, new0, new1, read1;

		if (!start || len < 0x830000) {
			pr_warn("l40_fix: %s BAR0 start=%pa len=%pa too small\n",
				pci_name(pdev), &start, &len);
			pci_dev_put(pdev);
			continue;
		}
		bar = ioremap(start, 0x900000);
		if (!bar) {
			pr_err("l40_fix: %s ioremap failed\n", pci_name(pdev));
			pci_dev_put(pdev);
			continue;
		}
		plm = readl(bar + FEAT_PLM);
		if (plm != 0xffffffffu) {
			pr_info("l40_fix: %s FEAT_PLM=%#x, not opened by the UEFI app; skip\n",
				pci_name(pdev), plm);
			iounmap(bar);
			pci_dev_put(pdev);
			continue;
		}
		old0 = readl(bar + FEAT_SS0);
		old1 = readl(bar + FEAT_SS1);
		writel(SS0_UNLOCK, bar + FEAT_SS0);
		writel(SS1_UNLOCK, bar + FEAT_SS1);
		new0 = readl(bar + FEAT_SS0);
		new1 = readl(bar + FEAT_SS1);
		read1 = readl(bar + FEAT_READOUT);
		pr_info("l40_fix: %s PLM=%#x SS0 %#x -> %#x SS1 %#x -> %#x READOUT=%#x\n",
			pci_name(pdev), plm, old0, new0, old1, new1, read1);
		iounmap(bar);
		pci_dev_put(pdev);
		found++;
	}
	pr_info("l40_fix: processed %d L40(s)\n", found);
	return 0;
}

static void __exit l40_fix_exit(void)
{
}

module_init(l40_fix_init);
module_exit(l40_fix_exit);
MODULE_LICENSE("GPL-2.0-only");
MODULE_DESCRIPTION("Re-apply L40 SM speed-select override after NVIDIA init");
