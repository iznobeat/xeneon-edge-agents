use std::{collections::HashMap, path::PathBuf, process::Stdio, time::Duration};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tokio::{process::Command, time::timeout};

use crate::{
    config::{BasecampConfig, CardTableConfig},
    model::{BasecampBoard, BasecampCard, BasecampLane, BasecampNotification, BasecampSnapshot},
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_NOTIFICATIONS: usize = 8;
const MAX_CARDS_PER_LANE: usize = 12;
const MAX_TEXT_CHARS: usize = 160;
const MAX_NAME_CHARS: usize = 48;
// Only links back into Basecamp itself are ever opened.
const URL_PREFIXES: [&str; 2] = ["https://app.basecamp.com/", "https://3.basecamp.com/"];

// The CLI's built-in --jq keeps only what the dashboard shows, plus the URL
// the daemon keeps to itself.
const NOTIFICATIONS_JQ: &str = r#"{capped: (.unreads_capped // false), total: ((.unreads // []) | length), items: [(.unreads // [])[:8][] | {id: (.id | tostring), kind: (.type // ""), title: (.title // ""), project: (.bucket_name // ""), creator: (.creator.name // ""), unread_at: (.unread_at // .updated_at // ""), url: (.app_url // "")}]}"#;
const COLUMNS_JQ: &str = r#"[.[] | {id: (.id | tostring), title: (.title // ""), type: (.type // ""), color: (.color // ""), count: (.cards_count // 0)}]"#;
const CARDS_JQ: &str = r#"[.[] | select((.completed // false) | not) | {id: (.id | tostring), title: (.title // ""), assignees: [(.assignees // [])[] | (.name // "")], due_on: .due_on, comments: (.comments_count // 0), url: (.app_url // "")}]"#;

#[derive(Debug, Deserialize)]
struct NotificationsOutput {
    capped: bool,
    total: u32,
    items: Vec<NotificationRow>,
}

#[derive(Debug, Deserialize)]
struct NotificationRow {
    id: String,
    kind: String,
    title: String,
    project: String,
    creator: String,
    unread_at: String,
    url: String,
}

#[derive(Debug, Deserialize)]
struct ColumnRow {
    id: String,
    title: String,
    #[serde(rename = "type")]
    kind: String,
    color: String,
    count: u32,
}

#[derive(Debug, Deserialize)]
struct CardRow {
    id: String,
    title: String,
    assignees: Vec<String>,
    due_on: Option<String>,
    comments: u32,
    url: String,
}

/// One Basecamp sample: the published snapshot plus the item URLs that the
/// daemon keeps private and opens by id.
#[derive(Debug, Default)]
pub struct BasecampSample {
    pub snapshot: BasecampSnapshot,
    pub urls: HashMap<String, String>,
}

/// Reads the Hey! inbox and configured card tables through the user's
/// signed-in Basecamp CLI. Only fixed read commands are run.
#[derive(Debug, Clone)]
pub struct BasecampClient {
    config: BasecampConfig,
}

impl BasecampClient {
    pub fn new(config: BasecampConfig) -> Self {
        Self { config }
    }

    /// A missing CLI or expired login is reported as an unavailable
    /// snapshot; one unreadable card table only marks that board.
    pub async fn sample(&self) -> BasecampSample {
        let mut sample = BasecampSample::default();
        match self
            .json::<NotificationsOutput>(&["notifications", "list"], NOTIFICATIONS_JQ)
            .await
        {
            Ok(output) => {
                sample.snapshot.available = true;
                sample.snapshot.unread_count = output.total;
                sample.snapshot.unreads_capped = output.capped;
                for row in output.items.into_iter().take(MAX_NOTIFICATIONS) {
                    // Notifications and cards share one id map; prefix them so
                    // equal numbers can never open the wrong item.
                    let Some(id) = item_id('n', &row.id) else {
                        continue;
                    };
                    remember_url(&mut sample.urls, &id, &row.url);
                    sample.snapshot.notifications.push(BasecampNotification {
                        id,
                        kind: notification_kind(&row.kind).into(),
                        title: bounded(&row.title, MAX_TEXT_CHARS),
                        project: bounded(&row.project, MAX_NAME_CHARS),
                        creator: bounded(&row.creator, MAX_NAME_CHARS),
                        unread_at: bounded(&row.unread_at, 40),
                    });
                }
            }
            Err(error) => {
                tracing::debug!(%error, "Basecamp notifications unavailable");
                return sample;
            }
        }
        for table in &self.config.card_tables {
            let board = match self.board(table, &mut sample.urls).await {
                Ok(board) => board,
                Err(error) => {
                    tracing::debug!(%error, table = table.card_table, "Basecamp card table unavailable");
                    BasecampBoard {
                        label: table.label.clone(),
                        available: false,
                        lanes: Vec::new(),
                        not_now: 0,
                        done: 0,
                    }
                }
            };
            sample.snapshot.boards.push(board);
        }
        sample
    }

    async fn board(
        &self,
        table: &CardTableConfig,
        urls: &mut HashMap<String, String>,
    ) -> Result<BasecampBoard> {
        let project = table.project.to_string();
        let card_table = table.card_table.to_string();
        let scope = [
            "--in",
            project.as_str(),
            "--card-table",
            card_table.as_str(),
        ];
        let columns: Vec<ColumnRow> = self
            .json(
                &[&["cards", "columns"][..], &scope[..]].concat(),
                COLUMNS_JQ,
            )
            .await?;
        let mut board = BasecampBoard {
            label: table.label.clone(),
            available: true,
            lanes: Vec::new(),
            not_now: 0,
            done: 0,
        };
        for column in columns {
            match column.kind.as_str() {
                "Kanban::NotNowColumn" => board.not_now = column.count,
                "Kanban::DoneColumn" => board.done = column.count,
                "Kanban::Triage" | "Kanban::Column" => {
                    let limit = MAX_CARDS_PER_LANE.to_string();
                    let args = [
                        &["cards", "list"][..],
                        &scope[..],
                        &["--column", column.id.as_str(), "--limit", limit.as_str()][..],
                    ]
                    .concat();
                    let rows: Vec<CardRow> = self.json(&args, CARDS_JQ).await?;
                    let cards = rows
                        .into_iter()
                        .take(MAX_CARDS_PER_LANE)
                        .filter_map(|row| {
                            let id = item_id('c', &row.id)?;
                            remember_url(urls, &id, &row.url);
                            Some(BasecampCard {
                                id,
                                title: bounded(&row.title, MAX_TEXT_CHARS),
                                assignees: row
                                    .assignees
                                    .iter()
                                    .filter(|name| !name.trim().is_empty())
                                    .take(4)
                                    .map(|name| bounded(name, MAX_NAME_CHARS))
                                    .collect(),
                                due_on: row
                                    .due_on
                                    .filter(|due| due.len() == 10)
                                    .map(|due| bounded(&due, 10)),
                                comments: row.comments,
                            })
                        })
                        .collect();
                    board.lanes.push(BasecampLane {
                        title: bounded(&column.title, MAX_NAME_CHARS),
                        color: lane_color(&column.color).into(),
                        total: column.count,
                        cards,
                    });
                }
                _ => {}
            }
        }
        Ok(board)
    }

    async fn json<T: for<'de> Deserialize<'de>>(&self, args: &[&str], jq: &str) -> Result<T> {
        let output = timeout(
            COMMAND_TIMEOUT,
            Command::new(&self.config.bin)
                .kill_on_drop(true)
                .args(
                    self.config
                        .profile
                        .as_deref()
                        .map(|profile| ["--profile", profile])
                        .into_iter()
                        .flatten(),
                )
                .args(args)
                .args(["--agent", "--jq", jq])
                .env("BASECAMP_NONINTERACTIVE", "1")
                .stdin(Stdio::null())
                .stderr(Stdio::piped())
                .output(),
        )
        .await
        .context("timed out waiting for the Basecamp CLI")?
        .with_context(|| format!("running {}", PathBuf::from(&self.config.bin).display()))?;
        if !output.status.success() {
            bail!(
                "Basecamp CLI failed: {}",
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(200)
                    .collect::<String>()
            );
        }
        serde_json::from_slice(&output.stdout).context("parsing Basecamp CLI output")
    }
}

/// `n<id>` for notifications and `c<id>` for cards, from Basecamp's numeric id.
fn item_id(prefix: char, id: &str) -> Option<String> {
    ((1..=19).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| format!("{prefix}{id}"))
}

fn remember_url(urls: &mut HashMap<String, String>, id: &str, url: &str) {
    if is_basecamp_url(url) {
        urls.insert(id.to_owned(), url.to_owned());
    }
}

pub fn is_basecamp_url(url: &str) -> bool {
    url.len() <= 512
        && URL_PREFIXES.iter().any(|prefix| url.starts_with(prefix))
        && !url
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

fn notification_kind(kind: &str) -> &'static str {
    match kind {
        "Comment" => "comment",
        "Bulletin" => "announcement",
        "Completion" => "done",
        "BoostReport" => "boost",
        _ => "update",
    }
}

/// Basecamp column colors mapped onto the portal's theme roles.
fn lane_color(color: &str) -> &'static str {
    match color {
        "red" => "red",
        "orange" => "orange",
        "yellow" => "yellow",
        "green" => "green",
        "blue" => "blue",
        "aqua" => "cyan",
        "purple" | "pink" => "magenta",
        _ => "accent",
    }
}

