# SPDX-License-Identifier: GPL-2.0-only
CARGO  ?= cargo
ASSETS ?= assets
CACHE  ?= $(HOME)/.cache/ada-unlock-uefi
DIST   ?= dist
EFI    := target/x86_64-unknown-uefi/release/ada-unlock.efi
ESP_ASSETS := booter_load_ad102.bin gsp_boot_ad102.bin gsp_boot_desc_ad102.bin \
              fwimage_ad102.bin fwsignature_ad102.bin \
              v67-ad102-four-plm.bin v67-ad102-bar1-64g.bin

.PHONY: all efi assets dist check clean

all: dist

efi:
	$(CARGO) efi

assets:
	python3 tools/make_assets.py --cache $(CACHE) --out $(ASSETS)

dist: efi assets check
	rm -rf $(DIST)
	mkdir -p $(DIST)/EFI/ada-unlock
	cp $(EFI) $(DIST)/EFI/ada-unlock/ada-unlock.efi
	for f in $(ESP_ASSETS); do cp $(ASSETS)/$$f $(DIST)/; done
	cp $(ASSETS)/assets.json $(DIST)/
	cp ada-unlock.ini.example $(DIST)/
	cp docs/INSTALL.md $(DIST)/INSTALL.md
	@echo
	@echo "ESP staging tree: $(DIST)/"
	@echo "Copy its contents to the ESP root, edit ada-unlock.ini, then add the boot entry."

check: efi
	python3 tools/check_efi.py $(EFI)
	python3 tools/check_assets.py $(ASSETS)

clean:
	rm -rf target $(DIST)
