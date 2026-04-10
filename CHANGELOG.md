# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/telepair/telepair/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/telepair/telepair/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/telepair/telepair/releases/tag/v0.1.0
