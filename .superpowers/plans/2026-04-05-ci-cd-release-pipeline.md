# CI/CD & Release Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users install Telepair with one command — prebuilt binary or Docker pull — backed by automated CI on every commit.

**Architecture:** Four new files: `.github/workflows/ci.yml` (quality gate), `.github/workflows/release.yml` (build + publish on tag), `Dockerfile` (multi-stage image), `.dockerignore` (build context filter). No changes to existing source code.

**Tech Stack:** GitHub Actions, Docker (multi-stage, QEMU for multi-arch), `cross` (Rust cross-compilation for ARM64), ghcr.io (container registry)

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `.dockerignore` | Create | Exclude build artifacts, docs, tests from Docker context |
| `Dockerfile` | Create | Multi-stage build: Rust backend + Node frontend + slim runtime |
| `.github/workflows/ci.yml` | Create | Quality gate: clippy, cargo test, vitest, type-check, Playwright E2E |
| `.github/workflows/release.yml` | Create | On `v*` tag: build 3 binaries + Docker image, create GitHub Release |

---

### Task 1: Create `.dockerignore`

**Files:**
- Create: `.dockerignore`

- [ ] **Step 1: Create the file**

```
target/
node_modules/
web/dist/
.git/
.github/
docs/
*.md
!README.md
.env
.DS_Store
```

- [ ] **Step 2: Verify syntax**

Run: `cat .dockerignore | head -20`
Expected: File contents displayed with no errors.

- [ ] **Step 3: Commit**

```bash
git add .dockerignore
git commit -s -m "chore: add .dockerignore"
```

---

### Task 2: Create `Dockerfile`

**Files:**
- Create: `Dockerfile`
- Reference: `docs/deployment.md:92-117` (existing template)

- [ ] **Step 1: Create the Dockerfile**

Based on the docs template, refined with proper layer caching and a non-root runtime user:

```dockerfile
# Stage 1: Build backend
FROM rust:1.85-bookworm AS backend
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY migrations/ migrations/
RUN cargo build --release

# Stage 2: Build frontend
FROM node:18-bookworm-slim AS frontend
WORKDIR /build/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ .
RUN npm run build

# Stage 3: Runtime
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=backend /build/target/release/telepair /usr/local/bin/
COPY --from=frontend /build/web/dist /opt/telepair/web/dist

EXPOSE 7700
VOLUME /root/.telepair

CMD ["telepair", "--web-dir", "/opt/telepair/web/dist"]
```

Key improvements over docs template:
- Frontend stage separates `npm ci` from `COPY web/` for better layer caching
- Uses `npm ci` instead of `npm install` for reproducible builds
- Explicit base image tags (`-bookworm`, `-bookworm-slim`)

- [ ] **Step 2: Verify Docker build locally**

Run: `docker build -t telepair:test .`
Expected: Successful 3-stage build. Final image based on `debian:bookworm-slim`.

Note: This step takes several minutes (Rust compilation). Skip if short on time — the CI/CD pipeline will validate this.

- [ ] **Step 3: Quick smoke test**

Run: `docker run --rm -d --name telepair-test -p 7701:7700 telepair:test && sleep 3 && curl -s http://localhost:7701/api/health && docker stop telepair-test`
Expected: Health check returns a response. Container starts and stops cleanly.

- [ ] **Step 4: Commit**

```bash
git add Dockerfile
git commit -s -m "chore: add Dockerfile for multi-stage production build"
```

---

### Task 3: Create CI Workflow

**Files:**
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Create the workflow file**

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1

jobs:
  rust:
    name: Rust (clippy + test)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy

      - uses: Swatinem/rust-cache@v2

      - name: Clippy
        run: cargo clippy --workspace -- -D warnings

      - name: Test
        run: cargo test --workspace

  frontend:
    name: Frontend (vitest + type-check)
    runs-on: ubuntu-latest
    defaults:
      run:
        working-directory: web
    steps:
      - uses: actions/checkout@v4

      - uses: actions/setup-node@v4
        with:
          node-version: 18
          cache: npm
          cache-dependency-path: web/package-lock.json

      - run: npm ci
      - run: npm test
      - run: npm run type-check

  e2e:
    name: E2E (Playwright)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2

      - name: Build backend
        run: cargo build

      - uses: actions/setup-node@v4
        with:
          node-version: 18
          cache: npm
          cache-dependency-path: web/package-lock.json

      - name: Build frontend
        working-directory: web
        run: npm ci && npm run build

      - name: Install Playwright Chromium
        working-directory: web
        run: npx playwright install --with-deps chromium

      - name: Run E2E tests
        working-directory: web
        run: npm run e2e
