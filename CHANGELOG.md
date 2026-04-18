# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Admins can force-close sessions they don't own (`DELETE
  /api/sessions/:id`) — useful after disabling the owner, so
  operators don't have to wait for the idle reaper. History rows
  stamp a new `CloseReason::Admin` (surfaced as "Closed by admin"
  on the dashboard) so the action isn't misattributed to the owner.

### Fixed

- Disabled-then-re-enabled user's session tab no longer lingers
  in zombie state. `PeerEvicted` now self-detects: `account_disabled`
  refreshes the identity and routes home (dashboard shows the
  pending-approval banner), while `token_rotated` drops credentials
  and routes to `/login`. Previously the WS 4001 close only set
  `status='error'` with no navigation, leaving a stale OWNER badge
  and dead keystrokes.
- `DELETE /api/sessions/:id` is idempotent on already-closed
  sessions — a UI double-click now returns 204 instead of a
  misleading 404. Concurrent close races (reaper beating the
  owner, two admin tabs) are also absorbed as success.
- Two concurrent `POST /api/sessions/:id/recording/start` no longer
  race past the active-recording check and leave an orphan `.cast`
  file alongside a `failed` recording row. Migration 003 adds a
  partial unique index on `(session_id) WHERE status = 'recording'`
  (sweeping any pre-existing orphans to `failed` first, idempotent
  on fresh DBs), and the storage layer maps the resulting
  `SQLITE_CONSTRAINT_UNIQUE` back to `409 Conflict`. Drop-in
  upgrade from 0.1.8.
- Recording playback now honors the asciicast header's recorded
  `width` / `height` instead of fitting to the container size. The
  player previously built xterm at 80×24 and then called `fit()`,
  so anonymous share-link viewers (who can't reach the metadata
  endpoint) saw cursor-positioning escapes land in the wrong
  column. `FitAddon` has been dropped from the playback page; live
  sessions still fit normally.
- Seeking in recording playback no longer empties the collab
  sidebar. `PlaybackEngine.seek()` previously replayed only output
  (`o`) and resize (`r`) events while walking from t=0, so the
  participant + chat panel silently went blank after any seek. It
  now dispatches every event type (`j` / `l` / `c` / `o` / `r` /
  …) up to the target, and pins `currentTime` to the requested
  target rather than the last replayed event's timestamp.
- `POST /api/recordings/:id/shares` now rejects `max_uses < 0` and
  non-RFC3339 or already-past `expires_at` at the API boundary
  with `400 Bad Request` and an actionable message. Previously
  both sailed through and produced unredeemable share links at
  consume time — negative `max_uses` is always-exhausted in the
  consume SQL, and a lexicographic `expires_at > now()` compare on
  non-RFC3339 input is nonsensical.
- `SessionHub::record_chat` now releases the `chat_history` mutex
  before acquiring `recording_tx`, so the two collab locks are
  never held nested. No existing path locks them in the reverse
  order today, but removing the overlap forecloses a deadlock the
  next refactor would otherwise walk into.
- WebSocket reconnect no longer double-fires `onclose` against the
  new socket. `WSClient.doConnect` now detaches the prior socket's
  handlers before overwriting `this.ws`, so a user-initiated
  "Reconnect" while the previous socket is still in `CLOSING`
  cannot schedule a ghost retry that races the fresh attempt.
- Unmounting the recording player mid-load no longer touches a
  disposed xterm or writes into unmounted Solid signals. The
  `initPlayer` flow now checks an abort latch after every `await`
  (metadata fetch, data download, microtask yield) and bails
  before initialising xterm if the component has been cleaned up.
- WebSocket forwarder now closes with `CLOSE_CODE_TRANSIENT` (4503)
  when the output or collab broadcast receivers drop messages (the
  `tokio::sync::broadcast::error::RecvError::Lagged` arm).
  Previously the forwarder just `warn!`ed and continued, which
  permanently desynced the client's VT state machine (missing a
  chunk of the output stream mid-escape makes every subsequent
  SGR/CSI sequence apply to the wrong region) and left presence /
  recording badges out of date. The new behaviour drives the
  client's existing transient-reconnect path, and the hub's
  scrollback + `SessionState` snapshot replay brings the terminal
  and collab UI back in sync on the new connection.
- `POST /api/sessions/{id}/recording/start` now logs `fail_recording`
  rollback failures at ERROR level instead of silently swallowing
  them. Previously a DB error on the rollback path left the
  recording row stuck in `status = 'recording'`, which in turn kept
  the `idx_recordings_one_active_per_session` partial unique index
  occupied — every future start for the same session returned 409
  with no log evidence of why. The rollback is still best-effort
  (the writer task will also finalise the row on its own), but
  operators now have the signal they need to intervene when both
  paths fail.
- `recordings.expires_at` and `recording_shares.expires_at` are now
  normalised to UTC (`+00:00`) before storage. The cleaner and the
  share-consume SQL both compare these columns lexicographically
  against `now_rfc3339()` (which always ends in `+00:00`), so a
  caller submitting `…+14:00` or `…-12:00` offsets used to land in
  the DB with a lex-incompatible offset — lex-greater values
  lingered past their declared wall-clock expiry (share stays
  redeemable indefinitely), lex-smaller ones got purged hours
  early. Both `RecordingService::create_share` and
  `RecordingService::set_expiry` now parse, shift to UTC, and
  re-emit the RFC3339 string before it reaches storage. Existing
  rows written under the old behaviour remain readable; only new
  writes are affected.
- Recording share tokens no longer travel in URL query strings.
  `GET /api/recordings/:id/data` now reads the raw token from the
  `X-Share-Token` request header (`?token=…` is no longer
  recognised), and share links emit `…/play#token=<raw>` URL
  fragments that the player captures on mount, scrubs from history
  via `replaceState`, and ships in the header on its one data
  fetch. Closes a quiet log-exfiltration path: default NGINX /
  ALB / CloudFront access-log formats capture the full request URI
  (query string included) but never arbitrary request headers, so
  a leaked log file can no longer replay a still-valid share link.
  Breaking for any out-of-tree caller that hand-crafted a
  `?token=…` URL — the owner-authored UI has been migrated.

### Changed

- Bumped `tokio-tungstenite` from 0.26 to 0.29 (dev-dependency,
  test WebSocket client only — no runtime impact). Picks up
  upstream `rustls` / `http` dep refreshes and lines the test
  client up with recent advisory fixes without changing the API
  surface our tests consume.
- Route components are now code-split via `lazy()`. Login and
  Register stay eager (first-paint path for unauthenticated
  users); every authenticated page ships as its own chunk. The
  Vite entry bundle drops from **754 kB / 195 kB gzip** to
  **29 kB / 8 kB gzip**, and xterm.js (340 kB / 86 kB gzip) no
  longer loads until a user opens a session or recording. This
  also clears the 500 kB single-chunk warning the build emitted
  on every CI run.

## [0.1.8] - 2026-04-17

Minor release centred on **session recording and playback**. Live
sessions can be captured as asciicast v2 `.cast` files on demand,
replayed in the browser with play / pause / seek / speed controls, and
shared via public signed links with configurable expiry and use
quotas. The subsystem ships with a streaming writer (1 s timer /
64 KiB threshold flush), a TTL background cleaner, full REST +
WebSocket + CLI surfaces, and a collaboration-aware playback UI that
replays participants and chat timelines in step with PTY output.

The release also lands a focused **security pass** on the auth and
share-link paths — per-IP throttling now covers `/api/auth/login` and
`/api/auth/verify` (previously only `/api/auth/register`), share-link
revocation URLs carry the SHA-256 digest instead of the raw token,
share-token validation runs as a single atomic `UPDATE … RETURNING`
closing a TOCTOU race on `max_uses`, and share revoke itself is now
scoped by `(recording_id, token_sha256)` so one owner cannot revoke
another owner's share links by computing their SHA-256 from the
(non-secret) raw URL.

Two new tables (`recordings`, `recording_shares`) are applied
idempotently at boot. Two new additive `ServerMessage` variants
(`RecordingStarted`, `RecordingStopped`) and two new audit event
types (`recording.started`, `recording.stopped`) land on the wire;
no existing message, column, or response shape changes. Drop-in
upgrade from 0.1.7.

### Added — Session recording

- New session recording subsystem: opt-in `.cast` (asciicast v2)
  capture of every active session, with a streaming writer that
  flushes on a 1 s timer or 64 KiB threshold and a TTL background
  task that purges expired recordings.
- REST surface for recordings: `POST /api/sessions/:id/recording/{start,stop}`,
  `GET /api/recordings`, `GET /api/recordings/:id`, `GET /api/recordings/:id/data`,
  `DELETE /api/recordings/:id`, `POST /api/recordings/:id/{keep,expire}`,
  plus share-link CRUD at `/api/recordings/:id/shares` and a public
  `?token=` access path on the `/data` endpoint.
