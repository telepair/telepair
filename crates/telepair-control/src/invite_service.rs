use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};

use telepair_core::audit::{AuditEvent, AuditEventType, AuditSink};
use telepair_core::auth::{is_unique_violation, random_guest_name};
use telepair_core::error::{Error, Result};
use telepair_core::permission::Role;
use telepair_core::session::{InviteToken, RedeemIdentity, SessionStatus, User};
use telepair_core::storage::{SqliteStorage, Storage};

/// Max attempts when a fresh guest's random name collides with an
/// existing `users.name` row. `guest-<nanoid8>` has ≈47 bits of
/// entropy; a collision is vanishing, but we still bound the loop so
/// a corrupted DB can't spin forever. Each attempt runs the full
/// `redeem_invite` transaction — the rolled-back failure leaves
/// `used_count` untouched, so retrying is safe.
const GUEST_NAME_MAX_ATTEMPTS: usize = 5;

use crate::session_service::SessionService;

/// Hard cap on invite `max_uses`. An invite that can be redeemed 10k
/// times is not an invite, it's a public URL — reject those at the
/// service layer so a typo in the UI can't produce one by accident.
pub const MAX_INVITE_USES: i32 = 100;

/// Hard cap on invite TTL in minutes. Week-long invites are already
/// pushing it; month-long invites are a credential-handling mistake
/// waiting to happen. Requests asking for longer are clamped.
pub const MAX_INVITE_TTL_MINUTES: i64 = 7 * 24 * 60;

/// Input DTO for [`InviteService::create`]. The HTTP layer parses
/// whatever body shape it likes and translates into this struct — the
/// service does not care about JSON.
///
/// All three TTL inputs can be passed; the service resolves precedence
/// `expires_at` > `expires_in_secs` > `expires_in_minutes` and applies
/// [`MAX_INVITE_TTL_MINUTES`] clamping + positivity validation in one
/// place. Keeping resolution out of the HTTP layer means a CLI or other
/// non-HTTP caller cannot bypass the ceiling by pre-computing an
/// absolute timestamp.
#[derive(Debug, Clone)]
pub struct CreateInviteParams {
    pub role: Role,
    pub max_uses: i32,
    pub expires_in_minutes: Option<i64>,
    pub expires_in_secs: Option<i64>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Output DTO for [`InviteService::create`]. Carries the raw token
/// that the HTTP layer will hand back to the caller exactly once.
/// Serialized directly into the `POST /api/sessions/:id/invites`
/// response body — the field names ARE the wire format.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CreateInviteResult {
    pub session_id: String,
    /// Raw bearer token — the DB only stores its SHA-256 digest,
    /// making this the only chance the caller has to capture the
    /// plaintext value.
    pub token: String,
    pub role: Role,
    pub max_uses: i32,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Output DTO for [`InviteService::redeem`]. `issued_token` is
/// `Some(raw)` when a fresh scoped guest was minted, and `None` when
/// an authenticated caller joined (or reused their seat) under their
/// existing identity.
///
/// Serialized directly into the `POST /api/invite/redeem` response
/// body; `issued_token` is renamed to `token` on the wire so the
/// frontend keeps reading the single-word field name it's used to.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RedeemResult {
    pub session_id: String,
    pub role: Role,
    #[serde(rename = "token")]
    pub issued_token: Option<String>,
}

/// Read-side view of an invite for the owner-facing management
/// dialog. Deliberately **does not** carry the raw token — the
/// `token_prefix` is the first 8 chars of the sha256 digest so the
/// UI can give each row a stable, non-sensitive label without
/// leaking a value that could be used to redeem. Clients revoke a
/// row by its full `token_sha256` (the PK).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InviteSummary {
    /// Full SHA-256 digest — used as the URL path parameter on
    /// `DELETE /api/sessions/:id/invites/:token_sha256`. Opaque to
    /// humans; the UI only shows the prefix.
    pub token_sha256: String,
    /// First 8 chars of `token_sha256` — a short stable label the
    /// UI renders next to each row ("ab12cd34"). Does not collide
    /// in practice inside a single session's invite list.
    pub token_prefix: String,
    pub session_id: String,
    pub role: Role,
    pub max_uses: i32,
    pub used_count: i32,
    /// Derived: `max_uses - used_count`, clamped to zero. Precomputed
    /// server-side so every client (CLI, web UI) renders the same
    /// number without reproducing the arithmetic.
    pub remaining_uses: i32,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
}

