use std::{collections::HashMap, process::Stdio, time::Duration};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::{process::Command, time::timeout};

use crate::{
    config::GithubConfig,
    model::{GithubNotification, GithubPullRequest, GithubSnapshot},
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PULL_REQUESTS: usize = 30;
const MAX_NOTIFICATIONS: usize = 12;
const MAX_TEXT_CHARS: usize = 160;
const MAX_NAME_CHARS: usize = 64;
const WEB_PREFIX: &str = "https://github.com/";
const API_REPOS_PREFIX: &str = "https://api.github.com/repos/";

// gh's built-in --jq keeps only what the dashboard shows plus the URL the
// daemon keeps to itself.
const PULL_REQUESTS_JQ: &str = r#"{total: (.total_count // 0), items: [.items[] | {id: (.id | tostring), number: (.number // 0), title: (.title // ""), author: (.user.login // ""), draft: (.draft // false), comments: (.comments // 0), at: (.updated_at // ""), repo: ((.repository_url // "") | split("/") | .[-1]), url: (.html_url // "")}]}"#;
const NOTIFICATIONS_JQ: &str = r#"[.[] | {id: (.id | tostring), reason: (.reason // ""), at: (.updated_at // ""), title: (.subject.title // ""), type: (.subject.type // ""), url: (.subject.url // ""), repo: (.repository.full_name // ""), repo_url: (.repository.html_url // "")}]"#;

#[derive(Debug, Deserialize)]
struct PullRequestSearch {
    total: u32,
    items: Vec<PullRequestRow>,
}

#[derive(Debug, Deserialize)]
struct PullRequestRow {
    id: String,
    number: u32,
    title: String,
    author: String,
    draft: bool,
    comments: u32,
    at: String,
    repo: String,
    url: String,
}

#[derive(Debug, Deserialize)]
struct NotificationRow {
    id: String,
    reason: String,
    at: String,
    title: String,
    #[serde(rename = "type")]
    kind: String,
    url: Option<String>,
    repo: String,
    repo_url: String,
}

/// One GitHub sample: the published snapshot plus the github.com URLs the
/// daemon keeps private and opens by id.
#[derive(Debug, Default)]
pub struct GithubSample {
    pub snapshot: GithubSnapshot,
    pub urls: HashMap<String, String>,
}

/// Reads recent organization commits and unread notifications through the
/// user's signed-in `gh` CLI. Only fixed read-only `gh api` calls are made;
/// a configured account's token reaches `gh` through its environment.
#[derive(Debug, Clone)]
pub struct GithubClient {
    config: GithubConfig,
}

impl GithubClient {
    pub fn new(config: GithubConfig) -> Self {
        Self { config }
    }

    pub async fn sample(&self) -> GithubSample {
        let mut sample = GithubSample::default();
        sample.snapshot.organization = self.config.organization.clone();
        let token = match &self.config.user {
            Some(user) => match self.token(user).await {
                Ok(token) => Some(token),
                Err(error) => {
                    tracing::debug!(%error, "GitHub account unavailable");
                    return sample;
                }
            },
            None => None,
        };
        let token = token.as_deref();

        let notifications: Vec<NotificationRow> = match self
            .api(token, "notifications?per_page=30", NOTIFICATIONS_JQ)
            .await
        {
            Ok(rows) => rows,
            Err(error) => {
                tracing::debug!(%error, "GitHub notifications unavailable");
                return sample;
            }
        };
        sample.snapshot.available = true;
        for row in notifications.into_iter().take(MAX_NOTIFICATIONS) {
            if !row.id.bytes().all(|byte| byte.is_ascii_digit()) || row.id.len() > 20 {
                continue;
            }
            let id = format!("n{}", row.id);
            if let Some(url) = notification_url(row.url.as_deref(), &row.kind, &row.repo_url) {
                sample.urls.insert(id.clone(), url);
            }
            sample.snapshot.notifications.push(GithubNotification {
                id,
                kind: notification_kind(&row.kind).into(),
                title: bounded(&row.title, MAX_TEXT_CHARS),
                repo: bounded(&row.repo, MAX_NAME_CHARS * 2),
                reason: bounded(&row.reason, 32),
                at: unix_seconds(&row.at),
            });
        }

        let query = format!(
            "q=org:{} is:pr is:open archived:false",
            self.config.organization
        );
        let per_page = format!("per_page={MAX_PULL_REQUESTS}");
        let search: PullRequestSearch = match self
            .api_with(
                token,
                &[
                    "-X",
                    "GET",
                    "search/issues",
                    "-f",
                    &query,
                    "-f",
                    "sort=updated",
                    "-f",
                    "order=desc",
                    "-f",
                    &per_page,
                ],
                PULL_REQUESTS_JQ,
            )
            .await
        {
            Ok(search) => search,
            Err(error) => {
                tracing::debug!(%error, organization = self.config.organization, "GitHub pull requests unavailable");
                return sample;
            }
        };
        sample.snapshot.open_pull_requests = search.total;
        for row in search.items.into_iter().take(MAX_PULL_REQUESTS) {
            if row.id.is_empty()
                || row.id.len() > 20
                || !row.id.bytes().all(|byte| byte.is_ascii_digit())
            {
                continue;
            }
            let id = format!("p{}", row.id);
            if is_github_url(&row.url) {
                sample.urls.insert(id.clone(), row.url.clone());
            }
            sample.snapshot.pull_requests.push(GithubPullRequest {
                id,
                repo: bounded(&row.repo, MAX_NAME_CHARS),
                number: row.number,
                title: bounded(&row.title, MAX_TEXT_CHARS),
                author: bounded(&row.author, MAX_NAME_CHARS),
                draft: row.draft,
                comments: row.comments.min(9_999),
                at: unix_seconds(&row.at),
            });
        }
        sample
    }

    async fn token(&self, user: &str) -> Result<String> {
        let output = self
            .run(
                None,
                &["auth", "token", "--hostname", "github.com", "--user", user],
            )
            .await?;
        let token = String::from_utf8(output).context("gh token was not UTF-8")?;
        let token = token.trim();
        if token.is_empty() || token.chars().any(char::is_whitespace) {
            bail!("gh returned no token for {user}");
        }
        Ok(token.to_owned())
    }

    async fn api<T: for<'de> Deserialize<'de>>(
        &self,
        token: Option<&str>,
        path: &str,
        jq: &str,
    ) -> Result<T> {
        self.api_with(token, &[path], jq).await
    }

    /// `gh api <args> --jq <jq>` with fixed arguments chosen by this module.
    async fn api_with<T: for<'de> Deserialize<'de>>(
        &self,
        token: Option<&str>,
        args: &[&str],
        jq: &str,
    ) -> Result<T> {
        let mut command = vec!["api"];
        command.extend_from_slice(args);
        command.extend_from_slice(&["--jq", jq]);
        let output = self.run(token, &command).await?;
        serde_json::from_slice(&output).context("parsing gh api output")
    }

    async fn run(&self, token: Option<&str>, args: &[&str]) -> Result<Vec<u8>> {
        let mut command = Command::new(&self.config.bin);
        command
            .kill_on_drop(true)
            .args(args)
            .env("GH_PROMPT_DISABLED", "1")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(token) = token {
            command.env("GH_TOKEN", token);
        }
        let output = timeout(COMMAND_TIMEOUT, command.output())
            .await
            .context("timed out waiting for gh")?
            .with_context(|| format!("running {}", self.config.bin.display()))?;
        if !output.status.success() {
            bail!(
                "gh failed: {}",
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(200)
                    .collect::<String>()
            );
        }
        Ok(output.stdout)
    }
}

/// The github.com page for a notification: the pull request, issue, or
/// commit it is about, the releases page, or else the repository.
fn notification_url(api_url: Option<&str>, kind: &str, repo_url: &str) -> Option<String> {
    let repo_url = repo_url.trim_end_matches('/');
    let page = api_url
        .and_then(|url| url.strip_prefix(API_REPOS_PREFIX))
        .and_then(|path| {
            let mut parts = path.splitn(4, '/');
            let (owner, name, section, rest) =
                (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
            let section = match (kind, section) {
                ("PullRequest", "pulls") => "pull",
                ("Issue", "issues") | ("Discussion", "discussions") => section,
                ("Commit", "commits") => "commit",
                ("Release", "releases") => {
                    return Some(format!("{WEB_PREFIX}{owner}/{name}/releases"));
                }
                _ => return None,
            };
            Some(format!("{WEB_PREFIX}{owner}/{name}/{section}/{rest}"))
        })
        .unwrap_or_else(|| repo_url.to_owned());
    is_github_url(&page).then_some(page)
}

fn notification_kind(kind: &str) -> &'static str {
    match kind {
        "PullRequest" => "pr",
        "Issue" => "issue",
        "Release" => "release",
        "Commit" => "commit",
        "CheckSuite" | "WorkflowRun" => "ci",
        "Discussion" => "discussion",
        _ => "other",
    }
}

/// Only github.com pages are ever opened.
pub fn is_github_url(url: &str) -> bool {
    url.len() <= 512
        && url.starts_with(WEB_PREFIX)
        && url.len() > WEB_PREFIX.len()
        && !url
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

fn unix_seconds(timestamp: &str) -> u64 {
    OffsetDateTime::parse(timestamp, &Rfc3339)
        .map(|at| at.unix_timestamp().max(0) as u64)
        .unwrap_or(0)
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
    use std::path::PathBuf;

    use super::*;

    // A stand-in gh that answers each fixed call with recorded output and
    // checks that a configured account's token arrives in the environment.
    fn fake_gh(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("gh");
        crate::test_support::write_executable(
            &path,
            r#"#!/bin/sh
case "$*" in *gho_work*) exit 9 ;; esac
case "$1 $2" in
  "auth token") echo "gho_work_token" ; exit 0 ;;
esac
[ "$GH_TOKEN" = "gho_work_token" ] || exit 8
case "$2" in
  notifications*) echo '[{"id":"123","reason":"review_requested","at":"2026-10-28T10:00:00Z","title":"Fix login","type":"PullRequest","url":"https://api.github.com/repos/favo/app/pulls/42","repo":"favo/app","repo_url":"https://github.com/favo/app"},{"id":"124","reason":"ci_activity","at":"2026-10-28T09:00:00Z","title":"CI failed","type":"CheckSuite","url":null,"repo":"favo/app","repo_url":"https://github.com/favo/app"}]' ;;
  -X) case "$*" in *"q=org:favo is:pr is:open archived:false"*) echo '{"total":44,"items":[{"id":"9001","number":42,"title":"Fix login","author":"kari","draft":false,"comments":3,"at":"2026-10-28T13:00:00Z","repo":"app","url":"https://github.com/favo/app/pull/42"},{"id":"9002","number":7,"title":"WIP header","author":"ola","draft":true,"comments":0,"at":"2026-10-28T12:00:00Z","repo":"web","url":"https://evil.example/7"}]}' ;; *) exit 1 ;; esac ;;
  *) exit 1 ;;