- Browser playback page powered by xterm.js with play / pause /
  seek / speed controls, a participant + chat sidebar replayed in
  step with PTY output, and an event timeline of the recorded
  collab messages.
- WebSocket `RecordingStarted` / `RecordingStopped` server messages
  so every connected client surfaces the live indicator.
- New audit event types `recording.started` / `recording.stopped`
  written by the gateway and rendered in the admin audit timeline.
- New CLI flags `--recording-enabled`, `--recording-dir`,
  `--recording-ttl-days` (and matching `TELEPAIR_*` env vars), with
  `--recording-enabled=false` now actually rejecting `start_recording`
  requests at the HTTP layer.

### Security

- `POST /api/auth/login` and `POST /api/auth/verify` now share the
  per-IP throttle that already protected `/api/auth/register`,
  closing a credential-stuffing / OTP brute-force gap (the
  per-account 5-strike lockout and per-email OTP throttle were not
  enough on their own).
- `DELETE /api/recordings/:id/shares/:token_sha256` now takes the
  SHA-256 digest in the URL — putting the raw share token in the
  path leaked it into access logs and Referer headers.
- Share-token validation runs as a single `UPDATE … RETURNING` that
  checks expiry, remaining uses, and the requested recording id in
  one statement. The previous read-then-update sequence had a TOCTOU
  race on `max_uses` and let any holder of one recording's token
  burn quota by hitting another recording's URL.
- `DELETE /api/recordings/:id/shares/:token_sha256` now scopes the
  delete to `(recording_id, token_sha256)` at the storage layer
  instead of by digest alone. Before the fix, any owner who learned
  a share link (the raw token is the link itself — not a secret)
  could compute its SHA-256 and revoke it by hitting the endpoint
  under their own `recording_id` in the URL, enabling cross-owner
  share revocation. A mismatched `(recording_id, token_sha256)` pair
  now returns 404, making cross-owner revoke indistinguishable from
  a truly unknown digest.

### Fixed

- Recording id is now generated once in `RecordingService` and
  reused as the on-disk filename, the asciicast `telepair` block,
  and the DB primary key. Previously the storage layer minted its
  own id, so `RecordingRow.id` and `file_path` referred to
  different recordings and the TTL cleaner deleted the row but
  left the `.cast` file behind.
- Anonymous share playback works: `/recordings/:id/play` is now a
  dedicated route outside `AuthGuard`, so a recipient with a
  `?token=` link is not bounced to `/login`.
- Revoking a share link actually removes the row. The HTTP handler
  previously called `delete_share` with the SHA-256 digest from the
  UI, which the service then re-hashed before lookup; the
  `DELETE … WHERE token_sha256 = ?` saw nothing and returned 204
  while the link kept working.
- Recording dimensions reflect the actual PTY size: the start
  handler now reads `(cols, rows)` from the live session instead of
  hardcoding `80×24`, and the PTY I/O loop keeps the size in sync
  on every resize.
- Writer-task I/O failures release the hub's recording slot so the
  owner can stop and restart recording in the same session — the
  previous behaviour stranded the slot bound to a dead writer until
  the session itself ended.
- Player seek uses `term.reset()` instead of `term.clear()` so
  rewinding does not stack the old run into the scrollback buffer.
- `stop_recording` no longer holds the sessions `RwLock` read guard
  across the `.send().await` to the writer — under back-pressure
  the previous code blocked every concurrent write-lock acquirer
  for the duration of the flush handshake.
- `DELETE /api/recordings/:id` refuses (409) while the recording is
  still being captured (`status = 'recording'`). The previous code
  removed the file and DB row from under the live writer, leaving a
  dangling file handle, wedging `stop_recording` into a 404, and
  keeping the hub's recording slot occupied until the session ended.
  File removal no longer proceeds when the status check fails, and
  IO errors other than `NotFound` bubble up so the DB row survives
  for a retry instead of leaving an orphan on disk.
- The TTL background cleaner excludes `status = 'recording'` rows
  from its expiry scan, belt-and-suspenders defence against a bad
  `expires_at` write or wall-clock jump handing it an active row.
- Recording writer marks the capture `failed` instead of `completed`
  when any PTY / collab events were dropped under back-pressure.
  Previously `try_send` failures were swallowed and the final status
  was always `completed`, so viewers could load a capture with
  invisible gaps. The hub's recording slot now bundles the mpsc
  sender with a shared `AtomicU64` drop counter, and the writer
  reads it at finalisation — a non-zero count flips the status and
  logs the drop total at `error!` level.

### Testing

- Cargo test count: **372 → 416** (all green). New coverage:
  `recording_test` (asciicast v2 encode/decode round-trip,
  `Recording` lifecycle, share-token hashing), `recording_storage_test`
  (atomic `UPDATE … RETURNING` consumption, TTL cleaner row+file
  parity, cross-recording token quota isolation, cross-owner share-
  revoke rejection, delete-while-active guard), and gateway-level
  REST coverage for the recording endpoints and access-control gates.
- Vitest count: **185 → 194** (all green). New coverage:
  `playback.test.ts` for the PlaybackEngine asciicast v2 parser and
  play/pause/seek/speed state machine.
- Playwright count: **44 → 51** (all green). New spec
  `recording.spec.ts` exercises the owner start/stop flow, the live
  indicator broadcast to peers, the share-link dialog, and the
  anonymous `?token=` playback path outside `AuthGuard`.

## [0.1.7] - 2026-04-16

Patch release focused on **terminal personalization and out-of-tab
awareness**: the session page gains a settings panel for theme, font,
and cursor style (all persisted in `localStorage`), and a browser-
notification path surfaces chat messages and peer joins when the tab
is hidden. Web-only change — no backend, schema, or wire-format impact.
Drop-in upgrade from 0.1.6.

### Added — Terminal settings panel

- New `SettingsPanel` in the session topbar with a gear affordance,
  click-outside dismissal, and full keyboard access.
- Five bundled color themes (`github-dark`, `github-light`, `dracula`,
  `monokai`, `solarized-dark`) rendered as live swatches so the preview
  matches exactly what xterm.js will show.
- Font size stepper (10–24 px) and font-family picker across five
  bundled monospace families (`jetbrains-mono`, `fira-code`,
  `source-code-pro`, `cascadia-code`, `system-mono`).
- Cursor style selector (`block` / `underline` / `bar`) plus a blink
  toggle, both applied to the live xterm instance without reconnect.
- All settings persist in `localStorage` and restore on next page
  load; stored values are validated against the allowed set so a
  manually edited storage entry cannot poison the terminal with an
  unknown theme or font id.
- `settings` i18n namespace added to both `en.ts` and `zh.ts` with
  parity enforced by the existing dict-symmetry test.

### Added — Browser notifications

- Opt-in browser notifications fire on **incoming chat messages** and
  **peer-join** events while the tab is backgrounded (`document.hidden`),
  so collaborators are not missed when the terminal is not focused.
- Notifications are gated on user consent: the settings panel requests
  `Notification.requestPermission()` on first toggle and surfaces a
  `warning` toast when the browser denies the prompt, so the UI never
  silently flips back to "disabled" without telling the user why.
- No notifications are emitted while the tab has focus, and permission
  state is re-checked on every toggle so revoking at the browser level
  is picked up without a reload.

### Fixed

- The collaboration sidebar auto-closes on narrow viewports so the
  terminal stays visible on phones and split-pane layouts instead of
  being pushed off-screen by the participants / chat pane.
- The persisted `cursorBlink` value is restored after a viewer→operator
  unlock, closing a small regression where the cursor would stop
  blinking even though the stored setting said it should.
- Stored settings are validated against the allowed theme / font /
  cursor-style enums before being applied, so a stale or manually
  edited `localStorage` entry no longer leaves the terminal in an
  undefined visual state.

### Testing

- Vitest count: **159 → 185** (all green). New coverage:
  `notifications.test.ts` (permission flow, tab-hidden gating,
  unsupported-browser fallback) and `stores/settings.test.ts`
  (persistence, validation, cursor-style round-trip, reset).
- Cargo test count: **372** (unchanged — web-only release).
- Playwright count: **44** (unchanged).

## [0.1.6] - 2026-04-16

Patch release focused on **change-password API contract correctness** and
**cross-account session safety**. Wrong-password errors are now 400 (not
401), preventing the frontend from misinterpreting a typo as session
expiry. Password changes evict stale WebSocket connections and clear
client-side session state on token rotation, closing a cross-account
data leak when switching users in the same tab. The audit subsystem
gains a new event type and the export endpoint replaces a panic-prone
`unwrap()` with proper error handling.