impl InviteSummary {
    fn from_token(t: InviteToken) -> Self {
        let remaining = (t.max_uses - t.used_count).max(0);
        let token_prefix = t.token_prefix().to_string();
        Self {
            token_sha256: t.token_sha256,
            token_prefix,
            session_id: t.session_id,
            role: t.role,
            max_uses: t.max_uses,
            used_count: t.used_count,
            remaining_uses: remaining,
            expires_at: t.expires_at,
            created_at: t.created_at,
        }
    }
}

/// Invite-token business rules. Minting, redeeming, and validation
/// flow through one place so the gateway handler is pure transport +
/// serialization. The old design scattered TTL clamping, scoped-guest
/// cross-session rejection, closed-session gating, and the "already a
/// participant → no-op" short-circuit across the HTTP layer; pulling
/// them here gives a single target for testing and for future audit
/// hooks.
pub struct InviteService {
    storage: Arc<SqliteStorage>,
    sessions: Arc<SessionService>,
    audit: Arc<AuditSink>,
}

impl InviteService {
    /// Build an `InviteService`. As of v0.1.2 the constructor no
    /// longer needs a `TokenAuthProvider`: the old redeem path used
    /// `auth.create_guest` to mint scoped guests in a separate write,
    /// but the new atomic `Storage::redeem_invite` does the user
    /// INSERT inside its own transaction, so the auth handle is not
    /// needed here anymore.
    pub fn new(
        storage: Arc<SqliteStorage>,
        sessions: Arc<SessionService>,
        audit: Arc<AuditSink>,
    ) -> Self {
        Self {
            storage,
            sessions,
            audit,
        }
    }

