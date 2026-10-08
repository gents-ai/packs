# Scenario runs of the packs that drive a real repository through a model.
# Each needs an OpenAI-compatible endpoint and the repository it works on:
#   make maintain MAINTENANCE_ROOT=<repo>
#   make defend DEFENDING_ROOT=<repo>
#   make grok-port GROK_PORT_GENTS_ROOT=<gents checkout> GROK_PORT_CEILING=<dir>
#   make scan SCAN_ROOT=<repo>
#   make defend-page GENTS_ROOT=<gents checkout>
# Runs land under packs/gents/<pack>/runs/<job-id>/.

MAINTENANCE_ROOT ?=
MAINTENANCE_BRANCH ?=
MAINTENANCE_HEAD ?= HEAD
MAINTENANCE_PR_BASE ?= main
MAINTENANCE_PROMPT ?= Find the next small, behavior-preserving repository cleanup wave and package the strongest work into focused 1-3 finding commits on one shared branch and worktree.
MAINTENANCE_AREAS ?= auto
MAINTENANCE_MIN_AREAS ?= 5
MAINTENANCE_MAX_AREAS ?= 10
MAINTENANCE_HISTORY_DEPTH ?= 250
MAINTENANCE_PORT ?= 19192
MAINTENANCE_JOB_ID ?=
MAINTENANCE_KEEP_HOME ?=
MAINTENANCE_CONTEXT_WINDOW ?= 262144
MAINTENANCE_MAX_OUTPUT_TOKENS ?= 65536
MAINTENANCE_MAX_TURNS ?= 1000000
MAINTENANCE_TEMPERATURE ?= 1.0
MAINTENANCE_TOP_P ?= 0.95
MAINTENANCE_COMPACTION_THRESHOLD ?= 0.85
MAINTENANCE_DEADLINE_SECS ?= 86400
MAINTENANCE_AWAIT_TIMEOUT_SECS ?= 86400
MAINTENANCE_STREAM_LIVENESS_SECS ?= 1800
MAINTENANCE_STREAM_BATCH_MS ?= 5000
MAINTENANCE_RETRY_MAX_TRANSPORT ?= 720
MAINTENANCE_RETRY_MAX_RESAMPLE ?= 32
DEFENDING_ROOT ?=
DEFENDING_PROMPT ?= Map the repository's trust boundaries, find plausible exploitable vulnerabilities, adversarially verify them, and draft minimal reviewable fixes for confirmed findings.
DEFENDING_ENDPOINT ?= http://127.0.0.1:8080/v1
DEFENDING_MODEL ?= GLM-5.2
DEFENDING_MIN_AREAS ?= 4
DEFENDING_MAX_AREAS ?= 10
DEFENDING_MAX_CONCURRENT ?= 8
DEFENDING_PORT ?= 19193
DEFENDING_PAGE_PORT ?= 19194
GENTS_ROOT ?=
NPM ?= npm
DEFENDING_JOB_ID ?=
DEFENDING_KEEP_HOME ?=
DEFENDING_CONTEXT_WINDOW ?= 262144
DEFENDING_MAX_OUTPUT_TOKENS ?= 65536
DEFENDING_MAX_TURNS ?= 1000000
DEFENDING_TEMPERATURE ?= 1.0
DEFENDING_TOP_P ?= 0.95
DEFENDING_COMPACTION_THRESHOLD ?= 0.762939453125
DEFENDING_DEADLINE_SECS ?= 86400
DEFENDING_AWAIT_TIMEOUT_SECS ?= 86400
DEFENDING_STREAM_LIVENESS_SECS ?= 1800
DEFENDING_STREAM_BATCH_MS ?= 5000
DEFENDING_RETRY_MAX_TRANSPORT ?= 720
DEFENDING_RETRY_MAX_RESAMPLE ?= 32

SCAN_ROOT ?=
SCAN_PORT ?= 19197
SCAN_JOB_ID ?=
SCAN_KEEP_HOME ?=