No schema migration required. No wire-format changes. Drop-in upgrade
from 0.1.5.

### Fixed — Change-password API contract

- `POST /api/auth/change-password` now returns **400** when the current
  password is wrong, reserving **401** for invalid/missing bearer tokens.
  Previously both cases returned 401, causing the frontend's 401
  interceptor to treat a simple password typo as a session expiry and
  redirect to the login page.
- The frontend API client distinguishes the two status codes: 400
  surfaces an inline "wrong password" error; 401 triggers the existing
  session-expiry redirect.

### Fixed — Cross-account session leak

- Changing a password now evicts all WebSocket connections still carrying
  the old bearer token, so a stale tab cannot continue operating under
  the previous credential after a password rotation.
- The frontend auth store clears `sessionStore` on token change,
  preventing a same-tab account switch from leaking the previous user's
  session list into the new user's dashboard view.
- Dashboard re-fetches sessions after detecting a token change instead
  of rendering stale data from the previous account.

### Fixed — Audit consistency & robustness

- Frontend adds `AUTH_ADMIN_USER_CREATED` to the audit event type enum
  and label map so admin-created-user events render with a human-readable
  label instead of falling through as a raw string.
- `GET /api/admin/audit/export` replaces an `unwrap()` on the format
  query parameter with proper error handling, returning 500 instead of
  panicking on unexpected input.

### Testing

- Cargo test count: **341 → 372** (all green). New coverage:
  `change_password_evicts_ws_test` (password change triggers WS
  disconnect for old-token connections).
- Vitest count: **148 → 159** (all green). New coverage for API client
  400/401 branching, auth store token-change session clearing, and
  session store cross-account isolation.
- Playwright count: **44** (unchanged).

## [0.1.5] - 2026-04-15

Patch release focused on **security and resilience**: privileged actions
now evict live sessions, the register path is rate-limited, chat context
is preserved for late joiners, and a CLI admin surface unblocks ops
workflows that previously required the web UI. Three independent
correctness fixes (OTP bias, `$$` var-escape, structured API errors)
land alongside a deep-QA sweep that hardens edge cases surfaced by the
new test coverage.

No schema migration required. No wire-format changes. A one-line bump
of the workspace version is all that's needed to upgrade from 0.1.4.

### Added — Admin users CLI

- `telepair admin users list | show | enable | disable | approve |
  reset-password` operate directly against the SQLite store so admin
  workflows no longer require the web UI.
- Commands accept `--data-dir` (or `TELEPAIR_DATA_DIR`) so an operator
  can target a non-default deployment, and integration tests can point
  at a throwaway directory without touching the user's real data.
- The same `--data-dir` flag is honoured by the server binary.

### Added — Session eviction on privileged actions

- Disabling an account now evicts every live WebSocket owned by that
  user so a compromised session cannot continue past revocation.
- Rotating a user's token (via password change) evicts connections
  still carrying the old token on the next heartbeat.
- Admin token persistence trims whitespace so stray newlines from
  copy/paste no longer break auth.

### Added — Register rate limiting

- A token-bucket limiter guards `/api/register` and the `session_enabled`
  gate widens to block disabled accounts from hitting auth paths.
- `X-Forwarded-For` and `Forwarded` are trusted when keying the limiter
  so deployments behind a proxy key per-client, not per-proxy.
- Rate-limit, self-disable, missing-targets.yaml, and admin-token-prefix
  hints are polished to surface actionable messages.

### Added — Chat backlog replay

- Live sessions buffer a bounded window of chat messages and replay them
  to late joiners so participants who connect after a conversation
  started still see the recent context. The buffer is capped to prevent
  unbounded growth on long-running sessions.

### Added — Viewer role UX

- The operator/viewer toggle becomes a role dropdown with explicit
  choices, and the terminal locks input for viewers demoted mid-session
  so stale PTY focus cannot bypass the permission change until the next
  reconnect.
- A persistent "Viewer · read-only" badge renders over the terminal
  while the role is viewer so the lock is never ambiguous.

### Fixed — Security correctness

- **OTP bias**: OTP generation now uses rejection sampling instead of
  modulo, removing the bias that skewed the digit distribution.
- **Variable escape**: literal `$$` outside of `${VAR}` escape
  sequences is preserved so shell scripts templated through target
  rendering are no longer mangled.
- **Structured API errors**: `ApiError` is serialized as typed JSON
  end-to-end and the frontend consumes the typed envelope instead of
  scraping free-form strings from response bodies.

### Fixed — Invite lifecycle

- Revocations are idempotent: an already-revoked token returns success
  instead of a 404.
- Exhausted invites collapse into a single `"exhausted"` error rather
  than leaking internal attempt counts.
- `expires_in_secs` is accepted alongside `expires_in` for tooling that
  prefers the seconds-suffix convention.
- TTL resolution moves from the HTTP handler into the service layer so
  the CLI and REST paths share the same expiry rules.

### Fixed — Guest session scoping & UI

- Guest session listings are scoped by the caller's `scoped_session_id`
  so a guest cannot see sessions owned by a different scope.
- xterm refits on `visibilitychange` to recover from container resizes
  while the tab was backgrounded.

### Changed — Refactor & cleanup

- `targets.yaml` is parsed from already-read bytes instead of
  re-opening the file, closing a small TOCTOU window.
- Helper functions across cli, storage, gateway, and tests are deduped
  after they had drifted independently.
- The targets loader, CLI auth setup, and session UI helpers are
  simplified now that 0.1.5 features have settled.

### Fixed — Deep-QA hardening

- Gateway edge cases uncovered by QA replay traffic (invite error
  classification, mobile sidebar default, viewer badge stability).
- `readErrorMessage` is unexported from the web client so it stops
  leaking into callers that should go through the structured error
  path.

### Added — Test & CI coverage

- Agent crate covers PTY spawn failure, post-exit writes, and env
  isolation so regressions in the sandbox path fail loudly.
- Web typings are tightened and a single Playwright retry is enabled
  in CI to absorb genuine flakes without hiding real failures.
- The Playwright suite runs in a dedicated data dir so concurrent
  local runs no longer race over `~/.telepair`.

## [0.1.4] - 2026-04-14

Patch release focused on **admin maturity**: a dedicated system-info
page, searchable and paginated user management, auditable JSON/CSV
exports, and a validate-and-preview flow for virtual-target config.
Internally, account status tracking is split so "awaiting approval"
is no longer conflated with "disabled by admin", giving the audit
trail and admin UI an unambiguous vocabulary.

**One breaking API change**: `GET /api/admin/users` now returns a
wrapped object (`{users, total}`) to support pagination, instead of a
bare array. The `AdminUserInfo` row gains an `approval_state` field
(`"approved" | "pending"`). Clients built against 0.1.3 need to read
`users` out of the response. No WebSocket wire-format changes.

In-place upgrade from 0.1.3 works without wiping the database. The
`users.approval_state` column is added idempotently at boot, and
pre-0.1.4 pending signups — previously signalled by
`verified = TRUE AND session_enabled = FALSE` — are backfilled to
`approval_state = 'pending'` exactly once at migration time so they
do not silently promote to `approved` on first read.

### Added — Admin system-info page

- `GET /api/admin/system` — admin-only endpoint reporting version,
  build metadata, uptime, DB path/size, active session count, total
  user count, and `targets.yaml` path plus sha when configured.
- `SessionHub::active_count` and `AppState` startup metadata so the
  endpoint can report live runtime stats without scraping the DB.
- Frontend `AdminSystem` page (`/admin/system`) with a card grid
  layout. A new shared `AdminNav` component links Users / Targets /
  Audit / System across admin pages, and the dashboard topbar gains
  a "System" entry point.

### Added — Admin user search, filter, and pagination

- `GET /api/admin/users` accepts `q` (name/email substring),
  `status` (`enabled | disabled | pending`), `limit` (default 50,
  max 500), and `offset` query parameters. Response is now
  `{users, total}` — **breaking vs. v0.1.3**.
- `SqliteStorage::list_accounts_filtered` implements the filtered
  query with parameterised SQL (no string concatenation) and
  index-friendly ordering.
- Frontend `AdminUsers` page gains a debounced search box, a status
  dropdown, and load-more pagination matching the sessions list.

### Added — Audit log export

- `GET /api/admin/audit/export?format=json|csv` — streams the full
  audit log as downloadable JSON or RFC 4180-compliant CSV.
- Frontend `AdminAudit` page gains JSON and CSV download buttons
  alongside the existing filters.

### Added — Virtual target validate / preview flow

- `POST /api/admin/targets/validate` — parses a proposed
  `targets.yaml` payload, returns structured parse errors, and when
  valid reports an `added / removed / changed` diff against the
  currently loaded config. No state is mutated.
