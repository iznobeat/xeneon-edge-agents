use std::{path::PathBuf, process::Stdio, time::Duration};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::{io::AsyncWriteExt, process::Command, time::timeout};

use crate::model::{LyricLine, LyricsSnapshot, LyricsStatus, SpotifySnapshot};

const API_BASE: &str = "https://lrclib.net/api/";
// LRCLIB asks clients to name themselves.
const USER_AGENT: &str = concat!(
    "xeneon-edge-agents/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/jhgundersen/xeneon-edge-agents)"
);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RESPONSE_BYTES: &str = "2097152";
// LRCLIB answers 503 when busy; a lookup tries a few times before giving up.
const ATTEMPTS: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_millis(if cfg!(test) { 10 } else { 1_500 });
// LRCLIB's own matching window for a track's length.
const DURATION_TOLERANCE_S: f64 = 2.0;
const MAX_LINES: usize = 400;
const MAX_LINE_CHARS: usize = 200;

/// What a lookup is keyed on: the track as Spotify names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LyricsQuery {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_s: u64,
}

impl LyricsQuery {
    /// The query for what Spotify plays, if it names a track.
    pub fn for_track(spotify: &SpotifySnapshot) -> Option<Self> {
        (spotify.available && !spotify.title.trim().is_empty() && spotify.length_ms > 0).then(
            || Self {
                title: spotify.title.clone(),
                artist: spotify.artist.clone(),
                album: spotify.album.clone(),
                duration_s: (spotify.length_ms + 500) / 1000,
            },
        )
    }

    pub fn snapshot(&self, status: LyricsStatus, lines: Vec<LyricLine>) -> LyricsSnapshot {
        LyricsSnapshot {
            status,
            title: self.title.clone(),
            artist: self.artist.clone(),
            lines,
        }
    }
}

/// Looks lyrics up on LRCLIB through `curl`, like the other collectors shell
/// out to their tools. Only the track's title, artist, album, and length are
/// sent, and only to LRCLIB's two read-only endpoints.
#[derive(Debug, Clone)]
pub struct LyricsClient {
    curl_bin: PathBuf,
    api_base: String,
}

impl Default for LyricsClient {
    fn default() -> Self {
        Self::new("curl", API_BASE)
    }
}

impl LyricsClient {
    pub fn new(curl_bin: impl Into<PathBuf>, api_base: impl Into<String>) -> Self {
        Self {
            curl_bin: curl_bin.into(),
            api_base: api_base.into(),
        }
    }

    /// The lyrics for `query`: synced when LRCLIB has times, plain, or
    /// `Missing`. An error means LRCLIB could not be asked (busy, offline)
    /// and the lookup is worth repeating later.
    pub async fn lookup(&self, query: &LyricsQuery) -> Result<LyricsSnapshot> {
        // `get-cached` answers quickly from the tracks LRCLIB already holds
        // and the search is rarely busy either. `get` also asks LRCLIB's
        // external sources, so it is slow and often busy under load: it is
        // tried last, and only while no synced lyrics have turned up. The
        // exact lookups need an artist.
        let duration = query.duration_s.to_string();
        let params = [
            ("track_name", query.title.as_str()),
            ("artist_name", query.artist.as_str()),
            ("album_name", query.album.as_str()),
            ("duration", duration.as_str()),
        ];
        let exact = !query.artist.is_empty();
        let mut best = None;
        let mut failed = None;
        if exact {
            match self.get("get-cached", &params, ATTEMPTS).await {
                Ok(record) => best = record.and_then(|record| from_record(query, &record)),
                Err(error) => failed = Some(error),
            }
        }
        if !is_final(&best) {
            match self.search(query).await {
                Ok(found) => best = better(best, found),
                Err(error) => failed = Some(error),
            }
        }
        if exact && !is_final(&best) {
            // With plain lyrics in hand, a busy `get` is asked only once.
            let attempts = if best.is_some() { 1 } else { ATTEMPTS };
            match self.get("get", &params, attempts).await {
                Ok(record) => {
                    best = better(best, record.and_then(|record| from_record(query, &record)))
                }
                Err(error) => failed = Some(error),
            }
        }
        match (best, failed) {
            (Some(best), _) => Ok(best),
            // A lookup that could not be asked might still have had them.
            (None, Some(error)) => Err(error),
            (None, None) => Ok(query.snapshot(LyricsStatus::Missing, Vec::new())),
        }
    }