```

- [ ] **Step 2: Validate YAML syntax**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))"`
Expected: No errors (valid YAML).

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -s -m "ci: add CI workflow with Rust, frontend, and E2E checks"
```

---

### Task 4: Create Release Workflow

**Files:**
- Create: `.github/workflows/release.yml`

- [ ] **Step 1: Create the workflow file**

```yaml
name: Release

on:
  push:
    tags: ["v*"]

permissions:
  contents: write
  packages: write

env:
  CARGO_TERM_COLOR: always

jobs:
  # ── Build binaries ───────────────────────────────────────────────
  build:
    name: Build (${{ matrix.target }})
    runs-on: ${{ matrix.runner }}
    strategy:
      matrix:
        include:
          - target: x86_64-unknown-linux-gnu
            runner: ubuntu-latest
            use_cross: false
          - target: aarch64-unknown-linux-gnu
            runner: ubuntu-latest
            use_cross: true
          - target: aarch64-apple-darwin
            runner: macos-latest
            use_cross: false
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Install cross
        if: matrix.use_cross
        run: cargo install cross --locked

      - name: Build binary
        run: |
          if [ "${{ matrix.use_cross }}" = "true" ]; then
            cross build --release --target ${{ matrix.target }}
          else
            cargo build --release --target ${{ matrix.target }}
          fi

      - uses: actions/setup-node@v4
        with:
          node-version: 18
          cache: npm
          cache-dependency-path: web/package-lock.json

      - name: Build frontend
        working-directory: web
        run: npm ci && npm run build

      - name: Package tarball
        run: |
          ARCHIVE="telepair-${{ matrix.target }}.tar.gz"
          mkdir -p staging/web
          cp target/${{ matrix.target }}/release/telepair staging/
          cp -r web/dist staging/web/dist
          tar -czf "$ARCHIVE" -C staging .
          echo "ARCHIVE=$ARCHIVE" >> "$GITHUB_ENV"

      - uses: actions/upload-artifact@v4
        with:
          name: telepair-${{ matrix.target }}
          path: ${{ env.ARCHIVE }}

  # ── Docker image ─────────────────────────────────────────────────
  docker:
    name: Docker (multi-arch)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: docker/setup-qemu-action@v3
      - uses: docker/setup-buildx-action@v3

      - uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - name: Extract version from tag
        id: version
        run: echo "VERSION=${GITHUB_REF_NAME#v}" >> "$GITHUB_OUTPUT"

      - uses: docker/build-push-action@v6
        with:
          context: .
          platforms: linux/amd64,linux/arm64
          push: true
          tags: |
            ghcr.io/${{ github.repository }}:${{ steps.version.outputs.VERSION }}
            ghcr.io/${{ github.repository }}:latest

  # ── GitHub Release ───────────────────────────────────────────────
  release:
    name: GitHub Release
    needs: [build, docker]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: actions/download-artifact@v4
        with:
          path: artifacts
          merge-multiple: true

      - name: Create release
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          gh release create "$GITHUB_REF_NAME" \
            --title "$GITHUB_REF_NAME" \
            --generate-notes \
            artifacts/*.tar.gz
```

- [ ] **Step 2: Validate YAML syntax**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml'))"`
Expected: No errors (valid YAML).

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/release.yml
git commit -s -m "ci: add release workflow for binaries and Docker image"
```

---

### Task 5: Verify Everything Locally

- [ ] **Step 1: Run existing tests to confirm no regressions**

Run: `cargo test --workspace && cd web && npm test && npm run type-check && npm run e2e && cd ..`
Expected: All 68 Rust + 45 Vitest + 16 Playwright tests pass.

- [ ] **Step 2: Validate all workflow YAML files**

Run: `python3 -c "import yaml; [yaml.safe_load(open(f'.github/workflows/{f}')) for f in ['ci.yml','release.yml']]"`
Expected: No errors.

- [ ] **Step 3: Verify Dockerfile builds (optional, takes several minutes)**

Run: `docker build -t telepair:verify .`
Expected: Successful 3-stage build.

- [ ] **Step 4: Review all new files**

Run: `git diff --stat main`
Expected: 4 new files: `.dockerignore`, `Dockerfile`, `.github/workflows/ci.yml`, `.github/workflows/release.yml`
