# Portal protocol v1

The daemon socket and QML bridge exchange one JSON object per line. Unknown
fields must be ignored within schema version 1. Unknown message types or schema
versions must be rejected.

## Server messages

`snapshot` is a complete, keyed replacement:

```json
{
  "type": "snapshot",
  "schema_version": 1,
  "sequence": 8,
  "daemon_epoch": "2dc2c8f0-47db-4806-b8d8-a45baa7d47d6",
  "generated_at_ms": 1785384000000,
  "connection": "connected",
  "sessions": [],
  "agents": [],
  "agent_order": {"available": true, "mode": "grouped"},
  "backend": {"mode": "herdr", "switchable": true},
  "voice": {"state": "unavailable", "owned": false},
  "usage": {
    "providers": [
      {
        "id": "codex",
        "label": "CODEX",
        "kind": "quota",
        "available": true,
        "stale": false,
        "source": "rpc",
        "status": "allowed",
        "plan": "pro",
        "primary": {"label": "WEEKLY", "utilization": 0.74},
        "today_tokens": 53960987620,
        "tokens_per_hour": 1778512401
      }
    ]
  },
  "micro": {"connected": true, "battery": 45, "charging": false},
  "health": {}
}
```

Agent statuses are exactly `blocked`, `done`, `working`, `idle`, or `unknown`.
`review_ready` is a separate attention latch: explicit `done` and
`working`→`idle` set it, authoritative `blocked`, `working`, or `unknown`
clear it, and `idle` otherwise preserves it. A new Herdr focus transition to
that agent or a successful portal `open` acknowledges it.

Agent `display_name` is the safe `display_agent` or agent `name` given to the
pane (unless it merely repeats the agent type), then the matching Herdr tab
label unless it is Herdr's all-digit default, then `<agent> · <workspace>`.
Terminal titles and prompt-derived content are never display-name sources.
Optional `repository` and `worktree` values are sanitized checkout identity,
not terminal content. `launch_pending` means Herdr has observed a launch request
without yet observing a live agent; it is not treated as an agent state.
`state_change_seq` is Herdr's monotonic state-change ordering scalar. Priority
mode uses it descending within the same attention bucket so the EDGE and Herdr
panels retain the same tie ordering; grouped mode continues to use Space order.

Voice state is exactly `idle`, `recording`, `processing`, `error`, or
`unavailable`. `owned` says only whether this daemon instance owns the private
dictation marker; snapshots never carry transcripts, tooltips, command output,
or voice errors from stderr.

`usage` exposes only normalized provider capacity and coarse aggregate
activity. Utilization is a fraction from 0 through 1; reset timestamps, plan,
model, `today_tokens`, and `tokens_per_hour` are optional bounded metadata.
The aggregate token fields are provider-wide local activity totals, not
per-request records. Usage never includes credentials, prompts, message
contents, per-model history, cache/database contents, or provider response
bodies. `micro` is a
read-only normalized view of the local Codex Micro connection and optional
device status. It never exposes the Micro socket protocol to QML.

`backend` names the active agent manager, exactly `herdr` or `t3code`, and
`switchable` says whether this daemon accepts the manager commands below. A
T3 Code roster uses one session named `t3code` whose `protocol` is T3 Code's
server runtime descriptor version (currently 1) and whose `version` is
omitted. T3 Code cards always have `zoom: false`, no capabilities, and
`focused: false`; `display_name` is the T3 Code thread title, `workspace` is
the project title plus branch, `repository` and `worktree` are checkout leaf
names, and `state_change_seq` is the millisecond timestamp of the classifying
turn or settle event.

These additive v1 fields are optional for compatibility with older recorded
fixtures. Clients default voice to unavailable, review-ready and launch-pending
to false (except a legacy raw `done` agent), usage to an empty provider list,
Micro to disconnected, ordering to unavailable, and the backend to a
non-switchable `herdr`. When ordering is available, `grouped` preserves Herdr
Space order (or T3 Code project and thread recency) and `priority` promotes
attention states.
Connection and action failures are separate fields, not invented agent states.
Unavailable health metrics use `available: false` and omit `value`; they are
never encoded as a false zero.

The Herdr adapter reads herdr-core, the terminal runtime behind the Herdr app,
over its documented JSON-RPC socket (protocol generation 1). A Herdr roster
uses one session named after the core socket (normally `default`) whose
`version` is herdr-core's version and whose `protocol` is omitted. Herdr cards
have `open: true`, `zoom: false`, and no capabilities, because herdr-core has
no zoom or guarded actions. `focused` marks the terminal in the Herdr app's
focused pane while the app runs. A herdr-core reply that no longer matches the
documented terminal schema makes that refresh fail, and the session goes
`stale` with its actions removed, rather than guessing.

`action_result` acknowledges one command:

```json
{
  "type": "action_result",
  "schema_version": 1,
  "request_id": "touch-42",
  "ok": false,
  "code": "stale_snapshot",
  "message": "the portal snapshot changed before the action"
}
```

Stable failure codes include `invalid_command`, `stale_snapshot`,
`capability_expired`, `agent_state_changed`, `unsupported_protocol`,
`session_disconnected`, `target_unavailable`, `voice_unavailable`,
`voice_busy`, `voice_not_owned`, and `command_error`.

