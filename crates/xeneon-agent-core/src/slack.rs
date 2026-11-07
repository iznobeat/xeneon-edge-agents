use std::{
    collections::HashMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::Value;
use tokio::{io::AsyncWriteExt, process::Command, sync::Mutex, time::timeout};

use crate::{
    config::SlackConfig,
    model::{
        SlackChannel, SlackImage, SlackMessage, SlackNotification, SlackReaction, SlackSnapshot,
    },
};

const API_BASE: &str = "https://slack.com/api/";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const CHANNEL_DIRECTORY_TTL: Duration = Duration::from_secs(600);
const HISTORY_LIMIT: usize = 20;
// DMs are checked round-robin so a long DM list stays inside rate limits:
// conversations.info is Tier 3 (about 50 calls a minute) and also serves
// the channels, sampled every 30 seconds.
const DM_CHECKS_PER_SAMPLE: usize = 10;
const MENTION_LIMIT: usize = 10;
const MAX_NOTIFICATIONS: usize = 10;
const MAX_TEXT_CHARS: usize = 500;
const MAX_NAME_CHARS: usize = 48;
// Message subtypes that are conversation, not channel housekeeping.
const SHOWN_SUBTYPES: [&str; 4] = ["", "bot_message", "thread_broadcast", "file_share"];
const MAX_IMAGES_PER_MESSAGE: usize = 2;
const MAX_REACTIONS_PER_MESSAGE: usize = 8;
// Older mentions have nothing left to say on a glanceable strip.
const MENTION_MAX_AGE_SECONDS: u64 = 3 * 24 * 60 * 60;
// A 480 px thumbnail is far smaller; the cache lives on tmpfs.
const MAX_IMAGE_BYTES: &str = "1500000";
// Thumbnails come only from Slack's own file host.
const IMAGE_URL_PREFIX: &str = "https://files.slack.com/";
// A thumbnail that failed (e.g. files:read missing) is retried after this.
const IMAGE_RETRY_AFTER: Duration = Duration::from_secs(600);
pub const SLACK_IMAGE_DIR: &str = "xeneon-edge-agents/slack-images";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenFile {
    token: String,
}

#[derive(Debug, Clone)]
struct Identity {
    user_id: String,
    /// Workspace id from auth.test, used in `slack://` deep links.
    team_id: String,
}

#[derive(Debug, Default)]
struct SlackCache {
    /// Hash of the token the rest of the cache belongs to.
    token_hash: Option<u64>,
    identity: Option<Identity>,
    /// Last good copy of each channel, kept through a failed refresh.
    channels: HashMap<String, SlackChannel>,
    channel_ids: HashMap<String, String>,
    channel_ids_at: Option<Instant>,
    user_names: HashMap<String, String>,
    dm_cursor: usize,
    dm_unread: HashMap<String, SlackNotification>,
    image_failures: HashMap<String, Instant>,
}

/// One Slack sample: the published snapshot plus the deep links the daemon
/// keeps private and opens by id.
#[derive(Debug, Default)]
pub struct SlackSample {
    pub snapshot: SlackSnapshot,
    pub urls: HashMap<String, String>,
}

/// Reads watched channels, DMs, and @-mentions with the user's own Slack
/// user token through `curl`. Only fixed read-only Web API methods are used,
/// and the token reaches curl on stdin, never on its command line.
#[derive(Debug, Clone)]
pub struct SlackClient {
    config: SlackConfig,
    curl_bin: PathBuf,
    /// Where thumbnails are cached; images are skipped when unset.
    image_dir: Option<PathBuf>,
    cache: Arc<Mutex<SlackCache>>,
}

impl SlackClient {
    pub fn new(config: SlackConfig) -> Self {
        Self::with_curl(config, "curl")
    }

    pub fn with_curl(config: SlackConfig, curl_bin: impl Into<PathBuf>) -> Self {
        Self {
            config,
            curl_bin: curl_bin.into(),
            image_dir: std::env::var_os("XDG_RUNTIME_DIR")
                .map(|runtime| PathBuf::from(runtime).join(SLACK_IMAGE_DIR)),
            cache: Arc::new(Mutex::new(SlackCache::default())),
        }
    }

    pub fn with_image_dir(mut self, image_dir: Option<PathBuf>) -> Self {
        self.image_dir = image_dir;
        self
    }

    /// A missing token or a failing API is an unavailable snapshot; one
    /// unreadable channel only marks that channel.
    pub async fn sample(&self) -> SlackSample {
        let mut sample = SlackSample::default();
        let token = match self.config.token_file.as_deref().map(read_token) {
            Some(Ok(token)) => token,
            Some(Err(error)) => {
                tracing::debug!(%error, "Slack token unavailable");
                return sample;
            }
            None => return sample,
        };
        let api = Api {
            curl_bin: &self.curl_bin,
            token: &token,
        };
        let mut cache = self.cache.lock().await;
        let token_hash = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            token.hash(&mut hasher);
            hasher.finish()
        };
        if cache.token_hash != Some(token_hash) {
            // Another token may be another user or workspace entirely.
            *cache = SlackCache {
                token_hash: Some(token_hash),
                ..SlackCache::default()
            };
        }
        let identity = match self.identity(&api, &mut cache).await {
            Ok(identity) => identity,
            Err(error) => {
                tracing::debug!(%error, "Slack auth.test failed");
                return sample;
            }
        };
        sample.snapshot.available = true;

        if let Err(error) = self.refresh_channel_directory(&api, &mut cache).await {
            tracing::debug!(%error, "Slack channel directory unavailable");
        }
        for name in &self.config.channels {
            let fresh = match cache.channel_ids.get(name).cloned() {
                Some(id) => self
                    .channel(&api, &mut cache, &identity, name, &id, &mut sample.urls)
                    .await
                    .map_err(|error| {
                        tracing::debug!(%error, channel = %name, "Slack channel unavailable");
                    })
                    .ok(),
                None => None,
            };
            let channel = match fresh {
                Some(channel) => {
                    cache.channels.insert(name.clone(), channel.clone());
                    channel
                }
                // A failed refresh keeps the last good copy, so its messages
                // and cached thumbnails do not vanish for one tick.
                None => match cache.channels.get(name) {
                    Some(previous) => {
                        for message in &previous.messages {
                            if let Some((channel_id, ts)) = message.id.split_once('-') {
                                sample.urls.insert(
                                    message.id.clone(),
                                    deep_link(&identity, channel_id, ts),
                                );
                            }
                        }
                        previous.clone()
                    }
                    None => unavailable_channel(name),
                },
            };
            sample.snapshot.channels.push(channel);
        }

        if let Err(error) = self.refresh_dms(&api, &mut cache).await {
            tracing::debug!(%error, "Slack DM check failed");
        }
        let mut notifications: Vec<_> = cache.dm_unread.values().cloned().collect();
        notifications.sort_by_key(|notification| std::cmp::Reverse(notification.at));
        match self.mentions(&api, &mut cache, &identity).await {
            Ok(mentions) => notifications.extend(mentions),
            Err(error) => tracing::debug!(%error, "Slack mention search failed"),
        }
        // A DM's latest message can also be a mention of you; show it once.
        let mut seen = std::collections::HashSet::new();
        notifications.retain(|notification| seen.insert(notification.id.clone()));
        notifications.truncate(MAX_NOTIFICATIONS);
        self.prune_images(&sample.snapshot.channels);
        for notification in &notifications {
            if let Some((channel, ts)) = notification.id.split_once('-') {
                sample
                    .urls
                    .insert(notification.id.clone(), deep_link(&identity, channel, ts));
            }
        }
        sample.snapshot.notifications = notifications;
        sample
    }

    async fn identity(&self, api: &Api<'_>, cache: &mut SlackCache) -> Result<Identity> {
        if let Some(identity) = &cache.identity {
            return Ok(identity.clone());
        }
        let response = api.call("auth.test", &[]).await?;
        let user_id = response["user_id"]
            .as_str()
            .filter(|id| is_slack_id(id))
            .ok_or_else(|| anyhow!("auth.test returned no user"))?;
        let team_id = response["team_id"]
            .as_str()
            .filter(|id| is_slack_id(id))
            .ok_or_else(|| anyhow!("auth.test returned no workspace"))?;
        let identity = Identity {
            user_id: user_id.to_owned(),
            team_id: team_id.to_owned(),
        };
        cache.identity = Some(identity.clone());
        Ok(identity)
    }

    async fn refresh_channel_directory(&self, api: &Api<'_>, cache: &mut SlackCache) -> Result<()> {
        let fresh = cache
            .channel_ids_at
            .is_some_and(|at| at.elapsed() < CHANNEL_DIRECTORY_TTL);
        let complete = self
            .config
            .channels
            .iter()
            .all(|name| cache.channel_ids.contains_key(name));
        if fresh && complete {
            return Ok(());
        }
        let mut ids = HashMap::new();
        let mut cursor = String::new();
        // A few pages covers every channel a person is in.
        for _ in 0..5 {
            let mut params = vec![
                ("types", "public_channel,private_channel"),
                ("exclude_archived", "true"),
                ("limit", "999"),
            ];
            if !cursor.is_empty() {
                params.push(("cursor", cursor.as_str()));
            }
            let response = api.call("users.conversations", &params).await?;
            for channel in response["channels"].as_array().into_iter().flatten() {
                if let (Some(id), Some(name)) = (channel["id"].as_str(), channel["name"].as_str())
                    && is_slack_id(id)
                {
                    ids.insert(name.to_owned(), id.to_owned());
                }
            }
            cursor = response["response_metadata"]["next_cursor"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            if cursor.is_empty() {
                break;
            }
        }
        cache.channel_ids = ids;
        cache.channel_ids_at = Some(Instant::now());
        Ok(())
    }

    async fn channel(
        &self,
        api: &Api<'_>,
        cache: &mut SlackCache,
        identity: &Identity,
        name: &str,
        id: &str,
        urls: &mut HashMap<String, String>,
    ) -> Result<SlackChannel> {
        let info = api.call("conversations.info", &[("channel", id)]).await?;
        let last_read = info["channel"]["last_read"]
            .as_str()
            .and_then(parse_ts)
            .unwrap_or(f64::MAX);
        let limit = HISTORY_LIMIT.to_string();
        let history = api
            .call(
                "conversations.history",
                &[("channel", id), ("limit", &limit)],
            )
            .await?;
        let raw: Vec<Value> = history["messages"].as_array().cloned().unwrap_or_default();
        let mut messages = Vec::new();
        let mut unread = 0;
        for message in &raw {
            let subtype = message["subtype"].as_str().unwrap_or_default();
            let Some(ts) = message["ts"].as_str().filter(|ts| parse_ts(ts).is_some()) else {
                continue;
            };
            if !SHOWN_SUBTYPES.contains(&subtype) {
                continue;
            }
            let at = parse_ts(ts).unwrap_or_default();
            let is_unread =
                at > last_read && message["user"].as_str() != Some(identity.user_id.as_str());
            if is_unread {
                unread += 1;
            }
            let author = self.author(api, cache, message).await;
            let text = self
                .render(api, cache, message["text"].as_str().unwrap_or(""))
                .await;
            let images = self.images(api, cache, message).await;
            let reactions = reactions(message);
            // A file-only post would otherwise render as an empty message.
            let text = if text.trim().is_empty() && images.is_empty() {
                file_summary(message)
            } else {
                text
            };
            let item_id = format!("{id}-{ts}");
            urls.insert(item_id.clone(), deep_link(identity, id, ts));
            messages.push(SlackMessage {
                id: item_id,
                author,
                text,
                images,
                reactions,
                at: at as u64,
                replies: message["reply_count"].as_u64().unwrap_or(0).min(9_999) as u32,
                unread: is_unread,
            });
        }
        // History is newest first; the feed reads top to bottom, oldest first.
        messages.reverse();
        Ok(SlackChannel {
            name: bounded(name, MAX_NAME_CHARS),
            available: true,
            unread,
            unread_capped: !raw.is_empty() && unread == raw.len() as u32,
            messages,
        })
    }

    async fn refresh_dms(&self, api: &Api<'_>, cache: &mut SlackCache) -> Result<()> {
        let response = api
            .call(
                "users.conversations",
                &[
                    ("types", "im"),
                    ("exclude_archived", "true"),
                    ("limit", "200"),
                ],
            )
            .await?;
        let dms: Vec<(String, String)> = response["channels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|dm| dm["is_user_deleted"].as_bool() != Some(true))
            .filter_map(|dm| {
                Some((
                    dm["id"].as_str().filter(|id| is_slack_id(id))?.to_owned(),
                    dm["user"].as_str().filter(|id| is_slack_id(id))?.to_owned(),
                ))
            })
            .collect();
        cache
            .dm_unread
            .retain(|id, _| dms.iter().any(|(dm, _)| id.starts_with(dm.as_str())));
        if dms.is_empty() {
            return Ok(());
        }
        let start = cache.dm_cursor % dms.len();
        for offset in 0..DM_CHECKS_PER_SAMPLE.min(dms.len()) {
            let (dm, user) = &dms[(start + offset) % dms.len()];
            let info = match api.call("conversations.info", &[("channel", dm)]).await {
                Ok(info) => info,
                Err(error) => {
                    // One failing DM (or a rate limit) must not stall the rest.
                    tracing::debug!(%error, dm = %dm, "Slack DM check failed");
                    continue;
                }
            };
            let channel = &info["channel"];
            let unread = channel["unread_count_display"].as_u64().unwrap_or(0);
            cache
                .dm_unread
                .retain(|id, _| !id.starts_with(&format!("{dm}-")));
            let latest = &channel["latest"];
            let Some(ts) = latest["ts"].as_str().filter(|ts| parse_ts(ts).is_some()) else {
                continue;
            };
            if unread == 0 {
                continue;
            }
            let name = self.user_name(api, cache, user).await;
            let text = self
                .render(api, cache, latest["text"].as_str().unwrap_or(""))
                .await;
            let id = format!("{dm}-{ts}");
            cache.dm_unread.insert(
                id.clone(),
                SlackNotification {
                    id,
                    kind: "dm".into(),
                    source: name.clone(),
                    author: name,
                    text,
                    at: parse_ts(ts).unwrap_or_default() as u64,
                    unread: unread.min(9_999) as u32,
                },
            );
        }
        cache.dm_cursor = (start + DM_CHECKS_PER_SAMPLE) % dms.len();
        Ok(())
    }

    async fn mentions(
        &self,
        api: &Api<'_>,
        cache: &mut SlackCache,
        identity: &Identity,
    ) -> Result<Vec<SlackNotification>> {
        let query = format!("<@{}>", identity.user_id);
        let count = MENTION_LIMIT.to_string();
        let response = api
            .call(
                "search.messages",
                &[("query", &query), ("sort", "timestamp"), ("count", &count)],
            )
            .await?;
        let mut mentions = Vec::new();
        let oldest = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|now| now.as_secs().saturating_sub(MENTION_MAX_AGE_SECONDS))
            .unwrap_or(0);
        for item in response["messages"]["matches"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let (Some(channel), Some(ts)) = (
                item["channel"]["id"].as_str().filter(|id| is_slack_id(id)),
                item["ts"].as_str().filter(|ts| parse_ts(ts).is_some()),
            ) else {
                continue;
            };
            let source = match item["channel"]["name"].as_str() {
                Some(name) if !name.starts_with('U') && !name.starts_with('D') => {
                    format!("#{}", bounded(name, MAX_NAME_CHARS))
                }
                _ => "DM".into(),
            };
            let author = self.author(api, cache, item).await;
            let text = self
                .render(api, cache, item["text"].as_str().unwrap_or(""))
                .await;
            mentions.push(SlackNotification {
                id: format!("{channel}-{ts}"),
                kind: "mention".into(),
                source,
                author,
                text,
                at: parse_ts(ts).unwrap_or_default() as u64,
                unread: 0,
            });
            if mentions.last().is_some_and(|mention| mention.at < oldest) {
                mentions.pop();
            }
        }
        Ok(mentions)
    }

    /// Caches up to two image thumbnails of a message and describes them.
    async fn images(
        &self,
        api: &Api<'_>,
        cache: &mut SlackCache,
        message: &Value,
    ) -> Vec<SlackImage> {
        let Some(directory) = &self.image_dir else {
            return Vec::new();
        };
        let mut images = Vec::new();
        for file in message["files"].as_array().into_iter().flatten() {
            if images.len() == MAX_IMAGES_PER_MESSAGE {
                break;
            }
            let Some(id) = file["id"].as_str().filter(|id| is_slack_id(id)) else {
                continue;
            };
            if !file["mimetype"]
                .as_str()
                .unwrap_or_default()
                .starts_with("image/")
            {
                continue;
            }
            let Some((url, width, height)) = ["thumb_480", "thumb_360"].iter().find_map(|key| {
                let url = file[*key].as_str()?;
                let width = file[format!("{key}_w")].as_u64()?;
                let height = file[format!("{key}_h")].as_u64()?;
                Some((url, width, height))
            }) else {
                continue;
            };
            let Some(extension) = image_extension(url) else {
                continue;
            };
            if !url.starts_with(IMAGE_URL_PREFIX) || width == 0 || height == 0 {
                continue;
            }
            let name = format!("{id}.{extension}");
            let path = directory.join(&name);
            if !path.exists() {
                if cache
                    .image_failures
                    .get(id)
                    .is_some_and(|failed| failed.elapsed() < IMAGE_RETRY_AFTER)
                {
                    continue;
                }
                if let Err(error) = api.download_image(url, directory, &name).await {
                    tracing::debug!(%error, file = id, "Slack thumbnail unavailable");
                    cache.image_failures.insert(id.to_owned(), Instant::now());
                    continue;
                }
                cache.image_failures.remove(id);
            }
            images.push(SlackImage {
                file: name,
                width: width.min(4_096) as u32,
                height: height.min(4_096) as u32,
            });
        }
        images
    }

    /// Removes cached thumbnails no published message refers to anymore.
    fn prune_images(&self, channels: &[SlackChannel]) {
        let Some(directory) = &self.image_dir else {
            return;
        };
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        let keep: Vec<&str> = channels
            .iter()
            .flat_map(|channel| &channel.messages)
            .flat_map(|message| &message.images)
            .map(|image| image.file.as_str())
            .collect();
        for entry in entries.flatten() {
            let name = entry.file_name();
            if !keep.iter().any(|kept| name == *kept) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    async fn author(&self, api: &Api<'_>, cache: &mut SlackCache, message: &Value) -> String {
        if let Some(user) = message["user"].as_str().filter(|id| is_slack_id(id)) {
            return self.user_name(api, cache, user).await;
        }
        let fallback = message["username"]
            .as_str()
            .or_else(|| message["bot_profile"]["name"].as_str())
            .unwrap_or("Slack");
        bounded(fallback, MAX_NAME_CHARS)
    }

    async fn user_name(&self, api: &Api<'_>, cache: &mut SlackCache, user: &str) -> String {
        if let Some(name) = cache.user_names.get(user) {
            return name.clone();
        }
        let name = match api.call("users.info", &[("user", user)]).await {
            Ok(response) => {
                let profile = &response["user"]["profile"];
                [
                    profile["display_name"].as_str(),
                    profile["real_name"].as_str(),
                    response["user"]["name"].as_str(),
                ]
                .into_iter()
                .flatten()
                .find(|name| !name.trim().is_empty())
                .map(|name| bounded(name, MAX_NAME_CHARS))
                .unwrap_or_else(|| user.to_owned())
            }
            // Leave it uncached so the next sample retries.
            Err(_) => return user.to_owned(),
        };
        cache.user_names.insert(user.to_owned(), name.clone());
        name
    }

    /// Slack mrkdwn to plain text: user and channel references become names,
    /// links their labels, and HTML entities their characters.
    async fn render(&self, api: &Api<'_>, cache: &mut SlackCache, text: &str) -> String {
        let mut rendered = String::new();
        let mut rest = text;
        while let Some(start) = rest.find('<') {
            rendered.push_str(&rest[..start]);
            let Some(end) = rest[start..].find('>') else {
                rendered.push_str(&rest[start..]);
                rest = "";
                break;
            };
            let token = &rest[start + 1..start + end];
            let (target, label) = match token.split_once('|') {
                Some((target, label)) => (target, Some(label)),
                None => (token, None),
            };
            let replacement = if let Some(user) = target.strip_prefix('@') {
                if is_slack_id(user) {
                    format!("@{}", self.user_name(api, cache, user).await)
                } else {
                    format!("@{user}")
                }
            } else if let Some(channel) = target.strip_prefix('#') {
                format!("#{}", label.unwrap_or(channel))
            } else if let Some(special) = target.strip_prefix('!') {
                match label {
                    Some(label) => label.to_owned(),
                    None => format!("@{}", special.split('^').next().unwrap_or(special)),
                }
            } else {
                label.unwrap_or(target).to_owned()
            };
            rendered.push_str(&replacement);
            rest = &rest[start + end + 1..];
        }
        rendered.push_str(rest);
        let decoded = rendered
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&");
        bounded(&emojify(&decoded), MAX_TEXT_CHARS)
    }
}

struct Api<'a> {
    curl_bin: &'a Path,
    token: &'a str,
}

impl Api<'_> {
    async fn call(&self, method: &str, params: &[(&str, &str)]) -> Result<Value> {
        let query = params
            .iter()
            .map(|(key, value)| format!("{key}={}", percent_encode(value)))
            .collect::<Vec<_>>()
            .join("&");
        // curl reads its URL and the Authorization header from a config on
        // stdin, so the token never appears in the process list.
        let url = format!("{API_BASE}{method}?{query}");
        if !is_config_safe(&url) {
            bail!("refusing an unsafe Slack request URL");
        }
        let config = format!(
            "url = \"{url}\"\nheader = \"Authorization: Bearer {}\"\n",
            self.token
        );
        let mut child = Command::new(self.curl_bin)
            .kill_on_drop(true)
            .args([
                "--silent",
                "--show-error",
                "--max-time",
                "15",
                "--config",
                "-",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("running {}", self.curl_bin.display()))?;
        let mut stdin = child.stdin.take().context("curl stdin unavailable")?;
        stdin.write_all(config.as_bytes()).await?;
        drop(stdin);
        let output = timeout(REQUEST_TIMEOUT, child.wait_with_output())
            .await
            .context("timed out waiting for Slack")??;
        if !output.status.success() {
            bail!("Slack request failed for {method}");
        }
        let response: Value =
            serde_json::from_slice(&output.stdout).context("parsing Slack response")?;
        if response["ok"].as_bool() != Some(true) {
            bail!(
                "Slack {method} error: {}",
                response["error"].as_str().unwrap_or("unknown")
            );
        }
        Ok(response)
    }
}

impl Api<'_> {
    /// Downloads one thumbnail into `directory/name`, keeping it only when
    /// Slack answered 200 with an image (a missing files:read scope yields a
    /// redirect to the sign-in page instead).
    async fn download_image(&self, url: &str, directory: &Path, name: &str) -> Result<()> {
        fs::create_dir_all(directory)
            .with_context(|| format!("creating {}", directory.display()))?;
        let partial = directory.join(format!(".{name}.part"));
        // Every value becomes one quoted line of curl's config; a quote,
        // backslash, or newline from Slack's data must not add another line.
        let output = partial.display().to_string();
        if !is_config_safe(url) || !is_config_safe(&output) {
            bail!("refusing an unsafe Slack thumbnail URL or cache path");
        }
        let config = format!(
            "url = \"{url}\"\nheader = \"Authorization: Bearer {}\"\noutput = \"{}\"\n",
            self.token, output
        );
        let mut child = Command::new(self.curl_bin)
            .kill_on_drop(true)
            .args([
                "--silent",
                "--max-time",
                "15",
                "--max-filesize",
                MAX_IMAGE_BYTES,
                "--write-out",
                "%{http_code} %{content_type}",
                "--config",
                "-",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("running {}", self.curl_bin.display()))?;
        let mut stdin = child.stdin.take().context("curl stdin unavailable")?;
        stdin.write_all(config.as_bytes()).await?;
        drop(stdin);
        let output = timeout(REQUEST_TIMEOUT, child.wait_with_output())
            .await
            .context("timed out downloading a Slack thumbnail")??;
        let status = String::from_utf8_lossy(&output.stdout);
        let is_image = output.status.success()
            && status.starts_with("200 image/")
            && fs::metadata(&partial).is_ok_and(|metadata| metadata.len() > 0);
        if !is_image {
            let _ = fs::remove_file(&partial);
            bail!("Slack thumbnail download returned {status}");
        }
        fs::rename(&partial, directory.join(name))?;
        Ok(())
    }
}

/// "📎 name" for each attached file, used when a message has no text and
/// no thumbnail could be shown.
fn file_summary(message: &Value) -> String {
    let names: Vec<String> = message["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|file| file["title"].as_str().or_else(|| file["name"].as_str()))
        .map(|name| format!("📎 {}", bounded(name, 80)))
        .collect();
    names.join("  ")
}

fn reactions(message: &Value) -> Vec<SlackReaction> {
    message["reactions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|reaction| {
            let name = reaction["name"].as_str()?;
            let count = reaction["count"].as_u64().filter(|count| *count > 0)?;
            Some(SlackReaction {
                emoji: emoji_for(name)
                    .map_or_else(|| bounded(&format!(":{name}:"), 34), str::to_owned),
                count: count.min(9_999) as u32,
            })
        })
        .take(MAX_REACTIONS_PER_MESSAGE)
        .collect()
}

/// A Slack shortcode (`thumbsup`, `wave::skin-tone-3`) as its emoji; custom
/// workspace emoji have none.
fn emoji_for(name: &str) -> Option<&'static str> {
    let base = name.split("::").next().unwrap_or(name);
    emojis::get_by_shortcode(base).map(|emoji| emoji.as_str())
}

/// Replaces `:shortcode:` (and a trailing `:skin-tone-N:`) with emoji,
/// leaving unknown codes, times like 12:30:45, and URLs untouched.
fn emojify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(':') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let name_len = after
            .find(|character: char| {
                !(character.is_ascii_alphanumeric() || "_+-".contains(character))
            })
            .unwrap_or(after.len());
        let name = &after[..name_len];
        let closed = after[name_len..].starts_with(':');
        match (closed && !name.is_empty())
            .then(|| emoji_for(name))
            .flatten()
        {
            Some(emoji) => {
                out.push_str(emoji);
                let mut tail = &after[name_len + 1..];
                if let Some(skin) = tail.strip_prefix(":skin-tone-")
                    && let Some(end) = skin.find(':')
                    && skin[..end].bytes().all(|byte| byte.is_ascii_digit())
                {
                    tail = &skin[end + 1..];
                }
                rest = tail;
            }
            None => {
                out.push(':');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Printable ASCII without quote, backslash, or space: safe inside a quoted
/// curl config value.
fn is_config_safe(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| (0x21..=0x7e).contains(&byte) && byte != b'"' && byte != b'\\')
}

fn image_extension(url: &str) -> Option<&'static str> {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    ["png", "jpg", "jpeg", "gif", "webp"]
        .into_iter()
        .find(|extension| path.ends_with(&format!(".{extension}")))
}

/// The token file must belong to the user alone and hold a user token.
fn read_token(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path).with_context(|| format!("reading {}", path.display()))?;
    if metadata.permissions().mode() & 0o077 != 0 {
        bail!(
            "{} must be readable by its owner only (chmod 600)",
            path.display()
        );
    }
    let file: TokenFile = toml::from_str(&fs::read_to_string(path)?)
        .with_context(|| format!("parsing {}", path.display()))?;
    let valid = file.token.starts_with("xoxp-")
        && file.token.len() <= 256
        && file
            .token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    if !valid {
        bail!("{} must hold a Slack user token (xoxp-...)", path.display());
    }
    Ok(file.token)
}

