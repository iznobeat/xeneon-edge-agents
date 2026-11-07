use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{AgentBackend, DashboardKind};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Agent manager observed at startup unless a persisted portal choice
    /// overrides it. The portal can switch between `herdr` and `t3code`.
    pub agent_backend: AgentBackend,
    pub voxtype_bin: PathBuf,
    pub herdr_refresh_ms: u64,
    pub health_refresh_ms: u64,
    pub voice_refresh_ms: u64,
    pub usage_refresh_ms: u64,
    pub micro_refresh_ms: u64,
    pub screen: ScreenConfig,
    pub desktop: DesktopConfig,
    /// Enabled dashboards in switcher order. Usually set per machine in
    /// `dashboards.toml` beside this file.
    pub dashboards: Vec<DashboardKind>,
    pub spotify: SpotifyConfig,
    pub basecamp: BasecampConfig,
    pub slack: SlackConfig,
    pub github: GithubConfig,
    pub game: GameConfig,
    pub t3code: T3codeConfig,
    pub herdr: HerdrConfig,
}

/// Where the Herdr manager reads herdr-core and reaches the Herdr app. Both
/// default to the locations those programs use for the current user.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HerdrConfig {
    /// herdr-core's JSON-RPC socket. Defaults to
    /// `$XDG_RUNTIME_DIR/herdr-core/$HC_SESSION.sock` (session `default`).
    pub core_socket: Option<PathBuf>,
    /// The Herdr app's command socket. Defaults to
    /// `$HERDR_APP_HOME/herdr-app.sock`, then
    /// `$XDG_CONFIG_HOME/herdr-app/herdr-app.sock`.
    pub app_socket: Option<PathBuf>,
}

impl HerdrConfig {
    pub fn core_socket(&self) -> Result<PathBuf> {
        if let Some(path) = &self.core_socket {
            return Ok(path.clone());
        }
        let runtime = env::var_os("XDG_RUNTIME_DIR")
            .filter(|value| !value.is_empty())
            .context("XDG_RUNTIME_DIR is not set; set herdr.core_socket")?;
        let session = env::var("HC_SESSION")
            .ok()
            .filter(|session| is_session_name(session))
            .unwrap_or_else(|| "default".to_owned());
        Ok(PathBuf::from(runtime)
            .join("herdr-core")
            .join(format!("{session}.sock")))
    }

    pub fn app_socket(&self) -> Result<PathBuf> {
        if let Some(path) = &self.app_socket {
            return Ok(path.clone());
        }
        if let Some(home) = env::var_os("HERDR_APP_HOME").filter(|value| !value.is_empty()) {
            return Ok(PathBuf::from(home).join("herdr-app.sock"));
        }
        config_home()
            .map(|root| root.join("herdr-app/herdr-app.sock"))
            .context("neither XDG_CONFIG_HOME nor HOME is set; set herdr.app_socket")
    }
}