    /// The best search match. Spotify titles often carry a suffix ("-
    /// Remastered 2011", "(Live)") and several artists that LRCLIB files
    /// differently, so it searches the bare title and first artist and
    /// matches by length.
    async fn search(&self, query: &LyricsQuery) -> Result<Option<LyricsSnapshot>> {
        let title = bare_title(&query.title);
        let artist = query.artist.split(", ").next().unwrap_or_default();
        let found = self
            .get(
                "search",
                &[("track_name", title), ("artist_name", artist)],
                ATTEMPTS,
            )
            .await?;
        Ok(found
            .as_ref()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|record| {
                record["duration"].as_f64().is_some_and(|seconds| {
                    (seconds - query.duration_s as f64).abs() <= DURATION_TOLERANCE_S
                })
            })
            .filter_map(|record| from_record(query, record))
            .min_by_key(rank))
    }

    /// One LRCLIB request; `None` when it has no answer for these parameters
    /// (404, another 4xx, or a body that is not JSON), which asking again
    /// would not change. Only a busy or unreachable LRCLIB is an error.
    async fn get(
        &self,
        endpoint: &str,
        params: &[(&str, &str)],
        attempts: u32,
    ) -> Result<Option<Value>> {
        let query = params
            .iter()
            .map(|(key, value)| format!("{key}={}", percent_encode(value)))
            .collect::<Vec<_>>()
            .join("&");
        let url = format!("{}{endpoint}?{query}", self.api_base);
        if !is_config_safe(&url) {
            bail!("refusing an unsafe LRCLIB request URL");
        }
        let mut attempt = 1;
        loop {
            match self.request(&url).await? {
                (200, body) => {
                    return Ok(serde_json::from_slice(&body)
                        .inspect_err(|error| tracing::debug!(%error, "unreadable LRCLIB answer"))
                        .ok());
                }
                (status, _) if status != 429 && (400..500).contains(&status) => return Ok(None),
                (status, _) if attempt < attempts => {
                    tracing::debug!(status, endpoint, "LRCLIB busy, retrying");
                    attempt += 1;
                    tokio::time::sleep(RETRY_DELAY).await;
                }
                (status, _) => bail!("LRCLIB {endpoint} answered {status}"),
            }
        }
    }

    async fn request(&self, url: &str) -> Result<(u16, Vec<u8>)> {
        // The URL goes on curl's config on stdin like the other collectors',
        // so what is playing stays out of the process list.
        let config = format!("url = \"{url}\"\nuser-agent = \"{USER_AGENT}\"\n");
        let mut child = Command::new(&self.curl_bin)
            .kill_on_drop(true)
            .args([
                "--silent",
                "--max-time",
                "12",
                "--max-filesize",
                MAX_RESPONSE_BYTES,
                "--write-out",
                "\n%{http_code}",
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
            .context("timed out waiting for LRCLIB")??;
        if !output.status.success() {
            bail!("LRCLIB request failed");
        }
        let split = output
            .stdout
            .iter()
            .rposition(|byte| *byte == b'\n')
            .context("curl gave no status code")?;
        let status = std::str::from_utf8(&output.stdout[split + 1..])
            .ok()
            .and_then(|code| code.trim().parse().ok())
            .context("curl gave no status code")?;
        let mut body = output.stdout;
        body.truncate(split);
        Ok((status, body))
    }
}

/// Synced lyrics come first, then an instrumental, then plain lyrics.
fn rank(snapshot: &LyricsSnapshot) -> u8 {
    match snapshot.status {
        LyricsStatus::Synced => 0,
        LyricsStatus::Instrumental => 1,
        LyricsStatus::Plain => 2,
        _ => 3,
    }
}

fn better(
    current: Option<LyricsSnapshot>,
    found: Option<LyricsSnapshot>,
) -> Option<LyricsSnapshot> {
    match (current, found) {
        (Some(current), Some(found)) if rank(&found) < rank(&current) => Some(found),
        (Some(current), _) => Some(current),
        (None, found) => found,
    }
}

/// Nothing better than these is left to look for.
fn is_final(best: &Option<LyricsSnapshot>) -> bool {
    best.as_ref().is_some_and(|snapshot| rank(snapshot) <= 1)
}

/// The lyrics in one LRCLIB record, preferring its synced lines; `None`
/// when it has neither kind.
fn from_record(query: &LyricsQuery, record: &Value) -> Option<LyricsSnapshot> {
    if record["instrumental"].as_bool() == Some(true) {
        return Some(query.snapshot(LyricsStatus::Instrumental, Vec::new()));
    }
    let synced = parse_synced(record["syncedLyrics"].as_str().unwrap_or_default());
    if !synced.is_empty() {
        return Some(query.snapshot(LyricsStatus::Synced, synced));
    }
    let plain = parse_plain(record["plainLyrics"].as_str().unwrap_or_default());
    (!plain.is_empty()).then(|| query.snapshot(LyricsStatus::Plain, plain))
}

/// LRC lines (`[01:02.34]text`, one or more stamps per line) in time order.
/// Tags such as `[ar:Artist]` and lines without a stamp are dropped; an empty
/// text marks an instrumental break.
fn parse_synced(lrc: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for raw in lrc.lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();
        while let Some(after) = rest.strip_prefix('[')
            && let Some(end) = after.find(']')
            && let Some(at_ms) = parse_stamp(&after[..end])
        {
            stamps.push(at_ms);
            rest = after[end + 1..].trim_start();
        }
        let text = bounded(rest).trim().to_owned();
        lines.extend(stamps.into_iter().map(|at_ms| LyricLine {
            at_ms,
            text: text.clone(),
        }));
    }
    lines.sort_by_key(|line| line.at_ms);
    lines.truncate(MAX_LINES);
    lines
}