    /// Mint a new invite token for `session_id`. `owner` must be the
    /// owner of that session and the session must still be active.
    /// Policy checks (role, max_uses, TTL) all surface as
    /// [`Error::InvalidInput`] so callers see 400; ownership /
    /// closed-session failures surface as 403 / 410 via the centralized
    /// error mapping.
    pub async fn create(
        &self,
        owner: &User,
        session_id: &str,
        params: CreateInviteParams,
    ) -> Result<CreateInviteResult> {
        // Ownership + alive gate. `require_active_owned` emits the
        // right 403/410 distinction so the HTTP layer doesn't have to
        // re-derive it.
        self.sessions
            .require_active_owned(owner, session_id)
            .await?;

        // Only Operator / Viewer invites are allowed. Owner role means
        // "I am *the* owner" — not a thing you hand out.
        if params.role == Role::Owner {
            return Err(Error::InvalidInput("cannot invite with Owner role".into()));
        }

        if params.max_uses < 1 || params.max_uses > MAX_INVITE_USES {
            return Err(Error::InvalidInput(format!(
                "max_uses must be between 1 and {MAX_INVITE_USES}"
            )));
        }

        // Resolve expiry. Precedence: `expires_at` > `expires_in_secs`
        // > `expires_in_minutes`. The two input flavours intentionally
        // carry different policies:
        //
        // * Relative TTL (`expires_in_secs`/`_minutes`) is silently
        //   clamped to the ceiling because UI sliders can overshoot as
        //   a benign UX mistake and clamping is friendlier than a 400.
        //   See `create_clamps_huge_ttl_to_ceiling`.
        //
        // * `expires_at` is an explicit wall-clock pick, so silently
        //   rewriting it would lie to the caller — a client asking
        //   for "Jan 1 2030" and getting a week-long token back has
        //   no way to notice the downgrade. We reject out-of-range
        //   absolute timestamps loudly. This is the teeth behind
        //   `MAX_INVITE_TTL_MINUTES`; without it, any direct-API
        //   caller could bypass the hard ceiling by passing an
        //   absolute timestamp.
        //
        // Negative / zero relative TTL is rejected so a slider
        // overshoot does not silently produce a never-expires invite.
        let now = Utc::now();
        let max_ttl = Duration::minutes(MAX_INVITE_TTL_MINUTES);
        let expires_at = if let Some(at) = params.expires_at {
            if at <= now {
                return Err(Error::InvalidInput(
                    "expires_at must be in the future".into(),
                ));
            }
            if at > now + max_ttl {
                return Err(Error::InvalidInput(format!(
                    "expires_at must not exceed {MAX_INVITE_TTL_MINUTES} minutes from now"
                )));
            }
            Some(at)
        } else if let Some(secs) = params.expires_in_secs {
            if secs <= 0 {
                return Err(Error::InvalidInput(
                    "expires_in_secs must be positive".into(),
                ));
            }
            let clamped = Duration::seconds(secs).min(max_ttl);
            Some(now + clamped)
        } else if let Some(minutes) = params.expires_in_minutes {
            if minutes <= 0 {
                return Err(Error::InvalidInput(
                    "expires_in_minutes must be positive".into(),
                ));
            }
            let clamped = Duration::minutes(minutes).min(max_ttl);
            Some(now + clamped)
        } else {
            None
        };

        let (invite, raw_token) = self
            .storage
            .create_invite(session_id, params.role, params.max_uses, expires_at)
            .await?;

        // Audit the mint. We deliberately log the sha256 prefix, not
        // the raw token — the token is only visible to the caller for
        // exactly the lifetime of this response. Role and max_uses
        // live in `detail` so the history view can render a chip
        // without a second lookup.
        self.audit
            .record(
                AuditEvent::new(AuditEventType::InviteMinted)
                    .with_actor(owner.id, owner.name.clone())
                    .with_session(session_id.to_string())
                    .with_detail(serde_json::json!({
                        "token_prefix": invite.token_prefix(),
                        "role": invite.role.as_str(),
                        "max_uses": invite.max_uses,
                        "expires_at": invite.expires_at,
                    })),
            )
            .await;

        Ok(CreateInviteResult {
            session_id: session_id.to_string(),
            token: raw_token,
            role: invite.role,
            max_uses: invite.max_uses,
            expires_at: invite.expires_at,
        })
    }