fn bounded(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    // A stand-in CLI that answers each read command with recorded output.
    fn fake_cli(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("basecamp");
        crate::test_support::write_executable(
            &path,
            r#"#!/bin/sh
case "$1 $2" in
  "notifications list")
    echo '{"capped":false,"total":30,"items":[{"id":"11","kind":"Comment","title":"Re: Plan","project":"Produkt","creator":"Kari","unread_at":"2026-10-28T10:00:00Z","url":"https://app.basecamp.com/3120993/buckets/1/todos/11"},{"id":"12","kind":"Bulletin","title":"News","project":"Produkt","creator":"Ola","unread_at":"2026-10-28T09:00:00Z","url":"https://evil.example/12"}]}' ;;
  "cards columns")
    echo '[{"id":"1","title":"Triage","type":"Kanban::Triage","color":"","count":1},{"id":"2","title":"Not now","type":"Kanban::NotNowColumn","color":"","count":11},{"id":"3","title":"Doing","type":"Kanban::Column","color":"purple","count":2},{"id":"4","title":"Done","type":"Kanban::DoneColumn","color":"","count":106}]' ;;
  "cards list")
    echo '[{"id":"31","title":"Fix login","assignees":["Kari",""],"due_on":"2026-11-01","comments":2,"url":"https://3.basecamp.com/3120993/buckets/1/card_tables/cards/31"}]' ;;
  *) exit 1 ;;