fn is_session_name(session: &str) -> bool {
    !session.is_empty()
        && session.len() <= 64
        && session
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        && !session.starts_with('.')
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct T3codeConfig {
    /// T3 Code data directory. Defaults to `$T3CODE_HOME`, then `~/.t3`.
    pub home: Option<PathBuf>,
    /// How often the read-only T3 Code projection sequence is polled.
    pub refresh_ms: u64,
}

impl Default for T3codeConfig {
    fn default() -> Self {
        Self {
            home: None,
            refresh_ms: 1_000,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenConfig {
    pub connector: Option<String>,
    pub serial: Option<String>,
    pub model: Option<String>,
    pub touch_device: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopConfig {
    pub edge_output: Option<String>,
    pub herdr_class: String,
    /// Desktop apps the daemon may focus or launch. The portal can only name one of
    /// these by `id`; the daemon resolves everything else from this list.
    pub apps: Vec<DesktopAppConfig>,
}

pub const MAX_DESKTOP_APPS: usize = 4;
/// Per-machine shortcut list kept beside `config.toml`. The installer owns
/// `config.toml` and refuses to overwrite a modified copy, so the list lives
/// in a file it never writes.
pub const DESKTOP_APPS_FILE_NAME: &str = "desktop-apps.toml";
/// Per-machine list of enabled dashboards, also never written by the installer.
pub const DASHBOARDS_FILE_NAME: &str = "dashboards.toml";
pub const MAX_DASHBOARDS: usize = 6;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DashboardsFile {
    dashboards: Vec<DashboardKind>,
    #[serde(default)]
    spotify: Option<SpotifyConfig>,
    #[serde(default)]
    basecamp: Option<BasecampConfig>,
    #[serde(default)]
    slack: Option<SlackConfig>,
    #[serde(default)]
    github: Option<GithubConfig>,
    #[serde(default)]
    game: Option<GameConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpotifyConfig {
    /// Look up lyrics for the playing track on LRCLIB (lrclib.net), which
    /// then learns the title, artist, album, and length of what plays.
    pub lyrics: bool,
}

impl Default for SpotifyConfig {
    fn default() -> Self {
        Self { lyrics: true }
    }
}

pub const DEFAULT_GAME_URL: &str = "https://oh-no-more-agents.com/?full&fit";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GameConfig {
    /// Page shown full-bleed by the game dashboard; http or https only.
    pub url: String,
    /// Qt 6 `qml` runtime that hosts the page. Quickshell cannot load
    /// QtWebEngine itself, so the page runs in its own window.
    pub qml_bin: PathBuf,
}

impl Default for GameConfig {
    fn default() -> Self {
        Self {
            url: DEFAULT_GAME_URL.into(),
            qml_bin: PathBuf::from("/usr/lib/qt6/bin/qml"),
        }
    }
}

/// An absolute http(s) URL without whitespace or control characters.
fn is_game_url(url: &str) -> bool {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    url.len() <= 2048
        && rest.is_some_and(|rest| {
            !rest.is_empty()
                && !rest.starts_with(['/', '?', '#'])
                && !rest
                    .chars()
                    .any(|character| character.is_whitespace() || character.is_control())
        })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GithubConfig {
    /// The GitHub CLI, already signed in by the user.
    pub bin: PathBuf,
    /// Organization whose recently pushed repositories feed the commit list.
    pub organization: String,
    /// `gh` account to read as (`gh auth status`); the active one when unset.
    pub user: Option<String>,
}

impl Default for GithubConfig {
    fn default() -> Self {
        Self {
            bin: PathBuf::from("gh"),
            organization: String::new(),
            user: None,
        }
    }
}

/// GitHub logins and organization names: letters, digits, single hyphens.
fn is_github_name(name: &str) -> bool {
    (1..=39).contains(&name.len())
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

pub const MAX_SLACK_CHANNELS: usize = 6;
/// The Slack user token lives in its own owner-only file beside config.toml.
pub const SLACK_TOKEN_FILE_NAME: &str = "slack.toml";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SlackConfig {
    /// Channel names (without '#') shown as tabs, in order.
    pub channels: Vec<String>,
    /// Token file; `slack.toml` beside config.toml when unset.
    pub token_file: Option<PathBuf>,
}

pub const MAX_CARD_TABLES: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BasecampConfig {
    /// The Basecamp CLI, already signed in by the user.
    pub bin: PathBuf,
    /// Named CLI profile (`basecamp profile list`) whose identity the
    /// dashboard reads as; the CLI's default profile when unset.
    pub profile: Option<String>,
    /// Card tables the Basecamp dashboard can browse, in switcher order.
    pub card_tables: Vec<CardTableConfig>,
}

impl Default for BasecampConfig {
    fn default() -> Self {
        Self {
            bin: PathBuf::from("basecamp"),
            profile: None,
            card_tables: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardTableConfig {
    /// Switcher text, at most 24 characters.
    pub label: String,
    /// Basecamp project (bucket) id.
    pub project: u64,
    /// Card table id within that project.
    pub card_table: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DesktopAppsFile {
    #[serde(default)]
    apps: Vec<DesktopAppConfig>,
}
const DESKTOP_APP_ACCENTS: [&str; 7] = [
    "accent", "green", "blue", "cyan", "magenta", "orange", "yellow",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopAppConfig {
    /// Stable lowercase identifier the portal sends back.
    pub id: String,
    /// Button text.
    pub label: String,
    /// Desktop entry launched through `uwsm app` when no window exists.
    pub desktop_id: String,
    /// Exact Hyprland window class (or initial class) to focus.
    pub class: String,
    /// Initial window title preferred when several windows share the class.
    #[serde(default)]
    pub title: Option<String>,
    /// Theme color role for the button accent.
    #[serde(default = "default_app_accent")]
    pub accent: String,
}

fn default_app_accent() -> String {
    "accent".into()
}

fn default_desktop_apps() -> Vec<DesktopAppConfig> {
    vec![
        DesktopAppConfig {
            id: "spotify".into(),
            label: "Spotify".into(),
            desktop_id: "spotify.desktop".into(),
            class: "spotify".into(),
            title: Some("Spotify".into()),
            accent: "green".into(),
        },
        DesktopAppConfig {
            id: "discord".into(),
            label: "Discord".into(),
            desktop_id: "Discord.desktop".into(),
            class: "chrome-discord.com__channels_@me-Default".into(),
            title: Some("Discord".into()),
            accent: "blue".into(),
        },
    ]
}

impl DesktopAppConfig {
    fn validate(&self) -> Result<()> {
        let id_ok = !self.id.is_empty()
            && self.id.len() <= 32
            && self
                .id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if !id_ok {
            anyhow::bail!("desktop.apps id must be 1-32 lowercase letters, digits, or '-'");
        }
        let label = self.label.trim();
        if label.is_empty() || label.chars().count() > 16 {
            anyhow::bail!("desktop.apps '{}' label must be 1-16 characters", self.id);
        }
        if !self.desktop_id.ends_with(".desktop")
            || self.desktop_id.contains('/')
            || self.desktop_id.starts_with('-')
        {
            anyhow::bail!(
                "desktop.apps '{}' desktop_id must be a bare .desktop file name",
                self.id
            );
        }
        if self.class.trim().is_empty() {
            anyhow::bail!("desktop.apps '{}' class must not be empty", self.id);
        }
        if self
            .title
            .as_deref()
            .is_some_and(|title| title.trim().is_empty())
        {
            anyhow::bail!("desktop.apps '{}' title must not be empty", self.id);
        }
        if !DESKTOP_APP_ACCENTS.contains(&self.accent.as_str()) {
            anyhow::bail!(
                "desktop.apps '{}' accent must be one of {}",
                self.id,
                DESKTOP_APP_ACCENTS.join(", ")
            );
        }
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            agent_backend: AgentBackend::Herdr,
            voxtype_bin: PathBuf::from("voxtype"),
            herdr_refresh_ms: 5_000,
            health_refresh_ms: 1_000,
            voice_refresh_ms: 250,
            usage_refresh_ms: 30_000,
            micro_refresh_ms: 5_000,
            screen: ScreenConfig::default(),
            desktop: DesktopConfig::default(),
            dashboards: vec![DashboardKind::Agents],
            spotify: SpotifyConfig::default(),
            basecamp: BasecampConfig::default(),
            slack: SlackConfig::default(),
            github: GithubConfig::default(),
            game: GameConfig::default(),
            t3code: T3codeConfig::default(),
            herdr: HerdrConfig::default(),
        }
    }
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            edge_output: None,
            herdr_class: "herdr-app".into(),
            apps: default_desktop_apps(),
        }
    }
}

impl Config {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let runtime_output = env::var_os("XENEON_EDGE_OUTPUT");
        Self::load_with_runtime_output(path, runtime_output.as_deref())
    }

    fn load_with_runtime_output(
        path: Option<&Path>,
        runtime_output: Option<&OsStr>,
    ) -> Result<Self> {
        let path = path.map(PathBuf::from).or_else(default_config_path);
        let mut config = if let Some(path) = path.as_ref().filter(|path| path.exists()) {
            let contents = fs::read_to_string(path)
                .with_context(|| format!("reading config {}", path.display()))?;
            toml::from_str(&contents)
                .with_context(|| format!("parsing config {}", path.display()))?
        } else {
            Self::default()
        };
        if let Some(apps_path) = path
            .as_deref()
            .and_then(Path::parent)
            .map(|directory| directory.join(DESKTOP_APPS_FILE_NAME))
            .filter(|apps_path| apps_path.exists())
        {
            let contents = fs::read_to_string(&apps_path)
                .with_context(|| format!("reading {}", apps_path.display()))?;
            let file: DesktopAppsFile = toml::from_str(&contents)
                .with_context(|| format!("parsing {}", apps_path.display()))?;
            config.desktop.apps = file.apps;
        }
        if let Some(dashboards_path) = path
            .as_deref()
            .and_then(Path::parent)
            .map(|directory| directory.join(DASHBOARDS_FILE_NAME))
            .filter(|dashboards_path| dashboards_path.exists())
        {
            let contents = fs::read_to_string(&dashboards_path)
                .with_context(|| format!("reading {}", dashboards_path.display()))?;
            let file: DashboardsFile = toml::from_str(&contents)
                .with_context(|| format!("parsing {}", dashboards_path.display()))?;
            config.dashboards = file.dashboards;
            if let Some(spotify) = file.spotify {
                config.spotify = spotify;
            }
            if let Some(basecamp) = file.basecamp {
                config.basecamp = basecamp;
            }
            if let Some(mut slack) = file.slack {
                if slack.token_file.is_none() {
                    slack.token_file = dashboards_path
                        .parent()
                        .map(|directory| directory.join(SLACK_TOKEN_FILE_NAME));
                }
                config.slack = slack;
            }
            if let Some(github) = file.github {
                config.github = github;
            }
            if let Some(game) = file.game {
                config.game = game;
            }
        }
        config.apply_runtime_output(runtime_output)?;
        Ok(config)
    }

    pub fn herdr_refresh_interval(&self) -> Duration {
        Duration::from_millis(self.herdr_refresh_ms.max(250))
    }

    pub fn health_refresh_interval(&self) -> Duration {
        Duration::from_millis(self.health_refresh_ms.max(250))
    }

    pub fn voice_refresh_interval(&self) -> Duration {
        Duration::from_millis(self.voice_refresh_ms.max(250))
    }

    pub fn usage_refresh_interval(&self) -> Duration {
        Duration::from_millis(self.usage_refresh_ms.max(5_000))
    }

    pub fn micro_refresh_interval(&self) -> Duration {
        Duration::from_millis(self.micro_refresh_ms.max(1_000))
    }

    pub fn t3code_refresh_interval(&self) -> Duration {
        Duration::from_millis(self.t3code.refresh_ms.max(250))
    }

    pub fn validate(&self) -> Result<()> {
        let screen_identity = [
            ("screen.connector", self.screen.connector.as_deref()),
            ("screen.serial", self.screen.serial.as_deref()),
            ("screen.model", self.screen.model.as_deref()),
            ("screen.touch_device", self.screen.touch_device.as_deref()),
        ];
        for (name, value) in screen_identity {
            if value.is_some_and(|value| value.trim().is_empty()) {
                anyhow::bail!("{name} must not be empty");
            }
        }
        let configured_identity_fields = screen_identity
            .iter()
            .filter(|(_, value)| value.is_some())
            .count();
        if configured_identity_fields != 0 && configured_identity_fields != screen_identity.len() {
            anyhow::bail!(
                "screen identity must configure connector, serial, model, and touch_device together"
            );
        }
        if self
            .desktop
            .edge_output
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            anyhow::bail!("desktop.edge_output must not be empty");
        }
        if self.desktop.edge_output.is_some() && configured_identity_fields == 0 {
            anyhow::bail!("desktop.edge_output requires a complete screen identity");
        }
        if configured_identity_fields == screen_identity.len()
            && self.desktop.edge_output.as_deref() != self.screen.connector.as_deref()
        {
            anyhow::bail!("desktop.edge_output must match screen.connector");
        }
        for (name, path) in [
            ("herdr.core_socket", &self.herdr.core_socket),
            ("herdr.app_socket", &self.herdr.app_socket),
        ] {
            if path.as_ref().is_some_and(|path| !path.is_absolute()) {
                anyhow::bail!("{name} must be an absolute path");
            }
        }
        if self.voxtype_bin.as_os_str().is_empty() {
            anyhow::bail!("voxtype_bin must not be empty");
        }
        if self.desktop.herdr_class.trim().is_empty() {
            anyhow::bail!("desktop.herdr_class must not be empty");
        }
        if self.dashboards.is_empty() || self.dashboards.len() > MAX_DASHBOARDS {
            anyhow::bail!("dashboards must list 1-{MAX_DASHBOARDS} dashboards");
        }
        for (index, dashboard) in self.dashboards.iter().enumerate() {
            if self.dashboards[..index].contains(dashboard) {
                anyhow::bail!("dashboards lists {dashboard:?} more than once");
            }
        }
        if self.basecamp.bin.as_os_str().is_empty() {
            anyhow::bail!("basecamp.bin must not be empty");
        }
        if let Some(profile) = &self.basecamp.profile {
            let valid = !profile.is_empty()
                && profile.len() <= 64
                && !profile.starts_with('-')
                && profile
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte));
            if !valid {
                anyhow::bail!(
                    "basecamp.profile must be 1-64 letters, digits, '_', '.', or '-' and not start with '-'"
                );
            }
        }
        if self.basecamp.card_tables.len() > MAX_CARD_TABLES {
            anyhow::bail!("basecamp.card_tables allows at most {MAX_CARD_TABLES} tables");
        }
        for table in &self.basecamp.card_tables {
            let label = table.label.trim();
            if label.is_empty() || label.chars().count() > 24 {
                anyhow::bail!("basecamp.card_tables labels must be 1-24 characters");
            }
        }
        if self.dashboards.contains(&DashboardKind::Github)
            && !is_github_name(&self.github.organization)
        {
            anyhow::bail!("the github dashboard needs [github] organization = \"<org>\"");
        }
        if self.github.bin.as_os_str().is_empty() {
            anyhow::bail!("github.bin must not be empty");
        }
        if self
            .github
            .user
            .as_deref()
            .is_some_and(|user| !is_github_name(user))
        {
            anyhow::bail!("github.user must be a GitHub login");
        }
        if !is_game_url(&self.game.url) {
            anyhow::bail!("game.url must be an http:// or https:// URL");
        }
        if self.game.qml_bin.as_os_str().is_empty() {
            anyhow::bail!("game.qml_bin must not be empty");
        }
        if self.slack.channels.len() > MAX_SLACK_CHANNELS {
            anyhow::bail!("slack.channels allows at most {MAX_SLACK_CHANNELS} channels");
        }
        for (index, channel) in self.slack.channels.iter().enumerate() {
            let valid = !channel.is_empty()
                && channel.chars().count() <= 80
                && channel.chars().all(|character| {
                    character.is_lowercase()
                        || character.is_ascii_digit()
                        || "-_.".contains(character)
                        || (character.is_alphabetic() && !character.is_uppercase())
                });
            if !valid {
                anyhow::bail!("slack.channels must be channel names like \"dev\", without '#'");
            }
            if self.slack.channels[..index].contains(channel) {
                anyhow::bail!("slack.channels lists \"{channel}\" more than once");
            }
        }
        if self.desktop.apps.len() > MAX_DESKTOP_APPS {
            anyhow::bail!("desktop.apps allows at most {MAX_DESKTOP_APPS} shortcuts");
        }
        for (index, app) in self.desktop.apps.iter().enumerate() {
            app.validate()?;
            if self.desktop.apps[..index]
                .iter()
                .any(|earlier| earlier.id == app.id)
            {
                anyhow::bail!("desktop.apps id '{}' is used more than once", app.id);
            }
        }
        if self.herdr_refresh_ms < 250
            || self.health_refresh_ms < 250
            || self.voice_refresh_ms < 250
            || self.usage_refresh_ms < 5_000
            || self.micro_refresh_ms < 1_000
            || self.t3code.refresh_ms < 250
        {
            anyhow::bail!("refresh intervals are below their safe minimum");
        }
        if self
            .t3code
            .home
            .as_deref()
            .is_some_and(|home| home.as_os_str().is_empty())
        {
            anyhow::bail!("t3code.home must not be empty");
        }
        Ok(())
    }

    fn apply_runtime_output(&mut self, output: Option<&OsStr>) -> Result<()> {
        let Some(output) = output else {
            return Ok(());
        };
        let output = output
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("XENEON_EDGE_OUTPUT must be valid UTF-8"))?;
        if output.is_empty()
            || !output
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_.:-".contains(character))
        {
            anyhow::bail!("XENEON_EDGE_OUTPUT is not a safe connector name");
        }
        if self.screen.serial.is_none()
            || self.screen.model.is_none()
            || self.screen.touch_device.is_none()
        {
            anyhow::bail!("XENEON_EDGE_OUTPUT requires a commissioned screen identity");
        }
        self.screen.connector = Some(output.into());
        self.desktop.edge_output = Some(output.into());
        Ok(())
    }
}

fn config_home() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

fn default_config_path() -> Option<PathBuf> {
    config_home().map(|root| root.join("xeneon-edge-agents/config.toml"))
}

/// The game dashboard's window, installed with the portal's Quickshell files.
pub fn game_window_path() -> Option<PathBuf> {
    config_home().map(|root| root.join("quickshell/xeneon-edge-agents/game/GameWindow.qml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_explicit_config_uses_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let config = Config::load(Some(&temp.path().join("missing.toml"))).unwrap();
        assert_eq!(config.herdr_refresh_ms, 5_000);
    }

    #[test]
    fn absent_config_rejects_a_runtime_output_without_commissioned_identity() {
        let temp = tempfile::tempdir().unwrap();
        let error = Config::load_with_runtime_output(
            Some(&temp.path().join("missing.toml")),
            Some(OsStr::new("DP-2")),
        )
        .unwrap_err();

        assert!(error.to_string().contains("commissioned screen identity"));
    }

    #[test]
    fn herdr_section_names_absolute_sockets_and_ignores_the_old_binary() {
        let parsed: Config = toml::from_str(
            "herdr_bin = \"herdr\"\n[herdr]\ncore_socket = \"/run/user/1/herdr-core/work.sock\"\napp_socket = \"/home/u/.config/herdr-app/herdr-app.sock\"\n",
        )
        .unwrap();
        parsed.validate().unwrap();
        assert_eq!(
            parsed.herdr.core_socket().unwrap(),
            Path::new("/run/user/1/herdr-core/work.sock")
        );
        assert_eq!(
            parsed.herdr.app_socket().unwrap(),
            Path::new("/home/u/.config/herdr-app/herdr-app.sock")
        );
        assert_eq!(parsed.desktop.herdr_class, "herdr-app");

        let relative: Config = toml::from_str("[herdr]\ncore_socket = \"core.sock\"\n").unwrap();
        assert!(relative.validate().is_err());
        assert!(toml::from_str::<Config>("[herdr]\nbinary = \"hc\"\n").is_err());
        assert!(is_session_name("default"));
        for unsafe_name in ["", "../x", ".hidden", "a/b"] {
            assert!(!is_session_name(unsafe_name), "{unsafe_name}");
        }
    }

    #[test]
    fn backend_defaults_to_herdr_and_parses_t3code_section() {
        let config = Config::default();
        assert_eq!(config.agent_backend, AgentBackend::Herdr);
        assert_eq!(config.t3code.refresh_ms, 1_000);
        assert!(config.t3code.home.is_none());

        let parsed: Config = toml::from_str(
            "agent_backend = \"t3code\"\n[t3code]\nhome = \"/tmp/t3\"\nrefresh_ms = 500\n",
        )
        .unwrap();
        assert_eq!(parsed.agent_backend, AgentBackend::T3code);
        assert_eq!(parsed.t3code.home.as_deref(), Some(Path::new("/tmp/t3")));
        assert_eq!(parsed.t3code.refresh_ms, 500);
        parsed.validate().unwrap();

        assert!(toml::from_str::<Config>("agent_backend = \"tmux\"\n").is_err());
        let too_fast = Config {
            t3code: T3codeConfig {
                refresh_ms: 10,
                ..T3codeConfig::default()
            },
            ..Config::default()
        };
        assert!(too_fast.validate().is_err());
    }

    fn app(id: &str) -> String {
        format!(
            "[[desktop.apps]]\nid = \"{id}\"\nlabel = \"{id}\"\ndesktop_id = \"{id}.desktop\"\nclass = \"{id}\"\n"
        )
    }

    #[test]
    fn desktop_apps_parse_zero_to_four_shortcuts() {
        assert_eq!(Config::default().desktop.apps.len(), 2);

        let none: Config = toml::from_str("[desktop]\napps = []\n").unwrap();
        none.validate().unwrap();
        assert!(none.desktop.apps.is_empty());

        let four: Config = toml::from_str(&["a", "b", "c", "d"].map(app).concat()).unwrap();
        four.validate().unwrap();
        assert_eq!(four.desktop.apps[3].id, "d");
        assert_eq!(four.desktop.apps[0].accent, "accent");
        assert_eq!(four.desktop.apps[0].title, None);

        let five: Config = toml::from_str(&["a", "b", "c", "d", "e"].map(app).concat()).unwrap();
        assert!(five.validate().is_err());
    }

    #[test]
    fn desktop_apps_file_beside_config_replaces_the_default_list() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("config.toml");
        fs::write(&config_path, "[desktop]\nherdr_class = \"herdr\"\n").unwrap();
        let loaded = Config::load_with_runtime_output(Some(&config_path), None).unwrap();
        assert_eq!(loaded.desktop.apps, Config::default().desktop.apps);

        let apps_path = temp.path().join(DESKTOP_APPS_FILE_NAME);
        fs::write(&apps_path, app("slack").replace("desktop.apps", "apps")).unwrap();
        let loaded = Config::load_with_runtime_output(Some(&config_path), None).unwrap();
        assert_eq!(loaded.desktop.apps.len(), 1);
        assert_eq!(loaded.desktop.apps[0].id, "slack");
        loaded.validate().unwrap();

        fs::write(&apps_path, "apps = []\n").unwrap();
        let loaded = Config::load_with_runtime_output(Some(&config_path), None).unwrap();
        assert!(loaded.desktop.apps.is_empty());

        fs::write(&apps_path, "[[apps]]\nid = \"x\"\nexec = \"sh\"\n").unwrap();
        assert!(Config::load_with_runtime_output(Some(&config_path), None).is_err());
    }

    #[test]
    fn shipped_desktop_apps_example_matches_the_defaults() {
        let file: DesktopAppsFile = toml::from_str(include_str!(
            "../../../config/xeneon-edge-agents/desktop-apps.toml.example"
        ))
        .unwrap();
        let config = Config {
            desktop: DesktopConfig {
                apps: file.apps,
                ..DesktopConfig::default()
            },
            ..Config::default()
        };
        config.validate().unwrap();
        let ids: Vec<_> = config.desktop.apps.iter().map(|app| &app.id).collect();
        assert_eq!(ids, ["spotify", "discord"]);
    }

    #[test]
    fn dashboards_file_selects_enabled_dashboards_in_order() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("config.toml");
        fs::write(&config_path, "").unwrap();
        let loaded = Config::load_with_runtime_output(Some(&config_path), None).unwrap();
        assert_eq!(loaded.dashboards, [DashboardKind::Agents]);

        let dashboards_path = temp.path().join(DASHBOARDS_FILE_NAME);
        fs::write(&dashboards_path, "dashboards = [\"spotify\", \"agents\"]\n").unwrap();
        let loaded = Config::load_with_runtime_output(Some(&config_path), None).unwrap();
        assert_eq!(
            loaded.dashboards,
            [DashboardKind::Spotify, DashboardKind::Agents]
        );
        loaded.validate().unwrap();

        for invalid in [
            "dashboards = []\n",
            "dashboards = [\"agents\", \"agents\"]\n",
        ] {
            fs::write(&dashboards_path, invalid).unwrap();
            let loaded = Config::load_with_runtime_output(Some(&config_path), None).unwrap();
            assert!(loaded.validate().is_err(), "accepted: {invalid}");
        }
        fs::write(
            &dashboards_path,
            concat!(
                "dashboards = [\"basecamp\"]\n",
                "[[basecamp.card_tables]]\n",
                "label = \"Sprint\"\nproject = 1\ncard_table = 2\n",
            ),
        )
        .unwrap();
        let loaded = Config::load_with_runtime_output(Some(&config_path), None).unwrap();
        assert_eq!(loaded.basecamp.card_tables[0].card_table, 2);
        assert_eq!(loaded.basecamp.bin, PathBuf::from("basecamp"));
        assert_eq!(loaded.basecamp.profile, None);
        loaded.validate().unwrap();
        for (profile, valid) in [
            ("jonh", true),
            ("jonh_ki", true),
            ("--help", false),
            ("a b", false),
        ] {
            let mut config = loaded.clone();
            config.basecamp.profile = Some(profile.into());
            assert_eq!(config.validate().is_ok(), valid, "{profile}");
        }

        fs::write(&dashboards_path, "dashboards = [\"myspace\"]\n").unwrap();
        assert!(Config::load_with_runtime_output(Some(&config_path), None).is_err());
    }

    #[test]
    fn game_dashboard_defaults_to_the_live_site_and_takes_only_web_urls() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("config.toml");
        fs::write(&config_path, "").unwrap();
        let dashboards_path = temp.path().join(DASHBOARDS_FILE_NAME);
        fs::write(&dashboards_path, "dashboards = [\"agents\", \"game\"]\n").unwrap();
        let loaded = Config::load_with_runtime_output(Some(&config_path), None).unwrap();
        assert_eq!(
            loaded.dashboards,
            [DashboardKind::Agents, DashboardKind::Game]
        );
        assert_eq!(loaded.game.url, DEFAULT_GAME_URL);
        loaded.validate().unwrap();

        fs::write(
            &dashboards_path,
            "dashboards = [\"game\"]\n[game]\nurl = \"http://localhost:8788/?full&fit\"\n",
        )
        .unwrap();
        let loaded = Config::load_with_runtime_output(Some(&config_path), None).unwrap();
        assert_eq!(loaded.game.url, "http://localhost:8788/?full&fit");
        assert_eq!(loaded.game.qml_bin, PathBuf::from("/usr/lib/qt6/bin/qml"));
        loaded.validate().unwrap();

        for url in [
            "",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://",
            "https:///path",
            "https://host/a b",
            "https://host/\n",
        ] {
            let mut config = loaded.clone();
            config.game.url = url.into();
            assert!(config.validate().is_err(), "accepted: {url:?}");
        }
    }

    #[test]
    fn spotify_lyrics_are_on_unless_switched_off() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("config.toml");
        fs::write(&config_path, "").unwrap();
        let dashboards_path = temp.path().join(DASHBOARDS_FILE_NAME);
        fs::write(&dashboards_path, "dashboards = [\"spotify\"]\n").unwrap();
        let loaded = Config::load_with_runtime_output(Some(&config_path), None).unwrap();
        assert!(loaded.spotify.lyrics);

        fs::write(
            &dashboards_path,
            "dashboards = [\"spotify\"]\n[spotify]\nlyrics = false\n",
        )
        .unwrap();
        let loaded = Config::load_with_runtime_output(Some(&config_path), None).unwrap();
        assert!(!loaded.spotify.lyrics);
    }

    #[test]
    fn shipped_dashboards_example_parses() {
        let file: DashboardsFile = toml::from_str(include_str!(
            "../../../config/xeneon-edge-agents/dashboards.toml.example"
        ))
        .unwrap();
        assert_eq!(
            file.dashboards,
            [
                DashboardKind::Agents,
                DashboardKind::Basecamp,
                DashboardKind::Spotify
            ]
        );
        assert_eq!(file.basecamp.unwrap().card_tables[0].label, "Sprint");
    }

    #[test]
    fn desktop_apps_reject_unsafe_or_ambiguous_entries() {
        let duplicate: Config = toml::from_str(&["a", "a"].map(app).concat()).unwrap();
        assert!(duplicate.validate().is_err());

        for broken in [
            app("Bad Id"),
            app("ok").replace("ok.desktop", "../ok.desktop"),
            app("ok").replace("ok.desktop", "--help.desktop"),
            app("ok").replace("ok.desktop", "ok"),
            app("ok").replace("label = \"ok\"", "label = \"   \""),
            app("ok").replace("class = \"ok\"", "class = \"\""),
            app("ok") + "accent = \"chartreuse\"\n",
        ] {
            let parsed: Config = toml::from_str(&broken).unwrap();
            assert!(parsed.validate().is_err(), "accepted: {broken}");
        }
        assert!(toml::from_str::<Config>(&(app("ok") + "exec = \"rm -rf\"\n")).is_err());
    }

    #[test]
    fn config_rejects_unsafe_poll_rate() {
        let config = Config {
            health_refresh_ms: 10,
            ..Config::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn runtime_output_replaces_the_commissioned_connector_atomically() {
        let mut config = Config {
            screen: ScreenConfig {
                connector: Some("DP-1".into()),
                serial: Some("CX123456".into()),
                model: Some("XENEON EDGE".into()),
                touch_device: Some("wch.cn-touchscreen-1".into()),
            },
            desktop: DesktopConfig {
                edge_output: Some("DP-1".into()),
                ..DesktopConfig::default()
            },
            ..Config::default()
        };

        config
            .apply_runtime_output(Some(OsStr::new("DP-2")))
            .unwrap();

        assert_eq!(config.screen.connector.as_deref(), Some("DP-2"));
        assert_eq!(config.desktop.edge_output.as_deref(), Some("DP-2"));
        config.validate().unwrap();
    }

    #[test]
    fn runtime_output_rejects_shell_metacharacters() {
        let mut config = Config {
            screen: ScreenConfig {
                serial: Some("CX123456".into()),
                model: Some("XENEON EDGE".into()),
                touch_device: Some("wch.cn-touchscreen-1".into()),
                ..ScreenConfig::default()
            },
            ..Config::default()
        };

        let error = config
            .apply_runtime_output(Some(OsStr::new("DP-2;reboot")))
            .unwrap_err();

        assert!(error.to_string().contains("safe connector name"));
    }

    #[test]
    fn partial_screen_identity_is_rejected() {
        let mut config = Config::default();
        config.screen.connector = Some("DP-1".into());
        assert!(config.validate().is_err());
    }

    #[test]
    fn complete_screen_identity_requires_matching_desktop_output() {
        let mut config = Config {
            screen: ScreenConfig {
                connector: Some("DP-1".into()),
                serial: Some("CX123".into()),
                model: Some("XENEON EDGE".into()),
                touch_device: Some("corsair-xeneon-edge-touchscreen".into()),
            },
            ..Config::default()
        };
        config.desktop.edge_output = Some("DP-2".into());
        assert!(config.validate().is_err());

        config.desktop.edge_output = Some("DP-1".into());
        assert!(config.validate().is_ok());
    }
}