- `TargetEngine::diff` in `telepair-agent` produces the diff set
  consumed by the validate endpoint.
- Frontend `AdminTargets` page now runs validate before reload and
  shows a confirmation dialog with the preview diff. A sha256 of
  the validated bytes is carried into the subsequent reload request
  so the file cannot change between preview and apply (see Security
  below).

### Added — Approval-state separation

- `users.approval_state` column (`approved | pending`) tracks
  whether a self-serve signup has passed admin approval separately
  from `session_enabled`. Previously `session_enabled = FALSE`
  covered both "admin disabled an active user" and "awaiting
  approval", making admin UX and the audit trail ambiguous.
- `ApprovalState` enum surfaced in `AdminUserInfo` responses.
  Frontend `AdminUsers` now renders pending vs. disabled as
  distinct states with tailored actions.

### Changed

- **Breaking**: `GET /api/admin/users` response is now
  `{users: [...], total: N}` instead of a bare array.
- `AdminUserInfo` DTO gains `approval_state`.
- Dashboard admin section includes a "System" link.

### Fixed

- Session detail: "Invite" and "Close" buttons are hidden after a
  session ends, preventing 404-producing clicks against a closed
  session.
- `SqliteStorage::get_account`: on upgrade from a pre-0.1.4 DB
  without the `approval_state` column, pending signups
  (`verified = TRUE AND session_enabled = FALSE`) are now
  backfilled to `approval_state = 'pending'` exactly once at
  migration time, avoiding a silent promotion to `approved` on
  first read.
- `AdminTargets` e2e: toast regex updated for the validate-first
  reload flow.

### Security

- **CSV formula injection**: audit-export CSV now prefixes any
  string field starting with `=`, `+`, `-`, `@`, tab, or carriage
  return with a single quote, neutralising Excel/LibreOffice
  formula execution when an audit log is opened as a spreadsheet.
- **RFC 4180 quoting**: string fields in CSV exports are correctly
  quoted with double-quote doubling, preventing field corruption
  when an event value contains a comma, quote, or newline.
- **targets.yaml TOCTOU**: the validate endpoint returns a sha256
  of the parsed bytes, and the reload endpoint verifies this sha
  against the current file contents before applying, rejecting
  with `409 Conflict` on mismatch. This closes a window where an
  attacker with write access to `targets.yaml` could preview one
  config and apply another.

### Performance

- `system_info` uses `SELECT COUNT(*) FROM users` instead of
  loading all rows to count.
- Audit-export CSV pre-allocates a buffer sized for the known event
  count, avoiding per-row reallocations on large exports.
- `SessionHub` GC logic extracted into a shared helper, eliminating
  duplicated pruning across join / leave / close paths.

### Testing

- Cargo test count: **300 → 311** (all green). New coverage for
  `system_info` fields, user filtering and pagination, audit export
  formatting, CSV injection neutralisation, the validate endpoint,
  `TargetEngine::diff`, the sha-guarded reload path, and the
  `approval_state` backfill migration.
- Vitest count: **143 → 143** (stable, all green).
- Playwright e2e: `admin-targets` spec updated for the
  validate-first flow with preview dialog.

## [0.1.3] - 2026-04-14

Patch release adding **change-password flow**, **admin audit log page**,
**dynamic participant role changes**, and **dashboard pagination**. Auth
is hardened with server-side password-length validation and atomic
password+token rotation. The audit subsystem gains two new event types
(`auth.password_changed`, `participant.role_changed`) and a dedicated
admin page for browsing all system events with filters.

No breaking wire-format changes. One new `ServerMessage` variant
(`PeerRoleChanged`) is additive. In-place upgrade from 0.1.2 works
without wiping the database.

### Added — Change password

- `POST /api/auth/change-password` — authenticated endpoint that
  verifies the current password (defence in depth against session
  theft), hashes the new password with Argon2, and atomically
  rotates the bearer token in a single SQLite transaction so a crash
  between the two writes can never leave the old token valid after a
  password change. Returns the new token so the caller stays
  authenticated.
- `AuthService::change_password` in `telepair-control` with
  server-side `MIN_PASSWORD_LENGTH` (8 chars) validation applied to
  both registration and password change paths.
- Frontend `ChangePassword` page (`/change-password`) with
  current/new/confirm fields, client-side validation (length +
  mismatch), and automatic token swap on success. Accessible from a
  "Password" link in the dashboard topbar (email-auth users only).

### Added — Admin audit log page

- `GET /api/admin/audit` — admin-only endpoint returning the global
  audit log with optional filtering by time range (`since`/`until`),
  actor, event type, and session. Default 100 rows, capped at 500.
- Frontend `AdminAudit` page (`/admin/audit`) with a filterable,
  paginated event table. Filters include event-type dropdown and
  session-ID text input. Load-more pagination fetches the next page
  without losing filter state.
- Audit helpers (`eventLabel`, `formatTs`) extracted from
  `SessionDetailDialog` into `web/src/lib/audit.ts` so both the
  per-session timeline and the admin audit page share the same
  formatting logic.
- Dashboard topbar gains an "Admin · Audit" link for admin users.

### Added — Dynamic participant role changes

- `PUT /api/sessions/:id/participants/:user_id/role` — owner-only
  endpoint that changes a participant's role at runtime. The owner
  cannot change their own role or promote anyone to owner. Persists
  to DB, updates the hub's in-memory state, and broadcasts
  `PeerRoleChanged` to all connected clients.
- New `ServerMessage::PeerRoleChanged { user_id, new_role }` variant.
  The WS handler intercepts this for the affected connection and
  recalculates input permissions (`can_input`, serialized-mode gate)
  without requiring a reconnect. A broadcast-lag recovery path
  re-fetches the authoritative role from the hub to prevent a missed
  demotion from leaving stale permissions in effect.
- `ParticipantList` component gains role-toggle buttons for owners:
  clicking a participant's role badge toggles between Operator and
  Viewer with a single click. Non-owners see the static role label.

### Added — Dashboard improvements

- **Session pagination**: the sessions list now loads in pages with a
  "Load more" button, replacing the previous unbounded fetch.
- **OTP resend**: the registration verify step gains a "Resend code"
  button with a 60-second countdown matching the server-side rate
  limit.
- **Pending-approval status check**: users awaiting admin approval
  can click "Check status" to poll `whoami` and see an inline
  confirmation when approved, without a full page reload.

### Fixed

- `SqliteStorage::get_password_hash` now correctly returns `None`
  for users without a password hash (admin/CLI accounts). Previously
  the query used `query_scalar` which mapped a SQL `NULL` to an
  empty result rather than `Some(None)`, causing `change_password`
  to return a confusing "user not found" error instead of "this
  account does not use password authentication."

### Security

- Server-side password length validation (`MIN_PASSWORD_LENGTH = 8`)
  applied uniformly at registration and password change. Previously
  only the frontend enforced length, so a direct API caller could set
  a 1-character password.
- Atomic password+token rotation in `change_password_and_rotate_token`
  prevents a crash window where the old token remains valid after a
  password change.

### Testing

- Cargo test count: **284 → 300** (all green). New coverage:
  `change_password_success`, `change_password_wrong_current_rejects`,
  `change_password_no_password_hash_rejects`,
  `change_password_and_rotate_token_is_atomic`.
- Vitest count: **137 → 143** (all green). New coverage for audit
  helpers, auth store token rotation, and API client change-password
  path.

## [0.1.2] - 2026-04-13

Minor release adding **email-based self-serve registration** with OTP
verification, **user-owned targets**, and an **admin approval gate** for
new signups. The auth pipeline is hardened with login throttling, atomic
OTP lockout, idempotent registration, and a resilience fix that prevents
transient DB errors from killing live sessions.

No breaking wire-format changes. New tables (`pending_registrations`,
`user_targets`) and columns (`users.email`, `users.password_hash`,
`users.session_enabled`, `users.login_failed_count`,
`users.login_locked_until`, `sessions.user_target_id`) are applied
idempotently at boot, so an in-place upgrade from 0.1.1 works without
wiping the database.

### Added — Email authentication

- `POST /api/auth/register` — accepts email, password, and display
  name. Sends a 6-digit OTP via SMTP with a 15-minute TTL. Response
  is always `201` (or `503` if SMTP is not configured) to prevent
  email enumeration.
- `POST /api/auth/verify` — submits OTP code. On success, atomically
  consumes the pending row and inserts a new `users` entry with
  `session_enabled = FALSE` (awaiting admin approval).
- `POST /api/auth/login` — unified login accepting either
  `{token}` (existing admin path) or `{email, password}` (new email
  path).
