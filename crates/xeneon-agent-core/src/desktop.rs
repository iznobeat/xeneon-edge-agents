use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout};

use crate::config::DesktopConfig;

const PORTAL_PREVIEW_CLASS_PREFIX: &str = "org.xeneon-edge.agent-portal-preview";
/// Layer namespace of the live portal; its geometry tells the game window
/// where the header ends.
const PORTAL_LAYER_NAMESPACE: &str = "xeneon-edge-agent-portal";
/// The game dashboard's window: Qt's `qml` runtime with a fixed title that the
/// Hyprland module's window rule also matches.
const GAME_WINDOW_CLASS: &str = "org.qt-project.qml";
pub const GAME_WINDOW_TITLE: &str = "xeneon-edge-game";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DesktopApp {
    /// Index into the configured `desktop.apps` shortcuts.
    Configured(usize),
    T3code,
    Game,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameOutcome {
    /// The window is mapped and sits under the portal header.
    Placed,
    /// The window was started or is still starting.
    Launching,
    /// The window exists but the portal has not shrunk to its header yet.
    AwaitingHeader,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopAppOutcome {
    Focused,
    LaunchedAndFocused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DesktopAppSpec<'a> {
    desktop_id: &'a str,
    class: &'a str,
    main_title: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct DesktopController {
    config: DesktopConfig,
    remembered_window: Arc<Mutex<Option<String>>>,
    launching_since: Arc<Mutex<HashMap<DesktopApp, Instant>>>,
    hyprctl_bin: PathBuf,
    uwsm_bin: PathBuf,
    launch_poll_interval: Duration,
    launch_observation_timeout: Duration,
    launch_coalesce_for: Duration,
}

#[derive(Debug, Deserialize)]
struct HyprClient {
    address: String,
    #[serde(default)]
    class: String,
    #[serde(default, rename = "initialClass")]
    initial_class: String,
    #[serde(default)]
    title: String,
    #[serde(default, rename = "initialTitle")]
    initial_title: String,
    pid: i32,
    monitor: i64,
    #[serde(default)]
    floating: bool,
    #[serde(default, rename = "focusHistoryID")]
    focus_history_id: Option<i64>,
    #[serde(default)]
    at: [i64; 2],
    #[serde(default)]
    size: [i64; 2],
}

#[derive(Debug, Deserialize)]
struct HyprMonitor {
    id: i64,
    name: String,
    #[serde(default)]
    focused: bool,
    #[serde(default)]
    y: i64,
    #[serde(default)]
    width: i64,
    #[serde(default)]
    height: i64,
    #[serde(default = "unit_scale")]
    scale: f64,
    #[serde(default)]
    transform: i64,
}

fn unit_scale() -> f64 {
    1.0
}

#[derive(Debug, Deserialize)]
struct HyprLayerMonitor {
    levels: HashMap<String, Vec<HyprLayer>>,
}

#[derive(Debug, Deserialize)]
struct HyprLayer {
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    #[serde(default)]
    namespace: String,
}

/// Global logical rectangle for the game window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GameArea {
    x: i64,
    y: i64,
    width: i64,
    height: i64,
}

impl DesktopController {
    pub fn new(config: DesktopConfig) -> Self {
        Self {
            config,
            remembered_window: Arc::new(Mutex::new(None)),
            launching_since: Arc::new(Mutex::new(HashMap::new())),
            hyprctl_bin: PathBuf::from("hyprctl"),
            uwsm_bin: PathBuf::from("uwsm"),
            launch_poll_interval: Duration::from_millis(100),
            launch_observation_timeout: Duration::from_secs(12),
            launch_coalesce_for: Duration::from_secs(20),
        }
    }

    pub async fn remember_non_edge_focus(&self) {
        let Ok(active) = hypr_json::<HyprClient>(&self.hyprctl_bin, &["activewindow", "-j"]).await
        else {
            return;
        };
        let Ok(monitors) =
            hypr_json::<Vec<HyprMonitor>>(&self.hyprctl_bin, &["monitors", "-j"]).await
        else {
            return;
        };
        let Some(active_monitor) = monitors
            .iter()
            .find(|monitor| monitor.id == active.monitor)
            .or_else(|| monitors.iter().find(|monitor| monitor.focused))
        else {
            return;
        };
        if self
            .config
            .edge_output
            .as_deref()
            .is_some_and(|edge| edge == active_monitor.name)
        {
            return;
        }
        if active.address.is_empty() {
            return;
        }
        if is_portal_preview_class(&active.class) {
            return;
        }
        *self.remembered_window.lock().await = Some(active.address);
    }

    pub async fn restore_focus(&self) -> Result<()> {
        let address = self
            .remembered_window
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no non-EDGE window has been observed"))?;
        let (clients, _) = hypr_state(&self.hyprctl_bin).await?;
        if !clients.iter().any(|client| client.address == address) {
            bail!("remembered window no longer exists");
        }
        dispatch_focus(&self.hyprctl_bin, &address).await
    }

    /// Opens a URL the daemon already validated with its registered handler
    /// (the browser, or the Slack app for `slack://`). It runs as its own
    /// graphical service so the handler never inherits the daemon's sandbox.
    pub async fn open_url(&self, url: &str) -> Result<()> {
        let status = Command::new(&self.uwsm_bin)
            .args(["app", "-t", "service", "--", "xdg-open", url])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .context("opening URL")?;
        if !status.success() {
            bail!("URL open command failed");
        }
        Ok(())
    }

    /// Resolves a portal-supplied shortcut id against the configured list.
    pub fn configured_app(&self, id: &str) -> Option<DesktopApp> {
        self.config
            .apps
            .iter()
            .position(|app| app.id == id)
            .map(DesktopApp::Configured)
    }

    pub async fn focus_or_launch(&self, app: DesktopApp) -> Result<DesktopAppOutcome> {
        let spec = desktop_app_spec(&self.config, app)?;
        let clients = hypr_json::<Vec<HyprClient>>(&self.hyprctl_bin, &["clients", "-j"]).await?;
        match select_desktop_candidate(&clients, spec)? {
            Some(client) => {
                self.launching_since.lock().await.remove(&app);
                self.focus_off_edge(client).await?;
                Ok(DesktopAppOutcome::Focused)
            }
            None => {
                let now = Instant::now();
                let mut launches = self.launching_since.lock().await;
                let launch_in_flight = launches
                    .get(&app)
                    .is_some_and(|started| now.duration_since(*started) < self.launch_coalesce_for);
                if !launch_in_flight {
                    launches.insert(app, now);
                }
                drop(launches);
                if !launch_in_flight && let Err(error) = launch_desktop(&self.uwsm_bin, spec).await
                {
                    self.launching_since.lock().await.remove(&app);
                    return Err(error);
                }
                self.wait_for_launched_client(app, spec).await
            }
        }
    }

    /// Focuses an app the portal opened, first sending it to another monitor
    /// when it sits on the EDGE: the portal's overlay would hide it there. With
    /// no other monitor it stays put, since the EDGE is then all there is.
    async fn focus_off_edge(&self, client: &HyprClient) -> Result<()> {
        if let Some(edge) = self.config.edge_output.as_deref() {
            let monitors =
                hypr_json::<Vec<HyprMonitor>>(&self.hyprctl_bin, &["monitors", "-j"]).await?;
            if let Some(target) = off_edge_target(&monitors, client.monitor, edge) {
                dispatch(
                    &self.hyprctl_bin,
                    &move_to_monitor_expression(&client.address, &target.name)?,
                )
                .await
                .context("moving a desktop app off the EDGE")?;
            }
        }
        dispatch_focus(&self.hyprctl_bin, &client.address).await
    }

    async fn wait_for_launched_client(
        &self,
        app: DesktopApp,
        spec: DesktopAppSpec<'_>,
    ) -> Result<DesktopAppOutcome> {
        let deadline = Instant::now() + self.launch_observation_timeout;
        loop {
            let clients =
                match hypr_json::<Vec<HyprClient>>(&self.hyprctl_bin, &["clients", "-j"]).await {
                    Ok(clients) => clients,
                    Err(error) => {
                        self.launching_since.lock().await.remove(&app);
                        return Err(error);
                    }
                };
            let candidate = match select_desktop_candidate(&clients, spec) {
                Ok(candidate) => candidate,
                Err(error) => {
                    self.launching_since.lock().await.remove(&app);
                    return Err(error);
                }
            };
            match candidate {
                Some(client) => {
                    self.launching_since.lock().await.remove(&app);
                    self.focus_off_edge(client).await?;
                    return Ok(DesktopAppOutcome::LaunchedAndFocused);
                }
                None if Instant::now() < deadline => sleep(self.launch_poll_interval).await,
                None => {
                    self.launching_since.lock().await.remove(&app);
                    bail!("desktop client did not map before the launch deadline");
                }
            }
        }
    }

    /// Focuses an already mapped fixed desktop client without launching it.
    /// Agent focus never starts an application; a missing window is an error.
    pub async fn focus_existing(&self, app: DesktopApp) -> Result<()> {
        let spec = desktop_app_spec(&self.config, app)?;
        let clients = hypr_json::<Vec<HyprClient>>(&self.hyprctl_bin, &["clients", "-j"]).await?;
        match select_desktop_candidate(&clients, spec)? {
            Some(client) => dispatch_focus(&self.hyprctl_bin, &client.address).await,
            None => bail!("no {} desktop client is mapped", spec.class),
        }
    }

    /// Starts the game window when it is missing and keeps it exactly under
    /// the portal header. The portal shrinks its layer to the header while the
    /// game dashboard is shown, so whatever the layer no longer covers on its
    /// monitor is the game's area; the window floats there by window rule.
    pub async fn show_game(
        &self,
        qml_bin: &Path,
        window_qml: &Path,
        url: &str,
    ) -> Result<GameOutcome> {
        let clients = hypr_json::<Vec<HyprClient>>(&self.hyprctl_bin, &["clients", "-j"]).await?;
        let Some(window) = clients.iter().find(|client| is_game_client(client)) else {
            let now = Instant::now();
            let mut launches = self.launching_since.lock().await;
            let launch_in_flight = launches
                .get(&DesktopApp::Game)
                .is_some_and(|started| now.duration_since(*started) < self.launch_coalesce_for);
            if !launch_in_flight {
                if !window_qml.is_file() {
                    bail!("game window {} is not installed", window_qml.display());
                }
                launches.insert(DesktopApp::Game, now);
                drop(launches);
                if let Err(error) = launch_service(
                    &self.uwsm_bin,
                    &[
                        qml_bin.as_os_str(),
                        window_qml.as_os_str(),
                        OsStr::new("--"),
                        OsStr::new(url),
                    ],
                )
                .await
                {
                    self.launching_since.lock().await.remove(&DesktopApp::Game);
                    return Err(error);
                }
            }
            return Ok(GameOutcome::Launching);
        };
        self.launching_since.lock().await.remove(&DesktopApp::Game);
        let layers =
            hypr_json::<HashMap<String, HyprLayerMonitor>>(&self.hyprctl_bin, &["layers", "-j"])
                .await?;
        let monitors =
            hypr_json::<Vec<HyprMonitor>>(&self.hyprctl_bin, &["monitors", "-j"]).await?;
        let Some(area) = game_area(&layers, &monitors, self.config.edge_output.as_deref()) else {
            return Ok(GameOutcome::AwaitingHeader);
        };
        if window.at != [area.x, area.y] || window.size != [area.width, area.height] {
            let selector = window_selector(&window.address)?;
            // A resize keeps the window centered, so it goes before the move.
            dispatch(
                &self.hyprctl_bin,
                &format!(
                    "hl.dsp.window.resize({{ x = {}, y = {}, window = \"{selector}\" }})",
                    area.width, area.height
                ),
            )
            .await?;
            dispatch(
                &self.hyprctl_bin,
                &format!(
                    "hl.dsp.window.move({{ x = {}, y = {}, window = \"{selector}\" }})",
                    area.x, area.y
                ),
            )
            .await?;
        }
        Ok(GameOutcome::Placed)
    }

    pub async fn activate_herdr(&self, session: &str, socket_path: &Path) -> Result<()> {
        let (clients, _) = hypr_state(&self.hyprctl_bin).await?;
        let processes = tokio::task::spawn_blocking(process_table)
            .await
            .context("inventorying Herdr client processes")?;
        let candidate = select_herdr_candidate(
            &clients,
            &processes,
            session,
            socket_path,
            &self.config.herdr_class,
        )?;
        dispatch_focus(&self.hyprctl_bin, &candidate.address).await
    }
}

fn select_herdr_candidate<'a>(
    clients: &'a [HyprClient],
    processes: &HashMap<i32, ProcessInfo>,
    session: &str,
    socket_path: &Path,
    identity_hint: &str,
) -> Result<&'a HyprClient> {
    let session_clients: Vec<_> = clients
        .iter()
        .filter(|client| client_hosts_session_in(client.pid, session, socket_path, processes))
        .collect();
    if session_clients.is_empty() {
        bail!("no compositor window hosts Herdr session {session}");
    }

    // Native Herdr windows can use the configured class/title. Terminal-hosted
    // Herdr views inherit the terminal class, so the hint may legitimately
    // match none; exact process/session ownership remains the authority.
    let hint = identity_hint.to_ascii_lowercase();
    let hinted: Vec<_> = session_clients
        .iter()
        .copied()
        .filter(|client| {
            client.class.to_ascii_lowercase().contains(&hint)
                || client.initial_class.to_ascii_lowercase().contains(&hint)
                || client.title.to_ascii_lowercase().contains(&hint)
                || client.initial_title.to_ascii_lowercase().contains(&hint)
        })
        .collect();
    let candidates = if hinted.is_empty() {
        session_clients
    } else {
        hinted
    };
    if candidates.len() == 1 {
        return Ok(candidates[0]);
    }

    if candidates
        .iter()
        .any(|client| client.focus_history_id.is_none())
    {
        bail!("Herdr session {session} has compositor windows without focus history");
    }

    let Some(most_recent) = candidates
        .iter()
        .filter_map(|client| client.focus_history_id)
        .min()
    else {
        bail!("Herdr session {session} has no usable focus history");
    };
    let recent: Vec<_> = candidates
        .iter()
        .copied()
        .filter(|client| client.focus_history_id == Some(most_recent))
        .collect();
    if recent.len() != 1 {
        bail!(
            "Herdr session {session} has {} equally recent compositor windows",
            recent.len()
        );
    }
    Ok(recent[0])
}

