GENTS ?= gents

include scenarios.mk
PACKS := $(sort $(notdir $(wildcard packs/gents/*)))

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
	@echo "make grok-port GROK_PORT_GENTS_ROOT=<gents> GROK_PORT_CEILING=<dir>  grok_tui_port"

list:
	@printf '%s\n' $(PACKS)

# Runs every suite, even after a failure, then fails naming the packs that did.
test:
	@failed=""; for pack in $(PACKS); do \
		GENTS="$(GENTS)" scripts/test-pack.sh packs/gents/$$pack || failed="$$failed $$pack"; \
	done; \
	test -z "$$failed" || { echo "failed:$$failed" >&2; exit 1; }

$(addprefix test-,$(PACKS)): test-%:
	GENTS="$(GENTS)" scripts/test-pack.sh packs/gents/$*