GROK_PORT_CEILING ?=
GROK_PORT_GENTS_ROOT ?=
GROK_PORT_GROK_ROOT ?= $(CURDIR)/packs/gents/grok_tui_port/recon_input
GROK_PORT_PROMPT ?= Map the Grok TUI wire from grok-build and implement a Gents-only thin client. Do not add DefraDB ACP or Grok permission UI. Prove model name, context window, tool-call semantics, subprocesses, subagents, and interrupts with live GLM turns.
GROK_PORT_BASE_SHA ?= HEAD
GROK_PORT_PR_BASE ?= main
GROK_PORT_BRANCH ?= agent/grok-tui-port-pack9
GROK_PORT_ENDPOINT_1 ?= http://127.0.0.1:8000/v1
GROK_PORT_MODEL ?= GLM-5.3-Flash-NVFP4
GROK_PORT_MIN_SURFACES ?= 13
GROK_PORT_MAX_SURFACES ?= 13
GROK_PORT_MAX_CONCURRENT_1 ?= 16
GROK_PORT_PORT ?= 19195
GROK_PORT_LIVE_PORT ?= 19196
GROK_PORT_JOB_ID ?=
GROK_PORT_KEEP_HOME ?=
GROK_PORT_CONTEXT_WINDOW ?= 524288
GROK_PORT_MAX_OUTPUT_TOKENS ?= 65536
GROK_PORT_MAX_TURNS ?= 1000000
GROK_PORT_TEMPERATURE ?= 1.0
GROK_PORT_RECON_TEMPERATURE ?= $(GROK_PORT_TEMPERATURE)
GROK_PORT_IMPLEMENT_TEMPERATURE ?= $(GROK_PORT_TEMPERATURE)
GROK_PORT_REVIEW_TEMPERATURE ?= $(GROK_PORT_TEMPERATURE)
GROK_PORT_CODE_REVIEW_TEMPERATURE ?= $(GROK_PORT_TEMPERATURE)
GROK_PORT_TOP_P ?= 0.95
GROK_PORT_REASONING_EFFORT ?= high
GROK_PORT_CODE_REVIEW_REASONING_EFFORT ?= $(GROK_PORT_REASONING_EFFORT)
GROK_PORT_COMPACTION_THRESHOLD ?= 0.762939453125