fn desktop_candidates<'c>(
    clients: &'c [HyprClient],
    spec: DesktopAppSpec<'_>,
) -> Vec<&'c HyprClient> {
    clients
        .iter()
        .filter(|client| {
            client.class.eq_ignore_ascii_case(spec.class)
                || client.initial_class.eq_ignore_ascii_case(spec.class)
        })
        .collect()
}

fn select_desktop_candidate<'c>(
    clients: &'c [HyprClient],
    spec: DesktopAppSpec<'_>,
) -> Result<Option<&'c HyprClient>> {
    let candidates = desktop_candidates(clients, spec);
    if candidates.len() <= 1 {
        return Ok(candidates.into_iter().next());
    }

    let titled: Vec<_> = candidates
        .iter()
        .copied()
        .filter(|client| {
            spec.main_title
                .is_some_and(|title| client.initial_title.eq_ignore_ascii_case(title))
        })
        .collect();
    let candidates = if titled.is_empty() {
        candidates
    } else {
        titled
    };
    if candidates.len() == 1 {
        return Ok(candidates.into_iter().next());
    }

    let tiled: Vec<_> = candidates
        .iter()
        .copied()
        .filter(|client| !client.floating)
        .collect();
    let candidates = if tiled.is_empty() { candidates } else { tiled };
    if candidates.len() == 1 {
        return Ok(candidates.into_iter().next());
    }

    if candidates
        .iter()
        .any(|client| client.focus_history_id.is_none())
    {
        bail!("desktop candidates lack complete focus history");
    }

    let best_history = candidates
        .iter()
        .filter_map(|client| client.focus_history_id)
        .min();
    if let Some(best_history) = best_history {
        let recent: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|client| client.focus_history_id == Some(best_history))
            .collect();
        if recent.len() == 1 {
            return Ok(recent.into_iter().next());
        }
    }

    bail!(
        "ambiguous {} desktop clients after title, tiling, and focus-history checks",
        spec.class
    )
}