/// `mm:ss`, `mm:ss.x`, `mm:ss.xx`, or `mm:ss.xxx` as milliseconds.
fn parse_stamp(stamp: &str) -> Option<u64> {
    let (minutes, seconds) = stamp.split_once(':')?;
    let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let digits = |text: &str, max: usize| {
        (!text.is_empty() && text.len() <= max && text.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| text.parse::<u64>().ok())
            .flatten()
    };
    let minutes = digits(minutes, 3)?;
    let whole = digits(whole, 2).filter(|seconds| *seconds < 60)?;
    let fraction_ms = if fraction.is_empty() {
        0
    } else {
        digits(fraction, 3)? * 10_u64.pow(3 - fraction.len() as u32)
    };
    Some((minutes * 60 + whole) * 1000 + fraction_ms)
}

/// Plain lyrics as untimed lines, keeping single blank lines between verses.
fn parse_plain(text: &str) -> Vec<LyricLine> {
    let mut lines: Vec<LyricLine> = Vec::new();
    for raw in text.lines() {
        let line = bounded(raw).trim().to_owned();
        let blank_after_blank =
            line.is_empty() && lines.last().is_none_or(|previous| previous.text.is_empty());
        if !blank_after_blank {
            lines.push(LyricLine {
                at_ms: 0,
                text: line,
            });
        }
    }
    while lines.last().is_some_and(|line| line.text.is_empty()) {
        lines.pop();
    }
    lines.truncate(MAX_LINES);
    lines
}

/// The title without Spotify's version suffix: "Song - Remastered 2011" and
/// "Song (Live)" become "Song".
fn bare_title(title: &str) -> &str {
    let title = title.split(" - ").next().unwrap_or(title);
    match title.rfind(" (") {
        Some(start) if start > 0 && title.ends_with(')') => &title[..start],
        _ => title,
    }
    .trim()
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

/// Printable ASCII without quote or backslash: safe inside a quoted curl
/// config value.
fn is_config_safe(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| (0x21..=0x7e).contains(&byte) && byte != b'"' && byte != b'\\')
}

