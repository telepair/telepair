use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};

use telepair_core::audit::{AuditEvent, AuditEventType, AuditSink};
use telepair_core::auth::TokenAuthProvider;
use telepair_core::error::{Error, Result};
use telepair_core::permission::Role;
use telepair_core::session::{InviteToken, SessionStatus, User};
use telepair_core::storage::{SqliteStorage, Storage};

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
#[derive(Debug, Clone)]
pub struct CreateInviteParams {
    pub role: Role,
    pub max_uses: i32,
    pub expires_in_minutes: Option<i64>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Output DTO for [`InviteService::create`]. Carries the raw token
/// that the HTTP layer will hand back to the caller exactly once.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct RedeemResult {
    pub session_id: String,
    pub role: Role,
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
        let token_prefix = t.token_sha256.chars().take(8).collect::<String>();
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
    auth: Arc<TokenAuthProvider>,
    audit: Arc<AuditSink>,
}

impl InviteService {
    pub fn new(
        storage: Arc<SqliteStorage>,
        sessions: Arc<SessionService>,
        auth: Arc<TokenAuthProvider>,
        audit: Arc<AuditSink>,
    ) -> Self {
        Self {
            storage,
            sessions,
            auth,
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

        // Resolve expiry: absolute timestamp wins over TTL. Negative /
        // zero TTL is rejected loudly so a slider overshoot doesn't
        // silently produce a never-expires invite; positive TTL is
        // clamped to the hard ceiling.
        let expires_at = match (params.expires_at, params.expires_in_minutes) {
            (Some(at), _) => {
                if at <= Utc::now() {
                    return Err(Error::InvalidInput(
                        "expires_at must be in the future".into(),
                    ));
                }
                Some(at)
            }
            (None, Some(minutes)) if minutes > 0 => {
                let clamped = minutes.min(MAX_INVITE_TTL_MINUTES);
                Some(Utc::now() + Duration::minutes(clamped))
            }
            (None, Some(_)) => {
                return Err(Error::InvalidInput(
                    "expires_in_minutes must be positive".into(),
                ));
            }
            (None, None) => None,
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
        let token_prefix = invite.token_sha256.chars().take(8).collect::<String>();
        self.audit
            .record(
                AuditEvent::new(AuditEventType::InviteMinted)
                    .with_actor(owner.id, owner.name.clone())
                    .with_session(session_id.to_string())
                    .with_detail(serde_json::json!({
                        "token_prefix": token_prefix,
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
    /// `None`, a throwaway scoped guest is minted *after* the invite
    /// is successfully consumed so a rejected token never leaves an
    /// orphan user behind.
    pub async fn redeem(&self, existing_user: Option<User>, token: &str) -> Result<RedeemResult> {
        // Preview the invite without consuming. Lets us run the
        // scoped-guest check and the closed-session check before we
        // burn a use — prevents the old bug where a revoked/closed
        // session silently drained `max_uses` and dropped a ghost
        // participant.
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

        // Target session must still be alive. 410 Gone (SessionClosed)
        // is different from 404 (SessionNotFound) — the invite knew
        // about a real session that has since been retired.
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
        if let Some(ref user) = existing_user {
            let participants = self.sessions.list_participants(&preview.session_id).await?;
            if let Some(existing) = participants.iter().find(|p| p.user_id == user.id) {
                return Ok(RedeemResult {
                    session_id: preview.session_id.clone(),
                    role: existing.role,
                    issued_token: None,
                });
            }
        }

        // Atomic consume — validates expiry + max_uses + increments
        // used_count in one transaction.
        let invite = self.storage.consume_invite(token).await?;

        // Mint-after-consume ordering is load-bearing: it guarantees
        // that a rejected redeem never leaks a guest user row.
        let (user_id, actor_name, issued_token, as_guest) = match existing_user {
            Some(u) => (u.id, u.name, None, false),
            None => {
                let (guest, raw_token) = self.auth.create_guest(&invite.session_id).await?;
                (guest.id, guest.name, Some(raw_token), true)
            }
        };

        self.sessions
            .upsert_participant(&invite.session_id, user_id, invite.role)
            .await?;

        // Two audit rows: the invite redemption and the participant
        // join that happened because of it. Keeping them separate
        // lets the "who joined this session" timeline stay useful
        // even when filtered down to ParticipantJoined only.
        let token_prefix = invite.token_sha256.chars().take(8).collect::<String>();
        self.audit
            .record(
                AuditEvent::new(AuditEventType::InviteRedeemed)
                    .with_actor(user_id, actor_name.clone())
                    .with_session(invite.session_id.clone())
                    .with_detail(serde_json::json!({
                        "token_prefix": token_prefix,
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

        Ok(RedeemResult {
            session_id: invite.session_id,
            role: invite.role,
            issued_token,
        })
    }

    /// Look up an invite by token without consuming it. Thin wrapper
    /// retained for the HTTP layer's preview flows (e.g. "does this
    /// link still point at a live session?"). Does NOT check expiry
    /// / max_uses — prefer [`redeem`] when you actually want to
    /// enforce them.
    pub async fn preview(&self, token: &str) -> Result<InviteToken> {
        self.storage.find_invite(token).await
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

    /// Revoke (hard-delete) an invite row. Requires ownership +
    /// that the invite actually belongs to the stated session —
    /// the path parameter `session_id` must match what the invite
    /// row points at. Mismatch returns `SessionNotFound` so a
    /// caller poking at `/api/sessions/X/invites/<token>` cannot
    /// probe for the existence of invites in session Y.
    pub async fn revoke(&self, owner: &User, session_id: &str, token_sha256: &str) -> Result<()> {
        self.sessions.require_owner(owner, session_id).await?;
        // Find the row first so we can verify it belongs to this
        // session. `find_invite` uses the raw-token lookup path
        // which hashes before comparison — here we already have the
        // SHA-256, so query storage directly by listing and
        // filtering. A dedicated `find_invite_by_sha256` would be
        // nicer but the blast radius of scanning a single session's
        // invites is tiny and avoids a new trait method.
        let rows = self.storage.list_invites_for_session(session_id).await?;
        let target = rows
            .into_iter()
            .find(|r| r.token_sha256 == token_sha256)
            .ok_or_else(|| {
                Error::InvalidInput(format!(
                    "invite {token_sha256} not found in session {session_id}"
                ))
            })?;
        let role = target.role;
        self.storage.revoke_invite(&target.token_sha256).await?;

        let token_prefix = target.token_sha256.chars().take(8).collect::<String>();
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
