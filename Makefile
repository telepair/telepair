# Telepair developer commands.
# Single entry point for fmt / lint / test / build across Rust + frontend.
# Run `make help` for the full target list.

WEB := web

.DEFAULT_GOAL := help
.PHONY: help install fmt fmt-check lint lint-rust lint-web \
        test test-rust test-web e2e \
        build build-rust build-web \
        all check dev clean

help: ## Show this help
	@awk 'BEGIN {FS = ":.*?## "; printf "Usage: make <target>\n\nTargets:\n"} \
	     /^[a-zA-Z_-]+:.*?## / {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}' \
	     $(MAKEFILE_LIST)

install: ## Install frontend deps + Playwright Chromium (one-time setup)
	cd $(WEB) && npm ci
	cd $(WEB) && npx playwright install chromium

# ---------- fmt ----------

fmt: ## Format Rust code
	cargo fmt --all

fmt-check: ## Check Rust formatting (no writes; CI-friendly)
	cargo fmt --all -- --check

# ---------- lint ----------

lint-rust: ## Clippy on the workspace, warnings as errors
	cargo clippy --workspace -- -D warnings

lint-web: ## TypeScript type-check
	cd $(WEB) && npm run type-check

lint: lint-rust lint-web ## Run all linters

# ---------- test ----------

test-rust: ## Rust workspace unit + integration tests
	cargo test --workspace

test-web: ## Frontend unit tests (vitest)
	cd $(WEB) && npm test

test: test-rust test-web ## Run all unit tests

e2e: build-rust build-web ## Playwright E2E (reuses the release binary + fresh frontend build)
	cd $(WEB) && npm run e2e

# ---------- build ----------

build-rust: ## Build Rust release binary
	cargo build --release

build-web: ## Build frontend production bundle
	cd $(WEB) && npm run build

build: build-rust build-web ## Build everything (release)

# ---------- aggregate / misc ----------

check: fmt-check lint test ## Run all CI gates locally (fmt + lint + test)

all: check build e2e ## Full pipeline: verify (fmt + lint + test + e2e) then build release artifacts

dev: ## Run backend in dev mode on :7700
	cargo run

clean: ## Remove build artifacts (Rust target + frontend dist/cache)
	cargo clean
	rm -rf $(WEB)/dist $(WEB)/node_modules/.vite