fn bounded(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_LINE_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, path::Path};

    use super::*;

    fn query() -> LyricsQuery {
        LyricsQuery {
            title: "Is He Worthy (Live)".into(),
            artist: "Shane & Shane, Someone".into(),
            album: "Hymns Live".into(),
            duration_s: 294,
        }
    }

    #[test]
    fn synced_lyrics_keep_every_stamp_in_time_order_and_drop_tags() {
        let lines = parse_synced(
            "[ar:Shane]\n[00:19.49]Do you feel\n[00:51.06]\n[01:02.5][00:30.123] Twice \u{7}\nno stamp\n[xx:10.00]bad\n",
        );
        let pairs: Vec<_> = lines
            .iter()
            .map(|line| (line.at_ms, line.text.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                (19_490, "Do you feel"),
                (30_123, "Twice"),
                (51_060, ""),
                (62_500, "Twice"),
            ]
        );
        assert_eq!(parse_stamp("03:07"), Some(187_000));
        assert_eq!(parse_stamp("00:61.00"), None);
        assert_eq!(parse_stamp("00:10.1234"), None);
    }

    #[test]
    fn plain_lyrics_keep_one_blank_line_between_verses() {
        let lines = parse_plain("\nOne\nTwo\n\n\n\nThree\n\n");
        let texts: Vec<_> = lines.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(texts, ["One", "Two", "", "Three"]);
        assert!(lines.iter().all(|line| line.at_ms == 0));
    }

    #[test]
    fn records_prefer_synced_lines_and_flag_instrumentals() {
        let record = serde_json::json!({
            "syncedLyrics": "[00:01.00]A",
            "plainLyrics": "A",
        });
        assert_eq!(
            from_record(&query(), &record).unwrap().status,
            LyricsStatus::Synced
        );
        let record = serde_json::json!({ "syncedLyrics": null, "plainLyrics": "A" });
        assert_eq!(
            from_record(&query(), &record).unwrap().status,
            LyricsStatus::Plain
        );
        let record = serde_json::json!({ "instrumental": true });
        assert_eq!(
            from_record(&query(), &record).unwrap().status,
            LyricsStatus::Instrumental
        );
        assert_eq!(from_record(&query(), &serde_json::json!({})), None);
    }

    #[test]
    fn version_suffixes_are_dropped_for_the_search() {
        assert_eq!(bare_title("Song - Remastered 2011"), "Song");
        assert_eq!(bare_title("Is He Worthy (Live)"), "Is He Worthy");
        assert_eq!(bare_title("(Untitled)"), "(Untitled)");
        assert_eq!(bare_title("Plain"), "Plain");
    }

    #[test]
    fn only_a_playing_track_with_a_length_is_looked_up() {
        let mut spotify = SpotifySnapshot {
            available: true,
            title: "Song".into(),
            length_ms: 215_400,
            ..SpotifySnapshot::default()
        };
        assert_eq!(LyricsQuery::for_track(&spotify).unwrap().duration_s, 215);
        spotify.length_ms = 0;
        assert_eq!(LyricsQuery::for_track(&spotify), None);
        assert_eq!(LyricsQuery::for_track(&SpotifySnapshot::default()), None);
    }

    // A stand-in curl that answers from files named after the endpoint and
    // logs the config it was given on stdin.
    fn fake_curl(dir: &Path, get: (u16, &str), search: (u16, &str)) -> PathBuf {
        for (name, (status, body)) in [("get", get), ("search", search)] {
            fs::write(dir.join(format!("{name}.body")), body).unwrap();
            fs::write(dir.join(format!("{name}.status")), status.to_string()).unwrap();
        }
        let path = dir.join("curl");
        fs::write(
            &path,
            format!(
                r#"#!/bin/sh
config=$(cat)
printf '%s\n' "$config" >> '{dir}/requests'
case "$config" in
  *"/api/get-cached?"*) name=get; [ -f '{dir}/cached.body' ] && name=cached ;;
  *"/api/get?"*) name=get ;;
  *) name=search ;;
