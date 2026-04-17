// web/src/lib/audit.ts
import { AuditEventType } from './protocol';
import type { TranslationKey, Translator } from '../i18n';

/** Map an audit event-type string to its localised label. Unknown
 *  variants fall through to the raw type so a backend that adds a new
 *  variant doesn't render an empty cell. */
export const AUDIT_EVENT_LABEL_KEYS: Record<AuditEventType, TranslationKey> = {
  [AuditEventType.SESSION_CREATED]: 'session_detail.event_session_created',
  [AuditEventType.SESSION_CLOSED]: 'session_detail.event_session_closed',
  [AuditEventType.PARTICIPANT_JOINED]: 'session_detail.event_participant_joined',
  [AuditEventType.INVITE_MINTED]: 'session_detail.event_invite_minted',
  [AuditEventType.INVITE_REDEEMED]: 'session_detail.event_invite_redeemed',
  [AuditEventType.INVITE_REVOKED]: 'session_detail.event_invite_revoked',
  [AuditEventType.TARGET_ACCESS_DENIED]: 'session_detail.event_target_access_denied',
  [AuditEventType.TARGET_RELOADED]: 'session_detail.event_target_reloaded',
  [AuditEventType.AUTH_LOGIN_FAILED]: 'session_detail.event_auth_login_failed',
  [AuditEventType.AUTH_REGISTER_REJECTED]: 'session_detail.event_auth_register_rejected',
  [AuditEventType.AUTH_REGISTER_COMPLETED]: 'session_detail.event_auth_register_completed',
  [AuditEventType.AUTH_VERIFY_FAILED]: 'session_detail.event_auth_verify_failed',
  [AuditEventType.AUTH_USER_ENABLED]: 'session_detail.event_auth_user_enabled',
  [AuditEventType.AUTH_USER_DISABLED]: 'session_detail.event_auth_user_disabled',
  [AuditEventType.AUTH_SESSION_ACCESS_DENIED]: 'session_detail.event_auth_session_access_denied',
  [AuditEventType.AUTH_PASSWORD_CHANGED]: 'session_detail.event_auth_password_changed',
  [AuditEventType.AUTH_ADMIN_USER_CREATED]: 'session_detail.event_auth_admin_user_created',
  [AuditEventType.PARTICIPANT_ROLE_CHANGED]: 'session_detail.event_participant_role_changed',
  [AuditEventType.RECORDING_STARTED]: 'session_detail.event_recording_started',
  [AuditEventType.RECORDING_STOPPED]: 'session_detail.event_recording_stopped',
};

export function eventLabel(t: Translator, type: string): string {
  const key = AUDIT_EVENT_LABEL_KEYS[type as AuditEventType];
  return key ? t(key) : type;
}

/** Format an ISO timestamp using the browser's locale. */
export function formatTs(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}
