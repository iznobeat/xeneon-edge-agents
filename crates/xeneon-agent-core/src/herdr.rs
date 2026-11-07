//! Herdr adapter over herdr-core, the terminal runtime behind the Herdr app.
//!
//! Every read uses herdr-core's documented JSON-RPC 2.0 socket (`hc schema`,
//! protocol generation 1): `server.status`, `terminal.list`, and
//! `events.subscribe`. Agents are live terminals whose program reports an
//! OSC 7501 status and that belong to a Herdr app space, directly or as a
//! sub-agent of one. The only action is focus, sent to the Herdr app's own
//! command socket as `agent.focus`; herdr-core offers no ordering preference
//! and no guarded approve/interrupt capabilities, so the daemon owns ordering
//! and those actions stay unavailable.
//!
//! Status messages, titles, argv, and screen contents are never read into
//! the portal model: they can carry prompt text.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{
        UnixStream,
        unix::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::{mpsc, oneshot},
    time::timeout,
};
use uuid::Uuid;

use crate::model::{AgentActions, AgentStatus, AgentView, SessionState, SessionView};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
/// Labels the Herdr app writes on the terminals it shows.
const SPACE_LABEL: &str = "herdr.space";
const WHERE_LABEL: &str = "where";
/// Fact changes that cannot change a card. Titles repaint many times a
/// second while an agent works, so they must never trigger a refresh.
const IGNORED_EVENTS: [&str; 4] = ["title", "size", "turn", "keep"];