fn desktop_app_spec(config: &DesktopConfig, app: DesktopApp) -> Result<DesktopAppSpec<'_>> {
    match app {
        DesktopApp::Configured(index) => {
            let configured = config
                .apps
                .get(index)
                .ok_or_else(|| anyhow!("desktop app {index} is not configured"))?;
            Ok(DesktopAppSpec {
                desktop_id: &configured.desktop_id,
                class: &configured.class,
                main_title: configured.title.as_deref(),
            })
        }
        DesktopApp::T3code => Ok(DesktopAppSpec {
            desktop_id: "t3code.desktop",
            class: "t3code",
            main_title: Some("T3 Code"),
        }),
        DesktopApp::Game => bail!("the game window is not a desktop entry"),
    }
}

/// Where a window on the EDGE should go: the focused other monitor, or else the
/// lowest-numbered one. Nothing when the window is elsewhere or the EDGE is the
/// only monitor.
fn off_edge_target<'m>(
    monitors: &'m [HyprMonitor],
    client_monitor: i64,
    edge_output: &str,
) -> Option<&'m HyprMonitor> {
    monitors
        .iter()
        .find(|monitor| monitor.id == client_monitor && monitor.name == edge_output)?;
    monitors
        .iter()
        .filter(|monitor| monitor.name != edge_output)
        .min_by_key(|monitor| (!monitor.focused, monitor.id))
}

fn is_game_client(client: &HyprClient) -> bool {
    (client.class == GAME_WINDOW_CLASS || client.initial_class == GAME_WINDOW_CLASS)
        && client.initial_title == GAME_WINDOW_TITLE
}

/// The part of the portal's monitor below the portal layer, when the portal
/// is shrunk to its header. A full-height portal yields nothing to place.
fn game_area(
    layers: &HashMap<String, HyprLayerMonitor>,
    monitors: &[HyprMonitor],
    edge_output: Option<&str>,
) -> Option<GameArea> {
    let mut portals = layers.iter().filter_map(|(monitor, levels)| {
        levels
            .levels
            .values()
            .flatten()
            .find(|layer| layer.namespace == PORTAL_LAYER_NAMESPACE)
            .map(|layer| (monitor.as_str(), layer))
    });
    let (monitor_name, portal) = portals.next()?;
    if portals.next().is_some() || edge_output.is_some_and(|edge| edge != monitor_name) {
        return None;
    }
    let monitor = monitors
        .iter()
        .find(|monitor| monitor.name == monitor_name)?;
    if monitor.scale <= 0.0 {
        return None;
    }
    let physical_height = if monitor.transform % 2 == 1 {
        monitor.width
    } else {
        monitor.height
    };
    let bottom = monitor.y + (physical_height as f64 / monitor.scale).round() as i64;
    let top = portal.y + portal.h;
    (portal.y == monitor.y && portal.h > 0 && top < bottom && portal.w > 0).then_some(GameArea {
        x: portal.x,
        y: top,
        width: portal.w,
        height: bottom - top,
    })
}

async fn launch_desktop(uwsm_bin: &Path, spec: DesktopAppSpec<'_>) -> Result<()> {
    launch_service(uwsm_bin, &[OsStr::new(spec.desktop_id)])
        .await
        .with_context(|| format!("launching desktop entry {}", spec.desktop_id))
}

