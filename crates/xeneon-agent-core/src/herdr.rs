use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    process::Command,
    sync::{mpsc, oneshot},
    time::timeout,
};
use uuid::Uuid;

use crate::model::{
    ActionCapability, ActionKind, AgentActions, AgentOrderMode, AgentStatus, AgentView,
    SessionState, SessionView,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
// Protocol 20 is the Omarchy Herdr v0.8.0.r13 base; protocol 22 is stock
// Herdr v0.9.1, which serves the same snapshot, subscription, focus, and zoom
// shapes but none of the fork-only ordering or capability APIs. Protocol 21 is
// deliberately absent: upstream and the XENEON fork both report it, so it does
// not identify which API series the server carries.
const SUPPORTED_HERDR_PROTOCOLS: [u32; 2] = [20, 22];
const STOCK_HERDR_PROTOCOL: u32 = 22;

fn supported_protocols_label() -> String {
    SUPPORTED_HERDR_PROTOCOLS
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(" or ")
}

#[derive(Debug, Clone)]
pub struct HerdrClient {
    binary: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionDescriptor {
    pub name: String,
    pub running: bool,
    pub socket_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct HerdrTarget {
    pub session: String,
    pub socket_path: PathBuf,
    pub pane_id: String,
    pub terminal_id: String,
    pub state_change_seq: u64,
    pub revision: u64,
}

#[derive(Debug)]
pub struct SessionObservation {
    pub session: SessionView,
    pub agents: Vec<AgentView>,
    pub targets: HashMap<String, HerdrTarget>,
    pub pane_ids: Vec<String>,
    pub agent_order: Option<AgentOrderMode>,
    /// The server has no ordering API at all, so the daemon owns the
    /// preference instead of reporting ordering as unavailable.
    pub daemon_owned_order: bool,
}

#[derive(Debug, Deserialize)]
struct SessionList {
    sessions: Vec<SessionDescriptor>,
}

#[derive(Debug, Deserialize)]
struct PingResult {
    result: Pong,
}

#[derive(Debug, Deserialize)]
struct Pong {
    version: String,
    protocol: u32,
}

#[derive(Debug, Deserialize)]
struct SnapshotResponse {
    result: SnapshotResult,
}

#[derive(Debug, Deserialize)]
struct SnapshotResult {
    snapshot: HerdrSnapshot,
}

#[derive(Debug, Deserialize)]
struct HerdrSnapshot {
    #[serde(default)]
    workspaces: Vec<HerdrWorkspace>,
    #[serde(default)]
    tabs: Vec<HerdrTab>,
    #[serde(default)]
    agents: Vec<HerdrAgent>,
}

#[derive(Debug, Deserialize)]
struct HerdrWorkspace {
    workspace_id: String,
    label: String,
    #[serde(default)]
    worktree: Option<HerdrWorkspaceWorktree>,
}

#[derive(Debug, Deserialize)]
struct HerdrWorkspaceWorktree {
    repo_name: String,
    checkout_path: PathBuf,
    #[serde(default)]
    is_linked_worktree: bool,
}

#[derive(Debug, Deserialize)]
struct HerdrTab {
    tab_id: String,
    #[serde(default)]
    label: String,
}

#[derive(Debug, Clone, Deserialize)]
struct HerdrAgent {
    terminal_id: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    display_agent: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    agent_status: AgentStatus,
    workspace_id: String,
    #[serde(default)]
    tab_id: String,
    pane_id: String,
    #[serde(default)]
    focused: bool,
    #[serde(default)]
    launch_pending: bool,
    #[serde(default)]
    state_change_seq: u64,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    foreground_cwd: Option<PathBuf>,
    #[serde(default)]
    revision: u64,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

impl HerdrClient {
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
        }
    }

    pub async fn discover(&self) -> Result<Vec<SessionDescriptor>> {
        let output = Command::new(&self.binary)
            .args(["session", "list", "--json"])
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await
            .with_context(|| format!("running {} session list", self.binary.display()))?;
        if !output.status.success() {
            bail!(
                "herdr session list failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let mut sessions: SessionList =
            serde_json::from_slice(&output.stdout).context("parsing Herdr session list")?;
        sessions.sessions.retain(|session| session.running);
        Ok(sessions.sessions)
    }

    pub async fn observe_session(
        &self,
        descriptor: &SessionDescriptor,
        daemon_epoch: &str,
        source_offset: usize,
        now_ms: u64,
    ) -> Result<SessionObservation> {
        let ping: PingResult = request(
            &descriptor.socket_path,
            json!({"id": request_id(), "method": "ping", "params": {}}),
        )
        .await?;
        if !SUPPORTED_HERDR_PROTOCOLS.contains(&ping.result.protocol) {
            return Ok(SessionObservation {
                session: SessionView {
                    name: descriptor.name.clone(),
                    state: SessionState::Incompatible,
                    version: Some(ping.result.version),
                    protocol: Some(ping.result.protocol),
                    last_sync_ms: Some(now_ms),
                    message: Some(format!(
                        "Herdr protocol {} is unsupported; expected {}",
                        ping.result.protocol,
                        supported_protocols_label()
                    )),
                },
                agents: Vec::new(),
                targets: HashMap::new(),
                pane_ids: Vec::new(),
                agent_order: None,
                daemon_owned_order: false,
            });
        }
        let response: SnapshotResponse = request(
            &descriptor.socket_path,
            json!({"id": request_id(), "method": "session.snapshot", "params": {}}),
        )
        .await?;
        // Stock Herdr has no ordering API, so asking would only return an
        // unknown-method error on every refresh.
        let daemon_owned_order = ping.result.protocol == STOCK_HERDR_PROTOCOL;
        let agent_order = if daemon_owned_order {
            None
        } else {
            match self.get_agent_order(descriptor).await {
                Ok(mode) => Some(mode),
                Err(error) => {
                    tracing::debug!(session = %descriptor.name, %error, "Herdr agent ordering API unavailable");
                    None
                }
            }
        };

        let workspaces: HashMap<_, _> = response
            .result
            .snapshot
            .workspaces
            .into_iter()
            .map(|workspace| (workspace.workspace_id.clone(), workspace))
            .collect();
        let tabs: HashMap<_, _> = response
            .result
            .snapshot
            .tabs
            .into_iter()
            .map(|tab| (tab.tab_id, tab.label))
            .collect();
        let mut targets = HashMap::new();
        let mut pane_ids = Vec::new();
        let mut agents = Vec::new();

        for (index, raw) in response.result.snapshot.agents.into_iter().enumerate() {
            let id = opaque_agent_id(daemon_epoch, &descriptor.name, &raw.terminal_id);
            let workspace = workspaces.get(&raw.workspace_id);
            // Herdr's workspace number means nothing on the EDGE; its name does.
            let workspace_label = workspace
                .and_then(|workspace| nonblank(Some(workspace.label.as_str())))
                .map_or_else(|| raw.workspace_id.clone(), str::to_owned);
            let agent_name = nonblank(raw.agent.as_deref()).unwrap_or("agent").to_owned();
            let display_name = display_name(
                &raw,
                &tabs,
                &agent_name,
                workspace.map(|workspace| workspace.label.as_str()),
            );
            let cwd = raw.foreground_cwd.as_ref().or(raw.cwd.as_ref());
            let repository = workspace
                .and_then(|workspace| workspace.worktree.as_ref())
                .map(|worktree| worktree.repo_name.clone())
                .or_else(|| {
                    cwd.and_then(|path| path.file_name())
                        .map(|name| name.to_string_lossy().into_owned())
                });
            let worktree = workspace
                .and_then(|workspace| workspace.worktree.as_ref())
                .filter(|worktree| worktree.is_linked_worktree)
                .and_then(|worktree| worktree.checkout_path.file_name())
                .map(|name| name.to_string_lossy().into_owned());
            let branch = cwd.and_then(|path| git_branch(path));
            let actions = actions_from_extra(&raw.extra, raw.revision);

            targets.insert(
                id.clone(),
                HerdrTarget {
                    session: descriptor.name.clone(),
                    socket_path: descriptor.socket_path.clone(),
                    pane_id: raw.pane_id.clone(),
                    terminal_id: raw.terminal_id,
                    state_change_seq: raw.state_change_seq,
                    revision: raw.revision,
                },
            );
            pane_ids.push(raw.pane_id);
            agents.push(AgentView {
                id,
                display_name,
                agent: agent_name,
                status: raw.agent_status,
                review_ready: false,
                launch_pending: raw.launch_pending,
                workspace: workspace_label,
                repository,
                worktree,
                branch,
                session: descriptor.name.clone(),
                focused: raw.focused,
                observed_for_seconds: 0,
                state_change_seq: raw.state_change_seq,
                source_order: source_offset + index,
                actions: AgentActions {
                    open: true,
                    zoom: true,
                    ..actions
                },
            });
        }

        Ok(SessionObservation {
            session: SessionView {
                name: descriptor.name.clone(),
                state: SessionState::Connected,
                version: Some(ping.result.version),
                protocol: Some(ping.result.protocol),
                last_sync_ms: Some(now_ms),
                message: None,
            },
            agents,
            targets,
            pane_ids,
            agent_order,
            daemon_owned_order,
        })
    }

    pub async fn set_agent_order(
        &self,
        descriptor: &SessionDescriptor,
        mode: AgentOrderMode,
    ) -> Result<()> {
        let order = match mode {
            AgentOrderMode::Grouped => "grouped",
            AgentOrderMode::Priority => "priority",
        };
        let response: Value = request(
            &descriptor.socket_path,
            json!({
                "id": request_id(),
                "method": "agent.order.set",
                "params": {"order": order}
            }),
        )
        .await?;
        if let Some(error) = response.get("error") {
            bail!("Herdr agent ordering failed: {error}");
        }
        if response["result"]["type"] != "agent_order" || response["result"]["order"] != order {
            bail!("unexpected Herdr agent ordering response");
        }
        Ok(())
    }

    pub async fn get_agent_order(&self, descriptor: &SessionDescriptor) -> Result<AgentOrderMode> {
        read_agent_order(&descriptor.socket_path).await
    }

    pub async fn perform(
        &self,
        target: &HerdrTarget,
        action: ActionKind,
        capability_id: Option<&str>,
    ) -> Result<()> {
        let payload = match action {
            ActionKind::Open => {
                json!({"id": request_id(), "method": "agent.focus", "params": {"target": target.pane_id}})
            }
            ActionKind::Zoom => {
                json!({"id": request_id(), "method": "pane.zoom", "params": {"pane_id": target.pane_id, "mode": "toggle"}})
            }
            ActionKind::Approve | ActionKind::Interrupt => {
                let capability_id = capability_id.ok_or_else(|| anyhow!("missing capability"))?;
                json!({"id": request_id(), "method": "agent.perform_action", "params": {"capability_id": capability_id}})
            }
            ActionKind::RestoreFocus
            | ActionKind::DesktopApp
            | ActionKind::SpotifyPlayPause
            | ActionKind::SpotifyNext
            | ActionKind::SpotifyPrevious
            | ActionKind::BasecampOpen
            | ActionKind::SlackOpen
            | ActionKind::GithubOpen
            | ActionKind::GameShow => {
                bail!("action is owned by the desktop adapter")
            }
            ActionKind::VoiceStart | ActionKind::VoiceStop | ActionKind::VoiceCancel => {
                bail!("voice action is not a Herdr action")
            }
            ActionKind::OrderGrouped | ActionKind::OrderPriority => {
                bail!("agent ordering is not an agent-target action")
            }
            ActionKind::BackendHerdr | ActionKind::BackendT3code => {
                bail!("agent manager selection is not an agent-target action")
            }
        };
        let response: Value = request(&target.socket_path, payload).await?;
        if let Some(error) = response.get("error") {
            bail!("Herdr action failed: {error}");
        }
        Ok(())
    }

    pub fn spawn_subscription(
        &self,
        descriptor: SessionDescriptor,
        pane_ids: Vec<String>,
        invalidations: mpsc::Sender<String>,
    ) -> (
        tokio::task::JoinHandle<()>,
        oneshot::Receiver<Result<(), String>>,
    ) {
        let (ready_tx, ready_rx) = oneshot::channel();
        let handle = tokio::spawn(async move {
            if let Err(error) = watch_events(&descriptor, &pane_ids, &invalidations, ready_tx).await
            {
                tracing::warn!(session = %descriptor.name, %error, "Herdr event subscription ended");
            }
            let _ = invalidations.try_send(descriptor.name);
        });
        (handle, ready_rx)
    }
}

async fn read_agent_order(socket_path: &Path) -> Result<AgentOrderMode> {
    let response: Value = request(
        socket_path,
        json!({"id": request_id(), "method": "agent.order.get", "params": {}}),
    )
    .await?;
    if let Some(error) = response.get("error") {
        bail!("Herdr agent ordering failed: {error}");
    }
    if response["result"]["type"] != "agent_order" {
        bail!("unexpected Herdr agent ordering response");
    }
    let mode = match response["result"]["order"].as_str() {
        Some("grouped") => AgentOrderMode::Grouped,
        Some("priority") => AgentOrderMode::Priority,
        _ => bail!("unexpected Herdr agent ordering response"),
    };
    Ok(mode)
}

/// The name given to the agent's pane wins, then a named tab. Herdr numbers
/// unnamed tabs, so an all-digit label says nothing; with neither, the card
/// is `<agent> · <workspace>`. A name equal to the agent type counts as none.
fn display_name(
    raw: &HerdrAgent,
    tabs: &HashMap<String, String>,
    agent_name: &str,
    workspace: Option<&str>,
) -> String {
    let given = nonblank(raw.display_agent.as_deref())
        .or_else(|| nonblank(raw.name.as_deref()))
        .filter(|name| !name.eq_ignore_ascii_case(agent_name));
    if let Some(name) = given {
        return name.to_owned();
    }
    if let Some(label) = tabs
        .get(&raw.tab_id)
        .and_then(|label| nonblank(Some(label)))
        .filter(|label| !label.chars().all(|character| character.is_ascii_digit()))
    {
        return label.to_owned();
    }
    match nonblank(workspace) {
        Some(workspace) => format!("{agent_name} · {workspace}"),
        None => agent_name.to_owned(),
    }
}

/// The branch checked out in the repository containing `start`, read from
/// git's own files (a `.git` directory, or a worktree's `gitdir:` pointer)
/// without running git. A detached HEAD yields its short commit.
fn git_branch(start: &Path) -> Option<String> {
    let mut directory = Some(start);
    for _ in 0..32 {
        let current = directory?;
        let dot_git = current.join(".git");
        if let Ok(metadata) = std::fs::metadata(&dot_git) {
            let git_dir = if metadata.is_dir() {
                dot_git
            } else {
                let pointer = std::fs::read_to_string(&dot_git).ok()?;
                let target = pointer.strip_prefix("gitdir:")?.trim();
                let target = Path::new(target);
                if target.is_absolute() {
                    target.to_path_buf()
                } else {
                    current.join(target)
                }
            };
            let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
            let head = head.trim();
            let branch = match head.strip_prefix("ref: refs/heads/") {
                Some(branch) => branch.to_owned(),
                None if head.len() >= 7 && head.bytes().all(|byte| byte.is_ascii_hexdigit()) => {
                    head[..7].to_owned()
                }
                None => return None,
            };
            let safe = !branch.is_empty()
                && branch.len() <= 64
                && branch
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_./+".contains(&byte));
            return safe.then_some(branch);
        }
        directory = current.parent();
    }
    None
}

fn nonblank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn actions_from_extra(extra: &HashMap<String, Value>, revision: u64) -> AgentActions {
    let values = extra.get("actions").and_then(Value::as_array);
    let Some(values) = values else {
        return AgentActions::default();
    };

    let mut actions = AgentActions::default();
    for value in values {
        let Some(kind) = value.get("action").and_then(Value::as_str) else {
            continue;
        };
        let Some(capability_id) = value.get("capability_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(action_kind) = (match kind {
            "approve" => Some(ActionKind::Approve),
            "interrupt" => Some(ActionKind::Interrupt),
            _ => None,
        }) else {
            continue;
        };
        let capability = ActionCapability {
            capability_id: capability_id.to_owned(),
            kind: action_kind,
            expires_at_ms: value
                .get("expires_at_unix_ms")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            expected_revision: value
                .get("revision")
                .and_then(Value::as_u64)
                .unwrap_or(revision),
        };
        match action_kind {
            ActionKind::Approve => actions.approve = Some(capability),
            ActionKind::Interrupt => actions.interrupt = Some(capability),
            _ => {}
        }
    }
    actions
}

async fn watch_events(
    descriptor: &SessionDescriptor,
    pane_ids: &[String],
    invalidations: &mpsc::Sender<String>,
    ready: oneshot::Sender<Result<(), String>>,
) -> Result<()> {
    let mut subscriptions = vec![
        json!({"type": "workspace.created"}),
        json!({"type": "workspace.updated"}),
        json!({"type": "workspace.renamed"}),
        json!({"type": "workspace.moved"}),
        json!({"type": "workspace.closed"}),
        json!({"type": "workspace.focused"}),
        json!({"type": "tab.created"}),
        json!({"type": "tab.closed"}),
        json!({"type": "tab.focused"}),
        json!({"type": "tab.renamed"}),
        json!({"type": "tab.moved"}),
        json!({"type": "pane.created"}),
        json!({"type": "pane.closed"}),
        json!({"type": "pane.focused"}),
        json!({"type": "pane.moved"}),
        json!({"type": "pane.exited"}),
        json!({"type": "pane.agent_detected"}),
    ];
    subscriptions.extend(
        pane_ids
            .iter()
            .map(|pane_id| json!({"type": "pane.agent_status_changed", "pane_id": pane_id})),
    );

    let stream = match timeout(
        REQUEST_TIMEOUT,
        UnixStream::connect(&descriptor.socket_path),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            let message = format!(
                "connecting subscription {}: {error}",
                descriptor.socket_path.display()
            );
            let _ = ready.send(Err(message.clone()));
            bail!(message);
        }
        Err(_) => {
            let message = "timed out connecting subscription".to_owned();
            let _ = ready.send(Err(message.clone()));
            bail!(message);
        }
    };
    let (read, mut write) = stream.into_split();
    let request = json!({
        "id": request_id(),
        "method": "events.subscribe",
        "params": {"subscriptions": subscriptions}
    });
    write
        .write_all(serde_json::to_string(&request)?.as_bytes())
        .await?;
    write.write_all(b"\n").await?;
    write.flush().await?;

    let mut lines = BufReader::new(read).lines();
    let acknowledgement = match timeout(REQUEST_TIMEOUT, lines.next_line()).await {
        Ok(Ok(Some(line))) => line,
        Ok(Ok(None)) => {
            let message = "subscription closed before acknowledgement".to_owned();
            let _ = ready.send(Err(message.clone()));
            bail!(message);
        }
        Ok(Err(error)) => {
            let message = format!("reading subscription acknowledgement: {error}");
            let _ = ready.send(Err(message.clone()));
            bail!(message);
        }
        Err(_) => {
            let message = "timed out waiting for subscription acknowledgement".to_owned();
            let _ = ready.send(Err(message.clone()));
            bail!(message);
        }
    };
    let acknowledgement: Value = match serde_json::from_str(&acknowledgement) {
        Ok(value) => value,
        Err(error) => {
            let message = format!("parsing subscription acknowledgement: {error}");
            let _ = ready.send(Err(message.clone()));
            bail!(message);
        }
    };
    if acknowledgement["result"]["type"] != "subscription_started" {
        let message = format!("unexpected subscription acknowledgement: {acknowledgement}");
        let _ = ready.send(Err(message.clone()));
        bail!(message);
    }
    let _ = ready.send(Ok(()));

    while let Some(line) = lines.next_line().await? {
        if serde_json::from_str::<Value>(&line).is_ok() {
            let _ = invalidations.try_send(descriptor.name.clone());
        }
    }
    bail!("Herdr subscription reached EOF")
}

async fn request<T: for<'de> Deserialize<'de>>(socket: &Path, request: Value) -> Result<T> {
    let stream = timeout(REQUEST_TIMEOUT, UnixStream::connect(socket))
        .await
        .context("timed out connecting to Herdr")?
        .with_context(|| format!("connecting to Herdr socket {}", socket.display()))?;
    let (read, mut write) = stream.into_split();
    let encoded = serde_json::to_vec(&request)?;
    write.write_all(&encoded).await?;
    write.write_all(b"\n").await?;
    write.flush().await?;

    let mut lines = BufReader::new(read).lines();
    let response = timeout(REQUEST_TIMEOUT, lines.next_line())
        .await
        .context("timed out waiting for Herdr response")??
        .ok_or_else(|| anyhow!("Herdr closed without a response"))?;
    serde_json::from_str(&response).with_context(|| format!("parsing Herdr response: {response}"))
}

fn opaque_agent_id(epoch: &str, session: &str, terminal_id: &str) -> String {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("{epoch}\0{session}\0{terminal_id}").as_bytes(),
    )
    .to_string()
}

fn request_id() -> String {
    format!("xeneon-{}", Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::UnixListener,
        sync::{mpsc, oneshot},
    };

    #[test]
    fn opaque_ids_change_with_daemon_epoch() {
        let first = opaque_agent_id("one", "default", "terminal");
        let second = opaque_agent_id("two", "default", "terminal");
        assert_ne!(first, second);
        assert_eq!(first, opaque_agent_id("one", "default", "terminal"));
    }

    #[test]
    fn legacy_capability_shapes_never_surface_guarded_actions() {
        let mut extra = HashMap::new();
        extra.insert(
            "action_capabilities".into(),
            json!([{"id":"unsafe","kind":"approve","expires_at_ms":99}]),
        );
        let actions = actions_from_extra(&extra, 1);
        assert!(actions.approve.is_none());
        assert!(actions.interrupt.is_none());
    }

    #[test]
    fn public_actions_parse_only_known_capabilities() {
        let mut extra = HashMap::new();
        extra.insert(
            "actions".into(),
            json!([
                {"capability_id":"approve-1","action":"approve","expires_at_unix_ms":99,"revision":7},
                {"capability_id":"interrupt-1","action":"interrupt","expires_at_unix_ms":101,"revision":8},
                {"capability_id":"other","action":"shell","expires_at_unix_ms":99}
            ]),
        );
        let actions = actions_from_extra(&extra, 3);
        assert_eq!(
            actions
                .approve
                .as_ref()
                .map(|value| value.capability_id.as_str()),
            Some("approve-1")
        );
        assert_eq!(
            actions.interrupt.as_ref().map(|value| (
                value.capability_id.as_str(),
                value.expires_at_ms,
                value.expected_revision
            )),
            Some(("interrupt-1", 101, 8))
        );
    }

    #[test]
    fn pane_name_wins_over_the_tab_label_in_card_identity() {
        let response: SnapshotResponse = serde_json::from_value(json!({
            "result": {
                "snapshot": {
                    "tabs": [{"tab_id": "w1:t2", "label": "  review portal  "}],
                    "agents": [{
                        "terminal_id": "term",
                        "agent": "codex",
                        "name": "agent name",
                        "display_agent": "display agent",
                        "title": "private prompt title",
                        "terminal_title_stripped": "private terminal title",
                        "workspace_id": "w1",
                        "tab_id": "w1:t2",
                        "pane_id": "w1:p2"
                    }]
                }
            }
        }))
        .unwrap();
        let raw = &response.result.snapshot.agents[0];
        let tabs = response
            .result
            .snapshot
            .tabs
            .into_iter()
            .map(|tab| (tab.tab_id, tab.label))
            .collect();

        assert_eq!(
            display_name(raw, &tabs, "codex", Some("xeneon")),
            "display agent"
        );
        let mut unnamed = raw.clone();
        unnamed.display_agent = None;
        unnamed.name = Some("codex".into());
        assert_eq!(
            display_name(&unnamed, &tabs, "codex", Some("xeneon")),
            "review portal"
        );
    }

    #[test]
    fn branch_is_read_from_git_files_including_worktrees() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(repo.join("src/deep")).unwrap();
        fs::write(repo.join(".git/HEAD"), "ref: refs/heads/feature/edge\n").unwrap();
        assert_eq!(
            git_branch(&repo.join("src/deep")).as_deref(),
            Some("feature/edge")
        );

        let worktree = temp.path().join("wt");
        let worktree_git = repo.join(".git/worktrees/wt");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&worktree_git).unwrap();
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", worktree_git.display()),
        )
        .unwrap();
        fs::write(
            worktree_git.join("HEAD"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .unwrap();
        assert_eq!(git_branch(&worktree).as_deref(), Some("0123456"));

        fs::write(repo.join(".git/HEAD"), "ref: refs/heads/evil\u{1b}[2J\n").unwrap();
        assert_eq!(git_branch(&repo), None);
        assert_eq!(git_branch(&temp.path().join("nowhere")), None);
    }

    #[test]
    fn card_identity_uses_only_safe_fallback_fields() {
        let raw: HerdrAgent = serde_json::from_value(json!({
            "terminal_id": "term",
            "agent": "codex",
            "name": "named agent",
            "display_agent": "  ",
            "title": "private prompt title",
            "terminal_title_stripped": "private terminal title",
            "workspace_id": "w1",
            "tab_id": "missing",
            "pane_id": "w1:p1"
        }))
        .unwrap();
        assert_eq!(
            display_name(&raw, &HashMap::new(), "codex", None),
            "named agent"
        );

        let canonical_only: HerdrAgent = serde_json::from_value(json!({
            "terminal_id": "term",
            "agent": "codex",
            "name": " ",
            "display_agent": "",
            "title": "private prompt title",
            "terminal_title_stripped": "private terminal title",
            "workspace_id": "w1",
            "tab_id": "missing",
            "pane_id": "w1:p1"
        }))
        .unwrap();
        assert_eq!(
            display_name(&canonical_only, &HashMap::new(), "codex", None),
            "codex"
        );

        // Herdr's default numbered tab names carry no meaning.
        let numbered = HashMap::from([("w1:t1".to_owned(), "1".to_owned())]);
        let mut in_numbered_tab = canonical_only.clone();
        in_numbered_tab.tab_id = "w1:t1".into();
        assert_eq!(
            display_name(&in_numbered_tab, &numbered, "codex", Some("lectern")),
            "codex · lectern"
        );
        let named = HashMap::from([("w1:t1".to_owned(), "review".to_owned())]);
        assert_eq!(
            display_name(&in_numbered_tab, &named, "codex", Some("lectern")),
            "review"
        );
    }

    async fn observe_unsupported_protocol(protocol: u32) -> SessionObservation {
        let temp = tempdir().unwrap();
        let socket = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(request["method"], "ping");
            let response = format!(
                "{{\"id\":\"ping\",\"result\":{{\"version\":\"unsupported\",\"protocol\":{protocol}}}}}\n"
            );
            write.write_all(response.as_bytes()).await.unwrap();
        });
        let descriptor = SessionDescriptor {
            name: "unsupported".into(),
            running: true,
            socket_path: socket,
        };

        let observation = HerdrClient::new("herdr")
            .observe_session(&descriptor, "epoch", 0, 1234)
            .await
            .unwrap();
        server.await.unwrap();
        observation
    }

    #[tokio::test]
    async fn unsupported_protocols_never_request_or_surface_session_data() {
        for protocol in [19, 21, 23, 999] {
            let observation = observe_unsupported_protocol(protocol).await;
            assert_eq!(observation.session.state, SessionState::Incompatible);
            assert_eq!(observation.session.protocol, Some(protocol));
            assert!(observation.agents.is_empty());
            assert!(observation.targets.is_empty());
            assert!(observation.pane_ids.is_empty());
            assert_eq!(observation.agent_order, None);
        }
    }

    #[tokio::test]
    async fn herdr_v0_8_protocol_20_requests_and_parses_the_snapshot() {
        let temp = tempdir().unwrap();
        let socket = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(request["method"], "ping");
            write
                .write_all(
                    b"{\"id\":\"ping\",\"result\":{\"version\":\"0.8.0\",\"protocol\":20}}\n",
                )
                .await
                .unwrap();

            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(request["method"], "session.snapshot");
            write
                .write_all(
                    b"{\"id\":\"snapshot\",\"result\":{\"snapshot\":{\"workspaces\":[{\"workspace_id\":\"w1\",\"number\":1,\"label\":\"xeneon\"}],\"tabs\":[{\"tab_id\":\"w1:t1\",\"label\":\"review\"}],\"agents\":[{\"terminal_id\":\"term-1\",\"agent\":\"codex\",\"agent_status\":\"blocked\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\",\"pane_id\":\"w1:p1\",\"revision\":4,\"state_change_seq\":9,\"actions\":[{\"capability_id\":\"interrupt-1\",\"action\":\"interrupt\",\"expires_at_unix_ms\":999,\"revision\":4}]}]}}}\n",
                )
                .await
                .unwrap();

            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(request["method"], "agent.order.get");
            write
                .write_all(
                    b"{\"id\":\"order\",\"result\":{\"type\":\"agent_order\",\"order\":\"grouped\"}}\n",
                )
                .await
                .unwrap();
        });
        let descriptor = SessionDescriptor {
            name: "default".into(),
            running: true,
            socket_path: socket,
        };

        let observation = HerdrClient::new("herdr")
            .observe_session(&descriptor, "epoch", 0, 1234)
            .await
            .unwrap();

        assert_eq!(observation.session.state, SessionState::Connected);
        assert_eq!(observation.session.version.as_deref(), Some("0.8.0"));
        assert_eq!(observation.session.protocol, Some(20));
        assert_eq!(observation.agent_order, Some(AgentOrderMode::Grouped));
        assert_eq!(observation.agents.len(), 1);
        assert_eq!(observation.agents[0].display_name, "review");
        assert_eq!(observation.agents[0].status, AgentStatus::Blocked);
        assert_eq!(
            observation.agents[0]
                .actions
                .interrupt
                .as_ref()
                .map(|capability| capability.capability_id.as_str()),
            Some("interrupt-1")
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn stock_herdr_v0_9_protocol_22_connects_without_fork_apis() {
        let temp = tempdir().unwrap();
        let socket = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(request["method"], "ping");
            write
                .write_all(
                    b"{\"id\":\"ping\",\"result\":{\"type\":\"pong\",\"version\":\"0.9.1\",\"protocol\":22,\"capabilities\":{\"live_handoff\":true,\"health_check\":true}}}\n",
                )
                .await
                .unwrap();

            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(request["method"], "session.snapshot");
            write
                .write_all(
                    b"{\"id\":\"snapshot\",\"result\":{\"type\":\"session_snapshot\",\"snapshot\":{\"version\":\"0.9.1\",\"protocol\":22,\"focused_workspace_id\":\"w1\",\"workspaces\":[{\"workspace_id\":\"w1\",\"number\":1,\"label\":\"xeneon\",\"focused\":true,\"agent_status\":\"working\",\"pane_count\":1,\"tab_count\":1,\"active_tab_id\":\"w1:t1\"}],\"tabs\":[{\"tab_id\":\"w1:t1\",\"workspace_id\":\"w1\",\"number\":1,\"label\":\"review\",\"focused\":true,\"agent_status\":\"working\",\"pane_count\":1}],\"panes\":[],\"layouts\":[],\"agents\":[{\"terminal_id\":\"term-1\",\"agent\":\"claude\",\"name\":\"claude\",\"agent_status\":\"working\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\",\"pane_id\":\"w1:p1\",\"focused\":true,\"interactive_ready\":true,\"revision\":3,\"state_change_seq\":7,\"terminal_title\":\"t\",\"terminal_title_stripped\":\"t\"}]}}}\n",
                )
                .await
                .unwrap();

            // Stock Herdr has no ordering API, so no third request is made.
            assert!(
                timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        let descriptor = SessionDescriptor {
            name: "default".into(),
            running: true,
            socket_path: socket,
        };

        let observation = HerdrClient::new("herdr")
            .observe_session(&descriptor, "epoch", 0, 1234)
            .await
            .unwrap();

        assert_eq!(observation.session.state, SessionState::Connected);
        assert_eq!(observation.session.version.as_deref(), Some("0.9.1"));
        assert_eq!(observation.session.protocol, Some(22));
        assert_eq!(observation.agent_order, None);
        assert!(observation.daemon_owned_order);
        assert_eq!(observation.agents.len(), 1);
        assert_eq!(observation.agents[0].display_name, "review");
        assert_eq!(observation.agents[0].status, AgentStatus::Working);
        assert!(observation.agents[0].actions.interrupt.is_none());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn missing_agent_order_api_disables_only_ordering() {
        let temp = tempdir().unwrap();
        let socket = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            for (method, response) in [
                (
                    "ping",
                    "{\"result\":{\"version\":\"0.8.0\",\"protocol\":20}}\n",
                ),
                (
                    "session.snapshot",
                    "{\"result\":{\"snapshot\":{\"workspaces\":[],\"tabs\":[],\"agents\":[]}}}\n",
                ),
                (
                    "agent.order.get",
                    "{\"error\":{\"code\":\"unknown_method\",\"message\":\"unsupported\"}}\n",
                ),
            ] {
                let (stream, _) = listener.accept().await.unwrap();
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                let request: Value =
                    serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                assert_eq!(request["method"], method);
                write.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let descriptor = SessionDescriptor {
            name: "default".into(),
            running: true,
            socket_path: socket,
        };

        let observation = HerdrClient::new("herdr")
            .observe_session(&descriptor, "epoch", 0, 1234)
            .await
            .unwrap();

        assert_eq!(observation.session.state, SessionState::Connected);
        assert_eq!(observation.agent_order, None);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn set_agent_order_sends_only_the_typed_idempotent_mode() {
        let temp = tempdir().unwrap();
        let socket = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(request["method"], "agent.order.set");
            assert_eq!(request["params"], json!({"order": "priority"}));
            assert!(request["params"].get("text").is_none());
            assert!(request["params"].get("keys").is_none());
            write
                .write_all(b"{\"result\":{\"type\":\"agent_order\",\"order\":\"priority\"}}\n")
                .await
                .unwrap();
        });
        let descriptor = SessionDescriptor {
            name: "default".into(),
            running: true,
            socket_path: socket,
        };

        HerdrClient::new("herdr")
            .set_agent_order(&descriptor, AgentOrderMode::Priority)
            .await
            .unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn get_agent_order_rejects_a_wrong_result_type() {
        let temp = tempdir().unwrap();
        let socket = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(request["method"], "agent.order.get");
            write
                .write_all(b"{\"result\":{\"type\":\"agent_view\",\"order\":\"grouped\"}}\n")
                .await
                .unwrap();
        });
        let descriptor = SessionDescriptor {
            name: "default".into(),
            running: true,
            socket_path: socket,
        };

        let error = HerdrClient::new("herdr")
            .get_agent_order(&descriptor)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unexpected"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn subscription_acknowledges_before_forwarding_invalidations() {
        let temp = tempdir().unwrap();
        let socket = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(request["method"], "events.subscribe");
            let subscription_types = request["params"]["subscriptions"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|subscription| subscription["type"].as_str())
                .collect::<Vec<_>>();
            assert!(subscription_types.contains(&"pane.agent_status_changed"));
            assert!(!subscription_types.contains(&"pane.updated"));
            assert!(!subscription_types.contains(&"layout.updated"));
            assert!(!subscription_types.contains(&"workspace.metadata_updated"));
            write
                .write_all(b"{\"id\":\"sub\",\"result\":{\"type\":\"subscription_started\"}}\n")
                .await
                .unwrap();
            write
                .write_all(
                    b"{\"event\":\"pane.agent_status_changed\",\"data\":{\"pane_id\":\"w1:p1\"}}\n",
                )
                .await
                .unwrap();
        });
        let descriptor = SessionDescriptor {
            name: "default".into(),
            running: true,
            socket_path: socket,
        };
        let (invalidations_tx, mut invalidations_rx) = mpsc::channel(2);
        let (ready_tx, ready_rx) = oneshot::channel();
        let watcher = tokio::spawn(async move {
            watch_events(&descriptor, &["w1:p1".into()], &invalidations_tx, ready_tx).await
        });

        assert_eq!(ready_rx.await.unwrap(), Ok(()));
        assert_eq!(invalidations_rx.recv().await.as_deref(), Some("default"));
        server.await.unwrap();
        assert!(watcher.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn bad_subscription_ack_never_becomes_ready() {
        let temp = tempdir().unwrap();
        let socket = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let _ = lines.next_line().await.unwrap();
            write
                .write_all(b"{\"id\":\"sub\",\"error\":{\"code\":\"invalid\"}}\n")
                .await
                .unwrap();
        });
        let descriptor = SessionDescriptor {
            name: "default".into(),
            running: true,
            socket_path: socket,
        };
        let (invalidations_tx, _) = mpsc::channel(1);
        let (ready_tx, ready_rx) = oneshot::channel();
        let result = watch_events(&descriptor, &[], &invalidations_tx, ready_tx).await;
        assert!(result.is_err());
        assert!(ready_rx.await.unwrap().is_err());
        server.await.unwrap();
    }
}