#[derive(Debug, Clone)]
pub struct HerdrClient {
    core_socket: PathBuf,
    app_socket: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SessionDescriptor {
    pub name: String,
    pub socket_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct HerdrTarget {
    pub session: String,
    pub app_socket: PathBuf,
    pub terminal: String,
    pub state_change_seq: u64,
}

#[derive(Debug)]
pub struct SessionObservation {
    pub session: SessionView,
    pub agents: Vec<AgentView>,
    pub targets: HashMap<String, HerdrTarget>,
}

#[derive(Debug, Deserialize)]
struct StatusResult {
    version: String,
}

#[derive(Debug, Deserialize)]
struct ListResult {
    terminals: Vec<CoreTerminal>,
}

#[derive(Debug, Clone, Deserialize)]
struct CoreTerminal {
    address: String,
    state: String,
    labels: HashMap<String, String>,
    cwd: PathBuf,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    status: Option<CoreStatus>,
    #[serde(default)]
    status_ms: Option<u64>,
    #[serde(default)]
    foreground: Option<Foreground>,
}

#[derive(Debug, Clone, Deserialize)]
struct CoreStatus {
    state: String,
    #[serde(default)]
    app: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Foreground {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
}

impl HerdrClient {
    pub fn new(core_socket: impl Into<PathBuf>, app_socket: impl Into<PathBuf>) -> Self {
        Self {
            core_socket: core_socket.into(),
            app_socket: app_socket.into(),
        }
    }

    /// The local herdr-core, when its socket exists. Discovery never starts a
    /// core: the `hc` CLI would, and a dashboard must not.
    pub async fn discover(&self) -> Result<Vec<SessionDescriptor>> {
        match tokio::fs::symlink_metadata(&self.core_socket).await {
            Ok(_) => Ok(vec![SessionDescriptor {
                name: session_name(&self.core_socket),
                socket_path: self.core_socket.clone(),
            }]),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error).with_context(|| {
                format!(
                    "inspecting herdr-core socket {}",
                    self.core_socket.display()
                )
            }),
        }
    }

    pub async fn observe_session(
        &self,
        descriptor: &SessionDescriptor,
        daemon_epoch: &str,
        source_offset: usize,
        now_ms: u64,
    ) -> Result<SessionObservation> {
        let status: StatusResult =
            core_request(&descriptor.socket_path, "server.status", json!({})).await?;
        let list: ListResult = core_request(
            &descriptor.socket_path,
            "terminal.list",
            json!({"all": false, "labels": {}}),
        )
        .await?;
        // Focus is best effort: the Herdr app window may be closed while its
        // agents keep running in the core.
        let focused_terminal = match self.focused_terminal().await {
            Ok(terminal) => terminal,
            Err(error) => {
                tracing::debug!(%error, "Herdr app focus is unavailable");
                None
            }
        };

        let (agents, targets) = build_agents(
            &list.terminals,
            &descriptor.name,
            &self.app_socket,
            focused_terminal.as_deref(),
            daemon_epoch,
            source_offset,
        );
        Ok(SessionObservation {
            session: SessionView {
                name: descriptor.name.clone(),
                state: SessionState::Connected,
                version: Some(status.version),
                protocol: None,
                last_sync_ms: Some(now_ms),
                message: None,
            },
            agents,
            targets,
        })
    }

    /// Shows the agent in the Herdr app. The app owns its window and layout;
    /// this sends only the typed `agent.focus` command for one terminal.
    pub async fn focus(&self, target: &HerdrTarget) -> Result<()> {
        let response = app_request(
            &target.app_socket,
            json!({"do": "agent.focus", "args": {"terminal": target.terminal}}),
        )
        .await?;
        if let Some(error) = response.get("error") {
            bail!("Herdr app refused agent focus: {error}");
        }
        if response.get("ok") != Some(&Value::Bool(true)) {
            bail!("unexpected Herdr app focus response");
        }
        Ok(())
    }

    pub fn spawn_subscription(
        &self,
        descriptor: SessionDescriptor,
        invalidations: mpsc::Sender<String>,
    ) -> (
        tokio::task::JoinHandle<()>,
        oneshot::Receiver<Result<(), String>>,
    ) {
        let (ready_tx, ready_rx) = oneshot::channel();
        let handle = tokio::spawn(async move {
            if let Err(error) = watch_events(&descriptor, &invalidations, ready_tx).await {
                tracing::warn!(session = %descriptor.name, %error, "herdr-core event subscription ended");
            }
        });
        (handle, ready_rx)
    }

    /// The terminal in the Herdr app's focused pane, if the app is running.
    async fn focused_terminal(&self) -> Result<Option<String>> {
        if tokio::fs::symlink_metadata(&self.app_socket).await.is_err() {
            return Ok(None);
        }
        let status = app_request(&self.app_socket, json!({"status": true})).await?;
        let Some(pane) = status.get("focused_pane").and_then(Value::as_str) else {
            return Ok(None);
        };
        let listing = app_request(&self.app_socket, json!({"ls": true})).await?;
        Ok(terminal_in_pane(&listing, pane))
    }
}

fn session_name(socket: &Path) -> String {
    socket
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("default")
        .to_owned()
}

fn terminal_in_pane(listing: &Value, pane: &str) -> Option<String> {
    let spaces = listing.get("spaces")?.as_array()?;
    spaces
        .iter()
        .filter_map(|space| space.get("tabs")?.as_array())
        .flatten()
        .filter_map(|tab| tab.get("terminals")?.as_array())
        .flatten()
        .find(|terminal| terminal.get("pane").and_then(Value::as_str) == Some(pane))
        .and_then(|terminal| terminal.get("terminal")?.as_str())
        .map(str::to_owned)
}

fn build_agents(
    terminals: &[CoreTerminal],
    session: &str,
    app_socket: &Path,
    focused_terminal: Option<&str>,
    daemon_epoch: &str,
    source_offset: usize,
) -> (Vec<AgentView>, HashMap<String, HerdrTarget>) {
    let by_address: HashMap<&str, &CoreTerminal> = terminals
        .iter()
        .map(|terminal| (terminal.address.as_str(), terminal))
        .collect();
    let mut agents = Vec::new();
    let mut targets = HashMap::new();

    for raw in terminals {
        // `machine/t3` lives on another machine: its folder is not this
        // filesystem, and the local Herdr app cannot be asked about it here.
        if raw.state != "live" || raw.address.contains('/') {
            continue;
        }
        let Some(status) = raw.status.as_ref() else {
            continue;
        };
        // Only terminals the Herdr app shows can be focused there: its own,
        // and sub-agents started from them.
        let Some(home) = herdr_home(raw, &by_address) else {
            continue;
        };
        let (space, tab) = split_where(home.labels.get(WHERE_LABEL).map(String::as_str));
        let agent_name = nonblank(status.app.as_deref())
            .or_else(|| nonblank(raw.foreground.as_ref().and_then(|fg| fg.name.as_deref())))
            .unwrap_or("agent")
            .to_owned();
        let display_name = display_name(raw.name.as_deref(), tab, &agent_name, space);
        let cwd = raw
            .foreground
            .as_ref()
            .and_then(|foreground| foreground.cwd.as_deref())
            .unwrap_or(&raw.cwd);
        let location = git_location(cwd);
        let id = opaque_agent_id(daemon_epoch, session, &raw.address);
        let state_change_seq = raw.status_ms.unwrap_or_default();

        targets.insert(
            id.clone(),
            HerdrTarget {
                session: session.to_owned(),
                app_socket: app_socket.to_path_buf(),
                terminal: raw.address.clone(),
                state_change_seq,
            },
        );
        agents.push(AgentView {
            id,
            display_name,
            agent: agent_name,
            status: agent_status(&status.state),
            review_ready: false,
            launch_pending: false,
            workspace: space.unwrap_or("Herdr").to_owned(),
            repository: location
                .as_ref()
                .map(|location| location.repository.clone())
                .or_else(|| {
                    cwd.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                }),
            worktree: location
                .as_ref()
                .and_then(|location| location.worktree.clone()),
            branch: location.and_then(|location| location.branch),
            session: session.to_owned(),
            focused: focused_terminal == Some(raw.address.as_str()),
            observed_for_seconds: 0,
            state_change_seq,
            source_order: source_offset + agents.len(),
            actions: AgentActions {
                open: true,
                ..AgentActions::default()
            },
        });
    }
    (agents, targets)
}

/// The terminal whose Herdr app labels place this one: itself, or the
/// nearest ancestor that carries them. Parent links are set once at start,
/// but the walk is bounded and cycle-safe regardless.
fn herdr_home<'a>(
    terminal: &'a CoreTerminal,
    by_address: &HashMap<&str, &'a CoreTerminal>,
) -> Option<&'a CoreTerminal> {
    let mut current = terminal;
    let mut seen = HashSet::new();
    loop {
        if current.labels.contains_key(SPACE_LABEL) {
            return Some(current);
        }
        if !seen.insert(current.address.as_str()) || seen.len() > 32 {
            return None;
        }
        current = by_address.get(current.parent.as_deref()?)?;
    }
}

