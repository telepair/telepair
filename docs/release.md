# Release Flow

Standard release procedure for telepair. This is the authoritative
document; the `Release Flow` section in `CLAUDE.md` is a summary that
points here.

## Invariants

1. **Releases are tag-driven.** Pushing a `v*` tag triggers
   `.github/workflows/release.yml`, which builds tarballs, publishes
   the GHCR image, and creates the GitHub Release with notes extracted
   from `CHANGELOG.md`. Humans do not run `gh release create`.
2. **Tags are immutable.** Once `vX.Y.Z` is pushed, it is never
   deleted, moved, or force-updated. A broken release is fixed by
   shipping `vX.Y.Z+1`, not by retagging.
3. **`main` is linear.** PRs merge via **Rebase and merge** only. No
   merge commits, no squash. This keeps release tags pointing at
   semantically meaningful commits.
4. **`main` CI must be green before tagging.** The published bits
   stay in lockstep with a verified build.

## Procedure

### 1. Prepare the release branch

Cut a branch (e.g. `release/vX.Y.Z` or a feature branch that
culminates in a release). Make sure it is rebased on the latest
`main`.

### 2. Write the `prepare` commit

The last commit on the branch **must** be `chore(release): prepare
vX.Y.Z`. After rebase-merge, this becomes the tip of `main` and the
tag points at it directly.

If you need to land a bug fix after writing the prepare commit,
`git rebase -i` to move the prepare commit back to the end — do not
stack fixes on top of it.

The prepare commit changes exactly these files:

- `crates/telepair-{agent,cli,control,core,gateway}/Cargo.toml` —
  bump `version = "X.Y.Z"` on all five crates
- `Cargo.lock` — the corresponding version lines only (run
  `cargo build` to regenerate, then stage only the telepair-crate
  lines)
- `web/package.json` — bump `"version": "X.Y.Z"`
- `web/package-lock.json` — matching bump (run `npm install` in
  `web/`)
- `CHANGELOG.md`:
  - Rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD`
  - Add a new empty `## [Unreleased]` heading above it
  - Add a 3–5 line release preamble summarizing the theme of the
    release (what it does, why it matters, compatibility)
  - Keep the existing `### Added / Changed / Fixed / Removed`
    subsections as edited during the cycle
  - Add a `### Testing` subsection listing test-count deltas:
    ```
    ### Testing
    - cargo: <prev> → <new>
    - vitest: <prev> → <new>
    - playwright: <prev> → <new>
    ```

Commit body should state what was bumped and confirm that local
`make all` passed.

### 3. Local gate

```bash
make all
```

Must be green. `make all` runs `fmt-check`, `lint` (clippy + tsc),
`test` (cargo + vitest), `build` (release binary + web bundle), and
`e2e` (Playwright). No exceptions.

### 4. Merge to `main`

Open a PR. Use **Rebase and merge** in the GitHub UI. Verify that
`main`'s tip is now the `chore(release): prepare vX.Y.Z` commit.

### 5. Wait for `main` CI

```bash
gh run list --branch main --workflow ci.yml --limit 1
```

Must show `completed success` on the prepare commit. If CI is red,
fix it with a new commit on `main` (same rebase PR flow) — do **not**
tag a red commit.

### 6. Tag and push

```bash
git fetch origin main
git tag -s vX.Y.Z origin/main -m "vX.Y.Z

<one-line summary of the release theme>

See CHANGELOG.md for the full release notes."
git push origin vX.Y.Z
```

The tag message follows the v0.1.8 pattern: title line, blank, one
sentence, blank, pointer to CHANGELOG.

Signing (`-s`) is required. Git identity must be
`Liys <liys87x@gmail.com>`.

### 7. Verify the Release workflow

```bash
gh run list --workflow release.yml --limit 1
```

Must show `completed success`. This run does three things:

- builds three target tarballs (`x86_64-unknown-linux-gnu`,
  `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`)
- publishes the `ghcr.io/telepair/telepair:X.Y.Z` and `:latest`
  image (amd64 only until
  [telepair/telepair#1](https://github.com/telepair/telepair/issues/1)
  lands a native arm64 runner)
- calls `gh release create` as `github-actions[bot]` with notes
  extracted from the `## [X.Y.Z]` section of `CHANGELOG.md`

### 8. Post-release verification

```bash
gh release view vX.Y.Z
docker pull ghcr.io/telepair/telepair:X.Y.Z
```

Confirm:

- three `telepair-*.tar.gz` assets present
- release notes rendered from CHANGELOG (not auto-generated
  fallback)
- docker image pulls and `docker run --rm ghcr.io/telepair/telepair:X.Y.Z --help` succeeds

## Failure recovery

| Failure | Response |
|---|---|
| `make all` red locally | Fix at the root; never skip with `--no-verify` or partial targets. |
| `main` CI red before tag | Hotfix commit on `main` via rebase PR. Retry from step 5. |
| Release workflow red after tag | Do **not** delete the tag. Ship `vX.Y.Z+1` from step 1, fixing whatever broke. |
| Wrong CHANGELOG content after tag | Ship `vX.Y.Z+1` with a corrected CHANGELOG; the published release notes for `vX.Y.Z` stay frozen. |
| Tag pushed but Release workflow never ran | Check Actions tab; if the workflow was never triggered (rare), re-run it manually via the UI. The tag stays as is. |

## Rationale for key rules

- **Why rebase-only:** linear history lets `git bisect` work
  cleanly, makes release tags point at a single semantic commit, and
  keeps `git log --oneline` readable for release notes.
- **Why `prepare` must be last:** it gives the tag a natural target
  (`origin/main` after merge) and makes "what changed for this
  release" a single-commit diff.
- **Why the workflow creates the Release (not humans):** the
  workflow guarantees notes come from the committed `CHANGELOG.md`
  and that all three tarballs plus the docker image are available
  before the release appears. A human running `gh release create`
  bypasses those guarantees.
- **Why tags are immutable:** downstream consumers may have pinned
  `vX.Y.Z` or cached `:X.Y.Z` from GHCR. Moving a tag silently
  changes what they get, which is worse than shipping a new patch.
