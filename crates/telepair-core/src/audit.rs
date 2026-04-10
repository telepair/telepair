//! Append-only audit log.
//!
//! The service layer records security- and lifecycle-meaningful events
//! through [`AuditSink`] — things like "who created this session",
//! "which invite was minted with what role", "who tried to reach an
//! admin-only target". High-rate data plane events (chat messages,
//! cursor pings, PTY bytes) are deliberately *not* audited: the table
//! would explode, and those events are not security facts.
//!
//! Shape:
//!
//! - [`AuditEventType`] — closed enum of everything we record. The
//!   string form is what lands in the DB, so adding a variant is a
//!   schema change.
//! - [`AuditEvent`] — a single row, with optional actor, optional
//!   session, and a free-form `detail` JSON blob for event-specific
//!   fields (role, close reason, target name, …).
//! - [`AuditFilter`] — query params for the CLI and the session
//!   detail timeline.
//! - [`AuditSink`] — a thin wrapper around [`SqliteStorage`] that
//!   services depend on. `record` is best-effort (logs-and-swallows);
//!   `query` propagates errors because callers want to surface them.

use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::Result;
use crate::storage::{SqliteStorage, Storage};

/// Canonical taxonomy. The order of variants here mirrors the order
/// they appear in the docs; the on-disk representation is the
/// dotted-lowercase string returned by [`AuditEventType::as_str`].
///
/// Intentionally small for v0.1.1. Two classes of event were
/// considered and deliberately left out:
///
/// - **Auth login** — telepair has no dedicated `POST /api/auth`
///   endpoint; bearer-token validation happens on every API request
///   via `extract_user`. Emitting an audit row per request would
///   flood the table without adding security signal. When a real
///   login flow lands, an `auth.*` family can be added as a
///   backward-compatible extension.
/// - **Participant left** — under reconnect-safe semantics the
///   `participants.left_at` column is only stamped by
///   `close_session`, so a standalone "someone left" event would
///   always coincide with `session.closed` on the same row. The
///   close event already carries the actor; a duplicate would be
///   noise.
///
/// `#[serde(rename = "...")]` on each variant keeps the JSON wire
/// form in lockstep with [`AuditEventType::as_str`] so the DB column,
/// the CLI `--type` flag, the CLI `--format json` output, and the
/// upcoming HTTP endpoint all speak the same dotted-lowercase
/// dialect. Default enum serde would emit `"ParticipantJoined"`,
/// which diverges from the `"participant.joined"` form in the
/// `audit_events.event_type` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AuditEventType {
    /// `POST /api/sessions` succeeded. Detail: `{target_name, input_mode}`.
    #[serde(rename = "session.created")]
    SessionCreated,
    /// A session transitioned from `active` to `closed`, for any
    /// reason (owner click, reaper, startup sweep). The `detail`
    /// blob carries `{reason, duration_s}`.
    #[serde(rename = "session.closed")]
    SessionClosed,
    /// A user became a participant on a session — via owner create
    /// or invite redemption. Detail: `{role}`.
    #[serde(rename = "participant.joined")]
    ParticipantJoined,
    /// A new invite row was inserted. Detail:
    /// `{role, max_uses, expires_at}`.
    #[serde(rename = "invite.minted")]
    InviteMinted,
    /// An invite was successfully consumed. Detail:
    /// `{as_guest: bool, role}`.
    #[serde(rename = "invite.redeemed")]
    InviteRedeemed,
    /// An invite row was hard-deleted by its owner.
    #[serde(rename = "invite.revoked")]
    InviteRevoked,
    /// A non-admin tried to create a session against an admin-only
    /// target. Detail: `{target_name}`.
    #[serde(rename = "target.access_denied")]
    TargetAccessDenied,
    /// An admin hot-reloaded the target registry from disk via
    /// `POST /api/admin/targets/reload`. Detail carries the path that
    /// was re-read and the new target count so the history view can
    /// answer "did the reload actually change anything?" without
    /// chasing yaml diffs. `{path, targets}`.
    #[serde(rename = "target.reloaded")]
    TargetReloaded,
}

impl AuditEventType {
    /// String form stored in `audit_events.event_type`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SessionCreated => "session.created",
            Self::SessionClosed => "session.closed",
            Self::ParticipantJoined => "participant.joined",
            Self::InviteMinted => "invite.minted",
            Self::InviteRedeemed => "invite.redeemed",
            Self::InviteRevoked => "invite.revoked",
            Self::TargetAccessDenied => "target.access_denied",
            Self::TargetReloaded => "target.reloaded",
        }
    }
}