/// The Herdr app writes `where` as `<space> / <tab>`.
fn split_where(value: Option<&str>) -> (Option<&str>, Option<&str>) {
    let Some(value) = nonblank(value) else {
        return (None, None);
    };
    match value.split_once(" / ") {
        Some((space, tab)) => (nonblank(Some(space)), nonblank(Some(tab))),
        None => (Some(value), None),
    }
}

fn agent_status(state: &str) -> AgentStatus {
    match state {
        "working" => AgentStatus::Working,
        // `error` needs a person just like a permission or question does.
        "blocked" | "error" => AgentStatus::Blocked,
        "done" => AgentStatus::Done,
        "idle" => AgentStatus::Idle,
        _ => AgentStatus::Unknown,
    }
}

/// The terminal's own name wins, then a named tab. The Herdr app numbers
/// unnamed tabs, so an all-digit label says nothing; with neither, the card
/// is `<agent> · <space>`. A name equal to the agent type counts as none.
fn display_name(
    name: Option<&str>,
    tab: Option<&str>,
    agent_name: &str,
    space: Option<&str>,
) -> String {
    if let Some(name) = nonblank(name).filter(|name| !name.eq_ignore_ascii_case(agent_name)) {
        return name.to_owned();
    }
    if let Some(tab) =
        nonblank(tab).filter(|tab| !tab.chars().all(|character| character.is_ascii_digit()))
    {
        return tab.to_owned();
    }
    match nonblank(space) {
        Some(space) => format!("{agent_name} · {space}"),
        None => agent_name.to_owned(),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct GitLocation {
    repository: String,
    worktree: Option<String>,
    branch: Option<String>,
}

/// The repository containing `start`, read from git's own files (a `.git`
/// directory, or a linked worktree's `gitdir:` pointer) without running git.
/// A detached HEAD yields its short commit.
fn git_location(start: &Path) -> Option<GitLocation> {
    let mut directory = Some(start);
    for _ in 0..32 {
        let current = directory?;
        let dot_git = current.join(".git");
        if let Ok(metadata) = std::fs::metadata(&dot_git) {
            let checkout_name = current.file_name()?.to_string_lossy().into_owned();
            let (git_dir, repository, worktree) = if metadata.is_dir() {
                (dot_git, checkout_name, None)
            } else {
                let pointer = std::fs::read_to_string(&dot_git).ok()?;
                let target = Path::new(pointer.strip_prefix("gitdir:")?.trim());
                let git_dir = if target.is_absolute() {
                    target.to_path_buf()
                } else {
                    current.join(target)
                };
                // `<repo>/.git/worktrees/<name>` names the main repository.
                let repository = git_dir
                    .parent()
                    .filter(|parent| parent.file_name().is_some_and(|name| name == "worktrees"))
                    .and_then(Path::parent)
                    .filter(|parent| parent.file_name().is_some_and(|name| name == ".git"))
                    .and_then(Path::parent)
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned());
                match repository {
                    Some(repository) => (git_dir, repository, Some(checkout_name)),
                    None => (git_dir, checkout_name, None),
                }
            };
            return Some(GitLocation {
                repository,
                worktree,
                branch: read_branch(&git_dir),
            });
        }
        directory = current.parent();
    }
    None
}

fn read_branch(git_dir: &Path) -> Option<String> {
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
    safe.then_some(branch)
}

fn nonblank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

async fn watch_events(
    descriptor: &SessionDescriptor,
    invalidations: &mpsc::Sender<String>,
    ready: oneshot::Sender<Result<(), String>>,
) -> Result<()> {
    let id = request_id();
    // The write half stays alive for the whole stream: closing it is how a
    // client ends its subscription.
    // A subscription that never starts reports only through `ready`: the
    // daemon marks the session stale and its repair tick retries, instead of
    // an invalidation that would retry at once and loop.
    let (mut lines, _write, mut words) = match subscribe(&descriptor.socket_path, &id).await {
        Ok(stream) => stream,
        Err(error) => {
            let _ = ready.send(Err(format!("{error:#}")));
            return Err(error);
        }
    };
    let _ = ready.send(Ok(()));

    let ended = async {
        while let Some(line) = lines.next_line().await? {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if words.invalidates(&message) {
                let _ = invalidations.try_send(descriptor.name.clone());
            }
        }
        bail!("herdr-core subscription reached EOF")
    }
    .await;
    // A stream that ran and then ended needs a fresh roster.
    let _ = invalidations.try_send(descriptor.name.clone());
    ended
}

/// Each terminal's last status word, so a `status` event that changes only
/// the program's message or progress does not refresh the roster.
#[derive(Debug, Default)]
struct StatusWords(HashMap<String, Option<String>>);

impl StatusWords {
    fn seed(&mut self, terminals: &Value) {
        self.0.clear();
        for terminal in terminals.as_array().into_iter().flatten() {
            if let Some(address) = terminal["address"].as_str() {
                self.0
                    .insert(address.to_owned(), status_word(&terminal["status"]));
            }
        }
    }

    /// Whether a subscription message can change the roster. Unknown fact
    /// types invalidate, so a newer core fails toward refreshing, never
    /// toward a stale card; a `missed` notice means changes were dropped.
    fn invalidates(&mut self, message: &Value) -> bool {
        let params = &message["params"];
        match message.get("method").and_then(Value::as_str) {
            Some("event") => match params["type"].as_str() {
                Some(kind) if IGNORED_EVENTS.contains(&kind) => false,
                Some("status") => {
                    // Live events carry `terminal` and `data.status` (null once
                    // a program stops reporting). Any other shape refreshes.
                    let Some(terminal) = params["terminal"].as_str() else {
                        return true;
                    };
                    if params["data"].get("status").is_none() {
                        return true;
                    }
                    let word = status_word(&params["data"]["status"]);
                    self.0.insert(terminal.to_owned(), word.clone()) != Some(word)
                }
                _ => true,
            },
            Some("heartbeat") => false,
            Some("missed") => {
                self.seed(&params["terminals"]);
                true
            }
            Some(_) => true,
            None => false,
        }
    }
}

fn status_word(status: &Value) -> Option<String> {
    status["state"].as_str().map(str::to_owned)
}

type Lines = tokio::io::Lines<BufReader<OwnedReadHalf>>;

/// Opens an event stream and waits for the subscription's snapshot reply,
/// which is the acknowledgement that every later fact change will arrive.
async fn subscribe(socket: &Path, id: &str) -> Result<(Lines, OwnedWriteHalf, StatusWords)> {
    let request = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "events.subscribe",
        "params": {"children_of": null, "labels": {}, "terminal": null}
    });
    let (mut lines, write) = timeout(REQUEST_TIMEOUT, send_line(socket, &request))
        .await
        .context("timed out connecting subscription")??;
    let acknowledgement = timeout(REQUEST_TIMEOUT, async {
        loop {
            let Some(line) = lines.next_line().await? else {
                bail!("subscription closed before acknowledgement");
            };
            let message: Value =
                serde_json::from_str(&line).context("parsing subscription acknowledgement")?;
            if message.get("id").and_then(Value::as_str) == Some(id) {
                return Ok(message);
            }
        }
    })
    .await
    .context("timed out waiting for subscription acknowledgement")??;
    if let Some(error) = acknowledgement.get("error") {
        bail!("herdr-core refused the subscription: {error}");
    }
    let result = &acknowledgement["result"];
    if !result["seq"].is_u64() || !result["terminals"].is_array() {
        bail!("unexpected subscription acknowledgement");
    }
    let mut words = StatusWords::default();
    words.seed(&result["terminals"]);
    Ok((lines, write, words))
}

