GENTS ?= gents

include scenarios.mk
PACKS := $(notdir $(wildcard packs/gents/*))

.DEFAULT_GOAL := help

.PHONY: help test list $(addprefix test-,$(PACKS))

help:
	@echo "make test          Run every pack's suite"
	@echo "make test-<pack>   Run one pack's suite"
	@echo "make list          List the packs"
	@echo "GENTS=<path>       Use this gents binary (default: gents on PATH)"
	@echo
	@echo "Scenarios (need a model endpoint; see each pack's README):"
	@echo "make maintain MAINTENANCE_ROOT=<repo>   repo_maintenance"
	@echo "make defend DEFENDING_ROOT=<repo>       defending_code"

list:
	@printf '%s\n' $(PACKS)

test: $(addprefix test-,$(PACKS))

$(addprefix test-,$(PACKS)): test-%:
	GENTS="$(GENTS)" scripts/test-pack.sh packs/gents/$*