fn unavailable_channel(name: &str) -> SlackChannel {
    SlackChannel {
        name: bounded(name, MAX_NAME_CHARS),
        available: false,
        unread: 0,
        unread_capped: false,
        messages: Vec::new(),
    }
}

/// Opens the conversation in the Slack desktop app at the message rather
/// than in a browser tab. `message` is not in Slack's documented deep link
/// list; a client that ignores it still opens the right conversation.
fn deep_link(identity: &Identity, channel: &str, ts: &str) -> String {
    format!(
        "slack://channel?team={}&id={channel}&message={ts}",
        identity.team_id
    )
}

fn parse_ts(ts: &str) -> Option<f64> {
    let (seconds, micros) = ts.split_once('.')?;
    let valid = !seconds.is_empty()
        && seconds.len() <= 12
        && micros.len() <= 8
        && seconds
            .bytes()
            .chain(micros.bytes())
            .all(|byte| byte.is_ascii_digit());
    valid.then(|| ts.parse().ok()).flatten()
}

fn is_slack_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 24
        && id
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

/// Only `slack://channel` deep links built from validated ids are opened.
pub fn is_slack_link(link: &str) -> bool {
    let Some(query) = link.strip_prefix("slack://channel?team=") else {
        return false;
    };
    let Some((team, rest)) = query.split_once("&id=") else {
        return false;
    };
    let Some((channel, ts)) = rest.split_once("&message=") else {
        return false;
    };
    is_slack_id(team) && is_slack_id(channel) && parse_ts(ts).is_some()
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn bounded(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|character| *character == '\n' || !character.is_control())
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // A stand-in curl that checks the token arrived on stdin and answers
    // each Web API method with recorded output.
    fn fake_curl(dir: &Path) -> PathBuf {
        let path = dir.join("curl");
        crate::test_support::write_executable(
            &path,
            r#"#!/bin/sh
config=$(cat)
case "$config" in *"Bearer xoxp-1-test"*) ;; *) exit 22 ;; esac
case "$*" in *xoxp*) exit 23 ;; esac
url=$(printf '%s\n' "$config" | sed -n 's/^url = "\(.*\)"$/\1/p')
out=$(printf '%s\n' "$config" | sed -n 's/^output = "\(.*\)"$/\1/p')
case "$url" in
  https://files.slack.com/*-ok/*) printf 'PNG' > "$out"; printf '200 image/png'; exit 0 ;;
  https://files.slack.com/*) printf '<html>' > "$out"; printf '302 text/html'; exit 0 ;;
  *auth.test*) echo '{"ok":true,"user_id":"UME","team_id":"TACME","url":"https://acme.slack.com/"}' ;;
  *users.conversations*types=im*) echo '{"ok":true,"channels":[{"id":"D1","user":"UKARI"}]}' ;;
  *users.conversations*) echo '{"ok":true,"channels":[{"id":"CDEV","name":"dev"}],"response_metadata":{"next_cursor":""}}' ;;
  *conversations.info*channel=D1*) echo '{"ok":true,"channel":{"id":"D1","unread_count_display":2,"latest":{"ts":"1700000300.000200","text":"lunch?"}}}' ;;
  *conversations.info*) echo '{"ok":true,"channel":{"id":"CDEV","last_read":"1700000100.000100"}}' ;;
  *conversations.history*) echo '{"ok":true,"messages":[{"ts":"1700000200.000300","user":"UKARI","text":"hi <@UME> see <https://x.dev|docs> &amp; <#CDEV|dev>","reply_count":3,"files":[{"id":"FIMG1","mimetype":"image/png","thumb_480":"https://files.slack.com/files-tmb/T-FIMG1-ok/x_480.png","thumb_480_w":480,"thumb_480_h":135},{"id":"FIMG2","mimetype":"image/png","thumb_480":"https://files.slack.com/files-tmb/T-FIMG2-denied/x_480.png","thumb_480_w":480,"thumb_480_h":200}]},{"ts":"1700000150.000100","subtype":"channel_join","user":"UOLA","text":"joined"},{"ts":"1700000050.000100","user":"UME","text":"older"}]}' ;;
  *users.info*user=UKARI*) echo '{"ok":true,"user":{"name":"kari","profile":{"display_name":"Kari","real_name":"Kari N"}}}' ;;
  *users.info*user=UME*) echo '{"ok":true,"user":{"name":"me","profile":{"display_name":"","real_name":"Jon"}}}' ;;
  *search.messages*) echo '{"ok":true,"messages":{"matches":[{"channel":{"id":"CDEV","name":"dev"},"user":"UKARI","ts":"'"$(date +%s)"'.000300","text":"hi <@UME>"},{"channel":{"id":"CDEV","name":"dev"},"user":"UKARI","ts":"1700000200.000300","text":"old <@UME>"}]}}' ;;
  *) echo '{"ok":false,"error":"unknown_method"}' ;;
