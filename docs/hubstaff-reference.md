# Hubstaff reference

A paraphrased summary of the Hubstaff public interfaces that matter to LBX
Timer. It is not a complete API mirror. Each section links its official
source; Hubstaff can change these interfaces, so check the source before relying
on a detail. IDs, names, and timestamps in examples and fixtures are fictional.

The current implementation uses the installed desktop CLI, described in
[hubstaff.md](hubstaff.md). The cloud and deep-link interfaces below are
reference material; they are outside the current read-only observer scope.

## Desktop control: deep links

Source: [Hubstaff Deep Link Support](https://support.hubstaff.com/hubstaff-deep-link-support/).

| Operation | URI |
| --- | --- |
| Start a project | `hubstaff://start-timer?projectId=123` |
| Start a task | `hubstaff://start-timer?taskId=456` |
| Stop tracking | `hubstaff://stop-timer` |

- Starting a task requires that it is assigned to the signed-in user.
- The links target the desktop app. They can launch it when it is closed, and
  open sign-in instead of tracking when the user is logged out.
- They are not browser or mobile timer APIs.
- There is no status query, elapsed-time response, or acknowledgement, so
  successfully opening a link does not prove that tracking started or stopped.

## Tracking-state events (read)

Source: [Tracking States](https://developer.hubstaff.com/reference/tracking_states/).

```http
GET https://api.hubstaff.com/v2/organizations/42/tracking_states?user_ids[]=7&occurred[start]=2026-11-08T00%3A00%3A00Z&occurred[stop]=2026-11-08T01%3A00%3A06Z
Authorization: Bearer <access_token>
```

**Request**

- Required: the organization ID, plus ISO 8601 `occurred[start]` and
  `occurred[stop]` (exclusive), spanning at most seven days.
- Filters: `user_ids[]`, `project_ids[]`, `task_ids[]`, `include_removed`.
- Pagination: `page_start_id`, `page_limit`.

**Response**

- An object with `tracking_states`. Each entry has `id`, `occurred_at`,
  `type` (`start` or `stop`), `user_id`, `project_id`, `project_type`, and
  `task_id`. Example: [tracking-states.json](fixtures/hubstaff/tracking-states.json).
- Events come from desktop and web clients. Timestamps are client-reported
  and can be backdated, and insertion order follows IDs, not event time.
- Deleted projects are excluded. Removed members are excluded unless
  requested.
- Documented errors: 400, 401, 403, 404, 429. A 403 can reflect plan
  restrictions.

The endpoint describes a history of starts and stops. It does not report the
live state of the timer.

## Webhooks (push)

Sources: [webhook guide](https://developer.hubstaff.com/webhooks/) and
[endpoint reference](https://developer.hubstaff.com/reference/webhooks/).

All paths are under `https://api.hubstaff.com`:

| Method | Path | Purpose |
| --- | --- | --- |
| POST | `/v2/organizations/{organization_id}/webhooks` | Subscribe an organization |
| POST | `/v2/projects/{project_id}/webhooks` | Subscribe a project |
| POST | `/v2/users/me/webhooks` | Subscribe the current user |
| GET | `/v2/webhooks/{webhook_id}` | Inspect a subscription |
| POST | `/v2/webhooks/{webhook_id}/activate` | Activate |
| DELETE | `/v2/webhooks/{webhook_id}` | Remove |

**Setup**

- Create body:
  `{"events":["timer.start","timer.stop"],"target_url":"https://example.invalid/hooks/hubstaff"}`.
- Scopes: `hubstaff:read` to read, `hubstaff:write` to change.
- Creation returns 201.
- Verification is an empty POST carrying `X-Hook-Secret`. The receiver
  replies 200 echoing that header, keeps the secret, and activates with the
  same header.
- The guide gives 200 for activation; the reference gives 201.
- Deletion returns 204.

**Delivery**

- Each delivery has `id`, `event`, `created_at`, and `payload`.
- The documented start payload includes user, organization, project,
  optional task, and `tracking_started_at`. Example:
  [timer-start.json](fixtures/hubstaff/timer-start.json). The stop payload's
  fields are not documented in these sources.
- `X-Hook-Signature` is an HMAC-SHA256 of the raw body with the saved
  secret. Compare it in constant time.
- Hubstaff retries transient failures. Persistent 404 or 410 responses can
  disable delivery.
- Delivery needs an address reachable from the internet; a listener on a
  desktop's localhost does not receive it.

## Authentication

Source: [Authentication](https://developer.hubstaff.com/authentication/).

| Purpose | URL |
| --- | --- |
| Discovery | `https://account.hubstaff.com/.well-known/openid-configuration` |
| Authorization | `https://account.hubstaff.com/authorizations/new` |
| Token and refresh | `https://account.hubstaff.com/access_tokens` |
| User info | `https://account.hubstaff.com/user_info` |
| Signing keys | `https://account.hubstaff.com/jwks.json` |

**Personal access tokens**

- API requests use `Authorization: Bearer <access_token>`.
- A personal access token starts as a refresh token. Exchange it with form
  fields `grant_type=refresh_token` and `refresh_token=…`, without client
  credentials.
- The response has `token_type`, `access_token`, `expires_in`, and
  `refresh_token`. Refresh tokens rotate, so the new one must be kept.
- Refreshing is limited to five attempts per hour per token, so access
  tokens should be cached rather than refreshed per request.

**Other options**

- OAuth uses the authorization-code flow with PKCE support.
- Organization tokens are a separate option intended for admin automation.
- DPoP-bound credentials additionally require DPoP proofs.

## Related APIs (not live timer control)

### Activities

Source: [Activities](https://developer.hubstaff.com/reference/activities/).

- `GET /v2/organizations/{organization_id}/activities` and
  `/v2/projects/{project_id}/activities` return ten-minute blocks. Both
  have `/daily` variants.
- Raw queries require `time_slot[start]` and `time_slot[stop]`, may filter
  by user, project, or task, and span at most seven days.
- Raw data can lag by about twenty minutes. Daily totals use the
  organization's time zone.
- These are reporting sources, not a live status.

### Manual time entries

Source: [Time Entries](https://developer.hubstaff.com/reference/time_entries/).

- `POST /v2/users/{user_id}/time_entries` adds manual time. It does not
  start the desktop timer.
- The JSON body requires `project_id`, ISO 8601 `start_time`, and `tracked`
  (integer seconds). Optional fields include `task_id` and `billable`;
  `note` is required unless organization settings relax it.
- Success is HTTP 201 with `{"success":true}`. Entries that need approval
  cannot be submitted through this endpoint.

```json
{"project_id":123,"start_time":"2026-11-08T01:00:00Z","tracked":5,"note":"Example only"}
```