impl FromStr for AuditEventType {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "session.created" => Ok(Self::SessionCreated),
            "session.closed" => Ok(Self::SessionClosed),
            "participant.joined" => Ok(Self::ParticipantJoined),
            "invite.minted" => Ok(Self::InviteMinted),
            "invite.redeemed" => Ok(Self::InviteRedeemed),
            "invite.revoked" => Ok(Self::InviteRevoked),
            "target.access_denied" => Ok(Self::TargetAccessDenied),
            "target.reloaded" => Ok(Self::TargetReloaded),
            _ => Err(format!("unknown audit event type: {s}")),
        }
    }
}

/// A single row from `audit_events`.
///
/// `actor_id` and `actor_name` are independently nullable:
/// - `actor_id` is `None` for events that happen before identity is
///   fully resolved (e.g. [`AuditEventType::TargetAccessDenied`]
///   against an unauthenticated caller, or a future auth.* family).
/// - `actor_name` is a **denormalized snapshot** captured at
///   insertion time. The audit log never joins back to `users`, so
///   renaming a user does not rewrite history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// SQLite autoincrement id. `None` for an event that has not
    /// been persisted yet.
    pub id: Option<i64>,
    pub ts: DateTime<Utc>,
    pub actor_id: Option<Uuid>,
    pub actor_name: Option<String>,
    pub event_type: AuditEventType,
    pub session_id: Option<String>,
    /// Free-form JSON blob. `Value::Null` is the "no extra data"
    /// sentinel and is stored as SQL `NULL`, not the literal string
    /// `"null"`, so queries that filter on a present detail work
    /// naturally.
    #[serde(default)]
    pub detail: JsonValue,
}

impl AuditEvent {
    /// Build a new event with `ts = now()` and no actor/session. The
    /// caller adds context via the `with_*` builders.
    pub fn new(event_type: AuditEventType) -> Self {
        Self {
            id: None,
            ts: Utc::now(),
            actor_id: None,
            actor_name: None,
            event_type,
            session_id: None,
            detail: JsonValue::Null,
        }
    }

    pub fn with_actor(mut self, id: Uuid, name: impl Into<String>) -> Self {
        self.actor_id = Some(id);
        self.actor_name = Some(name.into());
        self
    }

    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_detail(mut self, detail: JsonValue) -> Self {
        self.detail = detail;
        self
    }
}

/// Query parameters for [`AuditSink::query`]. Every field is optional
/// — the default value returns the most recent rows across all
/// dimensions.
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    /// Inclusive lower bound on `ts`.
    pub since: Option<DateTime<Utc>>,
    /// Exclusive upper bound on `ts`.
    pub until: Option<DateTime<Utc>>,
    pub actor_id: Option<Uuid>,
    pub session_id: Option<String>,
    /// Restrict to these types. Empty list = all types (the default).
    pub event_types: Vec<AuditEventType>,
    /// Max rows returned. `None` = 100 (sane default for humans).
    pub limit: Option<i64>,
    pub offset: i64,
}

/// Concrete audit sink backed by a [`SqliteStorage`].
///
/// Not a trait because every production and test path uses the same
/// backing storage — telepair is single-process by design. Kept as a
/// struct so the writes go through a single choke point that can log
/// failures without every caller repeating the boilerplate.
#[derive(Clone)]
pub struct AuditSink {
    storage: Arc<SqliteStorage>,
}

impl AuditSink {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }

    /// Best-effort write. A failed audit insert logs at error level
    /// and swallows the error: a transient storage hiccup must never
    /// take down the business operation that triggered the event.
    /// The caller's happy path does not change based on audit
    /// success.
    pub async fn record(&self, event: AuditEvent) {
        if let Err(e) = self.storage.insert_audit_event(&event).await {
            tracing::error!(
                error = %e,
                event_type = event.event_type.as_str(),
                "failed to record audit event"
            );
        }
    }

    /// Read-only query. Propagates errors because callers (the
    /// `admin audit` CLI, the session-detail endpoint) want to
    /// surface them rather than silently render "no events".
    pub async fn query(&self, filter: AuditFilter) -> Result<Vec<AuditEvent>> {
        self.storage.list_audit_events(&filter).await
    }
}