esac
"#,
        );
        path
    }

    fn token_file(dir: &Path, mode: u32) -> PathBuf {
        let path = dir.join("slack.toml");
        fs::write(&path, "token = \"xoxp-1-test\"\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    fn client(dir: &Path, mode: u32) -> SlackClient {
        SlackClient::with_curl(
            SlackConfig {
                channels: vec!["dev".into(), "gone".into()],
                token_file: Some(token_file(dir, mode)),
            },
            fake_curl(dir),
        )
        .with_image_dir(Some(dir.join("images")))
    }

    #[tokio::test]
    async fn samples_channels_dms_and_mentions() {
        let temp = tempfile::tempdir().unwrap();
        let images = temp.path().join("images");
        fs::create_dir_all(&images).unwrap();
        fs::write(images.join("FSTALE.png"), "old").unwrap();
        let sample = client(temp.path(), 0o600).sample().await;
        let snapshot = &sample.snapshot;
        assert!(snapshot.available);

        // Only a real image is kept; a sign-in redirect is discarded, and
        // thumbnails no message refers to are pruned.
        assert_eq!(
            snapshot.channels[0].messages[1].images,
            [SlackImage {
                file: "FIMG1.png".into(),
                width: 480,
                height: 135
            }]
        );
        assert!(images.join("FIMG1.png").exists());
        assert!(!images.join("FIMG2.png").exists());
        assert!(!images.join("FSTALE.png").exists());
        assert_eq!(fs::read_dir(&images).unwrap().count(), 1);

        let dev = &snapshot.channels[0];
        assert!(dev.available);
        assert_eq!(dev.unread, 1);
        let texts: Vec<_> = dev.messages.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, ["older", "hi @Jon see docs & #dev"]);
        assert_eq!(dev.messages[1].author, "Kari");
        assert_eq!(dev.messages[1].replies, 3);
        assert!(dev.messages[1].unread && !dev.messages[0].unread);
        assert!(!snapshot.channels[1].available);

        let kinds: Vec<_> = snapshot
            .notifications
            .iter()
            .map(|n| n.kind.as_str())
            .collect();
        assert_eq!(kinds, ["dm", "mention"]);
        assert_eq!(snapshot.notifications[0].unread, 2);
        assert_eq!(snapshot.notifications[0].source, "Kari");
        assert_eq!(snapshot.notifications[1].source, "#dev");

        let id = &dev.messages[1].id;
        assert_eq!(
            sample.urls[id],
            "slack://channel?team=TACME&id=CDEV&message=1700000200.000300"
        );
        assert!(is_slack_link(&sample.urls[id]));
        let published = serde_json::to_string(snapshot).unwrap();
        assert!(!published.contains("slack://") && !published.contains("xoxp"));
    }

    #[tokio::test]
    async fn a_token_readable_by_others_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let sample = client(temp.path(), 0o644).sample().await;
        assert!(!sample.snapshot.available);
        assert!(sample.urls.is_empty());
    }

    #[test]
    fn shortcodes_become_emoji_and_everything_else_is_left_alone() {
        assert_eq!(emojify("hei :wave: :+1::skin-tone-3: ok"), "hei 👋 👍 ok");
        assert_eq!(emojify("at 12:30:45"), "at 12:30:45");
        assert_eq!(emojify("see https://x.dev/a:b"), "see https://x.dev/a:b");
        assert_eq!(emojify(":partyparrot: :"), ":partyparrot: :");
        let message = serde_json::json!({"reactions": [
            {"name": "thumbsup", "count": 3},
            {"name": "partyparrot", "count": 1},
            {"name": "wave::skin-tone-2", "count": 2},
            {"name": "zero", "count": 0}
        ]});
        let rendered: Vec<_> = reactions(&message)
            .into_iter()
            .map(|reaction| (reaction.emoji, reaction.count))
            .collect();
        assert_eq!(
            rendered,
            [
                ("👍".to_owned(), 3),
                (":partyparrot:".to_owned(), 1),
                ("👋".to_owned(), 2)
            ]
        );
    }

    #[test]
    fn curl_config_values_cannot_break_out_of_their_line() {
        assert!(is_config_safe(
            "https://files.slack.com/files-tmb/T-F1/x_480.png"
        ));
        for value in [
            "",
            "https://files.slack.com/a\"\nurl = \"https://evil",
            "https://files.slack.com/a\nheader = x",
            "https://files.slack.com/a\\b",
            "https://files.slack.com/a b",
        ] {
            assert!(!is_config_safe(value), "{value:?}");
        }
    }

    #[test]
    fn file_only_posts_name_their_files() {
        let message = serde_json::json!({"files": [
            {"name": "skjerm.png", "title": "Skjermbilde"},
            {"name": "notes.txt"}
        ]});
        assert_eq!(file_summary(&message), "📎 Skjermbilde  📎 notes.txt");
        assert_eq!(file_summary(&serde_json::json!({})), "");
    }

    #[test]
    fn only_validated_deep_links_are_openable() {
        assert!(is_slack_link(
            "slack://channel?team=TACME&id=C1&message=1700000200.000300"
        ));
        for link in [
            "slack://channel?team=TACME&id=C1",
            "slack://open?team=TACME&id=C1&message=1.2",
            "slack://channel?team=tacme&id=C1&message=1.2",
            "slack://channel?team=TACME&id=C1&message=1.2&x=y",
            "https://acme.slack.com/archives/C1/p1",
            "slack://channel?team=TACME&id=C1&message=1.2 --x",
        ] {
            assert!(!is_slack_link(link), "{link}");
        }
    }
}