.PHONY: maintain
maintain:
	@test -d "$(MAINTENANCE_ROOT)" || { echo "set MAINTENANCE_ROOT to the repository to maintain (got: $(MAINTENANCE_ROOT))" >&2; exit 2; }
	@case "$(MAINTENANCE_AREAS)" in auto) ;; ''|*[!0-9]*) echo "MAINTENANCE_AREAS must be auto or a positive integer: $(MAINTENANCE_AREAS)" >&2; exit 2;; *) test "$(MAINTENANCE_AREAS)" -gt 0 || { echo "MAINTENANCE_AREAS must be greater than zero" >&2; exit 2; };; esac
	@case "$(MAINTENANCE_MIN_AREAS)" in ''|*[!0-9]*) echo "MAINTENANCE_MIN_AREAS must be a positive integer: $(MAINTENANCE_MIN_AREAS)" >&2; exit 2;; esac
	@case "$(MAINTENANCE_MAX_AREAS)" in ''|*[!0-9]*) echo "MAINTENANCE_MAX_AREAS must be a positive integer: $(MAINTENANCE_MAX_AREAS)" >&2; exit 2;; esac
	@case "$(MAINTENANCE_HISTORY_DEPTH)" in ''|*[!0-9]*) echo "MAINTENANCE_HISTORY_DEPTH must be a positive integer: $(MAINTENANCE_HISTORY_DEPTH)" >&2; exit 2;; esac
	@test "$(MAINTENANCE_MIN_AREAS)" -ge 5 && test "$(MAINTENANCE_MAX_AREAS)" -ge "$(MAINTENANCE_MIN_AREAS)" || { echo "maintenance area bounds must satisfy 5 <= MAINTENANCE_MIN_AREAS <= MAINTENANCE_MAX_AREAS" >&2; exit 2; }
	@if test "$(MAINTENANCE_AREAS)" != auto; then test "$(MAINTENANCE_AREAS)" -ge "$(MAINTENANCE_MIN_AREAS)" && test "$(MAINTENANCE_AREAS)" -le "$(MAINTENANCE_MAX_AREAS)" || { echo "MAINTENANCE_AREAS must satisfy MAINTENANCE_MIN_AREAS <= MAINTENANCE_AREAS <= MAINTENANCE_MAX_AREAS" >&2; exit 2; }; fi
	@test "$(MAINTENANCE_HISTORY_DEPTH)" -gt 0 || { echo "MAINTENANCE_HISTORY_DEPTH must be greater than zero" >&2; exit 2; }
	@cd "$(MAINTENANCE_ROOT)" && git rev-parse --verify "$(MAINTENANCE_HEAD)^{commit}" >/dev/null || { echo "MAINTENANCE_HEAD is not a commit: $(MAINTENANCE_HEAD)" >&2; exit 2; }
	@command -v rust-analyzer >/dev/null 2>&1 || echo "warning: rust-analyzer not found on PATH; maintenance will fall back to file/search tools" >&2
	@maintenance_job_id="$(MAINTENANCE_JOB_ID)"; \
	if test -z "$$maintenance_job_id"; then maintenance_job_id="maintenance-$$(date -u +%Y%m%dT%H%M%SZ)-$$$$"; fi; \
	maintenance_branch="$(MAINTENANCE_BRANCH)"; \
	if test -z "$$maintenance_branch"; then maintenance_branch="agent/$$maintenance_job_id"; fi; \
	GENTS_MAINTENANCE_ROOT="$(abspath $(MAINTENANCE_ROOT))" \
	GENTS_MAINTENANCE_BRANCH="$$maintenance_branch" \
	GENTS_MAINTENANCE_HEAD_REF="$(MAINTENANCE_HEAD)" \
	GENTS_MAINTENANCE_PR_BASE="$(MAINTENANCE_PR_BASE)" \
	GENTS_MAINTENANCE_PROMPT="$(MAINTENANCE_PROMPT)" \
	GENTS_MAINTENANCE_AREA_COUNT="$(MAINTENANCE_AREAS)" \
	GENTS_MAINTENANCE_MIN_AREAS="$(MAINTENANCE_MIN_AREAS)" \
	GENTS_MAINTENANCE_MAX_AREAS="$(MAINTENANCE_MAX_AREAS)" \
	GENTS_MAINTENANCE_HISTORY_DEPTH="$(MAINTENANCE_HISTORY_DEPTH)" \
	GENTS_MAINTENANCE_CONTEXT_WINDOW="$(MAINTENANCE_CONTEXT_WINDOW)" \
	GENTS_MAINTENANCE_MAX_OUTPUT_TOKENS="$(MAINTENANCE_MAX_OUTPUT_TOKENS)" \
	GENTS_MAINTENANCE_MAX_TURNS="$(MAINTENANCE_MAX_TURNS)" \
	GENTS_MAINTENANCE_TEMPERATURE="$(MAINTENANCE_TEMPERATURE)" \
	GENTS_MAINTENANCE_TOP_P="$(MAINTENANCE_TOP_P)" \
	GENTS_MAINTENANCE_COMPACTION_THRESHOLD="$(MAINTENANCE_COMPACTION_THRESHOLD)" \
	GENTS_MAINTENANCE_DEADLINE_SECS="$(MAINTENANCE_DEADLINE_SECS)" \
	GENTS_MAINTENANCE_AWAIT_TIMEOUT_SECS="$(MAINTENANCE_AWAIT_TIMEOUT_SECS)" \
	GENTS_MAINTENANCE_STREAM_LIVENESS_SECS="$(MAINTENANCE_STREAM_LIVENESS_SECS)" \
	GENTS_MAINTENANCE_STREAM_BATCH_MS="$(MAINTENANCE_STREAM_BATCH_MS)" \
	GENTS_MAINTENANCE_RETRY_MAX_TRANSPORT="$(MAINTENANCE_RETRY_MAX_TRANSPORT)" \
	GENTS_MAINTENANCE_RETRY_MAX_RESAMPLE="$(MAINTENANCE_RETRY_MAX_RESAMPLE)" \
	"$(GENTS)" pack scenario run "$(CURDIR)/packs/gents/repo_maintenance" \
		--http-port "$(MAINTENANCE_PORT)" \
		--job-id "$$maintenance_job_id" \
		$(if $(MAINTENANCE_KEEP_HOME),--keep-home,)