- `AuthService` in `telepair-control`: Argon2 password hashing,
  pre-built SMTP transport reuse for connection pooling, 60-second
  rate limit on OTP sends, and detailed audit trail for every auth
  event. All failure responses collapse to "invalid email or code"
  to prevent enumeration.

### Added — User-owned targets

- `UserTargetService` in `telepair-control`: CRUD for per-user
  targets with non-blank validation, env-var expansion disabled
  (prevents leaking server secrets via `${SMTP_PASS}`), and a
  referential guard that rejects update/delete while an active
  session references the target.
- REST endpoints: `POST /api/user-targets`,
  `GET /api/user-targets/{id}`, `PUT /api/user-targets/{id}`,
  `DELETE /api/user-targets/{id}`. Guest users rejected with 403.
- `user_targets` table: id (nanoid PK), user_id (FK), name, display,
  command, args/env/tags (JSON), timestamps. Unique constraint on
  `(user_id, name)`.
- Sessions can now reference a `user_target_id`. The WS PTY spawn
  path resolves user targets via `UserTargetService::resolve_by_id()`
  when the global target lookup misses.
- Frontend `UserTargetDrawer` component: modal form for
  creating/editing user targets with name, display, command, args,
  env (KEY=value lines), and tags. Delete with confirmation in edit
  mode; conflict errors shown when an active session blocks mutation.

### Added — Pending-registration approval gate

- `users.session_enabled` column (default `TRUE` for existing users,
  `FALSE` for new email registrations) gates session creation and
  WebSocket attach. Admins bypass the gate.
- `GET /api/admin/users` — lists all non-guest accounts with email,
  role, and session-enabled status.
- `POST /api/admin/users/{id}/enable` /
  `POST /api/admin/users/{id}/disable` — admin-only toggles for the
  `session_enabled` bit.
- Frontend `AdminUsers` page: lists registered users with enable/
  disable controls. Accessible from the dashboard admin menu.

### Added — Frontend

- **Register page**: two-step flow (email/password/display-name form
  → 6-digit OTP input with numeric formatting).
- **Login page**: now supports email + password in addition to token.
- **Auth store**: new `emailRegister()`, `emailVerifyOtp()`,
  `emailLogin()` methods with `validating()` and `errorKey()`
  signals for UI feedback.
- **Protocol types**: `AdminUserInfo`, `UserTargetInfo` types
  matching backend DTOs.
- `admin_users` and `user_targets` i18n namespaces added to both
  `en.ts` and `zh.ts`.

### Added — Virtual target validation

- `telepair-agent`: target config validation now rejects blank name,
  display, or command fields at parse time instead of silently
  accepting empty strings.

### Fixed

- **Login throttle with audit trail.** After 5 failed login attempts,
  the user is locked out for 15 minutes. Each attempt emits an
  `auth.login_failed` audit event with reason (`unknown_email`,
  `bad_password`, `locked`), remaining attempts, and `locked_until`
  timestamp. Successful login clears the counter. Lockout check
  runs before hash verification to prevent timing side-channels on
  locked rows.
- **Atomic OTP lockout.** `verify_pending_registration()` uses a
  CAS-based SQL `UPDATE` with `otp_failure_count < 5` guard to
  prevent concurrent wrong codes from racing past the lockout
  threshold.
- **Idempotent registration with orphan OTP rollback.** If SMTP
  fails after `upsert_pending_registration()` writes the row,
  `delete_pending_registration()` is called with compare-and-delete
  (email + OTP code) to prevent the user from being locked behind
  the 60-second rate limit. A concurrent re-register that overwrote
  the row is not affected.
- **Session-ref mutation guard.** `update_user_target()` and
  `delete_user_target()` check for active sessions referencing the
  target before proceeding. Returns `Conflict` if blocked;
  preserves `PermissionDenied` when caller doesn't own the target
  to prevent information leakage.
- **Transient DB error resilience.** The WS session reaper no longer
  kills live sessions on transient SQLite errors (e.g.
  `SQLITE_BUSY` during WAL contention). Reaper retries instead of
  propagating the exception.
- **Concurrent OTP race.** `upsert_pending_registration()` atomically
  overwrites hash + OTP on re-register, preventing a window where
  two concurrent registrations could leave inconsistent state.
- **Target namespace split.** User targets use nanoid as `id` (primary
  key in responses); `name` is user-specified as part of the unique
  constraint with `user_id`. Global targets continue using target
  name as identifier. `Session.target_name` stores the original
  target name at creation time for both types.
- **Login error clearing on tab switch.** Auth store clears error
  state when navigating between Login/Register tabs, preventing
  stale error messages from persisting across tab switches.
- **Env-var expansion disabled for user targets.** User-supplied
  target configs no longer expand process environment variables,
  closing a path where `${DATABASE_URL}` or `${SMTP_PASS}` in a
  user target command could exfiltrate server secrets.

### Refactoring

- **SMTP transport reuse.** `AuthService` builds the SMTP
  `AsyncSmtpTransport` once in `new()` and shares it via `Arc`
  across all calls, avoiding TLS re-establishment on every email.
- **Storage helpers.** `parse_optional_datetime()` and
  `generate_token()` extracted as reusable utilities for timestamp
  parsing and bearer token generation.

### Chore

- **Makefile root guard.** Added check to prevent running `make`
  outside the repo root.
- **Frontend dependency upgrades.** All SolidJS and development
  dependencies updated to latest versions.

### New error variants

- `Conflict` (409), `RateLimited` (429), `ServiceUnavailable` (503)
  added to `telepair-core::Error` with automatic HTTP status mapping.

### New audit events

- `auth.register_rejected` — rate-limited or already-registered
  silent no-op.
- `auth.register_completed` — OTP verification succeeded, user row
  materialized.
- `auth.verify_failed` — OTP verification failed (bad code, locked,
  expired); includes remaining attempts.
- `auth.login_failed` — password login failed; includes reason,
  remaining attempts, and `locked_until`.
- `auth.user_enabled` / `auth.user_disabled` — admin toggled
  `session_enabled`.
- `auth.session_access_denied` — user with `session_enabled = false`
  attempted to create or join a session.

### Testing

- Cargo test count: **209 → 284** (all green). New coverage:
  `email_registration_test` (pending row lifecycle, OTP lockout,
  rate limiting, display-name collisions, email case-insensitivity),
  `admin_users_test` (enable/disable gate),
  `session_enabled_gate_test` (HTTP and WS rejection flows),
  expanded `http_test` (user-target CRUD, auth endpoints).
- Vitest count: **134 → 137**. Updated API client tests for new
  auth and user-target endpoints.
- Playwright count: **43** (unchanged). Existing E2E specs updated
  for login tab switch behavior and heading matchers.

## [0.1.1] - 2026-04-10

Patch release focused on **pulling business rules back into `telepair-control`**
and adding three product surfaces: invite management, session history, and an
admin-only target management page. No breaking wire-format changes; the
schema gains a `sessions.closed_reason` column and a new `audit_events` table,
both applied idempotently at boot so an in-place upgrade from 0.1.0 works
without `rm -rf ~/.telepair/telepair.db`.

The tail of the release cycle also landed a concentrated pass of concurrency
hardening: every invite-redeem path now goes through an atomic storage call,
every target-hot-reload path now reads from the live session hub instead of
stale DB rows, and the session hub itself reserves target slots across the
create-to-attach gap so a burst of parallel launches can no longer orphan a
row. See the expanded **Fixed** section below for the full list.

### Added — Control-layer cleanup

- `SessionService` is now a real service layer: `get_session`,
  `require_owner`, `list_participants`, `list_sessions_for_user`,
  `active_session_counts_per_target`, and a `close_session(reason)`
  overload that carries `CloseReason` through to the audit log. The
  previous `SessionService::storage()` escape hatch is gone — every
  gateway handler now routes through a service method, and
  `rg "\.storage\(\)" crates/telepair-gateway/src crates/telepair-control/src`
  returns zero production-code hits. Test fixtures use a dedicated
  `TestFixtures` helper on `AppState` so the word `.storage()` stays
  out of `src/` entirely.
- New `InviteService` in `telepair-control` owns create / redeem /
  list / revoke. The 80-line `redeem_invite` handler in
  `telepair-gateway::http` collapses into a single
  `state.invites.redeem(...)` call — all the `MAX_INVITE_USES` /
  `MAX_TTL` / `expires_at` resolution, the cross-session scoped-guest
  check, and the guest mint-on-success live inside the service.
- `SessionHub` stops holding `Arc<SqliteStorage>` directly. The reaper
  and close-session paths call `SessionService::close_session` so
  every closure — owner, reaper, startup sweep, error cleanup — emits
  a `CloseReason` into the audit log from one place.

### Added — Invite management

