//! Stable JSON transport with typed command names and optional request correlation.
use anyhow::{bail, Result};
use serde::Deserialize;
use serde_json::Value;
#[derive(Clone, Copy, Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandName {
    GetState,
    GetConfig,
    History,
    FavoriteHistory,
    DeleteHistory,
    RetryHistory,
    PlayHistory,
    OpenRecordings,
    StartRecording,
    StopRecording,
    AbortRecording,
    ToggleListening,
    SetListening,
    Cancel,
    ClearError,
    SaveConfig,
    ReloadConfig,
    SetLogLevel,
    CaptureHotkey,
    TestMicrophone,
    Diagnostics,
    ExportDiagnostics,
    Quit,
    Benchmark,
    CancelBenchmark,
    BenchmarkStatus,
}
#[derive(Deserialize)]
pub struct Request {
    pub cmd: CommandName,
    #[serde(default)]
    pub request_id: Option<u64>,
    #[serde(default)]
    pub client_session: Option<String>,
}
impl Request {
    pub fn parse(value: &Value) -> Result<Self> {
        let request: Self = serde_json::from_value(value.clone())
            .map_err(|e| anyhow::anyhow!("Invalid speech service request: {e}"))?;
        if request
            .client_session
            .as_ref()
            .is_some_and(|s| s.len() > 128 || s.chars().any(char::is_control))
        {
            bail!("Invalid client session");
        }
        match request.cmd {
            CommandName::FavoriteHistory
            | CommandName::DeleteHistory
            | CommandName::RetryHistory
            | CommandName::PlayHistory => {
                if value["id"].as_i64().is_none_or(|id| id <= 0) {
                    bail!("Expected a positive history ID");
                }
            }
            _ => {}
        }
        if matches!(request.cmd, CommandName::SetListening) && !value["value"].is_boolean() {
            bail!("Expected a boolean");
        }
        if matches!(request.cmd, CommandName::FavoriteHistory) && !value["favorite"].is_boolean() {
            bail!("Expected favorite boolean");
        }
        if matches!(request.cmd, CommandName::SaveConfig) && !value["config"].is_object() {
            bail!("Expected a settings object");
        }
        Ok(request)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn rejects_malformed_commands_and_ids() {
        for value in [
            json!({
            "cmd":"typo"}
            ),
            json!({
            "cmd":"delete_history","id":false}
            ),
            json!({
            "cmd":"retry_history","id":-1}
            ),
            json!({
            "cmd":"get_state","request_id":"one"}
            ),
        ] {
            assert!(Request::parse(&value).is_err());
        }
        let request = Request::parse(&json!({
        "cmd":"get_state","request_id":42}
        ))
        .unwrap();
        assert_eq!(request.request_id, Some(42));
    }
}
