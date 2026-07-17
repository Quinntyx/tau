//! Headless picker and command models; GPUI dialogs are thin renderers over these.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOption {
    pub id: String,
    pub provider: String,
    pub recent: bool,
    pub favorite: bool,
}

impl ModelOption {
    pub fn provider_label(&self) -> String {
        self.provider.clone()
    }
}

/// State shared by the model overlay and its renderer.  Keeping the query and
/// cursor here makes keyboard and mouse views use exactly the same ordering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelPicker {
    pub models: Vec<ModelOption>,
    pub query: String,
    pub selected: usize,
}

impl ModelPicker {
    pub fn new(models: Vec<ModelOption>) -> Self {
        Self {
            models,
            ..Self::default()
        }
    }
    pub fn results(&self) -> Vec<ModelOption> {
        fuzzy_models(&self.models, &self.query)
    }
    pub fn move_selection(&mut self, delta: isize) {
        let count = self.results().len();
        if count > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(count as isize) as usize;
        } else {
            self.selected = 0;
        }
    }
    pub fn selected_model(&self) -> Option<ModelOption> {
        self.results().into_iter().nth(self.selected)
    }
    pub fn toggle_favorite(&mut self, id: &str) -> bool {
        if let Some(model) = self.models.iter_mut().find(|m| m.id == id) {
            model.favorite = !model.favorite;
            true
        } else {
            false
        }
    }
    pub fn mark_recent(&mut self, id: &str) -> bool {
        if let Some(model) = self.models.iter_mut().find(|m| m.id == id) {
            model.recent = true;
            true
        } else {
            false
        }
    }
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
    result.sort_by(|a, b| {
        (
            !a.favorite,
            !a.recent,
            a.provider.to_lowercase(),
            a.id.to_lowercase(),
        )
            .cmp(&(
                !b.favorite,
                !b.recent,
                b.provider.to_lowercase(),
                b.id.to_lowercase(),
            ))
    });
    result
}