/// Runs a desktop entry or command line as its own graphical service.
async fn launch_service(uwsm_bin: &Path, command: &[&OsStr]) -> Result<()> {
    let mut child = Command::new(uwsm_bin)
        // Bypass uwsm-app's runtime-directory lock/FIFO wrapper: the daemon's
        // sandbox can read the UWSM pipes but may only write its owned runtime
        // directory. A graphical service also avoids moving this already
        // service-owned caller into a scope.
        .args(["app", "-t", "service", "--"])
        .args(command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("launching graphical service")?;

    match timeout(Duration::from_millis(500), child.wait()).await {
        Ok(status) => {
            if status?.success() {
                Ok(())
            } else {
                bail!("desktop launch command failed")
            }
        }
        Err(_) => {
            tokio::spawn(async move {
                if let Ok(status) = child.wait().await
                    && !status.success()
                {
                    tracing::warn!(?status, "desktop application exited unsuccessfully");
                }
            });
            Ok(())
        }
    }
}

fn is_portal_preview_class(class: &str) -> bool {
    class.starts_with(PORTAL_PREVIEW_CLASS_PREFIX)
}

async fn hypr_state(hyprctl_bin: &Path) -> Result<(Vec<HyprClient>, Vec<HyprMonitor>)> {
    let clients = hypr_json(hyprctl_bin, &["clients", "-j"]).await?;
    let monitors = hypr_json(hyprctl_bin, &["monitors", "-j"]).await?;
    Ok((clients, monitors))
}

async fn hypr_json<T: for<'de> Deserialize<'de>>(hyprctl_bin: &Path, args: &[&str]) -> Result<T> {
    let output = Command::new(hyprctl_bin)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("running hyprctl")?;
    if !output.status.success() {
        bail!(
            "hyprctl failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("parsing hyprctl JSON")
}

async fn dispatch_focus(hyprctl_bin: &Path, address: &str) -> Result<()> {
    dispatch(hyprctl_bin, &focus_expression(address)?)
        .await
        .context("restoring Hyprland focus")
}

async fn dispatch(hyprctl_bin: &Path, expression: &str) -> Result<()> {
    let output = Command::new(hyprctl_bin)
        .args(["dispatch", expression])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("running a Hyprland dispatch")?;
    if !output.status.success() {
        bail!(
            "Hyprland dispatch failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim(),
        );
    }
    Ok(())
}

fn focus_expression(address: &str) -> Result<String> {
    Ok(format!(
        "hl.dsp.focus({{ window = \"{}\" }})",
        window_selector(address)?
    ))
}

fn move_to_monitor_expression(address: &str, monitor: &str) -> Result<String> {
    let safe = !monitor.is_empty()
        && monitor
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'));
    if !safe {
        bail!("Hyprland monitor name is not a plain connector name");
    }
    Ok(format!(
        "hl.dsp.window.move({{ monitor = \"{monitor}\", window = \"{}\" }})",
        window_selector(address)?
    ))
}

fn window_selector(address: &str) -> Result<String> {
    let Some(hex) = address.strip_prefix("0x") else {
        bail!("Hyprland window address lacks 0x prefix");
    };
    if hex.is_empty() || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("Hyprland window address is not hexadecimal");
    }
    Ok(format!("address:{address}"))
}

fn client_hosts_session_in(
    client_pid: i32,
    session: &str,
    socket_path: &Path,
    processes: &HashMap<i32, ProcessInfo>,
) -> bool {
    processes.iter().any(|(pid, process)| {
        is_descendant(*pid, client_pid, processes)
            && command_matches_session(&process.command, &process.environment, session, socket_path)
    })
}

#[derive(Debug)]
struct ProcessInfo {
    parent: i32,
    command: Vec<String>,
    environment: Vec<String>,
}

fn process_table() -> HashMap<i32, ProcessInfo> {
    let mut table = HashMap::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return table;
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        let path = entry.path();
        let Some(parent) = process_parent(&path.join("stat")) else {
            continue;
        };
        let command = nul_fields(&path.join("cmdline"));
        let environment = if interactive_herdr_session(&command).is_some() {
            nul_fields(&path.join("environ"))
        } else {
            Vec::new()
        };
        table.insert(
            pid,
            ProcessInfo {
                parent,
                command,
                environment,
            },
        );
    }
    table
}

fn process_parent(path: &Path) -> Option<i32> {
    let stat = fs::read_to_string(path).ok()?;
    let end = stat.rfind(')')?;
    stat.get(end + 2..)?.split_whitespace().nth(1)?.parse().ok()
}

fn nul_fields(path: &Path) -> Vec<String> {
    fs::read(path)
        .unwrap_or_default()
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .collect()
}

fn is_descendant(pid: i32, ancestor: i32, table: &HashMap<i32, ProcessInfo>) -> bool {
    let mut current = pid;
    let mut visited = HashSet::new();
    for _ in 0..32 {
        if current == ancestor {
            return true;
        }
        if !visited.insert(current) {
            return false;
        }
        let Some(process) = table.get(&current) else {
            return false;
        };
        if process.parent <= 1 {
            return false;
        }
        current = process.parent;
    }
    false
}

fn command_matches_session(
    command: &[String],
    environment: &[String],
    session: &str,
    socket_path: &Path,
) -> bool {
    let Some(command_session) = interactive_herdr_session(command) else {
        return false;
    };
    let expected_socket = socket_path.to_string_lossy();
    let environment_session = environment
        .iter()
        .find_map(|value| value.strip_prefix("HERDR_SESSION="));
    let environment_socket = environment
        .iter()
        .find_map(|value| value.strip_prefix("HERDR_SOCKET_PATH="));
    match command_session {
        HerdrClientSession::Named(value) => {
            !environment_session.is_some_and(|environment| environment != value) && value == session
        }
        HerdrClientSession::Default => {
            if environment_session.is_some_and(|value| value != session)
                || environment_socket.is_some_and(|value| value != expected_socket)
            {
                return false;
            }
            if environment_session.is_some() || environment_socket.is_some() {
                return true;
            }
            session == "default"
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HerdrClientSession<'a> {
    Default,
    Named(&'a str),
}

fn interactive_herdr_session(command: &[String]) -> Option<HerdrClientSession<'_>> {
    let is_herdr = command
        .first()
        .and_then(|value| Path::new(value).file_name())
        .is_some_and(|name| name == "herdr");
    if !is_herdr {
        return None;
    }
    if let [_, session, attach, name] = command
        && session == "session"
        && attach == "attach"
        && !name.is_empty()
    {
        return Some(HerdrClientSession::Named(name));
    }

    let mut named = None;
    let mut index = 1;
    while index < command.len() {
        let argument = &command[index];
        if argument == "--handoff" {
            index += 1;
            continue;
        }
        if argument == "--session" {
            let value = command.get(index + 1)?;
            if value.is_empty() || named.replace(value.as_str()).is_some() {
                return None;
            }
            index += 2;
            continue;
        }
        if let Some(value) = argument.strip_prefix("--session=") {
            if value.is_empty() || named.replace(value).is_some() {
                return None;
            }
            index += 1;
            continue;
        }
        // All positional tokens are CLI subcommands, while remote/no-session
        // and informational flags do not host the local persistent session.
        return None;
    }
    Some(named.map_or(HerdrClientSession::Default, HerdrClientSession::Named))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn named_session_requires_exact_identity() {
        assert!(command_matches_session(
            &["/usr/bin/herdr".into(), "--session".into(), "work".into()],
            &[],
            "work",
            Path::new("/tmp/work.sock")
        ));
        assert!(!command_matches_session(
            &["/usr/bin/herdr".into(), "--session".into(), "other".into()],
            &[],
            "work",
            Path::new("/tmp/work.sock")
        ));
        assert!(command_matches_session(
            &[
                "herdr".into(),
                "session".into(),
                "attach".into(),
                "work".into()
            ],
            &[],
            "work",
            Path::new("/tmp/work.sock")
        ));
        assert!(!command_matches_session(
            &[
                "herdr".into(),
                "session".into(),
                "attach".into(),
                "work".into()
            ],
            &[],
            "default",
            Path::new("/tmp/default.sock")
        ));
        assert!(!command_matches_session(
            &["herdr".into(), "--session".into(), "work".into()],
            &["HERDR_SESSION=default".into()],
            "default",
            Path::new("/tmp/default.sock")
        ));
        assert!(!command_matches_session(
            &["herdr".into(), "--session".into(), "work".into()],
            &["HERDR_SESSION=default".into()],
            "work",
            Path::new("/tmp/work.sock")
        ));
        assert!(command_matches_session(
            &["herdr".into(), "--session".into(), "work".into()],
            &["HERDR_SOCKET_PATH=/tmp/default.sock".into()],
            "work",
            Path::new("/tmp/work.sock")
        ));
        assert!(!command_matches_session(
            &["herdr".into(), "--session".into(), "work".into()],
            &["HERDR_SOCKET_PATH=/tmp/default.sock".into()],
            "default",
            Path::new("/tmp/default.sock")
        ));
    }

    #[test]
    fn default_does_not_claim_named_client() {
        assert!(!command_matches_session(
            &["herdr".into(), "--session".into(), "work".into()],
            &[],
            "default",
            Path::new("/tmp/default.sock")
        ));
        assert!(command_matches_session(
            &["herdr".into()],
            &[],
            "default",
            Path::new("/tmp/default.sock")
        ));
        assert!(!command_matches_session(
            &["herdr".into()],
            &["HERDR_SOCKET_PATH=/tmp/named.sock".into()],
            "default",
            Path::new("/tmp/default.sock")
        ));
    }

    #[test]
    fn only_interactive_herdr_clients_can_host_a_session() {
        for command in [
            vec!["herdr", "agent", "wait", "reviewer"],
            vec!["herdr", "status"],
            vec!["herdr", "api", "state"],
            vec!["herdr", "workspace", "list"],
            vec!["herdr", "update"],
            vec!["herdr", "server"],
            vec!["herdr", "--remote", "host"],
            vec!["herdr", "--no-session"],
            vec!["herdr", "--skill"],
        ] {
            assert!(
                !command_matches_session(
                    &command.into_iter().map(String::from).collect::<Vec<_>>(),
                    &[],
                    "default",
                    Path::new("/tmp/default.sock")
                ),
                "CLI command must not claim the compositor client"
            );
        }
        assert!(command_matches_session(
            &["herdr".into(), "--handoff".into()],
            &[],
            "default",
            Path::new("/tmp/default.sock")
        ));
        assert!(command_matches_session(
            &["herdr".into(), "--session=work".into(), "--handoff".into()],
            &[],
            "work",
            Path::new("/tmp/work.sock")
        ));
    }

    #[test]
    fn terminal_hosted_herdr_uses_most_recent_exact_session_window() {
        let clients: Vec<HyprClient> = serde_json::from_str(
            r#"[
                {"address":"0xold","class":"kitty","initialClass":"kitty","title":"older workspace","initialTitle":"kitty","pid":100,"monitor":0,"focusHistoryID":7},
                {"address":"0xnew","class":"kitty","initialClass":"kitty","title":"current workspace","initialTitle":"kitty","pid":200,"monitor":0,"focusHistoryID":1},
                {"address":"0xother","class":"kitty","initialClass":"kitty","title":"shell","initialTitle":"kitty","pid":300,"monitor":0,"focusHistoryID":2},
                {"address":"0xcli","class":"kitty","initialClass":"kitty","title":"recent CLI terminal","initialTitle":"kitty","pid":400,"monitor":0,"focusHistoryID":0}
            ]"#,
        )
        .unwrap();
        let processes = HashMap::from([
            (
                101,
                ProcessInfo {
                    parent: 100,
                    command: vec!["herdr".into()],
                    environment: vec![],
                },
            ),
            (
                201,
                ProcessInfo {
                    parent: 200,
                    command: vec!["/home/user/.local/bin/herdr".into()],
                    environment: vec![],
                },
            ),
            (
                301,
                ProcessInfo {
                    parent: 300,
                    command: vec!["bash".into()],
                    environment: vec![],
                },
            ),
            (
                401,
                ProcessInfo {
                    parent: 400,
                    command: vec![
                        "herdr".into(),
                        "agent".into(),
                        "wait".into(),
                        "reviewer".into(),
                    ],
                    environment: vec![],
                },
            ),
        ]);

        let selected = select_herdr_candidate(
            &clients,
            &processes,
            "default",
            Path::new("/tmp/default.sock"),
            "herdr",
        )
        .unwrap();
        assert_eq!(selected.address, "0xnew");
    }

    #[test]
    fn herdr_window_selection_prefers_native_identity_hint() {
        let clients: Vec<HyprClient> = serde_json::from_str(
            r#"[
                {"address":"0xterminal","class":"kitty","initialClass":"kitty","title":"workspace","initialTitle":"kitty","pid":100,"monitor":0,"focusHistoryID":0},
                {"address":"0xnative","class":"Herdr","initialClass":"Herdr","title":"Herdr","initialTitle":"Herdr","pid":200,"monitor":0,"focusHistoryID":4}
            ]"#,
        )
        .unwrap();
        let processes = HashMap::from([
            (
                101,
                ProcessInfo {
                    parent: 100,
                    command: vec!["herdr".into()],
                    environment: vec![],
                },
            ),
            (
                201,
                ProcessInfo {
                    parent: 200,
                    command: vec!["herdr".into()],
                    environment: vec![],
                },
            ),
        ]);

        let selected = select_herdr_candidate(
            &clients,
            &processes,
            "default",
            Path::new("/tmp/default.sock"),
            "herdr",
        )
        .unwrap();
        assert_eq!(selected.address, "0xnative");
    }

    #[test]
    fn herdr_window_selection_fails_closed_on_tied_focus_history() {
        let clients: Vec<HyprClient> = serde_json::from_str(
            r#"[
                {"address":"0x1","class":"kitty","initialClass":"kitty","title":"one","initialTitle":"kitty","pid":100,"monitor":0,"focusHistoryID":0},
                {"address":"0x2","class":"kitty","initialClass":"kitty","title":"two","initialTitle":"kitty","pid":200,"monitor":0,"focusHistoryID":0}
            ]"#,
        )
        .unwrap();
        let processes = HashMap::from([
            (
                101,
                ProcessInfo {
                    parent: 100,
                    command: vec!["herdr".into()],
                    environment: vec![],
                },
            ),
            (
                201,
                ProcessInfo {
                    parent: 200,
                    command: vec!["herdr".into()],
                    environment: vec![],
                },
            ),
        ]);

        assert!(
            select_herdr_candidate(
                &clients,
                &processes,
                "default",
                Path::new("/tmp/default.sock"),
                "herdr",
            )
            .is_err()
        );
    }

    #[test]
    fn herdr_window_selection_fails_closed_on_partial_focus_history() {
        let clients: Vec<HyprClient> = serde_json::from_str(
            r#"[
                {"address":"0x1","class":"kitty","initialClass":"kitty","title":"one","initialTitle":"kitty","pid":100,"monitor":0},
                {"address":"0x2","class":"kitty","initialClass":"kitty","title":"two","initialTitle":"kitty","pid":200,"monitor":0,"focusHistoryID":0}
            ]"#,
        )
        .unwrap();
        let processes = HashMap::from([
            (
                101,
                ProcessInfo {
                    parent: 100,
                    command: vec!["herdr".into()],
                    environment: vec![],
                },
            ),
            (
                201,
                ProcessInfo {
                    parent: 200,
                    command: vec!["herdr".into()],
                    environment: vec![],
                },
            ),
        ]);

        assert!(
            select_herdr_candidate(
                &clients,
                &processes,
                "default",
                Path::new("/tmp/default.sock"),
                "herdr",
            )
            .is_err()
        );
    }

    #[test]
    fn desktop_apps_are_fixed_allowlisted_targets() {
        assert_eq!(
            desktop_app_spec(&DesktopConfig::default(), DesktopApp::Configured(0)).unwrap(),
            DesktopAppSpec {
                desktop_id: "spotify.desktop",
                class: "spotify",
                main_title: Some("Spotify"),
            }
        );
        assert_eq!(
            desktop_app_spec(&DesktopConfig::default(), DesktopApp::Configured(1)).unwrap(),
            DesktopAppSpec {
                desktop_id: "Discord.desktop",
                class: "chrome-discord.com__channels_@me-Default",
                main_title: Some("Discord"),
            }
        );
        assert_eq!(
            desktop_app_spec(&DesktopConfig::default(), DesktopApp::T3code).unwrap(),
            DesktopAppSpec {
                desktop_id: "t3code.desktop",
                class: "t3code",
                main_title: Some("T3 Code"),
            }
        );
    }

    #[test]
    fn t3code_window_selection_matches_the_exact_desktop_class() {
        let clients: Vec<HyprClient> = serde_json::from_str(
            r#"[
                {"address":"0x1","class":"kitty","initialClass":"kitty","title":"t3code","initialTitle":"t3code","pid":1,"monitor":0,"floating":false,"focusHistoryID":0},
                {"address":"0x2","class":"t3code","initialClass":"t3code","title":"T3 Code (Alpha)","initialTitle":"T3 Code (Alpha)","pid":2,"monitor":0,"floating":false,"focusHistoryID":3}
            ]"#,
        )
        .unwrap();

        let selected = select_desktop_candidate(
            &clients,
            desktop_app_spec(&DesktopConfig::default(), DesktopApp::T3code).unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(selected.address, "0x2");
        assert!(
            select_desktop_candidate(
                &clients[..1],
                desktop_app_spec(&DesktopConfig::default(), DesktopApp::T3code).unwrap()
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn desktop_candidate_prefers_main_window_then_recent_focus() {
        let clients: Vec<HyprClient> = serde_json::from_str(
            r#"[
                {"address":"0x1","class":"spotify","initialClass":"spotify","title":"Quick Chat","initialTitle":"Quick Chat","pid":1,"monitor":0,"floating":true,"focusHistoryID":0},
                {"address":"0x2","class":"spotify","initialClass":"spotify","title":"Thread A","initialTitle":"Spotify","pid":2,"monitor":0,"floating":false,"focusHistoryID":4},
                {"address":"0x3","class":"spotify","initialClass":"spotify","title":"Thread B","initialTitle":"Spotify","pid":3,"monitor":0,"floating":false,"focusHistoryID":1}
            ]"#,
        )
        .unwrap();

        let selected = select_desktop_candidate(
            &clients,
            desktop_app_spec(&DesktopConfig::default(), DesktopApp::Configured(0)).unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(selected.address, "0x3");
    }

    #[test]
    fn desktop_candidate_fails_closed_when_narrowing_is_still_ambiguous() {
        let clients: Vec<HyprClient> = serde_json::from_str(
            r#"[
                {"address":"0x1","class":"spotify","initialClass":"spotify","title":"Thread A","initialTitle":"Spotify","pid":1,"monitor":0,"floating":false},
                {"address":"0x2","class":"spotify","initialClass":"spotify","title":"Thread B","initialTitle":"Spotify","pid":2,"monitor":0,"floating":false}
            ]"#,
        )
        .unwrap();

        assert!(
            select_desktop_candidate(
                &clients,
                desktop_app_spec(&DesktopConfig::default(), DesktopApp::Configured(0)).unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn desktop_candidate_fails_closed_on_partial_focus_history() {
        let clients: Vec<HyprClient> = serde_json::from_str(
            r#"[
                {"address":"0x1","class":"spotify","initialClass":"spotify","title":"Thread A","initialTitle":"Spotify","pid":1,"monitor":0,"floating":false},
                {"address":"0x2","class":"spotify","initialClass":"spotify","title":"Thread B","initialTitle":"Spotify","pid":2,"monitor":0,"floating":false,"focusHistoryID":0}
            ]"#,
        )
        .unwrap();

        assert!(
            select_desktop_candidate(
                &clients,
                desktop_app_spec(&DesktopConfig::default(), DesktopApp::Configured(0)).unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn focus_expression_uses_installed_lua_dispatcher_syntax() {
        assert_eq!(
            focus_expression("0x123abc").unwrap(),
            "hl.dsp.focus({ window = \"address:0x123abc\" })"
        );
        assert!(focus_expression("title:herdr").is_err());
        assert!(focus_expression("0x1\" })").is_err());
    }

    #[test]
    fn move_expression_uses_installed_lua_dispatcher_syntax() {
        assert_eq!(
            move_to_monitor_expression("0x123abc", "DP-1").unwrap(),
            "hl.dsp.window.move({ monitor = \"DP-1\", window = \"address:0x123abc\" })"
        );
        assert!(move_to_monitor_expression("0x123abc", "").is_err());
        assert!(move_to_monitor_expression("0x123abc", "DP-1\" })").is_err());
        assert!(move_to_monitor_expression("title:herdr", "DP-1").is_err());
    }

    fn monitors(entries: &[(i64, &str, bool)]) -> Vec<HyprMonitor> {
        serde_json::from_value(serde_json::Value::Array(
            entries
                .iter()
                .map(|(id, name, focused)| {
                    serde_json::json!({"id": id, "name": name, "focused": focused})
                })
                .collect(),
        ))
        .unwrap()
    }

    #[test]
    fn off_edge_target_prefers_the_focused_other_monitor() {
        let all = monitors(&[
            (1, "DP-1", false),
            (2, "DP-2", false),
            (3, "HDMI-A-1", true),
        ]);
        assert_eq!(off_edge_target(&all, 2, "DP-2").unwrap().name, "HDMI-A-1");
    }

    #[test]
    fn off_edge_target_falls_back_to_the_lowest_id_when_the_edge_has_focus() {
        let all = monitors(&[
            (3, "HDMI-A-1", false),
            (2, "DP-2", true),
            (1, "DP-1", false),
        ]);
        assert_eq!(off_edge_target(&all, 2, "DP-2").unwrap().name, "DP-1");
    }

    #[test]
    fn off_edge_target_leaves_other_windows_and_a_lone_edge_alone() {
        let all = monitors(&[(1, "DP-1", true), (2, "DP-2", false)]);
        assert!(off_edge_target(&all, 1, "DP-2").is_none());
        let lone = monitors(&[(2, "DP-2", true)]);
        assert!(off_edge_target(&lone, 2, "DP-2").is_none());
    }

    /// Runs `focus_or_launch` for Spotify against a fake Hyprland showing it on
    /// the given monitor, and returns the dispatches in order.
    async fn dispatches_for_spotify_on(monitor: i64) -> Vec<String> {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("dispatch.log");
        let hyprctl = temp.path().join("hyprctl");
        write_executable(
            &hyprctl,
            &format!(
                r#"#!/bin/sh
if [ "$1" = "clients" ]; then
  printf '%s\n' '[{{"address":"0xabc","class":"spotify","initialClass":"spotify","title":"Spotify","pid":42,"monitor":{monitor}}}]'
elif [ "$1" = "monitors" ]; then
  printf '%s\n' '[{{"id":1,"name":"DP-1","focused":false}},{{"id":2,"name":"DP-2","focused":true}}]'
elif [ "$1" = "dispatch" ]; then
  printf '%s\n' "$2" >> "{log}"
  printf '%s\n' 'ok'
fi
"#,
                log = log.display()
            ),
        );
        let config = DesktopConfig {
            edge_output: Some("DP-2".into()),
            ..DesktopConfig::default()
        };
        let mut controller = DesktopController::new(config);
        controller.hyprctl_bin = hyprctl;
        assert_eq!(
            controller
                .focus_or_launch(DesktopApp::Configured(0))
                .await
                .unwrap(),
            DesktopAppOutcome::Focused
        );
        fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[tokio::test]
    async fn app_on_the_edge_moves_to_another_monitor_before_focus() {
        assert_eq!(
            dispatches_for_spotify_on(2).await,
            [
                "hl.dsp.window.move({ monitor = \"DP-1\", window = \"address:0xabc\" })",
                "hl.dsp.focus({ window = \"address:0xabc\" })",
            ]
        );
    }

    #[tokio::test]
    async fn launched_app_that_lands_on_the_edge_moves_before_focus() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("mapped");
        let log = temp.path().join("dispatch.log");
        let hyprctl = temp.path().join("hyprctl");
        let uwsm = temp.path().join("uwsm");
        write_executable(
            &hyprctl,
            &format!(
                r#"#!/bin/sh
if [ "$1" = "clients" ]; then
  if [ -e "{marker}" ]; then
    printf '%s\n' '[{{"address":"0xabc","class":"spotify","initialClass":"spotify","title":"Spotify","pid":42,"monitor":2}}]'
  else
    printf '%s\n' '[]'
  fi
elif [ "$1" = "monitors" ]; then
  printf '%s\n' '[{{"id":1,"name":"DP-1","focused":false}},{{"id":2,"name":"DP-2","focused":true}}]'
elif [ "$1" = "dispatch" ]; then
  printf '%s\n' "$2" >> "{log}"
  printf '%s\n' 'ok'
fi
"#,
                marker = marker.display(),
                log = log.display()
            ),
        );
        write_executable(&uwsm, &format!("#!/bin/sh\n: > \"{}\"\n", marker.display()));
        let config = DesktopConfig {
            edge_output: Some("DP-2".into()),
            ..DesktopConfig::default()
        };
        let mut controller = DesktopController::new(config);
        controller.hyprctl_bin = hyprctl;
        controller.uwsm_bin = uwsm;
        controller.launch_poll_interval = Duration::from_millis(10);
        controller.launch_observation_timeout = Duration::from_secs(2);
        assert_eq!(
            controller
                .focus_or_launch(DesktopApp::Configured(0))
                .await
                .unwrap(),
            DesktopAppOutcome::LaunchedAndFocused
        );
        assert_eq!(
            fs::read_to_string(log).unwrap().lines().collect::<Vec<_>>(),
            [
                "hl.dsp.window.move({ monitor = \"DP-1\", window = \"address:0xabc\" })",
                "hl.dsp.focus({ window = \"address:0xabc\" })",
            ]
        );
    }

    #[tokio::test]
    async fn app_off_the_edge_is_only_focused() {
        assert_eq!(
            dispatches_for_spotify_on(1).await,
            ["hl.dsp.focus({ window = \"address:0xabc\" })"]
        );
    }

    fn edge_layers(portal_height: i64) -> HashMap<String, HyprLayerMonitor> {
        serde_json::from_value(serde_json::json!({
            "DP-2": {"levels": {
                "2": [{"x": 0, "y": 1152, "w": 2048, "h": 26, "namespace": "omarchy-bar"}],
                "3": [{"x": 0, "y": 1152, "w": 2048, "h": portal_height,
                       "namespace": "xeneon-edge-agent-portal"}]
            }},
            "DP-1": {"levels": {
                "2": [{"x": 0, "y": 0, "w": 2048, "h": 26, "namespace": "omarchy-bar"}]
            }}
        }))
        .unwrap()
    }

    fn edge_monitors() -> Vec<HyprMonitor> {
        serde_json::from_value(serde_json::json!([
            {"id": 0, "name": "DP-1", "y": 0, "width": 2560, "height": 1440, "scale": 1.25},
            {"id": 1, "name": "DP-2", "y": 1152, "width": 2560, "height": 720,
             "scale": 1.25, "transform": 0}
        ]))
        .unwrap()
    }

    #[test]
    fn game_area_is_the_edge_below_the_shrunk_portal_header() {
        assert_eq!(
            game_area(&edge_layers(76), &edge_monitors(), Some("DP-2")),
            Some(GameArea {
                x: 0,
                y: 1228,
                width: 2048,
                height: 500,
            })
        );
        // A full-height portal covers the game; nothing is placed.
        assert_eq!(
            game_area(&edge_layers(576), &edge_monitors(), Some("DP-2")),
            None
        );
        // Only the configured EDGE output hosts the game.
        assert_eq!(
            game_area(&edge_layers(76), &edge_monitors(), Some("DP-1")),
            None
        );
    }

    #[test]
    fn game_client_needs_the_qml_runtime_class_and_fixed_title() {
        let client = |class: &str, title: &str| -> HyprClient {
            serde_json::from_value(serde_json::json!({
                "address": "0x1", "class": class, "initialClass": class,
                "title": "page", "initialTitle": title, "pid": 1, "monitor": 1,
                "at": [0, 1228], "size": [2048, 500]
            }))
            .unwrap()
        };
        assert!(is_game_client(&client(
            "org.qt-project.qml",
            "xeneon-edge-game"
        )));
        assert!(!is_game_client(&client("org.qt-project.qml", "other")));
        assert!(!is_game_client(&client("chromium", "xeneon-edge-game")));
        assert_eq!(
            client("org.qt-project.qml", "xeneon-edge-game").size,
            [2048, 500]
        );
    }

    #[test]
    fn portal_preview_class_is_never_a_focus_restore_target() {
        assert!(is_portal_preview_class(
            "org.xeneon-edge.agent-portal-preview.123"
        ));
        assert!(!is_portal_preview_class("spotify"));
    }

    #[tokio::test]
    async fn concurrent_launch_requests_coalesce_until_one_client_maps() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("mapped");
        let launch_log = temp.path().join("launch.log");
        let focus_log = temp.path().join("focus.log");
        let hyprctl = temp.path().join("hyprctl");
        let uwsm = temp.path().join("uwsm");
        write_executable(
            &hyprctl,
            &format!(
                r#"#!/bin/sh
if [ "$1" = "clients" ]; then
  if [ -e "{marker}" ]; then
    printf '%s\n' '[{{"address":"0xdef","class":"chrome-discord.com__channels_@me-Default","initialClass":"chrome-discord.com__channels_@me-Default","title":"Discord","pid":43,"monitor":0}},{{"address":"0xabc","class":"spotify","initialClass":"spotify","title":"Spotify","pid":42,"monitor":0}}]'
  else
    printf '%s\n' '[{{"address":"0xdef","class":"chrome-discord.com__channels_@me-Default","initialClass":"chrome-discord.com__channels_@me-Default","title":"Discord","pid":43,"monitor":0}}]'
  fi
elif [ "$1" = "dispatch" ]; then
  printf '%s\n' "$*" >> "{focus_log}"
  printf '%s\n' 'ok'
else
  printf '%s\n' '{{}}'
fi
"#,
                marker = marker.display(),
                focus_log = focus_log.display()
            ),
        );
        write_executable(
            &uwsm,
            &format!(
                r#"#!/bin/sh
printf '%s\n' "$*" >> "{launch_log}"
( sleep 0.05; : > "{marker}" ) &
sleep 1
"#,
                launch_log = launch_log.display(),
                marker = marker.display()
            ),
        );

        let mut controller = DesktopController::new(DesktopConfig::default());
        controller.hyprctl_bin = hyprctl;
        controller.uwsm_bin = uwsm;
        controller.launch_poll_interval = Duration::from_millis(10);
        controller.launch_observation_timeout = Duration::from_secs(2);
        controller.launch_coalesce_for = Duration::from_secs(3);
        let controller = Arc::new(controller);

        let first_controller = Arc::clone(&controller);
        let first = tokio::spawn(async move {
            first_controller
                .focus_or_launch(DesktopApp::Configured(0))
                .await
        });
        tokio::time::sleep(Duration::from_millis(5)).await;
        let second_controller = Arc::clone(&controller);
        let second = tokio::spawn(async move {
            second_controller
                .focus_or_launch(DesktopApp::Configured(0))
                .await
        });
        let other_app_controller = Arc::clone(&controller);
        let other_app = tokio::spawn(async move {
            other_app_controller
                .focus_or_launch(DesktopApp::Configured(1))
                .await
        });

        assert_eq!(
            tokio::time::timeout(Duration::from_millis(150), other_app)
                .await
                .expect("another app must not wait for Spotify mapping")
                .unwrap()
                .unwrap(),
            DesktopAppOutcome::Focused
        );

        assert_eq!(
            first.await.unwrap().unwrap(),
            DesktopAppOutcome::LaunchedAndFocused
        );
        assert_eq!(
            second.await.unwrap().unwrap(),
            DesktopAppOutcome::LaunchedAndFocused
        );
        assert_eq!(fs::read_to_string(&launch_log).unwrap().lines().count(), 1);
        assert_eq!(
            fs::read_to_string(launch_log).unwrap().trim(),
            "app -t service -- spotify.desktop"
        );
        assert_eq!(fs::read_to_string(focus_log).unwrap().lines().count(), 3);
    }

    #[tokio::test]
    async fn failed_mapping_clears_launch_coalescing_state() {
        let temp = tempfile::tempdir().unwrap();
        let launch_log = temp.path().join("launch.log");
        let hyprctl = temp.path().join("hyprctl");
        let uwsm = temp.path().join("uwsm");
        write_executable(
            &hyprctl,
            r#"#!/bin/sh
if [ "$1" = "clients" ]; then
  printf '%s\n' '[]'
else
  printf '%s\n' '{}'
fi
"#,
        );
        write_executable(
            &uwsm,
            &format!(
                r#"#!/bin/sh
printf '%s\n' "$*" >> "{launch_log}"
"#,
                launch_log = launch_log.display()
            ),
        );

        let mut controller = DesktopController::new(DesktopConfig::default());
        controller.hyprctl_bin = hyprctl;
        controller.uwsm_bin = uwsm;
        controller.launch_poll_interval = Duration::from_millis(2);
        controller.launch_observation_timeout = Duration::from_millis(15);
        controller.launch_coalesce_for = Duration::from_secs(10);

        assert!(
            controller
                .focus_or_launch(DesktopApp::Configured(0))
                .await
                .is_err()
        );
        assert!(
            controller
                .focus_or_launch(DesktopApp::Configured(0))
                .await
                .is_err()
        );
        assert_eq!(fs::read_to_string(launch_log).unwrap().lines().count(), 2);
    }

    /// Written by a child process: a write fd held here would leak into any
    /// test forking at that moment, and running the stub then fails ETXTBSY.
    fn write_executable(path: &Path, contents: &str) {
        let mut writer = std::process::Command::new("sh")
            .args(["-c", "cat >\"$1\" && chmod 700 \"$1\"", "sh"])
            .arg(path)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        writer
            .stdin
            .take()
            .unwrap()
            .write_all(contents.as_bytes())
            .unwrap();
        assert!(writer.wait().unwrap().success());
    }
}