    /// Redeem an invite token. When `existing_user` is `Some`, the
    /// caller is already authenticated and is added under their
    /// existing identity (or, if they're already a member, the
    /// redeem is a no-op that does not consume a use). When it's
    /// `None`, a throwaway scoped guest is minted as part of the
    /// same storage transaction so a rejected redeem never leaks a
    /// guest user row.
    ///
    /// As of v0.1.2 the actual write step (`consume invite` →
    /// `create guest` → `upsert participant`) runs through a single
    /// [`Storage::redeem_invite`] transaction. That closes two v0.1.1
    /// bugs: (1) the session-active TOCTOU window between the
    /// service-layer pre-check and the participant upsert, and
    /// (2) the partial-failure path where a transient error
    /// between steps left `used_count` drained with no membership.
    pub async fn redeem(&self, existing_user: Option<User>, token: &str) -> Result<RedeemResult> {
        // Preview the invite without consuming. Still needed for
        // the scoped-guest cross-session check and the existing-
        // member short-circuit — both of which want to know the
        // target session id before we commit to a storage tx. The
        // definitive closed-session gate lives inside
        // `redeem_invite`'s WHERE clause, so this preview is an
        // optimization (fail fast on obviously-bad tokens) rather
        // than the authoritative check.
        let preview = self.storage.find_invite(token).await?;

        // Scoped guests can only redeem invites for the session they
        // were minted into. Anything else is a pivot attempt: 403.
        if let Some(ref user) = existing_user
            && let Some(ref scope) = user.scoped_session_id
            && preview.session_id != *scope
        {
            return Err(Error::PermissionDenied(
                "scoped guest cannot redeem invite for a different session".into(),
            ));
        }

        // Fast-path closed-session rejection so an obviously-dead
        // invite doesn't even reach the redeem transaction. The
        // transaction still re-checks `sessions.status = 'active'`
        // in its UPDATE guard — that's the TOCTOU-closing gate.
        // Here we just want the clearer 410 Gone error path on the
        // common case.
        let session = self
            .sessions
            .get_session_required(&preview.session_id)
            .await?;
        if session.status != SessionStatus::Active {
            return Err(Error::SessionClosed(preview.session_id.clone()));
        }

        // "Owner or existing member clicks their own invite": treat
        // as a no-op so the first sanity check of a share link doesn't
        // burn a use. The participant list already carries the role
        // snapshot, so we can return it directly. No audit row — the
        // participant is already recorded in the original join event.
        //
        // The lookup is routed through
        // `find_active_participant_role`, which does a single
        // SELECT-JOIN against `participants` and `sessions`. That
        // replaces the previous two-query sequence
        // (`get_session_required` + `list_participants`), which had
        // a narrow but real TOCTOU window — a concurrent
        // `close_session` committing between the two reads could
        // cause this branch to return a "you're a member" success
        // against a session that was already closed. The atomic
        // query guarantees both predicates resolve against the same
        // MVCC snapshot. The residual window between this query and
        // the `Ok(...)` return below is unavoidable without holding
        // a DB lock across the WS handshake; the WS layer's own
        // active-session check is the last line of defence for
        // that micro-gap, and its failure mode is a benign UX error.
        if let Some(ref user) = existing_user
            && let Some(existing_role) = self
                .sessions
                .find_active_participant_role(&preview.session_id, user.id)
                .await?
        {
            return Ok(RedeemResult {
                session_id: preview.session_id.clone(),
                role: existing_role,
                issued_token: None,
            });
        }

        // Atomic redeem: one storage transaction that consumes the
        // invite, INSERTs a scoped guest if needed, and upserts the
        // participant row. The UNIQUE(name) retry loop wraps the
        // whole transaction — a collision rolls back cleanly, so
        // `used_count` is untouched and we can safely retry with a
        // fresh random name.
        let as_guest = existing_user.is_none();
        let outcome = match existing_user {
            Some(ref user) => {
                self.storage
                    .redeem_invite(token, RedeemIdentity::Existing(user.id))
                    .await?
            }
            None => {
                let mut last_err: Option<Error> = None;
                let mut outcome = None;
                for _ in 0..GUEST_NAME_MAX_ATTEMPTS {
                    let name = random_guest_name();
                    match self
                        .storage
                        .redeem_invite(token, RedeemIdentity::NewGuest { name: &name })
                        .await
                    {
                        Ok(o) => {
                            outcome = Some(o);
                            break;
                        }
                        Err(e) if is_unique_violation(&e) => {
                            last_err = Some(e);
                            continue;
                        }
                        Err(e) => return Err(e),
                    }
                }
                outcome.ok_or_else(|| {
                    last_err
                        .unwrap_or_else(|| Error::Internal("exhausted guest name retries".into()))
                })?
            }
        };

        // Two audit rows: the invite redemption and the participant
        // join that happened because of it. Keeping them separate
        // lets the "who joined this session" timeline stay useful
        // even when filtered down to ParticipantJoined only.
        //
        // Suppressed entirely on the `was_already_member` storage
        // short path — that branch fires when a race-losing
        // concurrent redeem discovered the caller is already in
        // the session (upstream `find_active_participant_role`
        // said "no" before the race-winner committed). The
        // original join already produced its audit rows; emitting
        // another pair here would double-log the same membership
        // and silently turn a race-loss into a timeline artifact.
        let user_id = outcome.user_id;
        let actor_name = outcome.user_name;
        let invite = outcome.invite;
        if !outcome.was_already_member {
            self.audit
                .record(
                    AuditEvent::new(AuditEventType::InviteRedeemed)
                        .with_actor(user_id, actor_name.clone())
                        .with_session(invite.session_id.clone())
                        .with_detail(serde_json::json!({
                            "token_prefix": invite.token_prefix(),
                            "role": invite.role.as_str(),
                            "as_guest": as_guest,
                        })),
                )
                .await;
            self.audit
                .record(
                    AuditEvent::new(AuditEventType::ParticipantJoined)
                        .with_actor(user_id, actor_name)
                        .with_session(invite.session_id.clone())
                        .with_detail(serde_json::json!({ "role": invite.role.as_str() })),
                )
                .await;
        }

        Ok(RedeemResult {
            session_id: invite.session_id,
            role: invite.role,
            issued_token: outcome.issued_token,
        })
    }

