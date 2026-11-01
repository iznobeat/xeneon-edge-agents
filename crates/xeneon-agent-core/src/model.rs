use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Blocked,
    Done,
    Working,
    Idle,
    #[default]
    #[serde(other)]
    Unknown,
}

impl AgentStatus {
    pub fn attention_rank(self) -> u8 {
        match self {
            Self::Blocked => 0,
            Self::Done => 1,
            Self::Working => 2,
            Self::Idle => 3,
            Self::Unknown => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Connected,
    Degraded,
    Reconnecting,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Connected,
    Stale,
    Incompatible,
    Offline,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceState {
    Idle,
    Recording,
    Processing,
    Error,
    #[default]
    #[serde(other)]
    Unavailable,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceSnapshot {
    pub state: VoiceState,
    #[serde(default)]
    pub owned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Open,
    Zoom,
    Approve,
    Interrupt,
    RestoreFocus,
    DesktopApp,
    SpotifyPlayPause,
    SpotifyNext,
    SpotifyPrevious,
    BasecampOpen,
    SlackOpen,
    GithubOpen,
    GameShow,
    VoiceStart,
    VoiceStop,
    VoiceCancel,
    OrderGrouped,
    OrderPriority,
    BackendHerdr,
    BackendT3code,
}

/// A full-screen view the portal can switch between. The daemon publishes
/// the enabled list in order; the agent command center is `agents`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardKind {
    Agents,
    Spotify,
    Basecamp,
    Slack,
    Github,
    Game,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackStatus {
    Playing,
    Paused,
    #[default]
    Stopped,
}

/// Now-playing state read from Spotify's MPRIS interface. `position_ms` is
/// the position when the daemon sampled it; the portal advances it locally
/// while playing so the daemon need not publish every second.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpotifySnapshot {
    pub available: bool,
    pub status: PlaybackStatus,
    pub title: String,
    pub artist: String,
    pub album: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art_url: Option<String>,
    pub length_ms: u64,
    pub position_ms: u64,
    pub can_control: bool,
    pub can_go_next: bool,
    pub can_go_previous: bool,
}

/// How far the lyrics lookup for the current Spotify track got.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LyricsStatus {
    /// Lyrics are switched off or nothing is playing.
    #[default]
    Off,
    Loading,
    /// Lines carry the time they start at.
    Synced,
    /// Lines carry no times (`at_ms` is 0).
    Plain,
    Instrumental,
    /// The lyrics service has nothing for this track.
    Missing,
    /// The lookup failed; it is retried later.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LyricLine {
    pub at_ms: u64,
    pub text: String,
}

/// Lyrics for the track named by `title` and `artist`, which the portal
/// matches against the Spotify snapshot before showing them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LyricsSnapshot {
    pub status: LyricsStatus,
    pub title: String,
    pub artist: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<LyricLine>,
}

/// One unread item from the Basecamp Hey! inbox. Its URL stays in the
/// daemon; the portal opens it by `id` with `basecamp_open`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasecampNotification {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub project: String,
    pub creator: String,
    /// RFC 3339 time the item became unread.
    pub unread_at: String,
}

/// One card in a card table lane. Its URL stays in the daemon; the portal
/// opens it by `id` with `basecamp_open`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasecampCard {
    pub id: String,
    pub title: String,
    pub assignees: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_on: Option<String>,
    pub comments: u32,
}

/// A card table column shown as a lane. `cards` is capped; `total` is the
/// column's full open-card count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasecampLane {
    pub title: String,
    /// Theme color role derived from the Basecamp column color.
    pub color: String,
    pub total: u32,
    pub cards: Vec<BasecampCard>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasecampBoard {
    pub label: String,
    pub available: bool,
    pub lanes: Vec<BasecampLane>,
    pub not_now: u32,
    pub done: u32,
}

/// The signed-in person's Basecamp Hey! inbox and configured card tables,
/// read through the Basecamp CLI. `available` is false when the CLI is
/// missing or not signed in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasecampSnapshot {
    pub available: bool,
    pub unread_count: u32,
    /// The server caps unreads at 100; the count is then a lower bound.
    pub unreads_capped: bool,
    pub notifications: Vec<BasecampNotification>,
    pub boards: Vec<BasecampBoard>,
}

/// One message in a watched Slack channel, with mentions and links already
/// rendered to plain text. Its permalink stays in the daemon; the portal
/// opens it by `id` with `slack_open`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackMessage {
    pub id: String,
    pub author: String,
    pub text: String,
    /// Thumbnails the daemon downloaded into its runtime image directory.
    #[serde(default)]
    pub images: Vec<SlackImage>,
    #[serde(default)]
    pub reactions: Vec<SlackReaction>,
    /// Unix seconds.
    pub at: u64,
    pub replies: u32,
    /// Posted after the user's last-read marker.
    pub unread: bool,
}

