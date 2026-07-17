//! Headless picker models; GPUI dialogs are thin renderers over these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOption {
    pub id: String,
    pub provider: String,
    pub recent: bool,
    pub favorite: bool,
}

pub fn fuzzy_models(models: &[ModelOption], query: &str) -> Vec<ModelOption> {
    let query = query.to_lowercase();
    let mut result: Vec<_> = models
        .iter()
        .filter(|m| {
            query.is_empty()
                || m.id.to_lowercase().contains(&query)
                || m.provider.to_lowercase().contains(&query)
        })
        .cloned()
        .collect();
    result.sort_by_key(|m| (!m.favorite, !m.recent, m.id.clone()));
    result
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentOption {
    pub name: String,
    pub in_tab_cycle: bool,
}

pub fn next_agent(agents: &[AgentOption], current: Option<&str>, reverse: bool) -> Option<String> {
    let choices: Vec<_> = agents.iter().filter(|a| a.in_tab_cycle).collect();
    if choices.is_empty() {
        return None;
    }
    let index = current
        .and_then(|name| choices.iter().position(|a| a.name == name))
        .unwrap_or(if reverse { 0 } else { choices.len() - 1 });
    let next = if reverse {
        (index + choices.len() - 1) % choices.len()
    } else {
        (index + 1) % choices.len()
    };
    Some(choices[next].name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_order_and_agent_tab() {
        let models = vec![
            ModelOption {
                id: "x/slow".into(),
                provider: "x".into(),
                recent: false,
                favorite: false,
            },
            ModelOption {
                id: "x/fast".into(),
                provider: "x".into(),
                recent: true,
                favorite: true,
            },
        ];
        assert_eq!(fuzzy_models(&models, "fast")[0].id, "x/fast");
        let a = vec![
            AgentOption {
                name: "plan".into(),
                in_tab_cycle: true,
            },
            AgentOption {
                name: "rare".into(),
                in_tab_cycle: false,
            },
            AgentOption {
                name: "build".into(),
                in_tab_cycle: true,
            },
        ];
        assert_eq!(next_agent(&a, Some("plan"), false), Some("build".into()));
    }
}
