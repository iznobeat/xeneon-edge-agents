pub mod basecamp;
pub mod config;
pub mod desktop;
pub mod github;
pub mod health;
pub mod herdr;
pub mod lyrics;
pub mod micro;
pub mod model;
pub mod protocol;
pub mod runtime;
pub mod slack;
pub mod spotify;
pub mod t3code;
pub mod usage;
pub mod voice;

pub use config::Config;
pub use model::{
    ActionCapability, ActionKind, ActionResult, AgentActions, AgentBackend, AgentBackendSnapshot,
    AgentOrderMode, AgentOrderSnapshot, AgentStatus, AgentView, AiUsageSnapshot, ConnectionState,
    HealthSnapshot, Metric, MicroSnapshot, PortalCommand, PortalSnapshot, ProviderUsage,
    ServerMessage, SessionState, SessionView, UsageKind, UsageWindow, VoiceSnapshot, VoiceState,
};
pub use runtime::{DaemonRuntime, default_backend_state_path, socket_path};
pub use voice::cleanup_owned_dictation;

#[cfg(test)]
mod test_support {
    use std::{
        io::Write,
        path::Path,
        process::{Command, Stdio},
    };

    /// Writes an executable test stub from a child process. A write fd held
    /// by the test process could leak into a concurrent test's fork and fail
    /// the stub's exec with ETXTBSY.
    pub fn write_executable(path: &Path, contents: impl AsRef<[u8]>) {
        let mut writer = Command::new("sh")
            .args(["-c", "cat >\"$1\" && chmod 700 \"$1\"", "sh"])
            .arg(path)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        writer
            .stdin
            .take()
            .unwrap()
            .write_all(contents.as_ref())
            .unwrap();
        assert!(writer.wait().unwrap().success());
    }
}
