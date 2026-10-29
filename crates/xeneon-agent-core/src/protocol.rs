use thiserror::Error;

use crate::model::{ActionKind, AgentView, PortalCommand, SCHEMA_VERSION};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CommandError {
    #[error("unsupported portal schema version {0}")]
    UnsupportedSchema(u32),
    #[error("request_id is required")]
    MissingRequestId,
    #[error("agent_id is required for this action")]
    MissingAgentId,
    #[error("agent_id is not accepted for this action")]
    UnexpectedAgentId,
    #[error("capability_id is required for guarded actions")]
    MissingCapability,
    #[error("capability_id is not accepted for this action")]
    UnexpectedCapability,
    #[error("app_id is required for desktop_app")]
    MissingAppId,
    #[error("app_id is not accepted for this action")]
    UnexpectedAppId,
    #[error("a published item_id is required for basecamp_open and slack_open")]
    MissingItemId,
    #[error("item_id is not accepted for this action")]
    UnexpectedItemId,
}

pub fn validate_command(command: &PortalCommand) -> Result<(), CommandError> {
    if command.schema_version != SCHEMA_VERSION {
        return Err(CommandError::UnsupportedSchema(command.schema_version));
    }
    if command.request_id.trim().is_empty() {
        return Err(CommandError::MissingRequestId);
    }

    match (command.action, command.app_id.as_deref()) {
        (ActionKind::DesktopApp, None) => return Err(CommandError::MissingAppId),
        (ActionKind::DesktopApp, Some(id)) if id.trim().is_empty() => {
            return Err(CommandError::MissingAppId);
        }
        (ActionKind::DesktopApp, Some(_)) | (_, None) => {}
        (_, Some(_)) => return Err(CommandError::UnexpectedAppId),
    }

    match (command.action, command.item_id.as_deref()) {
        (ActionKind::BasecampOpen, Some(id)) if is_basecamp_item_id(id) => {}
        (ActionKind::BasecampOpen, _) => return Err(CommandError::MissingItemId),
        (ActionKind::SlackOpen, Some(id)) if is_slack_item_id(id) => {}
        (ActionKind::SlackOpen, _) => return Err(CommandError::MissingItemId),
        (ActionKind::GithubOpen, Some(id)) if is_github_item_id(id) => {}
        (ActionKind::GithubOpen, _) => return Err(CommandError::MissingItemId),
        (_, None) => {}
        (_, Some(_)) => return Err(CommandError::UnexpectedItemId),
    }

    match command.action {
        ActionKind::RestoreFocus
        | ActionKind::DesktopApp
        | ActionKind::SpotifyPlayPause
        | ActionKind::SpotifyNext
        | ActionKind::SpotifyPrevious
        | ActionKind::BasecampOpen
        | ActionKind::SlackOpen
        | ActionKind::GithubOpen
        | ActionKind::GameShow
        | ActionKind::VoiceStart
        | ActionKind::VoiceStop
        | ActionKind::VoiceCancel
        | ActionKind::OrderGrouped
        | ActionKind::OrderPriority
        | ActionKind::BackendHerdr
        | ActionKind::BackendT3code => {
            if command.agent_id.is_some() {
                return Err(CommandError::UnexpectedAgentId);
            }
            if command.capability_id.is_some() {
                return Err(CommandError::UnexpectedCapability);
            }
        }
        ActionKind::Open | ActionKind::Zoom => {
            if command.agent_id.is_none() {
                return Err(CommandError::MissingAgentId);
            }
            if command.capability_id.is_some() {
                return Err(CommandError::UnexpectedCapability);
            }
        }
        ActionKind::Approve | ActionKind::Interrupt => {
            if command.agent_id.is_none() {
                return Err(CommandError::MissingAgentId);
            }
            if command.capability_id.is_none() {
                return Err(CommandError::MissingCapability);
            }
        }
    }

    Ok(())
}