/// One reaction on a message: the emoji (or `:name:` for a custom one) and
/// how many people used it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackReaction {
    pub emoji: String,
    pub count: u32,
}

/// A Slack image thumbnail cached by the daemon. `file` is a bare file name
/// inside `$XDG_RUNTIME_DIR/xeneon-edge-agents/slack-images`; the portal
/// never fetches anything from Slack itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackImage {
    pub file: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackChannel {
    pub name: String,
    pub available: bool,
    pub unread: u32,
    /// The unread count reached the fetched page; it is a lower bound.
    pub unread_capped: bool,
    /// Newest last.
    pub messages: Vec<SlackMessage>,
}

/// A direct message or @-mention for the notification strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackNotification {
    pub id: String,
    /// `dm` or `mention`.
    pub kind: String,
    /// Sender for a DM; channel name for a mention.
    pub source: String,
    pub author: String,
    pub text: String,
    pub at: u64,
    /// Unread DM messages; 0 for mentions.
    pub unread: u32,
}

/// Watched channels plus DM and mention notifications, read with the user's
/// own Slack user token. `available` is false without a usable token.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackSnapshot {
    pub available: bool,
    pub channels: Vec<SlackChannel>,
    pub notifications: Vec<SlackNotification>,
}

/// An open pull request in the followed organization. Its URL stays in the
/// daemon; the portal opens it by `id` with `github_open`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubPullRequest {
    pub id: String,
    /// Repository name without the organization.
    pub repo: String,
    pub number: u32,
    pub title: String,
    pub author: String,
    pub draft: bool,
    pub comments: u32,
    /// Last update, Unix seconds.
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubNotification {
    pub id: String,
    /// `pr`, `issue`, `release`, `commit`, `ci`, `discussion`, or `other`.
    pub kind: String,
    pub title: String,
    /// `owner/name`.
    pub repo: String,
    /// GitHub's reason, e.g. `review_requested` or `mention`.
    pub reason: String,
    pub at: u64,
}

/// Open organization pull requests and the user's unread GitHub
/// notifications, read through the signed-in `gh` CLI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubSnapshot {
    pub available: bool,
    pub organization: String,
    /// Open pull requests in the organization, most recently updated first.
    pub open_pull_requests: u32,
    pub pull_requests: Vec<GithubPullRequest>,
    pub notifications: Vec<GithubNotification>,
}

/// Which agent manager the daemon observes and routes agent actions to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentBackend {
    #[default]
    Herdr,
    T3code,
}