.PHONY: defend
defend:
	@test -d "$(DEFENDING_ROOT)" || { echo "set DEFENDING_ROOT to the repository to defend (got: $(DEFENDING_ROOT))" >&2; exit 2; }
	@case "$(DEFENDING_MIN_AREAS)" in ''|*[!0-9]*) echo "DEFENDING_MIN_AREAS must be a positive integer: $(DEFENDING_MIN_AREAS)" >&2; exit 2;; esac
	@case "$(DEFENDING_MAX_AREAS)" in ''|*[!0-9]*) echo "DEFENDING_MAX_AREAS must be a positive integer: $(DEFENDING_MAX_AREAS)" >&2; exit 2;; esac
	@case "$(DEFENDING_MAX_CONCURRENT)" in ''|*[!0-9]*) echo "DEFENDING_MAX_CONCURRENT must be a positive integer: $(DEFENDING_MAX_CONCURRENT)" >&2; exit 2;; esac
	@test "$(DEFENDING_MIN_AREAS)" -gt 0 && test "$(DEFENDING_MAX_AREAS)" -ge "$(DEFENDING_MIN_AREAS)" || { echo "defending area bounds must satisfy 0 < DEFENDING_MIN_AREAS <= DEFENDING_MAX_AREAS" >&2; exit 2; }
	@test "$(DEFENDING_MAX_CONCURRENT)" -gt 0 || { echo "DEFENDING_MAX_CONCURRENT must be greater than zero" >&2; exit 2; }
	@command -v rust-analyzer >/dev/null 2>&1 || echo "warning: rust-analyzer not found on PATH; defending-code will fall back to file/search tools" >&2
	@defending_job_id="$(DEFENDING_JOB_ID)"; \
	if test -z "$$defending_job_id"; then defending_job_id="defending-$$(date -u +%Y%m%dT%H%M%SZ)-$$$$"; fi; \
	GENTS_DEFENDING_ROOT="$(abspath $(DEFENDING_ROOT))" \
	GENTS_DEFENDING_PROMPT="$(DEFENDING_PROMPT)" \
	GENTS_DEFENDING_ENDPOINT="$(DEFENDING_ENDPOINT)" \
	GENTS_DEFENDING_MODEL="$(DEFENDING_MODEL)" \
	GENTS_DEFENDING_MIN_AREAS="$(DEFENDING_MIN_AREAS)" \
	GENTS_DEFENDING_MAX_AREAS="$(DEFENDING_MAX_AREAS)" \
	GENTS_DEFENDING_MAX_CONCURRENT="$(DEFENDING_MAX_CONCURRENT)" \
	GENTS_DEFENDING_CONTEXT_WINDOW="$(DEFENDING_CONTEXT_WINDOW)" \
	GENTS_DEFENDING_MAX_OUTPUT_TOKENS="$(DEFENDING_MAX_OUTPUT_TOKENS)" \
	GENTS_DEFENDING_MAX_TURNS="$(DEFENDING_MAX_TURNS)" \
	GENTS_DEFENDING_TEMPERATURE="$(DEFENDING_TEMPERATURE)" \
	GENTS_DEFENDING_TOP_P="$(DEFENDING_TOP_P)" \
	GENTS_DEFENDING_COMPACTION_THRESHOLD="$(DEFENDING_COMPACTION_THRESHOLD)" \
	GENTS_DEFENDING_DEADLINE_SECS="$(DEFENDING_DEADLINE_SECS)" \
	GENTS_DEFENDING_AWAIT_TIMEOUT_SECS="$(DEFENDING_AWAIT_TIMEOUT_SECS)" \
	GENTS_DEFENDING_STREAM_LIVENESS_SECS="$(DEFENDING_STREAM_LIVENESS_SECS)" \
	GENTS_DEFENDING_STREAM_BATCH_MS="$(DEFENDING_STREAM_BATCH_MS)" \
	GENTS_DEFENDING_RETRY_MAX_TRANSPORT="$(DEFENDING_RETRY_MAX_TRANSPORT)" \
	GENTS_DEFENDING_RETRY_MAX_RESAMPLE="$(DEFENDING_RETRY_MAX_RESAMPLE)" \
	"$(GENTS)" pack scenario run "$(CURDIR)/packs/gents/defending_code" \
		--http-port "$(DEFENDING_PORT)" \
		--job-id "$$defending_job_id" \
		$(if $(DEFENDING_KEEP_HOME),--keep-home,)

