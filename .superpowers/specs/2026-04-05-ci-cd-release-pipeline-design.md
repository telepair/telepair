# CI/CD & Release Pipeline Design

## Goal

Enable users to install Telepair with a single command — prebuilt binary or Docker pull — backed by automated quality gates on every commit.

## Scope

- GitHub Actions CI workflow (quality gate on push/PR)
- GitHub Actions release workflow (build + publish on version tag)
- Dockerfile in repo root
- .dockerignore

Out of scope: docker-compose.yml (docs template sufficient), Makefile, version bumping automation.

## CI Workflow — `.github/workflows/ci.yml`

**Triggers:** push to `main`, all pull requests.

Three parallel jobs:

### Job: `rust`
- **Runner:** ubuntu-latest
- **Steps:** checkout, install Rust stable, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`
- **Caching:** `~/.cargo/registry` + `target/` keyed on `Cargo.lock` hash

### Job: `frontend`
- **Runner:** ubuntu-latest
- **Steps:** checkout, setup Node 18, `npm ci`, `npm test`, `npm run type-check`
- **Working directory:** `web/`

### Job: `e2e`
- **Runner:** ubuntu-latest
- **Steps:** checkout, install Rust stable, `cargo build`, setup Node 18, `npm ci && npm run build`, install Playwright Chromium, `npm run e2e`
- **Caching:** same as rust job + npm cache
- **Note:** Linux CI runners have PTY support; Playwright runs headless Chromium

## Release Workflow — `.github/workflows/release.yml`

**Trigger:** push tag matching `v*` (e.g., `v0.1.0`).

### Phase 1: Build Binaries (3 parallel jobs via matrix)

| Target | Runner | Toolchain |
|--------|--------|-----------|
| `x86_64-unknown-linux-gnu` | ubuntu-latest | native `cargo build --release` |
| `aarch64-unknown-linux-gnu` | ubuntu-latest | `cross build --release --target aarch64-unknown-linux-gnu` |
| `aarch64-apple-darwin` | macos-latest | native `cargo build --release` |

Each job:
1. Build the Rust binary
2. Build the frontend (`npm ci && npm run build` in `web/`)
3. Package into tarball: `telepair-{target}.tar.gz` containing:
   - `telepair` (binary)
   - `web/dist/` (frontend assets)
4. Upload tarball as workflow artifact

Frontend build runs on every matrix job to keep the workflow simple (no cross-job dependency for web assets). Frontend build is fast (~5s) so the duplication is acceptable.

### Phase 2: Docker Image (parallel with Phase 1)

- **Runner:** ubuntu-latest
- **Uses:** `docker/build-push-action` with QEMU for multi-arch
- **Platforms:** `linux/amd64`, `linux/arm64`
- **Registry:** `ghcr.io/telepair/telepair`
- **Tags:** `{version}` (e.g., `0.1.0`) + `latest`
- **Auth:** `GITHUB_TOKEN` (automatic, no extra secrets needed)

### Phase 3: GitHub Release (after Phase 1 + 2)

- Download all tarball artifacts
- Create GitHub Release from the tag
- Upload 3 tarballs as release assets
- Auto-generate release notes from commits since last tag

## Dockerfile

Multi-stage build, placed at repo root:

```
Stage 1: rust:1.85 — build backend binary
Stage 2: node:18 — build frontend
Stage 3: debian:bookworm-slim — runtime with only binary + web assets
```

Exposes port 7700. Data volume at `/root/.telepair`.

## .dockerignore

Exclude: `target/`, `node_modules/`, `.git/`, `web/dist/`, `*.md`, `docs/`, test artifacts.

## User Installation After This Ships

**Binary:**
```bash
# Download and extract
curl -fsSL https://github.com/telepair/telepair/releases/latest/download/telepair-x86_64-unknown-linux-gnu.tar.gz | tar xz
./telepair --web-dir web/dist
```

**Docker:**
```bash
docker run -d -p 7700:7700 -v telepair-data:/root/.telepair ghcr.io/telepair/telepair:latest
```

## Testing the Pipeline

Before tagging a real release:
1. Push to a branch and verify CI passes
2. Create a test tag (e.g., `v0.1.0-rc1`) to trigger the release workflow
3. Verify: GitHub Release created, 3 tarballs attached, Docker image pullable from ghcr.io
4. Delete the test release/tag if needed

## Files to Create/Modify

| File | Action |
|------|--------|
| `.github/workflows/ci.yml` | Create |
| `.github/workflows/release.yml` | Create |
| `Dockerfile` | Create (from docs template, refined) |
| `.dockerignore` | Create |
