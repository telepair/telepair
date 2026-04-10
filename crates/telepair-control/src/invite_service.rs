use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};

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
}

impl InviteService {
    pub fn new(
        storage: Arc<SqliteStorage>,
        sessions: Arc<SessionService>,
        auth: Arc<TokenAuthProvider>,
    ) -> Self {
        Self {
            storage,
            sessions,
            auth,
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
        // snapshot, so we can return it directly.
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
        let (user_id, issued_token) = match existing_user {
            Some(u) => (u.id, None),
            None => {
                let (guest, raw_token) = self.auth.create_guest(&invite.session_id).await?;
                (guest.id, Some(raw_token))
            }
        };

        self.sessions
            .upsert_participant(&invite.session_id, user_id, invite.role)
            .await?;

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
}