    /// List every invite row owned by `session_id`, sanitized into
    /// [`InviteSummary`]. Requires `owner` to actually own the
    /// session (not just a participant) so guests can't enumerate
    /// other peoples' invite links. Expired / exhausted rows are
    /// **included** — the management UI wants the full history and
    /// renders the state chip per row; filtering happens client-side
    /// if at all.
    pub async fn list_for_session(
        &self,
        owner: &User,
        session_id: &str,
    ) -> Result<Vec<InviteSummary>> {
        // require_owner (not require_active_owned) — an operator
        // should still be able to see the invite history of a
        // closed session so post-mortem "who did we invite"
        // investigations work.
        self.sessions.require_owner(owner, session_id).await?;
        let rows = self.storage.list_invites_for_session(session_id).await?;
        Ok(rows.into_iter().map(InviteSummary::from_token).collect())
    }

    /// Revoke (hard-delete) an invite row. Idempotent by design:
    /// an unknown sha, an already-revoked sha, and a cross-session
    /// probe all resolve to `Ok(())` so the HTTP layer can answer 204
    /// without leaking whether the invite exists. Cross-session side
    /// effects are still prevented — the storage delete only fires
    /// when the invite actually belongs to `session_id`.
    ///
    /// Ownership is still required (non-owners get `PermissionDenied` →
    /// 403); idempotency only collapses the "did the row exist?"
    /// distinction, not the auth gate.
    pub async fn revoke(&self, owner: &User, session_id: &str, token_sha256: &str) -> Result<()> {
        self.sessions.require_owner(owner, session_id).await?;
        // Single indexed PK lookup — direct SHA-256 hit, O(1) even
        // when a session has hundreds of invites. The alternative
        // (list-and-filter) parsed every row in the session just to
        // find one.
        let Some(target) = self
            .storage
            .find_invite_by_sha256(token_sha256)
            .await?
            .filter(|r| r.session_id == session_id)
        else {
            // Two shapes collapsed into "no-op": (1) already revoked
            // or never existed, (2) the sha exists but belongs to a
            // different session. Both read the same from the wire so
            // a probe can't enumerate cross-session invites, and a
            // race between two admins revoking the same row reports
            // success to both.
            return Ok(());
        };
        let role = target.role;
        let token_prefix = target.token_prefix().to_string();
        self.storage.revoke_invite(&target.token_sha256).await?;

        self.audit
            .record(
                AuditEvent::new(AuditEventType::InviteRevoked)
                    .with_actor(owner.id, owner.name.clone())
                    .with_session(session_id.to_string())
                    .with_detail(serde_json::json!({
                        "token_prefix": token_prefix,
                        "role": role.as_str(),
                    })),
            )
            .await;
        Ok(())
    }
}
