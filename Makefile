GENTS ?= gents
PACKS := $(notdir $(wildcard packs/gents/*))

.PHONY: help test list $(addprefix test-,$(PACKS))

help:
	@echo "make test          Run every pack's suite"
	@echo "make test-<pack>   Run one pack's suite"
	@echo "make list          List the packs"
	@echo "GENTS=<path>       Use this gents binary (default: gents on PATH)"

list:
	@printf '%s\n' $(PACKS)

test: $(addprefix test-,$(PACKS))

$(addprefix test-,$(PACKS)): test-%:
	GENTS="$(GENTS)" scripts/test-pack.sh packs/gents/$*