/// Basecamp items are `n<notification id>` or `c<card id>`.
fn is_basecamp_item_id(id: &str) -> bool {
    match id.split_at_checked(1) {
        Some(("n" | "c", number)) => {
            (1..=19).contains(&number.len()) && number.bytes().all(|byte| byte.is_ascii_digit())
        }
        _ => false,
    }
}

/// GitHub items are `p<pull request id>` or `n<notification id>`.
fn is_github_item_id(id: &str) -> bool {
    match id.split_at_checked(1) {
        Some(("p" | "n", number)) => {
            (1..=20).contains(&number.len()) && number.bytes().all(|byte| byte.is_ascii_digit())
        }
        _ => false,
    }
}

/// Slack items are `<conversation id>-<message ts>`, e.g. `C123-1700000000.000100`.
fn is_slack_item_id(id: &str) -> bool {
    let Some((conversation, ts)) = id.split_once('-') else {
        return false;
    };
    let Some((seconds, micros)) = ts.split_once('.') else {
        return false;
    };
    (1..=24).contains(&conversation.len())
        && conversation
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        && (1..=12).contains(&seconds.len())
        && (1..=8).contains(&micros.len())
        && seconds
            .bytes()
            .chain(micros.bytes())
            .all(|byte| byte.is_ascii_digit())
}

pub fn command_capability_matches(command: &PortalCommand, agent: &AgentView) -> bool {
    let expected = match command.action {
        ActionKind::Approve => agent.actions.approve.as_ref(),
        ActionKind::Interrupt => agent.actions.interrupt.as_ref(),
        _ => return true,
    };

    expected
        .zip(command.capability_id.as_ref())
        .is_some_and(|(capability, supplied)| capability.capability_id == *supplied)
}

#[cfg(test)]
mod tests {
    use crate::model::{AgentActions, AgentStatus};

    use super::*;

    fn command(action: ActionKind) -> PortalCommand {
        PortalCommand {
            schema_version: SCHEMA_VERSION,
            request_id: "request".into(),
            sequence: 7,
            agent_id: Some("agent".into()),
            action,
            capability_id: None,
            app_id: None,
            item_id: None,
        }
    }

    #[test]
    fn raw_approve_without_capability_is_rejected() {
        assert_eq!(
            validate_command(&command(ActionKind::Approve)),
            Err(CommandError::MissingCapability)
        );
    }

    #[test]
    fn restore_focus_does_not_require_agent() {
        let mut command = command(ActionKind::RestoreFocus);
        command.agent_id = None;
        assert_eq!(validate_command(&command), Ok(()));
    }

    #[test]
    fn voice_actions_are_typed_global_actions() {
        let mut command = command(ActionKind::VoiceStart);
        assert_eq!(
            validate_command(&command),
            Err(CommandError::UnexpectedAgentId)
        );
        command.agent_id = None;
        assert_eq!(validate_command(&command), Ok(()));
        command.capability_id = Some("not-accepted".into());
        assert_eq!(
            validate_command(&command),
            Err(CommandError::UnexpectedCapability)
        );
    }

    #[test]
    fn desktop_actions_are_typed_global_actions() {
        let mut command = command(ActionKind::DesktopApp);
        command.agent_id = None;
        assert_eq!(validate_command(&command), Err(CommandError::MissingAppId));
        command.app_id = Some("  ".into());
        assert_eq!(validate_command(&command), Err(CommandError::MissingAppId));
        command.app_id = Some("spotify".into());
        assert_eq!(validate_command(&command), Ok(()));
        command.agent_id = Some("agent".into());
        assert_eq!(
            validate_command(&command),
            Err(CommandError::UnexpectedAgentId)
        );
        command.agent_id = None;
        command.capability_id = Some("not-accepted".into());
        assert_eq!(
            validate_command(&command),
            Err(CommandError::UnexpectedCapability)
        );
    }