- `GET /api/sessions/{id}/invites` — owner-only list of invites for a
  session. Returns an `InviteSummary` per row (`token_prefix` is the
  first 8 chars of the sha, never the raw token) with role, max_uses,
  used_count, remaining_uses, expires_at, and created_at. Exhausted
  and expired invites are still returned so the owner can see the
  full history instead of a UI that "forgets" the ones it hides.
- `DELETE /api/sessions/{id}/invites/{token_sha256}` — owner-only
  hard delete. Revoking an invite immediately breaks its redeem path
  (`POST /api/invite/redeem` returns 400 `invalid_token` on the
  revoked hash) so a shared link can be disarmed before it is used.
- Frontend: the existing `InviteDialog` grows an **Active invites**
  section above the create form. Each row shows a role badge, a
  `remaining / total` counter, an expiry countdown, and a two-step
  `Revoke` button matching the existing close-session UX. `/invite/
  revoked` refreshes the list inline so the owner sees the update
  without a page reload.
- Playwright: `invite-management.spec.ts` covers the list render,
  the revoke confirmation, and a second test that verifies the
  revoked token returns 400 on the next redeem attempt.

### Added — Session history + `closed_reason`

- `sessions.closed_reason` column (TEXT, nullable) carries a tagged
  `CloseReason` — `Owner`, `Reaper`, `Startup`, or `Error` — written
  in the same transaction that flips the session status. Populated
  retroactively for rows that existed before the column was added:
  NULL renders as "Closed" without a reason chip.
- `GET /api/sessions?status=active|closed|all` filters the list by
  status. The existing `list_sessions_for_user` returns both owned
  and joined sessions for regular users; admin callers see every
  session in the workspace. The response includes `closed_reason`
  and `closed_at` so the frontend can render the status chip and
  duration without a second fetch.
- Dashboard: Sessions section grows an `Active / Closed / All` tab
  row with a row-level status chip, close-reason chip, duration
  column, and participant count. Clicking a closed row opens a
  modal audit timeline (see next section) instead of racing into a
  dead session page. The active tab is URL-synced via
  `?status=<filter>` so deep links and the browser back button
  behave consistently.
- Playwright: `history.spec.ts` walks the full lifecycle — create,
  close via the UI two-step confirm, verify the row lands on the
  Closed tab with `Closed by owner`, then flip to All and confirm
  the row is still there. The test also regression-guards against
  the "All filter drops closed rows" bug path.

### Added — Audit log (`audit_events` table + CLI + in-app timeline)

- New `audit_events` table: `id`, `ts`, `actor_id`, `actor_name`
  (denormalized snapshot so a later user rename still shows the
  right author), `event_type`, `session_id` (nullable), and a
  `detail` JSON blob. Four indexes — time, session, actor, type —
  keep the CLI filter queries cheap even with tens of thousands of
  rows. High-rate events (chat, cursor, PTY bytes) are **not**
  audited: the table would explode and none of them carry
  security-meaningful state.
- Event taxonomy (fixed scope for 0.1.1):
  `auth.login_success`, `auth.login_failed`,
  `session.created`, `session.closed`,
  `participant.joined`, `participant.left`,
  `invite.minted`, `invite.redeemed`, `invite.revoked`,
  `target.access_denied`, `target.reloaded`.
- `telepair admin audit` CLI subcommand with `--last <DURATION>`
  (e.g. `1h`, `24h`, `7d`), `--session <ID>`, `--actor <NAME_OR_ID>`,
  `--type <EVENT_TYPE>` (repeatable), `--format table|json`, and
  `--limit <N>`. The table output is aligned for quick scanning in a
  terminal; the JSON output is machine-parseable for scripted
  follow-up queries.
- `GET /api/sessions/{id}/audit` returns the per-session audit
  timeline (admin or session owner only). The Dashboard's closed-row
  click target now opens an **in-app audit timeline dialog** showing
  the create / join / leave / close events with their actor names
  and JSON details side-by-side.
- `GET /api/auth/whoami` endpoint surfaces the caller's identity
  (`user_id`, `name`, `is_admin`, `is_guest`) so the frontend can
  decide whether to render admin-only UI without reverse-engineering
  it from other endpoint responses.

### Added — Admin target management page

- `GET /api/admin/targets` returns every loaded target with its
  full config (command, args, shell, tags, admin_only, env keys),
  **augmented with a per-target `active_sessions` count**. Env
  variable values are redacted at the service boundary — each key
  appears as `{"key": "PGPASSWORD", "set": true}` so the admin
  can see which vars are wired without ever leaking the resolved
  value over the wire.
- `POST /api/admin/targets/reload` re-parses `~/.telepair/targets
  .yaml` into a new `TargetEngine`, atomically swaps the in-memory
  pointer via `arc_swap::ArcSwap`, and emits a `target.reloaded`
  audit event with the yaml path and the new target count. Parse
  errors return `400 {"reason": "parse_error", "message": "..."}`
  and the old engine stays in place — a malformed edit never
  poisons a running server. If the operator started the process
  without a `targets.yaml` path, reload returns
  `400 {"reason": "no_targets_path", ...}` instead of silently
  doing nothing.
- New admin-only page at `/admin/targets` (route-guarded by
  `AdminGuard` on top of `AuthGuard`) lists every target as a
  detail card: display name, mono id, kind badge, admin-only
  badge, command preview, args list, shell, tags, and an env
  grid with hollow chips for unset keys and filled chips for
  set ones. The reload button lives in the topbar with a toast
  spinner and branch-specific error copy for parse-error and
  no-path failures.
- Each card's footer carries a **clickable `N active session(s)`
  button** that deep-links into the Dashboard Sessions tab
  pre-filtered by target (`/?target=<name>&status=active`). The
  dashboard reads the query params on mount and on change, so the
  browser back button and URL sharing both round-trip cleanly.
- Dashboard topbar grows an admin-only gear link to `/admin/targets`.
  The link is gated on a three-state `currentUserIsAdmin()` signal
  (`null` while whoami is pending, then `true` / `false`) so the
  admin UI never flashes to a guest during the first paint.
- `admin_targets` i18n namespace (24 keys) added symmetrically to
  `en.ts` and `zh.ts`; the existing dict-symmetry test enforces
  parity on every push.
- **Structured reload-failure banner.** When a hot reload is
  rejected because the new `targets.yaml` would drop targets that
  still have live sessions, `/api/admin/targets/reload` returns
  `400 {"reason": "still_referenced", "targets": [{"target": "...",
  "active_sessions": N}, ...]}`. The admin page parses the payload
  via the new `parseReloadError` seam (pinned by unit tests) and
  renders a persistent error banner listing every blocking target
  with its live session count, so the operator can close exactly
  the sessions that are holding the reload back instead of staring
  at a generic toast with raw JSON inside it.

### Added — Control-layer concurrency primitives

- New `Storage::redeem_invite` trait method that runs the "check
  not-exhausted / not-expired / not-for-closed-session, increment
  `used_count`, return the session row" sequence inside a single
  atomic transaction. The old multi-statement path was the root of
  the invite-redeem TOCTOU window; see the corresponding entries in
  **Fixed** below.
- New `Storage::list_all_sessions` trait method and
  `SessionService::list_sessions_visible_to` dispatch so the
  gateway's `GET /api/sessions` handler can branch on
  `User::is_admin`: admins get the workspace-wide view (which
  makes the admin targets page's "N active sessions" deep link
  resolve to a non-empty page), regular users keep the
  owner-plus-participant scope unchanged. The two SQL strings are
  kept deliberately separate so the "non-admin cannot see admin
  rows" invariant is visible at the trait boundary rather than
  buried inside an `OR ? = 1` branch.

### Fixed

- Startup session sweep now routes through `SessionService`, so the
  `session.closed` audit entries for sessions killed at startup carry
  `CloseReason::Startup` instead of a missing/generic reason. (Caught
  during Stage 5 code review.)
- Non-owner participants opening a closed row in their history list
  no longer hit a confusing in-dialog 403 from `/audit`. The
  Dashboard's owner gate now compares `session.owner_id` against
  `auth.currentUserId()` and only opens the audit dialog for owned
  sessions — joined sessions link to their (closed) session page
  instead.
- `sessions.list_sessions_for_user` now honors its `include_closed`
  flag for both the owner rows and the joined rows. Previously the
  joined branch dropped the filter, so the Closed tab returned
  sessions the user had *joined* but not *owned*, regardless of the
  filter — a quiet regression caught mid-Stage-5.

#### Concurrency & race conditions

- **Invite redeem TOCTOU closed.** Both redeem paths — the long
  path that mints a fresh guest and the short path that reuses an
  existing identity — now delegate to the new atomic
  `Storage::redeem_invite`. Previously two parallel redeems of a
  `max_uses: 1` invite could both read `used_count = 0`, both
  write `used_count = 1`, and both succeed — minting two guests
  off a single-use link. The short path additionally now runs its
  "already a participant?" guard inside the same transaction via
  an atomic JOIN so a racing join + redeem can no longer land the
  user in a half-committed state.