esac
"#,
        );
        path
    }

    fn client(bin: PathBuf) -> BasecampClient {
        BasecampClient::new(BasecampConfig {
            bin,
            profile: None,
            card_tables: vec![CardTableConfig {
                label: "Sprint".into(),
                project: 43_666_346,
                card_table: 9_711_341_101,
            }],
        })
    }

    #[tokio::test]
    async fn samples_inbox_and_card_table_lanes() {
        let temp = tempfile::tempdir().unwrap();
        let sample = client(fake_cli(temp.path())).sample().await;
        let snapshot = &sample.snapshot;
        assert!(snapshot.available);
        assert_eq!(snapshot.unread_count, 30);
        assert_eq!(snapshot.notifications[0].kind, "comment");
        assert_eq!(snapshot.notifications[1].kind, "announcement");

        let board = &snapshot.boards[0];
        assert!(board.available);
        assert_eq!((board.not_now, board.done), (11, 106));
        let titles: Vec<_> = board.lanes.iter().map(|lane| lane.title.as_str()).collect();
        assert_eq!(titles, ["Triage", "Doing"]);
        assert_eq!(board.lanes[1].color, "magenta");
        assert_eq!(board.lanes[1].total, 2);
        let card = &board.lanes[0].cards[0];
        assert_eq!(card.assignees, ["Kari"]);
        assert_eq!(card.due_on.as_deref(), Some("2026-11-01"));

        // Only Basecamp URLs are kept, and none are published.
        assert!(sample.urls.contains_key("n11"));
        assert!(sample.urls.contains_key("c31"));
        assert!(!sample.urls.contains_key("n12"));
        assert_eq!(snapshot.notifications[0].id, "n11");
        assert_eq!(board.lanes[0].cards[0].id, "c31");
        let published = serde_json::to_string(snapshot).unwrap();
        assert!(!published.contains("https://"));
    }

    #[tokio::test]
    async fn a_configured_profile_is_passed_to_every_call() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("args.log");
        let bin = temp.path().join("basecamp");
        crate::test_support::write_executable(
            &bin,
            format!("#!/bin/sh\necho \"$@\" >> {}\nexit 1\n", log.display()),
        );
        let mut client = client(bin);
        client.config.profile = Some("jonh".into());
        client.sample().await;
        let calls = fs::read_to_string(&log).unwrap();
        assert!(
            calls.starts_with("--profile jonh notifications list"),
            "{calls}"
        );
    }

    #[tokio::test]
    async fn a_missing_cli_is_unavailable_not_an_error() {
        let sample = client("/nonexistent/basecamp".into()).sample().await;
        assert!(!sample.snapshot.available);
        assert!(sample.urls.is_empty());
    }

    #[test]
    fn only_basecamp_hosts_are_openable() {
        assert!(is_basecamp_url(
            "https://app.basecamp.com/1/buckets/2/todos/3"
        ));
        assert!(is_basecamp_url(
            "https://3.basecamp.com/1/buckets/2/todos/3"
        ));
        assert!(!is_basecamp_url("https://app.basecamp.com.evil.example/"));
        assert!(!is_basecamp_url("http://app.basecamp.com/1"));
        assert!(!is_basecamp_url("https://app.basecamp.com/1 --flag"));
    }
}
