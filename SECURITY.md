# Security Policy

Telepair brokers PTY sessions over WebSocket and signs share links
for session recordings. Compromise of a running server typically
implies RCE on the host, so we take security reports seriously.

## Reporting a Vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

Email **liys87x@gmail.com** with the subject line
`[telepair security]` and include:

- A description of the issue and its impact
- Steps to reproduce (proof-of-concept preferred)
- The affected version (`telepair --version` output)
- Deployment details that matter (TLS in front? reverse proxy?
  multi-tenant?)

PGP is not required; if you want encrypted correspondence, reply to
the acknowledgement and we'll negotiate a channel.

## Response Timeline

| Stage | Target |
|-------|--------|
| Acknowledgement | within 3 business days |
| Triage verdict (confirmed / won't-fix / needs-more-info) | within 7 business days |
| Fix released | next `vX.Y.Z+1` or the next minor if scope requires |

Credit in the release notes is offered by default; opt out if you
prefer.

## Supported Versions

Only the latest minor release receives security updates. Older
versions are unsupported — upgrade before reporting.

| Version | Supported |
|---------|-----------|
| Latest minor (currently `0.1.x`) | ✅ |
| Older | ❌ |

## Deployment Hardening

Running telepair safely:

- **Terminate TLS in front of the gateway.** The WS bearer token
  travels in the first frame after the upgrade handshake and is
  confidential only over `wss://`.
- **Restrict access to `~/.telepair/`.** The admin token file is
  created with `0o600` by the binary; do not relax.
- **Prefer least-privilege target users** — the `admin` role can
  force-close sessions and manage users, but does not bypass PTY
  permissions on the host. Run telepair as a dedicated unprivileged
  user whenever possible.
- **Rate-limit the unauthenticated auth endpoints at your reverse
  proxy** as defence-in-depth on top of the built-in per-IP
  throttle on `/api/auth/{register,login,verify}`.