    #[test]
    fn basecamp_open_takes_only_a_prefixed_numeric_item_id() {
        let mut command = command(ActionKind::BasecampOpen);
        command.agent_id = None;
        assert_eq!(validate_command(&command), Err(CommandError::MissingItemId));
        for bad in ["", "12", "n12a", "x12", "c../1", "c12345678901234567890"] {
            command.item_id = Some(bad.into());
            assert_eq!(validate_command(&command), Err(CommandError::MissingItemId));
        }
        command.item_id = Some("c9711341101".into());
        assert_eq!(validate_command(&command), Ok(()));

        let mut other = command.clone();
        other.action = ActionKind::VoiceStart;
        assert_eq!(
            validate_command(&other),
            Err(CommandError::UnexpectedItemId)
        );
    }

    #[test]
    fn slack_open_takes_only_a_conversation_and_ts_item_id() {
        let mut command = command(ActionKind::SlackOpen);
        command.agent_id = None;
        assert_eq!(validate_command(&command), Err(CommandError::MissingItemId));
        for bad in ["123", "c1-1.2", "C1-1", "C1-1.2.3", "C1-1.2 x", "-1.2"] {
            command.item_id = Some(bad.into());
            assert_eq!(
                validate_command(&command),
                Err(CommandError::MissingItemId),
                "{bad}"
            );
        }
        command.item_id = Some("CDEV1-1700000200.000300".into());
        assert_eq!(validate_command(&command), Ok(()));
    }

    #[test]
    fn github_open_takes_only_a_commit_or_notification_id() {
        let mut command = command(ActionKind::GithubOpen);
        command.agent_id = None;
        for bad in ["", "caaaaaaa1", "p", "n12a", "x123", "p123 ", "p-1"] {
            command.item_id = Some(bad.into());
            assert_eq!(
                validate_command(&command),
                Err(CommandError::MissingItemId),
                "{bad}"
            );
        }
        for good in ["p9001", "n123"] {
            command.item_id = Some(good.into());
            assert_eq!(validate_command(&command), Ok(()));
        }
    }

    #[test]
    fn app_id_is_rejected_outside_desktop_app() {
        let mut command = command(ActionKind::VoiceStart);
        command.agent_id = None;
        command.app_id = Some("spotify".into());
        assert_eq!(
            validate_command(&command),
            Err(CommandError::UnexpectedAppId)
        );
    }

    #[test]
    fn order_actions_are_typed_global_actions() {
        for action in [ActionKind::OrderGrouped, ActionKind::OrderPriority] {
            let mut command = command(action);
            assert_eq!(
                validate_command(&command),
                Err(CommandError::UnexpectedAgentId)
            );
            command.agent_id = None;
            assert_eq!(validate_command(&command), Ok(()));
            command.capability_id = Some("not-accepted".into());
            assert_eq!(
                validate_command(&command),
                Err(CommandError::UnexpectedCapability)
            );
        }
    }

    #[test]
    fn backend_actions_are_typed_global_actions() {
        for action in [ActionKind::BackendHerdr, ActionKind::BackendT3code] {
            let mut command = command(action);
            assert_eq!(
                validate_command(&command),
                Err(CommandError::UnexpectedAgentId)
            );
            command.agent_id = None;
            assert_eq!(validate_command(&command), Ok(()));
            command.capability_id = Some("not-accepted".into());
            assert_eq!(
                validate_command(&command),
                Err(CommandError::UnexpectedCapability)
            );
        }
    }

    #[test]
    fn capability_must_match_current_agent_snapshot() {
        let mut command = command(ActionKind::Approve);
        command.capability_id = Some("stale".into());
        let agent = AgentView {
            id: "agent".into(),
            display_name: "Agent".into(),
            agent: "codex".into(),
            status: AgentStatus::Blocked,
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
            source_order: 0,
            actions: AgentActions {
                open: true,
                zoom: true,
                approve: Some(crate::model::ActionCapability {
                    capability_id: "current".into(),
                    kind: ActionKind::Approve,
                    expires_at_ms: u64::MAX,
                    expected_revision: 2,
                }),
                interrupt: None,
            },
        };

        assert!(!command_capability_matches(&command, &agent));
    }
}