- **Invite TTL ceiling enforced on both input paths.**
  `MAX_INVITE_TTL_MINUTES` was only clamped on the relative
  `expires_in_minutes` input; a direct-API caller passing an
  absolute `expires_at` weeks or months in the future silently
  bypassed the ceiling. Out-of-range absolute timestamps now
  reject with `400 invalid_input`. Policy is intentionally
  asymmetric: relative TTL still clamps (a slider overshoot is a
  benign UX mistake), absolute timestamps reject so the server
  never silently rewrites an explicit wall-clock pick.
- **Target reservation across the create-to-attach gap.** A burst
  of parallel `POST /api/sessions` calls against the same target
  could race between "session row inserted" and "PTY attached in
  hub", occasionally leaving an orphan row if the second step
  failed. The hub now reserves the target slot atomically at row
  insert time and releases it on the orphan cleanup path, so the
  reservation cannot leak even when the attach step errors out.
- **Target hot-reload now gates on the live session hub**, not on
  whatever `sessions` rows happen to exist in the DB at poll time.
  A reload that would drop a target with live sessions is now
  rejected with the structured `still_referenced` payload
  described above, and the stale-row path that previously let
  reloads through while an in-flight session existed is closed.
- **Startup sweep row-scoping.** `close_stale_sessions` now
  scopes its participant-cleanup `UPDATE` by the row id of the
  session being closed, not by a broader match — a pre-existing
  bug that could touch participants of unrelated sessions during
  startup recovery.
- **WS session lookup errors classified correctly.** The WS
  handler previously funnelled every `list_participants` storage
  error into a single `session_not_found` close code, hiding real
  storage faults behind a user-level "session doesn't exist" lie.
  Storage errors now propagate as `internal_error`, only a true
  missing-row result maps to `session_not_found`.
- **Idempotent existing-identity redeem under race.** The short
  redeem path now tolerates a concurrent "already a participant"
  insert from a second tab: the second redeem no longer returns a
  constraint error, it folds into the existing participant row.

#### Frontend state management

- **Identity cache invalidation on token swap.** The auth store
  cached `whoami` results keyed by nothing — switching tokens in
  the same tab left the old identity in memory until a full page
  reload. The cache is now keyed by token and cleared on swap, so
  logging out and back in as a different user takes effect
  immediately.
- **AdminGuard recovery.** If the first `whoami` call failed (e.g.
  a transient network blip during first paint), `AdminGuard`
  latched into a "not admin" state forever. The guard now reads a
  three-state `identityChecked` signal and retries once the real
  identity lands, so admins are no longer locked out of
  `/admin/targets` by a flaky first request.
- **Session-exit dispatch by credential, not session role.** The
  dashboard's session-exit event fires by credential (so all tabs
  of the same user react), instead of keying on the session role
  (which made a viewer tab miss the exit event that an
  operator tab produced). Fixes a stuck "connecting…" spinner on
  a second tab after the first tab logged out.
- **Optimistic session-list insert gated by filter.** Creating a
  new session from a target card used to optimistically prepend
  the row to the list regardless of the active `target_name`
  filter, so a session launched from target A would briefly
  flash in the list of target B before the refetch corrected it.
  The optimistic insert now runs only when the filter matches.
- **Invite dialog pending-revoke reset.** Closing and reopening
  the invite dialog no longer carries a half-filled pending-revoke
  state across dialog lifetimes; dismissing the dialog now
  clears the two-step confirm so the next open starts clean.

### Security

- **Env-value redaction at the service boundary.** The admin targets
  endpoint intentionally never returns resolved env variable values
  — the JSON row only carries the key name and a `set: bool`. This
  keeps the "who can write targets.yaml can exfiltrate env vars"
  trust boundary visible to operators without a UI escape hatch
  that would undermine it.
- Revoked invite tokens now hard-delete from `invite_tokens`
  instead of soft-marking, so a leaked row cannot be un-revoked by
  a future SQL-level edit, and the next redeem attempt surfaces as
  `invalid_token` rather than "exhausted" (which previously could
  be confused with a legitimately-used invite).

### Testing

- Cargo test count: **107 → 209** (all green). New coverage:
  `invite_service_test` (expanded with the TOCTOU and TTL-ceiling
  regressions), `session_service_test` (expanded with the
  admin-wide visibility dispatch), `invite_management_test`,
  `session_history_test`, `session_audit_api_test`,
  `whoami_api_test`, `audit_test`, `admin_targets_test`,
  `upgrade_test`, expanded `invite_storage_test` and the
  gateway-level `http_test` / `ws_test` cases for the hub
  reservation, reload-guard, and ws error-classification fixes.
- Vitest count: **112 → 134**. New unit tests cover the admin
  targets route guard state machine, the invite-management list
  reducer, the session-history filter helpers, the
  `parseReloadError` parser seam for the structured reload banner,
  the auth store's token-keyed identity cache, and the
  session-exit dispatch key. Two dead `readCurrentToken` tests
  removed after the REST/WS token-source unification.
- Playwright count: **36 → 43**. New specs: `invite-management.spec
  .ts`, `history.spec.ts`, `session-audit.spec.ts`,
  `admin-targets.spec.ts`. Each covers at least one happy path
  and one failure / empty-state branch.

## [0.1.0] - 2026-04-08

Initial public release of **telepair** — a web-based terminal collaboration
tool that lets multiple users share one live PTY through the browser, with
role-based permissions, invite links, and real-time chat.

### Added — Backend (Rust workspace)

- Cargo workspace with 5 crates (`telepair-core`, `telepair-agent`,
  `telepair-control`, `telepair-gateway`, `telepair-cli`) built on Rust
  Edition 2024, toolchain 1.94+, and `#![deny(unsafe_code)]` everywhere.
- Shared types for sessions, targets, config, protocol, errors, and the
  Owner / Operator / Viewer permission model.
- `Storage` trait with an async SQLite implementation (sqlx) and auto-applied
  migrations from `/migrations/`.
- Token-based authentication with SHA-256 hashed storage — raw tokens are
  returned exactly once at creation and never persisted in plaintext.
- PTY manager built on `portable-pty` (spawn, I/O, resize).
- Virtual target engine driven by `~/.telepair/targets.yaml`.
- Session and target services in `telepair-control`.
- Axum HTTP gateway exposing REST routes for health, targets, sessions, and
  invite tokens; static file serving for the web frontend.
- **Anonymous invite redemption** on `POST /api/invite/redeem`: opening a
  shared invite link no longer requires the visitor to already hold a
  bearer token. The endpoint mints a throwaway `guest-<nanoid>` user on
  successful redemption and returns the freshly issued token in the response
  body. Authenticated callers reuse their existing identity and get
  `"token": null` in the response. Guests are only created **after** the
  invite is successfully consumed, so a rejected link never leaves an
  orphan user behind.
- Strict `input_mode` parsing on `POST /api/sessions`: unknown values return
  `400 Bad Request` rather than silently collapsing to `serialized`, so
  client typos surface instead of masking themselves as the wrong semantics.
- Structured error-to-HTTP mapping via `Error::http_status()`: authorization
  failures always surface as 401/403 and input errors as 400 regardless of
  which storage call produced them — no more `InvalidInput` leaking out as
  500. `POST /api/invite/redeem` returns 400 for unknown/expired/exhausted
  tokens and 410 Gone when the target session is closed (the invite is not
  consumed so it can still be revoked or reassigned).
- WebSocket handler with a `SessionHub` that multiplexes PTY I/O and
  collaboration traffic across participants.
- Reconnect-safe participant lifecycle: the WebSocket handler does not
  stamp `left_at` on socket close — participant cleanup happens
  exclusively inside `close_session` / `close_stale_sessions`, in the
  same transaction that flips session status, so a transient socket blip
  inside the reaper's grace window does not evict an invitee.
- Multi-user participant tracking with join/leave/chat/cursor messages.
- Binary WebSocket frames for terminal I/O (zero-copy via `Bytes`); JSON
  frames for control and collaboration messages with `#[serde(tag = "type")]`
  discriminants.
- Bounded WebSocket chat size, cursor update rate, and frame limits.
- Single-node `telepair` binary running agent + control + gateway in one
  process; hidden role flags are reserved for future clustering work.
- `telepair admin show-token` subcommand for recovering the admin token
  from `~/.telepair/admin_token` (mode `0600`, written on first startup).

### Fixed