esac
cat '{dir}/'$name.body
printf '\n%s' "$(cat '{dir}/'$name.status)"
"#,
                dir = dir.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[tokio::test]
    async fn an_exact_match_is_used_and_the_track_stays_off_the_command_line() {
        let temp = tempfile::tempdir().unwrap();
        let curl = fake_curl(
            temp.path(),
            (200, r#"{"syncedLyrics":"[00:19.49]Do you feel"}"#),
            (500, ""),
        );
        let client = LyricsClient::new(curl, "https://lrclib.net/api/");
        let lyrics = client.lookup(&query()).await.unwrap();
        assert_eq!(lyrics.status, LyricsStatus::Synced);
        assert_eq!(lyrics.title, "Is He Worthy (Live)");
        assert_eq!(lyrics.lines[0].text, "Do you feel");
        let requests = fs::read_to_string(temp.path().join("requests")).unwrap();
        assert!(requests.contains(
            "url = \"https://lrclib.net/api/get-cached?track_name=Is%20He%20Worthy%20%28Live%29&artist_name=Shane%20%26%20Shane%2C%20Someone&album_name=Hymns%20Live&duration=294\""
        ));
        assert!(requests.contains("user-agent = \"xeneon-edge-agents/"));
    }

    #[tokio::test]
    async fn a_miss_falls_back_to_a_search_matched_by_length() {
        let temp = tempfile::tempdir().unwrap();
        let curl = fake_curl(
            temp.path(),
            (404, r#"{"code":404}"#),
            (
                200,
                r#"[{"duration":250,"syncedLyrics":"[00:01.00]Wrong"},
                    {"duration":293.6,"plainLyrics":"Plain"},
                    {"duration":295,"syncedLyrics":"[00:02.00]Right"}]"#,
            ),
        );
        let client = LyricsClient::new(curl, "https://lrclib.net/api/");
        let lyrics = client.lookup(&query()).await.unwrap();
        assert_eq!(lyrics.status, LyricsStatus::Synced);
        assert_eq!(lyrics.lines[0].text, "Right");
        let requests = fs::read_to_string(temp.path().join("requests")).unwrap();
        assert!(
            requests
                .contains("search?track_name=Is%20He%20Worthy&artist_name=Shane%20%26%20Shane\"")
        );

        fs::write(temp.path().join("search.body"), "[]").unwrap();
        let lyrics = client.lookup(&query()).await.unwrap();
        assert_eq!(lyrics.status, LyricsStatus::Missing);
        assert!(lyrics.lines.is_empty());
    }

    #[tokio::test]
    async fn the_slow_get_is_asked_only_while_synced_lyrics_are_missing() {
        let temp = tempfile::tempdir().unwrap();
        let curl = fake_curl(
            temp.path(),
            (200, r#"{"syncedLyrics":"[00:05.00]From get"}"#),
            (
                200,
                r#"[{"duration":294,"syncedLyrics":"[00:02.00]From search"}]"#,
            ),
        );
        fs::write(temp.path().join("cached.status"), "404").unwrap();
        fs::write(temp.path().join("cached.body"), "{}").unwrap();
        let client = LyricsClient::new(curl, "https://lrclib.net/api/");
        let lyrics = client.lookup(&query()).await.unwrap();
        assert_eq!(lyrics.lines[0].text, "From search");
        let requests = fs::read_to_string(temp.path().join("requests")).unwrap();
        assert!(!requests.contains("/api/get?"));

        // Plain lyrics from the search are upgraded by a synced `get`.
        fs::write(
            temp.path().join("search.body"),
            r#"[{"duration":294,"plainLyrics":"Plain"}]"#,
        )
        .unwrap();
        let lyrics = client.lookup(&query()).await.unwrap();
        assert_eq!(lyrics.status, LyricsStatus::Synced);
        assert_eq!(lyrics.lines[0].text, "From get");
    }

    #[tokio::test]
    async fn answers_that_asking_again_would_not_change_are_missing_lyrics() {
        let temp = tempfile::tempdir().unwrap();
        let curl = fake_curl(temp.path(), (400, "bad"), (200, "not json"));
        let client = LyricsClient::new(curl, "https://lrclib.net/api/");
        let lyrics = client.lookup(&query()).await.unwrap();
        assert_eq!(lyrics.status, LyricsStatus::Missing);

        // Without an artist only the search is asked.
        fs::remove_file(temp.path().join("requests")).unwrap();
        let untagged = LyricsQuery {
            artist: String::new(),
            ..query()
        };
        client.lookup(&untagged).await.unwrap();
        let requests = fs::read_to_string(temp.path().join("requests")).unwrap();
        assert!(!requests.contains("/api/get"));
        assert_eq!(requests.matches("/api/search?").count(), 1);
    }

    #[tokio::test]
    async fn a_busy_or_missing_service_is_an_error_to_retry() {
        let temp = tempfile::tempdir().unwrap();
        let curl = fake_curl(temp.path(), (503, "busy"), (503, "busy"));
        let client = LyricsClient::new(curl, "https://lrclib.net/api/");
        assert!(client.lookup(&query()).await.is_err());
        let requests = fs::read_to_string(temp.path().join("requests")).unwrap();
        assert_eq!(
            requests.matches("/api/get-cached?").count(),
            ATTEMPTS as usize
        );
        assert_eq!(requests.matches("/api/get?").count(), ATTEMPTS as usize);
        // A busy exact lookup still lets the search answer, and with plain
        // lyrics in hand the busy `get` is asked only once more.
        fs::remove_file(temp.path().join("requests")).unwrap();
        fs::write(temp.path().join("search.status"), "200").unwrap();
        fs::write(
            temp.path().join("search.body"),
            r#"[{"duration":294,"plainLyrics":"Found"}]"#,
        )
        .unwrap();
        assert_eq!(
            client.lookup(&query()).await.unwrap().status,
            LyricsStatus::Plain
        );
        let requests = fs::read_to_string(temp.path().join("requests")).unwrap();
        assert_eq!(requests.matches("/api/get?").count(), 1);
        let missing = LyricsClient::new("/nonexistent/curl", "https://lrclib.net/api/");
        assert!(missing.lookup(&query()).await.is_err());
    }
}
