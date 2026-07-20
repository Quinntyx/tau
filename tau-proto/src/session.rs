//! Typed session navigator RPC messages.

use serde::{Deserialize, Deserializer, Serialize};

pub const METHOD_SESSION_CREATE: &str = "session.create";
pub const METHOD_SESSION_LIST: &str = "session.list";
pub const METHOD_SESSION_GET_HISTORY: &str = "session.get_history";
pub const METHOD_SESSION_RENAME: &str = "session.rename";
pub const METHOD_SESSION_ARCHIVE: &str = "session.archive";
pub const METHOD_SESSION_RESTORE: &str = "session.restore";

/// Identifiers are allocated by the project registry and deliberately opaque
/// to the session service.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(#[serde(deserialize_with = "deserialize_non_empty")] String);

impl ProjectId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(#[serde(deserialize_with = "deserialize_non_empty")] String);

impl SessionId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateParams {
    pub project_id: ProjectId,
    #[serde(deserialize_with = "deserialize_non_empty")]
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListParams {
    pub project_id: ProjectId,
    #[serde(default)]
    pub include_archived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryParams {
    pub project_id: ProjectId,
    pub session_id: SessionId,
    #[serde(default)]
    pub after_sequence: u64,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionIdParams {
    pub project_id: ProjectId,
    pub session_id: SessionId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameParams {
    pub project_id: ProjectId,
    pub session_id: SessionId,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: SessionId,
    pub project_id: ProjectId,
    pub cwd: String,
    pub title: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub archived_at: Option<i64>,
}

fn deserialize_non_empty<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.is_empty() {
        Err(serde::de::Error::custom("value must not be empty"))
    } else {
        Ok(value)
    }
}

pub type CreateResult = SessionRecord;
pub type ListResult = Vec<SessionRecord>;
pub type HistoryResult = Vec<crate::turn::SequencedEvent>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_project_and_session_ids_round_trip_without_registry_knowledge() {
        let params = RenameParams {
            project_id: ProjectId::new("opaque-project"),
            session_id: SessionId::new("opaque-session"),
            title: "Renamed".into(),
        };
        let encoded = serde_json::to_value(&params).unwrap();
        assert_eq!(encoded["project_id"], "opaque-project");
        let decoded: RenameParams = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded.project_id.as_str(), "opaque-project");
        assert_eq!(decoded.session_id.as_str(), "opaque-session");
    }

    #[test]
    fn required_ids_and_cwd_reject_empty_wire_values() {
        assert!(serde_json::from_value::<ProjectId>(serde_json::json!("")).is_err());
        assert!(serde_json::from_value::<SessionId>(serde_json::json!("")).is_err());
        assert!(
            serde_json::from_value::<CreateParams>(serde_json::json!({
                "project_id": "p", "cwd": ""
            }))
            .is_err()
        );
    }
}