impl AgentBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Herdr => "herdr",
            Self::T3code => "t3code",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBackendSnapshot {
    pub mode: AgentBackend,
    #[serde(default)]
    pub switchable: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOrderMode {
    #[default]
    Grouped,
    Priority,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentOrderSnapshot {
    pub available: bool,
    pub mode: AgentOrderMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionCapability {
    pub capability_id: String,
    pub kind: ActionKind,
    pub expires_at_ms: u64,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentActions {
    #[serde(default)]
    pub open: bool,
    #[serde(default)]
    pub zoom: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approve: Option<ActionCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupt: Option<ActionCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentView {
    pub id: String,
    pub display_name: String,
    pub agent: String,
    pub status: AgentStatus,
    #[serde(default)]
    pub review_ready: bool,
    #[serde(default)]
    pub launch_pending: bool,
    pub workspace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    /// Checked-out branch (or short commit when detached), read from the
    /// agent's working directory; never from terminal content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub session: String,
    pub focused: bool,
    pub observed_for_seconds: u64,
    #[serde(default)]
    pub state_change_seq: u64,
    pub source_order: usize,
    pub actions: AgentActions,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageKind {
    #[default]
    Quota,
    LocalBudget,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageWindow {
    pub label: String,
    pub utilization: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub id: String,
    pub label: String,
    pub kind: UsageKind,
    pub available: bool,
    pub stale: bool,
    pub source: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<UsageWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary: Option<UsageWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub today_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_per_hour: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AiUsageSnapshot {
    pub providers: Vec<ProviderUsage>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicroSnapshot {
    pub connected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery: Option<u8>,
    #[serde(default)]
    pub charging: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated_ms: Option<u64>,
}

pub fn sort_agents(agents: &mut [AgentView]) {
    agents.sort_by(|left, right| {
        agent_attention_rank(left)
            .cmp(&agent_attention_rank(right))
            .then_with(|| right.state_change_seq.cmp(&left.state_change_seq))
            .then_with(|| left.source_order.cmp(&right.source_order))
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn agent_attention_rank(agent: &AgentView) -> u8 {
    if agent.review_ready && matches!(agent.status, AgentStatus::Done | AgentStatus::Idle) {
        AgentStatus::Done.attention_rank()
    } else if agent.status == AgentStatus::Done {
        AgentStatus::Idle.attention_rank()
    } else {
        agent.status.attention_rank()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionView {
    pub name: String,
    pub state: SessionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metric {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    pub unit: String,
}

impl Metric {
    pub fn unavailable(unit: impl Into<String>) -> Self {
        Self {
            available: false,
            value: None,
            unit: unit.into(),
        }
    }

    pub fn available(value: f64, unit: impl Into<String>) -> Self {
        Self {
            available: true,
            value: Some(value),
            unit: unit.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthSnapshot {
    pub cpu: Metric,
    pub cpu_temperature: Metric,
    pub gpu: Metric,
    pub gpu_temperature: Metric,
    pub memory: Metric,
    pub network_down: Metric,
    pub network_up: Metric,
    pub battery: Metric,
}

impl Default for HealthSnapshot {
    fn default() -> Self {
        Self {
            cpu: Metric::unavailable("%"),
            cpu_temperature: Metric::unavailable("°C"),
            gpu: Metric::unavailable("%"),
            gpu_temperature: Metric::unavailable("°C"),
            memory: Metric::unavailable("%"),
            network_down: Metric::unavailable("B/s"),
            network_up: Metric::unavailable("B/s"),
            battery: Metric::unavailable("%"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortalSnapshot {
    pub schema_version: u32,
    pub sequence: u64,
    pub daemon_epoch: String,
    pub generated_at_ms: u64,
    pub connection: ConnectionState,
    pub sessions: Vec<SessionView>,
    pub agents: Vec<AgentView>,
    #[serde(default)]
    pub agent_order: AgentOrderSnapshot,
    #[serde(default)]
    pub backend: AgentBackendSnapshot,
    /// Header shortcuts from the daemon's `desktop.apps` configuration.
    #[serde(default)]
    pub desktop_apps: Vec<DesktopAppView>,
    /// Enabled dashboards in display order; always at least one.
    #[serde(default)]
    pub dashboards: Vec<DashboardKind>,
    /// Populated only while the Spotify dashboard is enabled.
    #[serde(default)]
    pub spotify: SpotifySnapshot,
    /// Lyrics for the playing track, while the Spotify dashboard is enabled
    /// and `spotify.lyrics` is on.
    #[serde(default)]
    pub lyrics: LyricsSnapshot,
    /// Populated only while the Basecamp dashboard is enabled.
    #[serde(default)]
    pub basecamp: BasecampSnapshot,
    /// Populated only while the Slack dashboard is enabled.
    #[serde(default)]
    pub slack: SlackSnapshot,
    /// Populated only while the GitHub dashboard is enabled.
    #[serde(default)]
    pub github: GithubSnapshot,
    pub voice: VoiceSnapshot,
    pub usage: AiUsageSnapshot,
    pub micro: MicroSnapshot,
    pub health: HealthSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopAppView {
    pub id: String,
    pub label: String,
    pub accent: String,
}

impl PortalSnapshot {
    pub fn empty(epoch: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            sequence: 0,
            daemon_epoch: epoch.into(),
            generated_at_ms: 0,
            connection: ConnectionState::Offline,
            sessions: Vec::new(),
            agents: Vec::new(),
            agent_order: AgentOrderSnapshot::default(),
            backend: AgentBackendSnapshot::default(),
            desktop_apps: Vec::new(),
            dashboards: vec![DashboardKind::Agents],
            spotify: SpotifySnapshot::default(),
            lyrics: LyricsSnapshot::default(),
            basecamp: BasecampSnapshot::default(),
            slack: SlackSnapshot::default(),
            github: GithubSnapshot::default(),
            voice: VoiceSnapshot::default(),
            usage: AiUsageSnapshot::default(),
            micro: MicroSnapshot::default(),
            health: HealthSnapshot::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortalCommand {
    pub schema_version: u32,
    #[serde(default)]
    pub request_id: String,
    pub sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub action: ActionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_id: Option<String>,
    /// Configured `desktop.apps` id; accepted only by `desktop_app`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// Published Basecamp notification id; accepted only by `basecamp_open`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionResult {
    pub schema_version: u32,
    pub request_id: String,
    pub ok: bool,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Snapshot {
        #[serde(flatten)]
        snapshot: Box<PortalSnapshot>,
    },
    ActionResult {
        #[serde(flatten)]
        result: ActionResult,
    },
    Notice {
        schema_version: u32,
        level: String,
        code: String,
        message: String,
    },
}

impl PartialOrd for AgentStatus {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AgentStatus {
    fn cmp(&self, other: &Self) -> Ordering {
        self.attention_rank().cmp(&other.attention_rank())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, status: AgentStatus, source_order: usize) -> AgentView {
        AgentView {
            id: id.into(),
            display_name: id.into(),
            agent: "codex".into(),
            status,
            review_ready: false,
            launch_pending: false,
            workspace: "workspace".into(),
            repository: None,
            worktree: None,
            branch: None,
            session: "default".into(),
            focused: false,
            observed_for_seconds: 0,
            state_change_seq: 0,
            source_order,
            actions: AgentActions::default(),
        }
    }

    #[test]
    fn attention_sort_uses_newest_change_then_source_order_within_status() {
        let mut done = agent("done", AgentStatus::Done, 1);
        done.review_ready = true;
        let mut agents = vec![
            agent("idle", AgentStatus::Idle, 0),
            agent("working-2", AgentStatus::Working, 3),
            agent("blocked", AgentStatus::Blocked, 9),
            agent("working-1", AgentStatus::Working, 2),
            done,
        ];

        sort_agents(&mut agents);

        let ids: Vec<_> = agents.iter().map(|agent| agent.id.as_str()).collect();
        assert_eq!(ids, ["blocked", "done", "working-1", "working-2", "idle"]);

        let mut older = agent("older", AgentStatus::Working, 0);
        older.state_change_seq = 3;
        let mut newer = agent("newer", AgentStatus::Working, 1);
        newer.state_change_seq = 9;
        let mut tied = vec![older, newer];
        sort_agents(&mut tied);
        assert_eq!(tied[0].id, "newer");
    }

    #[test]
    fn unknown_status_is_forward_compatible() {
        let status: AgentStatus = serde_json::from_str("\"future_state\"").unwrap();
        assert_eq!(status, AgentStatus::Unknown);
    }

    #[test]
    fn review_ready_idle_sorts_with_done_priority() {
        let mut review = agent("review", AgentStatus::Idle, 1);
        review.review_ready = true;
        let mut agents = vec![
            agent("working", AgentStatus::Working, 0),
            review,
            agent("blocked", AgentStatus::Blocked, 2),
        ];

        sort_agents(&mut agents);

        let ids: Vec<_> = agents.iter().map(|agent| agent.id.as_str()).collect();
        assert_eq!(ids, ["blocked", "review", "working"]);
    }

    #[test]
    fn active_attention_state_wins_an_inconsistent_review_latch() {
        let mut blocked = agent("blocked", AgentStatus::Blocked, 0);
        blocked.review_ready = true;
        let mut agents = vec![
            agent("done", AgentStatus::Done, 1),
            blocked,
            agent("working", AgentStatus::Working, 2),
        ];

        sort_agents(&mut agents);

        let ids: Vec<_> = agents.iter().map(|agent| agent.id.as_str()).collect();
        assert_eq!(ids, ["blocked", "working", "done"]);
    }

    #[test]
    fn acknowledged_done_sorts_as_idle() {
        let mut review = agent("review", AgentStatus::Idle, 2);
        review.review_ready = true;
        let mut agents = vec![
            agent("done", AgentStatus::Done, 0),
            agent("working", AgentStatus::Working, 1),
            review,
        ];

        sort_agents(&mut agents);

        let ids: Vec<_> = agents.iter().map(|agent| agent.id.as_str()).collect();
        assert_eq!(ids, ["review", "working", "done"]);
    }

    #[test]
    fn snapshot_message_is_flat_and_tagged() {
        let value = serde_json::to_value(ServerMessage::Snapshot {
            snapshot: Box::new(PortalSnapshot::empty("epoch")),
        })
        .unwrap();

        assert_eq!(value["type"], "snapshot");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["daemon_epoch"], "epoch");
        assert_eq!(value["backend"]["mode"], "herdr");
        assert_eq!(value["backend"]["switchable"], false);
    }

    #[test]
    fn backend_names_are_stable_wire_values() {
        assert_eq!(
            serde_json::to_value(AgentBackend::T3code).unwrap(),
            serde_json::json!("t3code")
        );
        assert_eq!(
            serde_json::from_str::<AgentBackend>("\"herdr\"").unwrap(),
            AgentBackend::Herdr
        );
        assert_eq!(AgentBackend::T3code.as_str(), "t3code");
        let legacy: PortalSnapshot = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "sequence": 1,
            "daemon_epoch": "epoch",
            "generated_at_ms": 0,
            "connection": "offline",
            "sessions": [],
            "agents": [],
            "voice": {"state": "unavailable"},
            "usage": {"providers": []},
            "micro": {"connected": false},
            "health": HealthSnapshot::default()
        }))
        .unwrap();
        assert_eq!(legacy.backend, AgentBackendSnapshot::default());
    }
}
