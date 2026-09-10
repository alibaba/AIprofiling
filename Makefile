# AIProf top-level Makefile
#
# Common workflows. Prefer overriding the vars on the command line rather
# than editing this file, e.g.:
#     make deploy-ui DEPLOY_HOST=root@your-server
#     make deploy-ui SKIP_INSTALL=1     # if node_modules is already good

WEBAPP_DIR       ?= ui/webapp
DEPLOY_HOST      ?= root@your-server
DEPLOY_CONTAINER ?= aiprof-server
# Path inside the container that dashboardServer.js serves as the SPA.
# Must match UI_DIST_DIR / the express.static wiring in dashboardServer.js.
DEPLOY_REMOTE_DIST ?= /app/server/dashboard/public/webapp/dist
# Staging dir on the deploy host; rsync into this, then docker-cp into the
# container. Kept outside the container so a container restart doesn't lose it.
DEPLOY_STAGING   ?= /tmp/aiprof-dist-new

# `npm ci` gives a clean, lockfile-driven install; skip it if you know
# node_modules already matches package-lock.json (much faster during
# iterative dev).
NPM              ?= npm

.PHONY: help ui-install ui-build deploy-ui test-native

help:
	@echo "make test-native  — GPU-free C++ regression test for the vendored cuprof patches"
	@echo "make ui-build     — install deps (unless SKIP_INSTALL=1) and vite build"
	@echo "make deploy-ui    — ui-build, rsync dist to $(DEPLOY_HOST), docker cp into $(DEPLOY_CONTAINER)"
	@echo ""
	@echo "Overridable vars: WEBAPP_DIR DEPLOY_HOST DEPLOY_CONTAINER DEPLOY_REMOTE_DIST NPM SKIP_INSTALL"

ui-install:
ifeq ($(SKIP_INSTALL),1)
	@echo "SKIP_INSTALL=1 set — skipping npm ci"
else
	cd $(WEBAPP_DIR) && $(NPM) ci
endif

ui-build: ui-install
	cd $(WEBAPP_DIR) && $(NPM) run build
	@echo "Built bundle: $$(grep -o 'index-[A-Za-z0-9_-]*\.js' $(WEBAPP_DIR)/dist/index.html | head -1)"

# Native regression test for the vendored cuprof patch series. Needs only a
# C++11 compiler - trace_writer.cc has no CUDA/CUPTI dependency, so this runs on
# a laptop and in CI without a GPU. See test/native/ and
# agent/collection_framework/src/plugins/cuprof/VENDOR.md.
test-native:
	$(MAKE) -C test/native

# End-to-end deploy: builds locally, syncs to the host's staging dir, then
# copies index.html + assets/ into the running server container. Kept
# idempotent — safe to re-run after a failed step.
deploy-ui: ui-build
	@echo ">>> rsync -> $(DEPLOY_HOST):$(DEPLOY_STAGING)"
	rsync -av --delete $(WEBAPP_DIR)/dist/ $(DEPLOY_HOST):$(DEPLOY_STAGING)/
	@echo ">>> docker cp into $(DEPLOY_CONTAINER):$(DEPLOY_REMOTE_DIST)"
	ssh $(DEPLOY_HOST) "\
	    docker exec $(DEPLOY_CONTAINER) sh -c 'rm -rf $(DEPLOY_REMOTE_DIST)/assets $(DEPLOY_REMOTE_DIST)/index.html' && \
	    docker cp $(DEPLOY_STAGING)/index.html $(DEPLOY_CONTAINER):$(DEPLOY_REMOTE_DIST)/index.html && \
	    docker cp $(DEPLOY_STAGING)/assets     $(DEPLOY_CONTAINER):$(DEPLOY_REMOTE_DIST)/ && \
	    echo 'Deployed bundle:' \
	      \$$(docker exec $(DEPLOY_CONTAINER) grep -o 'index-[A-Za-z0-9_-]*\.js' $(DEPLOY_REMOTE_DIST)/index.html | head -1)"