async fn core_request<T: for<'de> Deserialize<'de>>(
    socket: &Path,
    method: &str,
    params: Value,
) -> Result<T> {
    let id = request_id();
    let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    let response = timeout(REQUEST_TIMEOUT, async {
        let (mut lines, _write) = send_line(socket, &request).await?;
        loop {
            let line = lines
                .next_line()
                .await?
                .ok_or_else(|| anyhow!("herdr-core closed without a response"))?;
            let message: Value = serde_json::from_str(&line).context("parsing herdr-core reply")?;
            if message.get("id").and_then(Value::as_str) == Some(id.as_str()) {
                return Ok::<_, anyhow::Error>(message);
            }
        }
    })
    .await
    .with_context(|| format!("timed out waiting for herdr-core {method}"))??;
    if let Some(error) = response.get("error") {
        bail!("herdr-core {method} failed: {error}");
    }
    let result = response
        .get("result")
        .cloned()
        .ok_or_else(|| anyhow!("herdr-core {method} reply has no result"))?;
    serde_json::from_value(result).with_context(|| format!("herdr-core {method} schema changed"))
}

async fn app_request(socket: &Path, request: Value) -> Result<Value> {
    let response = timeout(REQUEST_TIMEOUT, async {
        let (mut lines, _write) = send_line(socket, &request).await?;
        lines
            .next_line()
            .await?
            .ok_or_else(|| anyhow!("Herdr app closed without a response"))
    })
    .await
    .context("timed out waiting for the Herdr app")??;
    serde_json::from_str(&response).context("parsing Herdr app reply")
}