.PHONY: grok-port
grok-port:
	@test -d "$(GROK_PORT_GENTS_ROOT)" || { echo "set GROK_PORT_GENTS_ROOT to the gents checkout to port into (got: $(GROK_PORT_GENTS_ROOT))" >&2; exit 2; }
	@test -d "$(GROK_PORT_CEILING)" || { echo "set GROK_PORT_CEILING to the operator tool ceiling (got: $(GROK_PORT_CEILING))" >&2; exit 2; }
	@test -f "$(GROK_PORT_GENTS_ROOT)/Cargo.toml" || { echo "GROK_PORT_GENTS_ROOT must be a gents checkout with a Cargo.toml (got: $(GROK_PORT_GENTS_ROOT))" >&2; exit 2; }
	@test -d "$(GROK_PORT_GROK_ROOT)" || { echo "GROK_PORT_GROK_ROOT must be the directory holding the audited ledger (got: $(GROK_PORT_GROK_ROOT))" >&2; exit 2; }
	@ceiling="$$(cd "$(GROK_PORT_CEILING)" && pwd -P)"; \
	for inside in "$$(cd "$(GROK_PORT_GENTS_ROOT)" && pwd -P)" "$$(cd "$(CURDIR)" && pwd -P)"; do \
		case "$$inside/" in "$$ceiling"/*) ;; *) echo "GROK_PORT_CEILING ($$ceiling) must contain both GROK_PORT_GENTS_ROOT and this repository; $$inside is outside it" >&2; exit 2;; esac; \
	done
	@case "$(GROK_PORT_MIN_SURFACES)" in ''|*[!0-9]*) echo "GROK_PORT_MIN_SURFACES must be a positive integer: $(GROK_PORT_MIN_SURFACES)" >&2; exit 2;; esac
	@case "$(GROK_PORT_MAX_SURFACES)" in ''|*[!0-9]*) echo "GROK_PORT_MAX_SURFACES must be a positive integer: $(GROK_PORT_MAX_SURFACES)" >&2; exit 2;; esac
	@case "$(GROK_PORT_MAX_CONCURRENT_1)" in ''|*[!0-9]*) echo "GROK_PORT_MAX_CONCURRENT_1 must be a positive integer: $(GROK_PORT_MAX_CONCURRENT_1)" >&2; exit 2;; esac
	@test "$(GROK_PORT_MIN_SURFACES)" -gt 0 && test "$(GROK_PORT_MAX_SURFACES)" -ge "$(GROK_PORT_MIN_SURFACES)" || { echo "grok-port surface bounds must satisfy 0 < GROK_PORT_MIN_SURFACES <= GROK_PORT_MAX_SURFACES" >&2; exit 2; }
	@test "$(GROK_PORT_MAX_CONCURRENT_1)" -gt 0 || { echo "GROK_PORT_MAX_CONCURRENT_1 must be greater than zero" >&2; exit 2; }
	@command -v rust-analyzer >/dev/null 2>&1 || echo "warning: rust-analyzer not found on PATH; grok-tui-port will fall back to file/search tools" >&2
	@grok_port_job_id="$(GROK_PORT_JOB_ID)"; \
	if test -z "$$grok_port_job_id"; then grok_port_job_id="grok-port-$$(date -u +%Y%m%dT%H%M%SZ)-$$$$"; fi; \
	grok_port_dep="$$(mktemp -d)" || exit 2; \
	trap 'rm -rf "$$grok_port_dep"' EXIT; \
	"$(GENTS)" pack build "$(CURDIR)/packs/gents/code_review" --out "$$grok_port_dep/code_review.pack" >/dev/null || exit 2; \
	grok_port_base_sha="$$(git -C "$(abspath $(GROK_PORT_GENTS_ROOT))" rev-parse --verify "$(GROK_PORT_BASE_SHA)^{commit}")" || exit 2; \
	grok_port_models="$$(curl --fail --silent --show-error --max-time 10 "$(GROK_PORT_ENDPOINT_1)/models")" || { echo "GLM preflight failed: $(GROK_PORT_ENDPOINT_1)/models" >&2; exit 2; }; \
	case "$$grok_port_models" in *'"id":"$(GROK_PORT_MODEL)"'*) ;; *) echo "GLM preflight did not advertise $(GROK_PORT_MODEL): $(GROK_PORT_ENDPOINT_1)" >&2; exit 2;; esac; \
	grok_port_max_context="$$(printf '%s' "$$grok_port_models" | python3 -c 'import json,sys; model=sys.argv[1]; rows=json.load(sys.stdin).get("data", []); print(max((int(row.get("max_model_len", 0)) for row in rows if row.get("id") == model), default=0))' "$(GROK_PORT_MODEL)")" || exit 2; \
	test "$$grok_port_max_context" -ge "$(GROK_PORT_CONTEXT_WINDOW)" || { echo "GLM preflight context $$grok_port_max_context is smaller than required $(GROK_PORT_CONTEXT_WINDOW): $(GROK_PORT_ENDPOINT_1)" >&2; exit 2; }; \
	GENTS_GROK_PORT_CEILING="$(abspath $(GROK_PORT_CEILING))" \
	GENTS_GROK_PORT_GENTS_ROOT="$(abspath $(GROK_PORT_GENTS_ROOT))" \
	GENTS_GROK_PORT_GROK_ROOT="$$(cd "$(GROK_PORT_GROK_ROOT)" && pwd -P)" \
	GENTS_GROK_PORT_SCRIPTS_DIR="$$(cd "$(CURDIR)" && pwd -P)/packs/gents/grok_tui_port/scripts" \
	GENTS_GROK_PORT_PROMPT="$(GROK_PORT_PROMPT)" \
	GENTS_GROK_PORT_BASE_SHA="$$grok_port_base_sha" \
	GENTS_GROK_PORT_PR_BASE="$(GROK_PORT_PR_BASE)" \
	GENTS_GROK_PORT_BRANCH="$(GROK_PORT_BRANCH)" \
	GENTS_GROK_PORT_ENDPOINT_1="$(GROK_PORT_ENDPOINT_1)" \
	GENTS_GROK_PORT_MODEL="$(GROK_PORT_MODEL)" \
	GENTS_GROK_PORT_MIN_SURFACES="$(GROK_PORT_MIN_SURFACES)" \
	GENTS_GROK_PORT_MAX_SURFACES="$(GROK_PORT_MAX_SURFACES)" \
	GENTS_GROK_PORT_MAX_CONCURRENT_1="$(GROK_PORT_MAX_CONCURRENT_1)" \
	GENTS_GROK_PORT_ORCHESTRATOR_HOME="$(CURDIR)/packs/gents/grok_tui_port/runs/$$grok_port_job_id/home" \
	GENTS_GROK_PORT_ORCHESTRATOR_GRAPHQL="http://127.0.0.1:$(GROK_PORT_PORT)/api/v0/graphql" \
	GENTS_GROK_PORT_LIVE_HOME="$(CURDIR)/packs/gents/grok_tui_port/runs/$$grok_port_job_id/live-home" \
	GENTS_GROK_PORT_LIVE_GRAPHQL="http://127.0.0.1:$(GROK_PORT_LIVE_PORT)/api/v0/graphql" \
	GENTS_GROK_PORT_LIVE_SOCKET="$(CURDIR)/packs/gents/grok_tui_port/runs/$$grok_port_job_id/grok-leader.sock" \
	GENTS_GROK_PORT_CONTEXT_WINDOW="$(GROK_PORT_CONTEXT_WINDOW)" \
	GENTS_GROK_PORT_MAX_OUTPUT_TOKENS="$(GROK_PORT_MAX_OUTPUT_TOKENS)" \
	GENTS_GROK_PORT_MAX_TURNS="$(GROK_PORT_MAX_TURNS)" \
	GENTS_GROK_PORT_TEMPERATURE="$(GROK_PORT_TEMPERATURE)" \
	GENTS_GROK_PORT_RECON_TEMPERATURE="$(GROK_PORT_RECON_TEMPERATURE)" \
	GENTS_GROK_PORT_IMPLEMENT_TEMPERATURE="$(GROK_PORT_IMPLEMENT_TEMPERATURE)" \
	GENTS_GROK_PORT_REVIEW_TEMPERATURE="$(GROK_PORT_REVIEW_TEMPERATURE)" \
	GENTS_GROK_PORT_CODE_REVIEW_TEMPERATURE="$(GROK_PORT_CODE_REVIEW_TEMPERATURE)" \
	GENTS_GROK_PORT_TOP_P="$(GROK_PORT_TOP_P)" \
	GENTS_GROK_PORT_REASONING_EFFORT="$(GROK_PORT_REASONING_EFFORT)" \
	GENTS_GROK_PORT_CODE_REVIEW_REASONING_EFFORT="$(GROK_PORT_CODE_REVIEW_REASONING_EFFORT)" \
	GENTS_GROK_PORT_COMPACTION_THRESHOLD="$(GROK_PORT_COMPACTION_THRESHOLD)" \
	GENTS_GROK_PORT_DEADLINE_SECS="$(GROK_PORT_DEADLINE_SECS)" \
	GENTS_GROK_PORT_AWAIT_TIMEOUT_SECS="$(GROK_PORT_AWAIT_TIMEOUT_SECS)" \
	GENTS_GROK_PORT_STREAM_LIVENESS_SECS="$(GROK_PORT_STREAM_LIVENESS_SECS)" \
	GENTS_GROK_PORT_STREAM_BATCH_MS="$(GROK_PORT_STREAM_BATCH_MS)" \
	GENTS_GROK_PORT_RETRY_MAX_TRANSPORT="$(GROK_PORT_RETRY_MAX_TRANSPORT)" \
	GENTS_GROK_PORT_RETRY_MAX_RESAMPLE="$(GROK_PORT_RETRY_MAX_RESAMPLE)" \
	"$(GENTS)" pack scenario run "$(CURDIR)/packs/gents/grok_tui_port" \
		--with-pack "$$grok_port_dep/code_review.pack" \
		--http-port "$(GROK_PORT_PORT)" \
		--job-id "$$grok_port_job_id" \
		$(if $(GROK_PORT_KEEP_HOME),--keep-home,)

.PHONY: scan
scan:
	@test -d "$(SCAN_ROOT)" || { echo "set SCAN_ROOT to the repository to scan (got: $(SCAN_ROOT))" >&2; exit 2; }
	@scan_job_id="$(SCAN_JOB_ID)"; \
	if test -z "$$scan_job_id"; then scan_job_id="scan-$$(date -u +%Y%m%dT%H%M%SZ)-$$$$"; fi; \
	GENTS_SCAN_ROOT="$$(cd "$(SCAN_ROOT)" && pwd -P)" \
	"$(GENTS)" pack scenario run "$(CURDIR)/packs/gents/security_scan" \
		--http-port "$(SCAN_PORT)" \
		--job-id "$$scan_job_id" \
		$(if $(SCAN_KEEP_HOME),--keep-home,)

# The live campaign visualizer ships in the gents repository (apps/review-demo).
.PHONY: defend-page
defend-page:
	@test -f "$(GENTS_ROOT)/apps/review-demo/package.json" || { echo "set GENTS_ROOT to a gents checkout that holds apps/review-demo (got: $(GENTS_ROOT))" >&2; exit 2; }
	@echo "page     http://127.0.0.1:$(DEFENDING_PAGE_PORT)/?pack=defending"
	@echo "runtime  http://127.0.0.1:$(DEFENDING_PORT)"
	@DEMO_RUNTIME_PORT="$(DEFENDING_PORT)" DEMO_PAGE_PORT="$(DEFENDING_PAGE_PORT)" VITE_DEMO_MODE=defending $(NPM) --prefix "$(GENTS_ROOT)/apps/review-demo" run dev

# Live model runs of plugin packs. scenarios/<pack>_live/ is a scenario pack
# that depends on <pack> and adds the one trigger handing a seeded prompt to
# that pack's agent; its fixtures are copied into a fresh tool root per run.
# The pack under test is built from LIVE_PACK_DIR and pre-stored with
# --with-pack, as grok-port does; experiment.json's expect gates the run
# (completed stage, completed tool calls naming the written files).
#   make live LIVE_PACK=charts LIVE_ENDPOINT=http://workstation-1:8000/v1
LIVE_PACK ?=
LIVE_ENDPOINT ?= http://workstation-1:8000/v1
LIVE_MODEL ?= GLM-5.3-Flash-NVFP4
LIVE_PACK_DIR ?= $(CURDIR)/packs/gents/$(LIVE_PACK)
LIVE_PORT ?= 19198
LIVE_JOB_ID ?=
LIVE_KEEP_HOME ?=
LIVE_AWAIT_TIMEOUT_SECS ?=
LIVE_SCENARIO = $(CURDIR)/scenarios/$(LIVE_PACK)_live

.PHONY: live
live:
	@test -n "$(LIVE_PACK)" || { echo "set LIVE_PACK to one of: $(patsubst %_live,%,$(notdir $(wildcard scenarios/*_live)))" >&2; exit 2; }
	@test -f "$(LIVE_SCENARIO)/experiment.json" || { echo "no live scenario for $(LIVE_PACK) (scenarios/$(LIVE_PACK)_live)" >&2; exit 2; }
	@test -f "$(LIVE_PACK_DIR)/manifest.json" || { echo "LIVE_PACK_DIR has no manifest.json: $(LIVE_PACK_DIR)" >&2; exit 2; }
	@curl --fail --silent --show-error --max-time 10 "$(LIVE_ENDPOINT)/models" | python3 -c 'import json,sys; sys.exit(0 if sys.argv[1] in [row.get("id") for row in json.load(sys.stdin).get("data", [])] else 1)' "$(LIVE_MODEL)" || { echo "GLM preflight: $(LIVE_ENDPOINT)/models is unreachable or does not advertise $(LIVE_MODEL)" >&2; exit 2; }
	@live_job_id="$(LIVE_JOB_ID)"; \
	if test -z "$$live_job_id"; then live_job_id="$(LIVE_PACK)-live-$$(date -u +%Y%m%dT%H%M%SZ)-$$$$"; fi; \
	live_run="$(LIVE_SCENARIO)/runs/$$live_job_id"; \
	test ! -e "$$live_run" || { echo "run directory already exists; choose a new LIVE_JOB_ID: $$live_run" >&2; exit 2; }; \
	live_root="$$live_run/work"; \
	mkdir -p "$(LIVE_SCENARIO)/runs" && mkdir "$$live_run" "$$live_root" && cp -R "$(LIVE_SCENARIO)/fixtures/." "$$live_root/" || exit 2; \
	live_dep="$$(mktemp -d)" || exit 2; \
	trap 'rm -rf "$$live_dep"' EXIT; \
	"$(GENTS)" pack build "$(LIVE_PACK_DIR)" --out "$$live_dep/$(LIVE_PACK).pack" >/dev/null || exit 2; \
	GENTS_LIVE_ROOT="$$(cd "$$live_root" && pwd -P)" \
	GENTS_LIVE_ENDPOINT="$(LIVE_ENDPOINT)" \
	GENTS_LIVE_MODEL="$(LIVE_MODEL)" \
	$(if $(LIVE_AWAIT_TIMEOUT_SECS),GENTS_LIVE_AWAIT_TIMEOUT_SECS="$(LIVE_AWAIT_TIMEOUT_SECS)",) \
	"$(GENTS)" pack scenario run "$(LIVE_SCENARIO)" \
		--with-pack "$$live_dep/$(LIVE_PACK).pack" \
		--grant-authority \
		--http-port "$(LIVE_PORT)" \
		--job-id "$$live_job_id" \
		$(if $(LIVE_KEEP_HOME),--keep-home,)
