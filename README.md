# XENEON EDGE Agent Command Center

[![CI](https://github.com/spencerbull/xeneon-edge-agents/actions/workflows/ci.yml/badge.svg)](https://github.com/spencerbull/xeneon-edge-agents/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

A native Omarchy command center for the Corsair XENEON EDGE. It keeps your
agent manager, Herdr or T3 Code, as the session and interaction authority while
providing a dedicated 2560x720 touch surface for agent status, safe triage,
provider capacity, and connected tool status.

[Watch the XENEON EDGE Agent Command Center in action on X.](https://x.com/SpencerGBull/status/2088048447525966150?s=20)

> [!NOTE]
> This is an early, hardware-specific project developed and validated on
> Omarchy with a Corsair XENEON EDGE. There is not yet a general-purpose
> packaged release; use the source installer and commissioning flow below.

## Highlights

- A glanceable command center and animated Ambient view for Herdr agents or
  T3 Code threads, switched with the on-screen Manager control.
- Exact, fail-closed display and touchscreen identity checks.
- Typed, narrowly scoped agent actions mediated by the Rust daemon.
- Normalized Claude, Codex, OpenCode, host-health, and connected-tool status.
- Reversible user-owned installation in XDG locations; no packaged Omarchy
  files are modified.

Production output matching is fail-closed: the portal creates no surface
unless the commissioned EDID, model, serial, and touchscreen all match exactly
and uniquely. The connector name is resolved at runtime so a normal port rename
after reboot or replug does not weaken the hardware identity.

## Components

- `xeneon-agentd`: Rust user daemon for Herdr or T3 Code agent state, host
  health, normalized AI usage and Codex Micro status, safe actions, optional
  Voxtype dictation, reconnects, and output/focus integration.
- `xeneon-agentctl qml-bridge`: NDJSON bridge used by Quickshell.
- `quickshell/`: standalone touch portal plus deterministic and live previews.
- `config/` and `scripts/`: reversible user-service, exact-identity hotplug,
  and Omarchy integration.

The agent manager remains authoritative. The portal can never send arbitrary
text, keys, or shell commands. Opening a Herdr card is one typed `agent.focus`
command to the Herdr app; approve and interrupt are unavailable because no
supported manager issues a capability that grounds them safely.

For the full boundary model, see the [architecture](docs/architecture.md) and
[protocol](docs/protocol.md) documentation.

## Agent managers

The header's **Manager** control switches the whole command center between two
agent managers. The daemon owns the choice, persists it under
`$XDG_STATE_HOME/xeneon-edge-agents/agent-backend.toml`, and starts on the
configured `agent_backend` (default `herdr`) when nothing is persisted.
Switching clears the previous roster, private action targets, review latches,
and focus history before the next manager is observed.

**Herdr** mode follows the Herdr app. The daemon reads herdr-core, the
terminal runtime behind the app, over its documented local socket: every live
agent the app shows, including sub-agents, with event-driven updates. Tapping
a card shows that agent in the Herdr app and raises its window. herdr-core has
no ordering preference, so the daemon keeps the grouped/priority choice.

**T3 Code** mode observes the local T3 Code server without any network or
authenticated API. The daemon reads only the bounded thread, session, turn, and
project projections from T3 Code's read-only SQLite state and gates everything
on T3 Code's own server runtime descriptor plus its live server process. One
card is shown per thread that is running, has a live provider session, or has
finished a turn the human has not yet settled. Pending approvals, unanswered
questions, actionable plans, and failed turns show as waiting; running turns
show as working; unsettled completed turns are review-ready; settled threads
are ready. Settling a thread in T3 Code acknowledges its review badge the same
way a Herdr focus transition does. Card names are T3 Code's own thread titles,
never message text.

T3 Code exposes no public thread-focus API on Linux, so a card tap activates
the exact `t3code` desktop window; zoom, approve, and interrupt are
unavailable, and the Order control is a daemon-owned preference. Messages,
activities, checkpoints, secrets, and provider payloads are never read, and
T3 Code state is never written. Preview the mode with
`scripts/preview t3code.ndjson`.

## Develop and preview

Requirements on Omarchy are Rust, Quickshell, Qt 6 QML tooling, Lua's `luac`,
Python 3, Herdr, and `just` through mise.

```bash
mise install
mise exec -- just check
mise run preview
```

The preview is a normal 1280x360 window on the current display and uses a
deterministic fixture. Explicit preview windows are the only path that may use
the primary display. Other fixture states can be selected explicitly:

```bash
scripts/preview health.ndjson
scripts/preview disconnected.ndjson
```

A live source preview builds private development binaries, starts a temporary
daemon/socket, and displays the same normalized Herdr and Voxtype state used by
production without installing files, enabling services, or changing the
fail-closed output identity:

```bash
scripts/preview --live
scripts/preview --live --ambient-after 6
```

The control center shows safe Herdr identity, state duration, repository and
worktree context, plus normalized Claude, Codex, and OpenCode capacity. The
daemon consumes Omarchy's schema-v1 Claude/Codex agent records, keeps legacy
cache compatibility, and derives OpenCode's local soft-budget windows from its
newly processed token counters (input, output, reasoning, and cache-write;
reused cache-read context is excluded) in the
read-only SQLite message ledger. The AI footer shows both reported usage
windows, resets, plan/freshness state, and bounded today/hour token activity
when available.
More than one dashboard can be enabled per machine in
`~/.config/xeneon-edge-agents/dashboards.toml` (see
`config/xeneon-edge-agents/dashboards.toml.example`); the header shows one tab
per enabled dashboard with a short live status. Besides the agent command
center there is a Spotify now-playing dashboard with play/pause, next, and
previous, and a Basecamp dashboard that shows configured card tables as lanes
next to the Hey! inbox, read through the signed-in Basecamp CLI; tapping a
card or item opens it in the browser. A Slack dashboard shows recent messages
from chosen channels next to your unread DMs and @-mentions, read with a
read-only Slack app you create from `config/slack/app-manifest.yaml`; the
user token stays in an owner-only `slack.toml` and never reaches the portal.
A GitHub dashboard lists an organization's open pull requests next to your
unread notifications, read through the signed-in `gh` CLI (optionally as a
named `gh` account).
The desktop apps the daemon may focus or launch are configured per machine in
`~/.config/xeneon-edge-agents/desktop-apps.toml` (Spotify and Discord when the
file is absent; see `config/xeneon-edge-agents/desktop-apps.toml.example`).
Opening the Spotify dashboard launches the `spotify` app when Spotify is not
running.
In Grouped order the cards are drawn as one lane per Herdr workspace, with the
workspace name above each lane; Priority order keeps the flat attention grid.
The header's right side holds a clock whose digits roll into place and whose
hairline sweeps smoothly across each minute; the voice and Micro controls are
not shown in this fork.
Tapping the AGENTS tab switches ordering: arriving shows Priority, and each
further tap flips between Grouped and Priority. The daemon keeps the value for
each manager.
The upper-right Screen and Palette controls are shared by the command-center
and Ambient views. Screen applies a reversible near-black portal veil. The
saved reduce-motion preference (or the system setting) still snaps the
ambient constellation to fixed positions and removes trail animation. These presentation settings persist across portal restarts.

Portal chrome follows Omarchy's active runtime theme. The named Quickshell
configuration reads the allowlisted colors in
`$XDG_STATE_HOME/omarchy/current/theme/colors.toml` and watches the stable
`theme.name` file, so `omarchy theme set` updates backgrounds, surfaces, text,
borders, and semantic accents without restarting the portal. Missing or
invalid colors fall back safely. State meaning stays stable while its hue comes
from the active theme: working uses blue, blocked uses yellow, review-ready
uses green, and errors use red. Light Omarchy themes are translated into
contrast-checked dark EDGE chrome while retaining their palette hues.
The top-right Palette control shows the live Omarchy swatches and lets you map
those allowlisted theme roles onto XENEON's Ready, Success, Working, Needs
Help, Review Ready, Error, Unknown, Recording, and Processing meanings. Mappings
apply immediately to the command center and Ambient view, persist beside the
Motion and Screen preferences, and can be reset to the reviewed defaults.
Arbitrary colors are not accepted; chrome and text contrast remain automatic.

The live preview exposes the real narrow agent-focus, desktop-app, and voice
actions. Voice
dictation is push-to-talk: press starts a portal-owned Voxtype recording,
release transcribes without automatic submission, and Cancel discards it. The
floating preview restores the last exact non-portal window before voice start
and stop so that window remains the laptop transcription target. The preview
also applies exact per-window opaque compositor properties so desktop content
cannot bleed through its dark surface.

`--ambient-after` accepts 1 through 300 seconds and only changes the explicit
preview window. It is useful for reviewing the staged dashboard-to-orbit
animation without changing the production 60-second inactivity policy. The
reference timing and motion breakdown live in
[`docs/concept-motion.md`](docs/concept-motion.md).

## Install without activating

Install the two local binaries, then seed simulator-safe user configuration:

```bash
cargo install --path crates/xeneon-agentd --root "$HOME/.local" --force
cargo install --path crates/xeneon-agentctl --root "$HOME/.local" --force
scripts/install.sh
```

The default installer copies the named Quickshell configuration and rendered
user units, plus an XDG desktop launcher named **XENEON EDGE Command Center**.
The launcher starts the two fixed user services without restarting an already
running portal; its secondary desktop action performs an explicit restart.
The default installer does not enable either service, edit Hyprland, or match
any production output. Existing user-owned or modified files are refused or
preserved.

The daemon reads Voxtype's runtime state when Omarchy Dictation is available.
Portal voice start/stop/cancel requests remain typed daemon actions; QML never
executes the `voxtype` CLI. Service startup and shutdown clean up only a
portal-owned recording. The same cleanup can be invoked directly for a private
preview daemon:

```bash
xeneon-agentd --config ~/.config/xeneon-edge-agents/config.toml --cleanup-dictation
```

```bash
scripts/check.sh
scripts/uninstall.sh
```

`check.sh` reports the physical gate as blocked in simulation mode. Uninstall
removes only installer-recorded files whose content is unchanged.

## Commission the physical EDGE

First capture the connected hardware identity without changing anything:

```bash
scripts/detect-hardware.sh
```

Production installation requires the complete output and touch identity
together:

```bash
scripts/install.sh \
  --apply-production \
  --connector '<DRM-CONNECTOR>' \
  --edid-sha256 '<64-CHARACTER-EDID-SHA256>' \
  --screen-serial '<EXACT-EDID-SERIAL>' \
  --screen-model '<EXACT-SCREEN-MODEL>' \
  --touch-device '<EXACT-HYPRLAND-DEVICE-NAME>' \
  --touch-bustype '0003' \
  --touch-vendor '<EXACT-USB-VENDOR-ID>' \
  --touch-product '<EXACT-USB-PRODUCT-ID>' \
  --touch-phys '<EXACT-KERNEL-PHYS>'
```

Use `--touch-uniq` when the device exposes a stable non-empty `uniq`; `phys` is
the fallback only when `uniq` is empty because the USB topology path can change
across boots. Providing both records both values but runtime identity prefers
the stable `uniq`. A live
production install performs the full output and touchscreen check before
changing any user file. Without `--activate`, it stages the generated module
but deliberately leaves `hyprland.lua` and all service states untouched. Add
`--activate` only after that exact identity passes while the device is
connected. Activation inserts the one user-owned require, validates and
reloads Hyprland, and enables the lightweight hotplug reconciler. Native
Hyprland monitor events then resolve the current connector, verify the complete
identity, map only the commissioned touchscreen, and start or stop the daemon
and portal together. A user-level `/dev/input` path watcher covers USB-only
touch disconnects/reconnects; the exact touch device stays disabled during
every transition until the full identity gate passes. The generated module
leaves Omarchy's `monitors.lua` ownership untouched.

The remaining physical checklist covers touch coordinates, focus restoration,
hotplug, DPMS, suspend/resume, lock-screen privacy, and read-only DDC discovery.
Brightness control stays disabled until exact read/restore is proven.

See the [commissioning guide](docs/commissioning.md) for the complete safety
and verification checklist. `TODO.md` is the resumable implementation ledger.

## Security and privacy

The portal intentionally excludes terminal contents, prompts, credentials, and
raw provider payloads. Production display matching fails closed when hardware
identity is absent or ambiguous, and QML cannot execute arbitrary shell input.

Please report security issues through the process in [SECURITY.md](SECURITY.md)
instead of opening a public issue.

## Contributing

Contributions are welcome. Start with [CONTRIBUTING.md](CONTRIBUTING.md), keep
the authority and hardware-safety boundaries intact, and run the repository
checks before opening a pull request.

## License

This project is available under the [MIT License](LICENSE).