/// Sends one JSON line and returns the reply stream with the write half,
/// which the caller keeps open until it has its reply.
async fn send_line(socket: &Path, request: &Value) -> Result<(Lines, OwnedWriteHalf)> {
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connecting to {}", socket.display()))?;
    let (read, mut write) = stream.into_split();
    let mut encoded = serde_json::to_vec(request)?;
    encoded.push(b'\n');
    write.write_all(&encoded).await?;
    write.flush().await?;
    Ok((BufReader::new(read).lines(), write))
}

fn opaque_agent_id(epoch: &str, session: &str, terminal: &str) -> String {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("{epoch}\0{session}\0{terminal}").as_bytes(),
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
    use tokio::net::UnixListener;

    fn terminal(value: Value) -> CoreTerminal {
        serde_json::from_value(value).unwrap()
    }

    /// A herdr-core terminal record with the required schema fields.
    fn record(address: &str, extra: Value) -> Value {
        let mut value = json!({
            "address": address,
            "argv": ["private", "argv"],
            "cwd": "/nonexistent/project",
            "state": "live",
            "created_ms": 1,
            "screen": "exact",
            "cols": 80,
            "rows": 24,
            "labels": {},
            "title": "private prompt title"
        });
        for (key, field) in extra.as_object().unwrap() {
            value[key] = field.clone();
        }
        value
    }

    fn herdr_labels(space_where: &str) -> Value {
        json!({"herdr.space": "sp1", "herdr.tab": "tb1", "where": space_where})
    }

    #[test]
    fn opaque_ids_change_with_daemon_epoch() {
        let first = opaque_agent_id("one", "default", "t1");
        assert_ne!(first, opaque_agent_id("two", "default", "t1"));
        assert_eq!(first, opaque_agent_id("one", "default", "t1"));
    }

    #[test]
    fn only_live_reporting_herdr_terminals_become_agents() {
        let terminals = [
            record(
                "t1",
                json!({"labels": herdr_labels("lectern / review"), "name": "lectern-qt",
                    "status": {"app": "codex", "state": "working", "msg": "private status"},
                    "status_ms": 42}),
            ),
            // A shell reports no status.
            record("t2", json!({"labels": herdr_labels("lectern / 1")})),
            // Another app's agent is not shown by the Herdr app.
            record(
                "t3",
                json!({"status": {"app": "claude-code", "state": "blocked"}}),
            ),
            // A finished agent is gone.
            record(
                "t4",
                json!({"state": "finished", "labels": herdr_labels("lectern / 1"),
                    "status": {"app": "codex", "state": "idle"}}),
            ),
            // Another machine's terminal is not this filesystem's.
            record(
                "vps/t1",
                json!({"labels": herdr_labels("vps / 1"), "status": {"state": "working"}}),
            ),
            // A sub-agent belongs to its parent's space and tab.
            record(
                "t5",
                json!({"parent": "t1", "status": {"app": "claude-code", "state": "error"}}),
            ),
        ]
        .map(terminal);

        let (agents, targets) = build_agents(
            &terminals,
            "default",
            Path::new("/tmp/app.sock"),
            Some("t5"),
            "epoch",
            3,
        );

        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].display_name, "lectern-qt");
        assert_eq!(agents[0].agent, "codex");
        assert_eq!(agents[0].workspace, "lectern");
        assert_eq!(agents[0].status, AgentStatus::Working);
        assert_eq!(agents[0].state_change_seq, 42);
        assert_eq!(agents[0].source_order, 3);
        assert!(!agents[0].focused);
        assert!(agents[0].actions.open);
        assert!(!agents[0].actions.zoom);
        assert!(agents[0].actions.approve.is_none());
        assert!(agents[0].actions.interrupt.is_none());

        assert_eq!(agents[1].display_name, "review");
        assert_eq!(agents[1].status, AgentStatus::Blocked);
        assert_eq!(agents[1].workspace, "lectern");
        assert!(agents[1].focused);
        assert_eq!(agents[1].source_order, 4);
        assert_eq!(targets[&agents[1].id].terminal, "t5");

        // Prompt-bearing fields never reach the portal model.
        let encoded = serde_json::to_string(&agents).unwrap();
        for private in ["private prompt title", "private status", "argv"] {
            assert!(!encoded.contains(private), "{private} leaked");
        }
    }

    #[test]
    fn parent_cycles_never_place_an_agent() {
        let terminals = [
            record(
                "t1",
                json!({"parent": "t2", "status": {"state": "working"}}),
            ),
            record(
                "t2",
                json!({"parent": "t1", "status": {"state": "working"}}),
            ),
        ]
        .map(terminal);
        let (agents, _) = build_agents(&terminals, "default", Path::new("/a"), None, "e", 0);
        assert!(agents.is_empty());
    }

    #[test]
    fn status_words_map_to_attention_states() {
        assert_eq!(agent_status("working"), AgentStatus::Working);
        assert_eq!(agent_status("blocked"), AgentStatus::Blocked);
        assert_eq!(agent_status("error"), AgentStatus::Blocked);
        assert_eq!(agent_status("done"), AgentStatus::Done);
        assert_eq!(agent_status("idle"), AgentStatus::Idle);
        assert_eq!(agent_status("thinking"), AgentStatus::Unknown);
    }

    #[test]
    fn card_names_follow_terminal_tab_then_space() {
        assert_eq!(
            display_name(Some("reviewer"), Some("fix"), "codex", Some("x")),
            "reviewer"
        );
        // A name equal to the agent type says nothing.
        assert_eq!(
            display_name(Some("Codex"), Some("fix login"), "codex", Some("x")),
            "fix login"
        );
        // The Herdr app numbers unnamed tabs.
        assert_eq!(
            display_name(Some(" "), Some("2"), "codex", Some("lectern")),
            "codex · lectern"
        );
        assert_eq!(display_name(None, None, "codex", None), "codex");
        assert_eq!(
            split_where(Some("BYBY / website")),
            (Some("BYBY"), Some("website"))
        );
        assert_eq!(split_where(Some("solo")), (Some("solo"), None));
        assert_eq!(split_where(Some("  ")), (None, None));
    }

    #[test]
    fn git_location_reads_repositories_and_linked_worktrees() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(repo.join("src/deep")).unwrap();
        fs::write(repo.join(".git/HEAD"), "ref: refs/heads/feature/edge\n").unwrap();
        assert_eq!(
            git_location(&repo.join("src/deep")),
            Some(GitLocation {
                repository: "repo".into(),
                worktree: None,
                branch: Some("feature/edge".into()),
            })
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
        assert_eq!(
            git_location(&worktree),
            Some(GitLocation {
                repository: "repo".into(),
                worktree: Some("wt".into()),
                branch: Some("0123456".into()),
            })
        );

        fs::write(repo.join(".git/HEAD"), "ref: refs/heads/evil\u{1b}[2J\n").unwrap();
        assert_eq!(git_location(&repo).unwrap().branch, None);
        assert_eq!(git_location(&temp.path().join("nowhere")), None);
    }

    #[test]
    fn only_card_relevant_facts_invalidate() {
        let event =
            |kind: &str| json!({"jsonrpc": "2.0", "method": "event", "params": {"type": kind}});
        let status = |terminal: &str, word: Value| {
            json!({"method": "event", "params": {"type": "status", "terminal": terminal,
                "data": {"status": word}}})
        };
        let mut words = StatusWords::default();
        words.seed(&json!([
            {"address": "t1", "status": {"state": "working"}},
            {"address": "t2", "status": null}
        ]));
        for kind in ["title", "size", "turn", "keep"] {
            assert!(!words.invalidates(&event(kind)), "{kind}");
        }
        assert!(!words.invalidates(&json!({"method": "heartbeat", "params": {"seq": 1}})));
        for kind in ["created", "finished", "removed", "labels", "doc", "new"] {
            assert!(words.invalidates(&event(kind)), "{kind}");
        }
        // Progress and messages leave the word alone.
        assert!(!words.invalidates(&status("t1", json!({"state": "working", "progress": 40}))));
        assert!(words.invalidates(&status("t1", json!({"state": "blocked"}))));
        assert!(!words.invalidates(&status("t1", json!({"state": "blocked", "msg": "x"}))));
        assert!(words.invalidates(&status("t2", json!({"state": "idle"}))));
        assert!(words.invalidates(&status("t9", json!({"state": "idle"}))));
        assert!(words.invalidates(&status("t9", Value::Null)));
        // An unexpected payload shape never suppresses a change.
        let drifted = json!({"method": "event", "params": {"type": "status",
            "terminal": "t1", "data": {"state": "idle"}}});
        assert!(words.invalidates(&drifted));
        assert!(words.invalidates(&drifted));
        assert!(words.invalidates(&json!({"method": "missed", "params": {"terminals": []}})));
        assert!(!words.invalidates(&json!({"id": "x", "result": {}})));
    }

    #[tokio::test]
    async fn discovery_needs_the_core_socket_and_never_starts_a_core() {
        let temp = tempdir().unwrap();
        let socket = temp.path().join("work.sock");
        let client = HerdrClient::new(&socket, temp.path().join("app.sock"));
        assert!(client.discover().await.unwrap().is_empty());
        let _listener = UnixListener::bind(&socket).unwrap();
        let sessions = client.discover().await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "work");
        assert_eq!(sessions[0].socket_path, socket);
    }

    /// Serves one JSON-RPC request per connection, checking each method.
    fn serve_core(
        listener: UnixListener,
        replies: Vec<(&'static str, Value)>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            for (method, result) in replies {
                let (stream, _) = listener.accept().await.unwrap();
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                let request: Value =
                    serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                assert_eq!(request["jsonrpc"], "2.0");
                assert_eq!(request["method"], method);
                // A notification before the reply must be skipped.
                let heartbeat = json!({"jsonrpc": "2.0", "method": "heartbeat", "params": {}});
                let reply = json!({"jsonrpc": "2.0", "id": request["id"], "result": result});
                write
                    .write_all(format!("{heartbeat}\n{reply}\n").as_bytes())
                    .await
                    .unwrap();
            }
        })
    }

    #[tokio::test]
    async fn observation_reads_status_and_terminals_from_the_core() {
        let temp = tempdir().unwrap();
        let socket = temp.path().join("default.sock");
        let server = serve_core(
            UnixListener::bind(&socket).unwrap(),
            vec![
                ("server.status", json!({"version": "0.1.18", "pid": 1})),
                (
                    "terminal.list",
                    json!({"hidden_finished": 0, "machines": [], "terminals": [record(
                        "t7",
                        json!({"labels": herdr_labels("xeneon / 1"),
                            "status": {"app": "claude-code", "state": "done"}}),
                    )]}),
                ),
            ],
        );
        let client = HerdrClient::new(&socket, temp.path().join("missing-app.sock"));
        let descriptor = client.discover().await.unwrap().remove(0);

        let observation = client
            .observe_session(&descriptor, "epoch", 0, 1234)
            .await
            .unwrap();
        server.await.unwrap();

        assert_eq!(observation.session.state, SessionState::Connected);
        assert_eq!(observation.session.version.as_deref(), Some("0.1.18"));
        assert_eq!(observation.session.protocol, None);
        assert_eq!(observation.agents.len(), 1);
        assert_eq!(observation.agents[0].display_name, "claude-code · xeneon");
        assert_eq!(observation.agents[0].status, AgentStatus::Done);
        assert!(!observation.agents[0].focused);
    }

    #[tokio::test]
    async fn schema_drift_fails_the_observation() {
        let temp = tempdir().unwrap();
        let socket = temp.path().join("default.sock");
        let server = serve_core(
            UnixListener::bind(&socket).unwrap(),
            vec![
                ("server.status", json!({"version": "9.0.0"})),
                (
                    "terminal.list",
                    json!({"terminals": [{"address": "t1", "state": "live"}]}),
                ),
            ],
        );
        let client = HerdrClient::new(&socket, temp.path().join("app.sock"));
        let descriptor = client.discover().await.unwrap().remove(0);
        let error = client
            .observe_session(&descriptor, "epoch", 0, 1)
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("schema changed"));
        server.await.unwrap();
    }

    /// Answers Herdr app requests in order, recording each one.
    fn serve_app(
        listener: UnixListener,
        replies: Vec<Value>,
    ) -> tokio::task::JoinHandle<Vec<Value>> {
        tokio::spawn(async move {
            let mut requests = Vec::new();
            for reply in replies {
                let (stream, _) = listener.accept().await.unwrap();
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                requests.push(
                    serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap(),
                );
                write
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .unwrap();
            }
            requests
        })
    }

    #[tokio::test]
    async fn focus_sends_only_the_typed_agent_focus_command() {
        let temp = tempdir().unwrap();
        let app = temp.path().join("herdr-app.sock");
        let server = serve_app(
            UnixListener::bind(&app).unwrap(),
            vec![
                json!({"ok": true}),
                json!({"error": "no running terminal \"t9\""}),
            ],
        );
        let client = HerdrClient::new(temp.path().join("core.sock"), &app);
        let target = HerdrTarget {
            session: "default".into(),
            app_socket: app.clone(),
            terminal: "t9".into(),
            state_change_seq: 1,
        };

        client.focus(&target).await.unwrap();
        assert!(client.focus(&target).await.is_err());
        let requests = server.await.unwrap();
        assert_eq!(
            requests[0],
            json!({"do": "agent.focus", "args": {"terminal": "t9"}})
        );
    }

    #[tokio::test]
    async fn focused_terminal_is_the_app_focused_pane() {
        let temp = tempdir().unwrap();
        let app = temp.path().join("herdr-app.sock");
        let server = serve_app(
            UnixListener::bind(&app).unwrap(),
            vec![
                json!({"focused_pane": "p2", "agent": "t1"}),
                json!({"spaces": [{"tabs": [{"terminals": [
                    {"terminal": "t1", "pane": "p1"},
                    {"terminal": "t2", "pane": "p2"}
                ]}]}]}),
            ],
        );
        let client = HerdrClient::new(temp.path().join("core.sock"), &app);
        assert_eq!(
            client.focused_terminal().await.unwrap().as_deref(),
            Some("t2")
        );
        let requests = server.await.unwrap();
        assert_eq!(requests, [json!({"status": true}), json!({"ls": true})]);
    }

    #[tokio::test]
    async fn subscription_acknowledges_before_forwarding_invalidations() {
        let temp = tempdir().unwrap();
        let socket = temp.path().join("default.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(request["method"], "events.subscribe");
            assert_eq!(
                request["params"],
                json!({"children_of": null, "labels": {}, "terminal": null})
            );
            let ack = json!({"jsonrpc": "2.0", "id": request["id"],
                "result": {"seq": 1, "terminals": []}});
            let title = json!({"jsonrpc": "2.0", "method": "event",
                "params": {"type": "title", "terminal": "t1", "data": {"title": "x"}}});
            let status = json!({"jsonrpc": "2.0", "method": "event",
                "params": {"type": "status", "terminal": "t1"}});
            write
                .write_all(format!("{ack}\n{title}\n{status}\n").as_bytes())
                .await
                .unwrap();
            // Holding the connection shows the client keeps it open.
            let _ = lines.next_line().await;
        });
        let descriptor = SessionDescriptor {
            name: "default".into(),
            socket_path: socket,
        };
        let (invalidations_tx, mut invalidations_rx) = mpsc::channel(4);
        let (ready_tx, ready_rx) = oneshot::channel();
        let watcher =
            tokio::spawn(
                async move { watch_events(&descriptor, &invalidations_tx, ready_tx).await },
            );

        assert_eq!(ready_rx.await.unwrap(), Ok(()));
        assert_eq!(invalidations_rx.recv().await.as_deref(), Some("default"));
        assert!(
            timeout(Duration::from_millis(100), invalidations_rx.recv())
                .await
                .is_err(),
            "a title change must not invalidate"
        );
        watcher.abort();
        server.abort();
    }

    #[tokio::test]
    async fn a_refused_subscription_never_becomes_ready() {
        let temp = tempdir().unwrap();
        let socket = temp.path().join("default.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            let reply = json!({"jsonrpc": "2.0", "id": request["id"],
                "error": {"code": -32000, "message": "machine_unknown"}});
            write
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        });
        let descriptor = SessionDescriptor {
            name: "default".into(),
            socket_path: socket,
        };
        let (invalidations_tx, mut invalidations_rx) = mpsc::channel(1);
        let (ready_tx, ready_rx) = oneshot::channel();
        assert!(
            watch_events(&descriptor, &invalidations_tx, ready_tx)
                .await
                .is_err()
        );
        assert!(ready_rx.await.unwrap().is_err());
        // Retrying at once would loop; the repair tick retries instead.
        assert!(invalidations_rx.try_recv().is_err());
        server.await.unwrap();
    }
}
