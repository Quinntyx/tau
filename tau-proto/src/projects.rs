//! Typed project-registry protocol shapes.
use serde::{Deserialize, Deserializer, Serialize};

pub const METHOD_PROJECT_LIST: &str = "project.list";
pub const METHOD_PROJECT_CREATE: &str = "project.create";
pub const METHOD_PROJECT_RENAME: &str = "project.rename";
pub const METHOD_PROJECT_REPATH: &str = "project.repath";
pub const METHOD_PROJECT_UNREGISTER: &str = "project.unregister";
pub const METHOD_PROJECT_REACTIVATE: &str = "project.reactivate";
pub const METHOD_PROJECT_NEW_ID: &str = "project.new_id";

/// The durable project record exposed on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root: String,
    pub active: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProjectListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_inactive: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectListResult {
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectCreateParams {
    pub name: String,
    pub root: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectCreateResult {
    pub project: Project,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRenameParams {
    pub project_id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRepathParams {
    pub project_id: String,
    pub root: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectMutationResult {
    pub project: Project,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectIdParams {
    pub project_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectActionResult {
    pub project: Project,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectNewIdParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectNewIdResult {
    pub project_id: String,
}

/// Reject empty (but otherwise unrestricted) required identifiers.
pub fn deserialize_non_empty<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.is_empty() {
        return Err(serde::de::Error::custom("value must not be empty"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_shapes_round_trip() {
        let project = Project {
            id: "p".into(),
            name: "demo".into(),
            root: "/tmp/demo".into(),
            active: true,
            created_at: 1,
            updated_at: 2,
        };
        let value = serde_json::to_value(ProjectCreateResult {
            project: project.clone(),
        })
        .unwrap();
        assert_eq!(
            serde_json::from_value::<ProjectCreateResult>(value)
                .unwrap()
                .project,
            project
        );
    }

    #[test]
    fn turn_project_id_rejects_empty() {
        let value =
            serde_json::json!({"project_id":"", "model":"m", "prompt":"p", "idempotency_key":"k"});
        assert!(serde_json::from_value::<crate::turn::TurnStartParams>(value).is_err());
    }
}