esac
"#,
        );
        path
    }

    #[tokio::test]
    async fn samples_org_commits_and_notifications_as_the_configured_account() {
        let temp = tempfile::tempdir().unwrap();
        let sample = GithubClient::new(GithubConfig {
            bin: fake_gh(temp.path()),
            organization: "favo".into(),
            user: Some("jon-work".into()),
        })
        .sample()
        .await;
        let snapshot = &sample.snapshot;
        assert!(snapshot.available);
        assert_eq!(snapshot.open_pull_requests, 44);
        let pulls: Vec<_> = snapshot
            .pull_requests
            .iter()
            .map(|pull| (pull.repo.as_str(), pull.number, pull.draft))
            .collect();
        assert_eq!(pulls, [("app", 42, false), ("web", 7, true)]);
        assert_eq!(sample.urls["p9001"], "https://github.com/favo/app/pull/42");
        // A non-github.com link is never kept.
        assert!(!sample.urls.contains_key("p9002"));
        let kinds: Vec<_> = snapshot
            .notifications
            .iter()
            .map(|n| n.kind.as_str())
            .collect();
        assert_eq!(kinds, ["pr", "ci"]);
        assert_eq!(sample.urls["n123"], "https://github.com/favo/app/pull/42");
        assert_eq!(sample.urls["n124"], "https://github.com/favo/app");
        let published = serde_json::to_string(snapshot).unwrap();
        assert!(!published.contains("https://") && !published.contains("gho_"));
    }

    #[tokio::test]
    async fn a_missing_cli_is_unavailable_not_an_error() {
        let sample = GithubClient::new(GithubConfig {
            bin: "/nonexistent/gh".into(),
            organization: "favo".into(),
            user: None,
        })
        .sample()
        .await;
        assert!(!sample.snapshot.available);
    }

    #[test]
    fn notification_links_and_hosts_are_checked() {
        let repo = "https://github.com/o/r";
        assert_eq!(
            notification_url(
                Some("https://api.github.com/repos/o/r/issues/7"),
                "Issue",
                repo
            )
            .as_deref(),
            Some("https://github.com/o/r/issues/7")
        );
        assert_eq!(
            notification_url(
                Some("https://api.github.com/repos/o/r/releases/9"),
                "Release",
                repo
            )
            .as_deref(),
            Some("https://github.com/o/r/releases")
        );
        assert_eq!(
            notification_url(Some("https://evil.example/x"), "Issue", repo).as_deref(),
            Some(repo)
        );
        assert!(!is_github_url("https://github.com.evil.example/x"));
        assert!(!is_github_url("http://github.com/x"));
        assert!(!is_github_url("https://github.com/x y"));
    }
}