## Client commands

```json
{
  "type": "command",
  "schema_version": 1,
  "request_id": "touch-42",
  "sequence": 8,
  "agent_id": "3e676d4c-4bbb-5d14-bc88-e040c176a84a",
  "action": "open"
}
```

The only actions are:

- `open`: show the fresh agent target in its manager and activate that
  manager's exact window.
- `zoom`: reserved; no supported manager offers it, so cards carry
  `zoom: false` and the daemon refuses it.
- `approve` and `interrupt`: require the capability ID present in the same
  current snapshot. The daemon forwards only that opaque ID.
- `restore_focus`: takes no agent or capability and restores a previously
  observed non-EDGE window if it still exists.
- `voice_start`: takes no agent or capability, requires Voxtype to report
  `idle`, claims the daemon's private ownership marker, and starts typed
  dictation without automatic submission.
- `voice_stop`: takes no agent or capability and stops only a recording owned
  by this daemon instance.
- `voice_cancel`: takes no agent or capability and discards only a recording
  owned by this daemon instance.
- `spotify_play_pause`, `spotify_next`, and `spotify_previous`: take no
  agent, capability, or `app_id` and make the matching fixed MPRIS call on
  the Spotify desktop client. They are refused unless `spotify` is listed in
  the snapshot's `dashboards`.
- `basecamp_open`: takes no agent, capability, or `app_id` and requires
  `item_id`, `n<id>` for a notification or `c<id>` for a card in the current
  `basecamp` snapshot. Snapshots never carry URLs; the daemon looks the id up
  in its own last Basecamp sample and opens only `app.basecamp.com` or
  `3.basecamp.com` links in the default browser. `item_id` is rejected on
  every other action.
- `slack_open`: takes no agent, capability, or `app_id` and requires
  `item_id`, a `<conversation>-<ts>` id of a message or notification in the
  current `slack` snapshot. Snapshots never carry links or the token; the
  daemon looks the id up in its own last Slack sample and opens only a
  validated `slack://channel?team=…&id=…&message=…` deep link, so the item
  opens in the Slack desktop app. Message images are thumbnails the daemon
  downloads with the token into `$XDG_RUNTIME_DIR/xeneon-edge-agents/slack-images`
  (needs the `files:read` scope); the snapshot names only the bare file.
- `github_open`: takes no agent, capability, or `app_id` and requires
  `item_id`, `p<id>` for a pull request or `n<id>` for a notification in the
  current `github` snapshot. Snapshots never carry URLs or tokens; the daemon
  opens only the `https://github.com/` page it recorded for that id.
- `desktop_app`: takes no agent or capability and requires `app_id`, one of
  the ids in the snapshot's `desktop_apps` (0-4 entries from the daemon's
  `desktop-apps.toml` or `[[desktop.apps]]` configuration). The daemon focuses the single exact
  compositor client with that entry's class, or launches its desktop entry
  when none exists. An id that is not configured is refused. `app_id` is
  rejected on every other action.
- `order_grouped` and `order_priority`: take no agent or capability and set a
  daemon-owned ordering preference for the active manager, persisted beside
  the manager choice. Neither herdr-core nor T3 Code has an ordering API.
- `backend_herdr` and `backend_t3code`: take no agent or capability, are gated
  on the current snapshot `sequence`, and switch the active agent manager.
  Repeating the current manager succeeds without changing state. A switch
  clears every card, private target, latch, and subscription, publishes a
  reconnecting snapshot with the new `backend.mode`, and persists the choice.

In Herdr mode `open` sends the Herdr app's typed `agent.focus` command for the
card's terminal and activates the exact `herdr-app` window; it never launches
the app. `zoom`, `approve`, and `interrupt` fail with `target_unavailable` or
`capability_expired`.

In T3 Code mode `open` activates the exact `t3code` desktop window and
acknowledges the card's review badge; it never launches T3 Code or navigates
threads, because T3 Code has no public thread-focus API. `zoom`, `approve`,
and `interrupt` fail with `target_unavailable` or `capability_expired`.

There is no method, key, text, shell-command, prompt, close, or server-control
passthrough. Desktop actions do not accept a desktop ID, executable, title,
class, or argument from QML. When an exact class has multiple windows, the
daemon narrows to the known initial main title, a tiled window, and then the uniquely
most-recent focus-history entry; a match still ambiguous after those checks
fails closed.
Herdr activation separately requires an interactive Herdr process with the
exact requested session identity in the compositor client's process tree;
transient Herdr CLI commands cannot claim a terminal. A configured native
class/title is preferred when present, otherwise multiple owning terminal
views narrow to the unique most-recent focus-history entry and fail closed on
missing or tied history.
Voice actions are single-attempt operations and return only
bounded static result messages. Host commands have a ten-second ceiling; a
failed or timed-out start may issue one distinct cancel cleanup attempt but is
never retried. Their `sequence` field is carried for one
command shape but is not used as a stale gate: a press/release hold may span
unrelated snapshots, and ownership plus live Voxtype state are authoritative.