pub fn model_groups(models: &[ModelOption], query: &str) -> Vec<(String, Vec<ModelOption>)> {
    let mut groups: Vec<(String, Vec<ModelOption>)> = Vec::new();
    for model in fuzzy_models(models, query) {
        if let Some((_, entries)) = groups.iter_mut().find(|(p, _)| p == &model.provider) {
            entries.push(model);
        } else {
            groups.push((model.provider.clone(), vec![model]));
        }
    }
    groups
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentOption {
    pub name: String,
    pub in_tab_cycle: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentPicker {
    pub agents: Vec<AgentOption>,
    pub query: String,
    pub selected: usize,
}

impl AgentPicker {
    pub fn new(agents: Vec<AgentOption>) -> Self {
        Self {
            agents,
            ..Self::default()
        }
    }
    pub fn results(&self) -> Vec<AgentOption> {
        search_agents(&self.agents, &self.query)
    }
    pub fn move_selection(&mut self, delta: isize) {
        let count = self.results().len();
        if count > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(count as isize) as usize;
        } else {
            self.selected = 0;
        }
    }
    pub fn selected_agent(&self) -> Option<AgentOption> {
        self.results().into_iter().nth(self.selected)
    }
    pub fn cycle(&self, current: Option<&str>, reverse: bool) -> Option<String> {
        next_agent(&self.agents, current, reverse)
    }
}

/// Move a cursor in a finite list, wrapping in either direction.
pub fn move_selection(selected: usize, count: usize, delta: isize) -> usize {
    if count == 0 {
        0
    } else {
        (selected.min(count - 1) as isize + delta).rem_euclid(count as isize) as usize
    }
}

pub fn search_agents(agents: &[AgentOption], query: &str) -> Vec<AgentOption> {
    let q = query.to_lowercase();
    agents
        .iter()
        .filter(|a| q.is_empty() || a.name.to_lowercase().contains(&q))
        .cloned()
        .collect()
}

pub fn next_agent(agents: &[AgentOption], current: Option<&str>, reverse: bool) -> Option<String> {
    let choices: Vec<_> = agents.iter().filter(|a| a.in_tab_cycle).collect();
    if choices.is_empty() {
        return None;
    }
    let index = current
        .and_then(|name| {
            choices
                .iter()
                .position(|a| a.name.eq_ignore_ascii_case(name))
        })
        .unwrap_or(if reverse { 0 } else { choices.len() - 1 });
    let next = if reverse {
        (index + choices.len() - 1) % choices.len()
    } else {
        (index + 1) % choices.len()
    };
    Some(choices[next].name.clone())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Agent(Option<String>),
    Agents,
    Model(Option<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerAction {
    SelectModel(String),
    SelectAgent(String),
    OpenAgents,
    OpenModels,
    Dismiss,
}

pub fn parse_command(input: &str) -> Option<Command> {
    let mut words = input.split_whitespace();
    let command = words.next()?.to_lowercase();
    let arg = words.next().map(str::to_owned);
    if words.next().is_some() {
        return None;
    }
    match command.as_str() {
        "/agent" => Some(Command::Agent(arg)),
        "/agents" if arg.is_none() => Some(Command::Agents),
        "/model" => Some(Command::Model(arg)),
        _ => None,
    }
}

/// Returns the command prefix being completed and its argument prefix.
pub fn command_parts(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim_start();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let command = parts.next()?;
    let argument = parts.next().unwrap_or("").trim_start();
    Some((command, argument))
}

pub fn command_suggestions(
    input: &str,
    agents: &[AgentOption],
    models: &[ModelOption],
) -> Vec<String> {
    let trimmed = input.trim_start();
    let words: Vec<_> = trimmed.split_whitespace().collect();
    let has_argument = words.len() > 1 || trimmed.chars().last().is_some_and(char::is_whitespace);
    if !has_argument {
        let prefix = words.first().copied().unwrap_or("").to_lowercase();
        return ["/agent", "/agents", "/model"]
            .into_iter()
            .filter(|x| x.starts_with(&prefix))
            .map(str::to_owned)
            .collect();
    }
    let prefix = if trimmed.chars().last().is_some_and(char::is_whitespace) {
        String::new()
    } else {
        words.last().unwrap_or(&"").to_lowercase()
    };
    match words[0].to_lowercase().as_str() {
        "/agent" => search_agents(agents, &prefix)
            .into_iter()
            .map(|a| a.name)
            .collect(),
        "/model" => fuzzy_models(models, &prefix)
            .into_iter()
            .map(|m| m.id)
            .collect(),
        _ => Vec::new(),
    }
}

pub fn command_action(command: Command) -> Option<PickerAction> {
    match command {
        Command::Agent(Some(name)) => Some(PickerAction::SelectAgent(name)),
        Command::Agent(None) | Command::Agents => Some(PickerAction::OpenAgents),
        Command::Model(Some(id)) => Some(PickerAction::SelectModel(id)),
        Command::Model(None) => Some(PickerAction::OpenModels),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn agent(name: &str, cycle: bool) -> AgentOption {
        AgentOption {
            name: name.into(),
            in_tab_cycle: cycle,
        }
    }
    #[test]
    fn filtering_grouping_and_reachability() {
        let m = vec![
            ModelOption {
                id: "X/Fast".into(),
                provider: "Cloud".into(),
                recent: true,
                favorite: true,
            },
            ModelOption {
                id: "local/slow".into(),
                provider: "Local".into(),
                recent: false,
                favorite: false,
            },
        ];
        assert_eq!(fuzzy_models(&m, "FAST")[0].id, "X/Fast");
        assert_eq!(model_groups(&m, "").len(), 2);
        let a = vec![
            agent("Plan", true),
            agent("rare", false),
            agent("Build", true),
        ];
        assert_eq!(search_agents(&a, "LAN")[0].name, "Plan");
        assert_eq!(next_agent(&a, Some("plan"), false), Some("Build".into()));
    }
    #[test]
    fn commands_parse_suggest_and_emit_actions() {
        assert_eq!(
            parse_command("/AGENT Plan"),
            Some(Command::Agent(Some("Plan".into())))
        );
        assert_eq!(
            command_action(Command::Model(None)),
            Some(PickerAction::OpenModels)
        );
        assert_eq!(
            command_suggestions("/agent p", &[agent("plan", true)], &[]),
            vec!["plan"]
        );
        assert!(parse_command("/agents extra").is_none());
    }
    #[test]
    fn picker_state_tracks_query_selection_and_recency() {
        let mut picker = ModelPicker::new(vec![
            ModelOption {
                id: "a".into(),
                provider: "p".into(),
                recent: false,
                favorite: false,
            },
            ModelOption {
                id: "b".into(),
                provider: "p".into(),
                recent: true,
                favorite: false,
            },
        ]);
        picker.query = "a".into();
        assert_eq!(picker.selected_model().unwrap().id, "a");
        assert!(picker.toggle_favorite("a"));
        assert!(picker.mark_recent("a"));
        assert_eq!(move_selection(0, 2, -1), 1);
        assert_eq!(
            command_suggestions("/model ", &[], &picker.models),
            vec!["a", "b"]
        );
    }
    #[test]
    fn agent_state_wraps_and_exposes_tab_cycle() {
        let mut picker = AgentPicker::new(vec![agent("one", true), agent("two", true)]);
        picker.move_selection(-1);
        assert_eq!(picker.selected_agent().unwrap().name, "two");
        assert_eq!(picker.cycle(Some("one"), false), Some("two".into()));
        assert_eq!(command_parts("  /agent one"), Some(("/agent", "one")));
    }
}