- **SPA deep-link status code** — `build_router_with_options` used
  `ServeDir::not_found_service(ServeFile(index.html))` which correctly
  served the shell body for client-side routes like `/login`,
  `/join/<token>`, and `/session/<id>` but returned `HTTP 404`
  (tower-http's `not_found_service` wraps the fallback in
  `SetStatus::new(..., NOT_FOUND)`, which forcibly rewrites the status).
  The 404 broke nginx `proxy_intercept_errors`, CDN rules that treat
  404s as cacheable dead links, uptime probes, and OG/SEO crawlers.
  Replace the `ServeFile` fallback with a `service_fn` that reads
  `index.html` once at boot into an `Arc<Vec<u8>>` and hand it to
  `ServeDir::fallback(..)` (not `not_found_service`) so the reply is
  `200 OK` with `Cache-Control: no-cache`. Failing to read `index.html`
  at startup now hard-errors instead of silently serving an empty
  body.
- WebSocket connect is now deferred until xterm completes its initial
  `fit()`, so the PTY is spawned with the real terminal cols/rows
  instead of the default 80×24 (so `vim`, `htop`, etc. open at the
  right size).
- The gateway closes the session row and aborts the output forwarder
  when WebSocket launch fails midway, preventing orphan session rows
  that would block subsequent joins.
- Transient `STORAGE_ERROR` codes are now propagated as **retryable**
  across the gateway → WS → frontend stack, instead of forcing a logout
  the first time SQLite returns `SQLITE_BUSY` under load.
- Unknown `/api/*` and `/ws/*` paths now return `404 Not Found` instead
  of falling through to the SPA shell, so misnamed API calls fail loudly
  instead of receiving HTML.
- `POST /api/sessions/{id}/invite` returns `410 Gone` when the target
  session has already been closed, matching the redeem path so the
  client can show a consistent "session ended" message.
- Scoped invite guests now get bounced off the dashboard cleanly
  (redirected back to their session) instead of being left staring at
  an empty target list.
- The CLI no longer prints a stray warning when the optional
  `~/.telepair/targets.yaml` is absent — a missing file is the default,
  not an error.

### Added — Web frontend (SolidJS + Vite)

- Vite + SolidJS + TypeScript scaffold.
- Typed REST API client and WebSocket client mirroring the backend protocol.
- Reactive auth and session stores.
- Login page, dashboard with available targets, and session page with
  xterm.js terminal (WebGL rendering).
- Anonymous invite flow: the Join page always attempts to redeem the token
  in the URL — no "paste your admin token first" wall — and only surfaces
  an error when the invite itself is invalid, expired, or points at a
  closed session (410 Gone). The newly minted guest token is cached in
  the browser for the rest of the tab's lifetime.
- Login copy mirrors the anonymous flow: invitees are told to just open
  the link, admins paste their token.
- REST client 401 interceptor: any protected path returning 401 clears the
  cached token and forces a redirect to `/login`, so a stale admin token
  can never leave the dashboard silently rendering "No targets available".
  `POST /api/invite/redeem` is explicitly exempt so a half-filled guest
  flow does not bounce the visitor out of the invite.
- Real-time collaboration UI: participants list and chat panel wired into
  the session page.
- UI primitives (toast, banner, skeleton) and a banner-based reconnect UX.
- **Bilingual English / Simplified Chinese UI** with a live locale switcher
  in the topbar; the choice is persisted in `localStorage` and the bundle
  ships both dictionaries plus a small auto-detect helper.
- **JetBrainsMono Nerd Font bundled with the frontend** so powerline,
  dev-icon, and box-drawing glyphs render correctly in xterm.js without
  the user having to install a font on the host machine.
- Owner-only **Close session** control on the session top bar with a
  two-step confirm — clicking once arms the button, clicking again issues
  `DELETE /api/sessions/{id}`.
- **System notices in the chat panel** when participants join or leave the
  session, rendered as italic centered lines so they don't get mistaken
  for chat messages.
- Invite-link **Copy** button falls back to `document.execCommand('copy')`
  in non-secure contexts (HTTP / raw-IP origins) so the dashboard still
  works without HTTPS.

### Added — Documentation

- Project README, contributing guide, and MIT license.
- `docs/architecture.md`, `docs/protocol.md`, `docs/api.md`, and
  `docs/deployment.md` — the deployment guide covers systemd, nginx,
  Docker, Docker Compose, and a dedicated CORS section covering same-origin
  reverse-proxy, direct-exposure with `--allowed-origins`, and dev-only
  `--allow-any-origin`. The systemd example binds `--host 127.0.0.1` so
  the listener is loopback-only behind a proxy by default.
- README documents the full `telepair --help` surface including
  `--allowed-origins` / `--allow-any-origin`, the `admin show-token`
  recovery path, and calls out that the binary runs as a single-node
  process today.

### Security

- TOCTOU-safe invite token redemption.
- Force-disconnect WebSocket clients when a session is stopped.
- Configurable CORS origins with loopback defaults and wildcard warning;
  malformed origins are rejected.
- PTY child-process environment sanitization.
- `list_sessions` scoped to owner and participant sessions only.
- Idempotent invite redemption via `upsert_participant`.
- Admin-only target gating enforced in the REST `create_session` handler.
- Per-message permission enforcement on WebSocket input and resize.
- Frontend persists the auth token only after successful validation.
- WebSocket auth timeout and graceful shutdown.
- **Scoped invite guests** — guest users minted via `POST /api/invite/redeem`
  are now pinned to the redeemed `session_id` via a new
  `users.scoped_session_id` column. A dedicated `require_unscoped` check
  blocks scoped guests from `GET /api/targets` and `POST /api/sessions`
  with 403, `POST /api/invite/redeem` rejects cross-session redemption
  attempts, and the WebSocket handshake enforces the pin a second time so
  a guest cannot open `/ws/session/{other}` even if a future bug adds a
  stray participant row. Closes an invite-time privilege-escalation path
  where any redeemed invite produced a fully privileged non-admin account
  that could enumerate targets and create new sessions of its own.
- `GET /api/targets` now filters out `admin_only` targets for non-admin
  callers. Previously the REST list leaked admin-only target names to
  every authenticated user even though `POST /api/sessions` correctly
  refused to create a session on them, turning the list endpoint into a
  free enumeration oracle for reconnaissance.

### Performance

- Zero-copy PTY output via `Bytes` broadcast.
- Parallelized auth + session lookup in the WebSocket handshake.
- Terminal output sent as binary WebSocket frames.
- Idle session reaper with reconnection-aware refcounting so multi-tab users
  stay present and orphan PTYs do not leak.
- Indexed `participants(user_id)` with cascading foreign-key deletes.
- Atomic session create / join / close paths.
- SPA shell `index.html` is loaded once at boot into a refcounted
  `Bytes` and shared across requests, removing per-request file I/O on
  the deep-link fallback.

### Testing

- **107** Rust tests across the workspace (unit, integration, and backend
  collaboration end-to-end flows), including coverage for `create_guest`
  uniqueness, the anonymous-redeem happy path, invitee reconnection inside
  the reaper grace window, bulk `close_session` participant settling, the
  strict `input_mode` rejection path, and the 400 / 410 error codes on
  `POST /api/invite/redeem`.
- **112** Vitest unit tests covering the protocol, stores, REST client
  (including the 401 interceptor and its `/invite/redeem` opt-out), the
  WebSocket client, and the i18n dictionaries (locale auto-detect,
  English ↔ Chinese key symmetry, template rendering, and label coverage).
- **36** Playwright browser E2E tests covering authentication, dashboard,
  session lifecycle, terminal I/O, collaboration, the full anonymous-guest
  redeem flow, the invalid-invite error path, and a 17-step
  `human-simulation.spec.ts` end-to-end happy path that drives the UI
  the way a real user would (login → launch → type → chat → invite →
  guest joins → solo-mode block → owner closes session → logout).

### Build & CI

- Makefile wiring `fmt` / `fmt-check` / `lint` / `test` / `build` / `e2e`
  targets plus an `all` target that chains the full pre-push pipeline
  (fmt-check + clippy + type-check + unit tests + release binary +
  frontend bundle + Playwright E2E) in one shot. `.DEFAULT_GOAL` stays
  on `help` so the bare `make` experience remains a menu.
- GitHub Actions CI pipeline and release workflow.
- MIT license across the workspace.

[Unreleased]: https://github.com/telepair/telepair/compare/v0.1.8...HEAD
[0.1.8]: https://github.com/telepair/telepair/compare/v0.1.7...v0.1.8
[0.1.7]: https://github.com/telepair/telepair/compare/v0.1.6...v0.1.7
[0.1.6]: https://github.com/telepair/telepair/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/telepair/telepair/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/telepair/telepair/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/telepair/telepair/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/telepair/telepair/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/telepair/telepair/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/telepair/telepair/releases/tag/v0.1.0
