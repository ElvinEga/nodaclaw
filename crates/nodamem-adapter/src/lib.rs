use std::{collections::HashSet, path::PathBuf, sync::Arc};

use {
    agent_api::{
        AgentApiService, OutcomeRecordDto, ProposeMemoryRequest, RecallContextRequest,
        RecordOutcomeRequest, adapters::openclaw::compact_recall_context,
    },
    chrono::Utc,
    memory_core::{Edge, MemoryPacket, MemoryStatus, Node, NodeId},
    memory_ingest::{AdmissionContext, IngestEvent, MessageEvent},
    memory_store::{StoreConfig, StoreRuntime},
    tokio::sync::Mutex,
    tracing::{debug, info, warn},
};

#[derive(Debug)]
pub struct NodamemAdapter {
    runtime: Mutex<StoreRuntime>,
    service: AgentApiService,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecallRequest {
    pub text: String,
    pub session_id: Option<String>,
    pub topic: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecallResult {
    pub prompt_context: String,
    pub packet: MemoryPacket,
    pub source_node_ids: Vec<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryProposalRequest {
    pub session_id: Option<String>,
    pub user_text: String,
    pub assistant_text: String,
    pub event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryProposalResult {
    pub candidate_node_count: usize,
    pub decision_count: usize,
    pub stored_node_count: usize,
    pub stored_edge_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutcomeFeedbackRequest {
    pub outcome_id: String,
    pub subject_node_id: Option<NodeId>,
    pub success: bool,
    pub usefulness: f32,
    pub prediction_correct: bool,
    pub user_accepted: bool,
    pub validated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeFeedbackResult {
    pub updated_trait_count: usize,
    pub update_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptMemoryFormatConfig {
    pub max_chars: usize,
    pub max_lessons: usize,
    pub max_preferences_and_goals: usize,
    pub max_general_memories: usize,
    pub max_traits: usize,
    pub include_checkpoint: bool,
}

impl Default for PromptMemoryFormatConfig {
    fn default() -> Self {
        Self {
            max_chars: 850,
            max_lessons: 3,
            max_preferences_and_goals: 3,
            max_general_memories: 2,
            max_traits: 3,
            include_checkpoint: true,
        }
    }
}

#[derive(Debug, Clone)]
struct StoreSnapshot {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    lessons: Vec<memory_core::Lesson>,
    checkpoints: Vec<memory_core::Checkpoint>,
    traits: Vec<memory_core::TraitState>,
}

impl NodamemAdapter {
    pub async fn open(config: StoreConfig) -> anyhow::Result<Self> {
        let runtime = StoreRuntime::open(config).await?;
        let service = AgentApiService::new_with_store_connections(
            runtime.database.connect()?,
            Some(runtime.database.connect()?),
        );
        Ok(Self {
            runtime: Mutex::new(runtime),
            service,
        })
    }

    pub async fn open_at(path: PathBuf) -> anyhow::Result<Self> {
        Self::open(StoreConfig {
            local_database_path: path,
            ..StoreConfig::default()
        })
        .await
    }

    pub async fn recall_context(
        &self,
        request: &RecallRequest,
    ) -> anyhow::Result<Option<RecallResult>> {
        if request.text.trim().is_empty() {
            debug!(
                session_id = request.session_id.as_deref().unwrap_or(""),
                "nodamem recall_context skipped empty query"
            );
            return Ok(None);
        }

        debug!(
            session_id = request.session_id.as_deref().unwrap_or(""),
            has_topic = request.topic.is_some(),
            query_len = request.text.trim().chars().count(),
            "nodamem recall_context called"
        );

        let runtime = self.runtime.lock().await;
        let snapshot = load_snapshot(&runtime).await?;
        drop(runtime);

        if snapshot.nodes.is_empty()
            && snapshot.edges.is_empty()
            && snapshot.lessons.is_empty()
            && snapshot.checkpoints.is_empty()
            && snapshot.traits.is_empty()
        {
            debug!(
                session_id = request.session_id.as_deref().unwrap_or(""),
                "nodamem recall_context skipped empty store snapshot"
            );
            return Ok(None);
        }

        let response = self.service.recall_context(&RecallContextRequest {
            text: request.text.clone(),
            session_id: request.session_id.clone(),
            topic: request.topic.clone(),
            nodes: snapshot.nodes,
            edges: snapshot.edges,
            lessons: snapshot.lessons,
            checkpoints: snapshot.checkpoints,
            traits: snapshot.traits,
        })?;

        let compact = compact_recall_context(response.clone());
        let prompt_context =
            format_prompt_memory_context(&compact, PromptMemoryFormatConfig::default());
        let source_node_ids = response.packet.nodes.iter().map(|node| node.id).collect();
        debug!(
            session_id = request.session_id.as_deref().unwrap_or(""),
            node_count = response.packet.nodes.len(),
            lesson_count = response.packet.lessons.len(),
            checkpoint_count = response.packet.checkpoints.len(),
            trait_count = response.packet.traits.len(),
            prompt_chars = prompt_context.len(),
            "nodamem recall_context produced prompt context"
        );

        Ok(Some(RecallResult {
            prompt_context,
            packet: response.packet,
            source_node_ids,
        }))
    }

    pub async fn propose_memory(
        &self,
        request: &MemoryProposalRequest,
    ) -> anyhow::Result<MemoryProposalResult> {
        debug!(
            session_id = request.session_id.as_deref().unwrap_or(""),
            event_id = %request.event_id,
            user_chars = request.user_text.trim().chars().count(),
            assistant_chars = request.assistant_text.trim().chars().count(),
            "nodamem propose_memory called"
        );
        let runtime = self.runtime.lock().await;
        let snapshot = load_snapshot(&runtime).await?;
        let response = self.service.propose_memory(&ProposeMemoryRequest {
            event: build_exchange_event(request),
            context: AdmissionContext {
                existing_nodes: snapshot.nodes.clone(),
                existing_edges: snapshot.edges.clone(),
            },
        })?;

        let accepted_ids: HashSet<NodeId> = response
            .admission_decisions
            .iter()
            .filter_map(|decision| match decision.action {
                memory_core::AdmissionAction::CreateNewNode
                | memory_core::AdmissionAction::SupersedeExistingNode { .. } => {
                    Some(decision.candidate_node_id)
                },
                _ => None,
            })
            .collect();

        let now = Utc::now();
        let mut stored_node_count = 0;
        let mut stored_edge_count = 0;

        for decision in &response.admission_decisions {
            if let memory_core::AdmissionAction::SupersedeExistingNode { target_node_id } =
                decision.action
            {
                if let Some(mut existing) =
                    runtime.repository().get_node_by_id(target_node_id).await?
                {
                    existing.status = MemoryStatus::Archived;
                    existing.updated_at = now;
                    runtime.repository().update_node(&existing).await?;
                    info!(
                        superseded_node_id = %target_node_id.0,
                        replacement_node_id = %decision.candidate_node_id.0,
                        "nodamem archived superseded preference or goal before durable write"
                    );
                }
            }
        }

        for node in response
            .ingest_output
            .candidate_nodes
            .iter()
            .filter(|candidate| accepted_ids.contains(&candidate.id))
        {
            let mut stored = node.clone();
            stored.status = MemoryStatus::Active;
            stored.updated_at = now;
            stored.last_accessed_at = Some(now);
            runtime.repository().insert_node(&stored).await?;
            stored_node_count += 1;
        }

        let persisted_node_ids: HashSet<NodeId> = runtime
            .repository()
            .list_nodes()
            .await?
            .into_iter()
            .map(|node| node.id)
            .collect();
        for edge in response
            .ingest_output
            .candidate_edges
            .iter()
            .filter(|edge| {
                persisted_node_ids.contains(&edge.from_node_id)
                    && persisted_node_ids.contains(&edge.to_node_id)
            })
        {
            runtime.repository().insert_edge(edge).await?;
            stored_edge_count += 1;
        }

        let result = MemoryProposalResult {
            candidate_node_count: response.ingest_output.candidate_nodes.len(),
            decision_count: response.admission_decisions.len(),
            stored_node_count,
            stored_edge_count,
        };
        debug!(
            session_id = request.session_id.as_deref().unwrap_or(""),
            event_id = %request.event_id,
            candidate_node_count = result.candidate_node_count,
            decision_count = result.decision_count,
            stored_node_count = result.stored_node_count,
            stored_edge_count = result.stored_edge_count,
            "nodamem propose_memory stored validated memory"
        );

        Ok(result)
    }

    pub async fn record_outcome(
        &self,
        request: &OutcomeFeedbackRequest,
    ) -> anyhow::Result<OutcomeFeedbackResult> {
        debug!(
            outcome_id = %request.outcome_id,
            success = request.success,
            user_accepted = request.user_accepted,
            validated = request.validated,
            "nodamem record_outcome called"
        );
        let runtime = self.runtime.lock().await;
        let existing_traits = runtime.repository().list_trait_states().await?;
        let existing_lessons = runtime.repository().list_lessons().await?;
        let existing_self_model = runtime.repository().load_latest_self_model().await?;
        let response = self.service.record_outcome(&RecordOutcomeRequest {
            existing_traits,
            existing_lessons,
            existing_self_model,
            outcome: OutcomeRecordDto {
                outcome_id: request.outcome_id.clone(),
                subject_node_id: request.subject_node_id,
                success: request.success,
                usefulness: request.usefulness,
                prediction_correct: request.prediction_correct,
                user_accepted: request.user_accepted,
                validated: request.validated,
            },
        })?;

        for trait_state in &response.updated_traits {
            runtime.repository().save_trait_state(trait_state).await?;
        }
        for trait_event in &response.trait_events {
            runtime.repository().append_trait_event(trait_event).await?;
        }
        if let Some(self_model) = &response.refreshed_self_model {
            runtime.repository().save_self_model(self_model).await?;
        }

        let result = OutcomeFeedbackResult {
            updated_trait_count: response.updated_traits.len(),
            update_count: response.updates.len(),
        };
        debug!(
            outcome_id = %request.outcome_id,
            success = request.success,
            updated_trait_count = result.updated_trait_count,
            update_count = result.update_count,
            trait_event_count = response.trait_events.len(),
            self_model_refreshed = response.refreshed_self_model.is_some(),
            "nodamem record_outcome applied updates"
        );

        Ok(result)
    }
}

async fn load_snapshot(runtime: &StoreRuntime) -> anyhow::Result<StoreSnapshot> {
    Ok(StoreSnapshot {
        nodes: runtime.repository().list_nodes().await?,
        edges: runtime.repository().list_edges().await?,
        lessons: runtime.repository().list_lessons().await?,
        checkpoints: runtime.repository().load_recent_checkpoints(3).await?,
        traits: runtime.repository().list_trait_states().await?,
    })
}

fn build_exchange_event(request: &MemoryProposalRequest) -> IngestEvent {
    let combined = format!(
        "User message:\n{}\n\nAssistant response:\n{}",
        request.user_text.trim(),
        request.assistant_text.trim()
    );
    IngestEvent::AssistantMessage(MessageEvent {
        event_id: request.event_id.clone(),
        session_id: request.session_id.clone(),
        message_id: None,
        text: combined,
    })
}

fn format_prompt_memory_context(
    response: &agent_api::adapters::openclaw_types::OpenClawRecallContextResponse,
    config: PromptMemoryFormatConfig,
) -> String {
    let mut seen = HashSet::new();
    let mut lines = vec!["## Verified Memory Context".to_owned()];

    if let Some(summary) = dedupe_text(&mut seen, truncate_line(&response.summary, 180)) {
        lines.push(String::new());
        lines.push(summary);
    }

    let lessons = response
        .lessons
        .iter()
        .filter_map(|lesson| {
            let text = truncate_line(
                &format!("{}: {}", lesson.title.trim(), lesson.statement.trim()),
                160,
            );
            dedupe_text(&mut seen, text)
        })
        .take(config.max_lessons)
        .collect::<Vec<_>>();
    if !lessons.is_empty() {
        lines.push(String::new());
        lines.push("Validated lessons:".to_owned());
        lines.extend(lessons.into_iter().map(|line| format!("- {line}")));
    }

    let preference_or_goal_nodes = response
        .nodes
        .iter()
        .filter(|node| is_preference_or_goal(node))
        .filter_map(|node| {
            let text = truncate_line(&format_memory_line(&node.title, &node.summary), 160);
            dedupe_text(&mut seen, text)
        })
        .take(config.max_preferences_and_goals)
        .collect::<Vec<_>>();
    if !preference_or_goal_nodes.is_empty() {
        lines.push(String::new());
        lines.push("Relevant preferences and goals:".to_owned());
        lines.extend(
            preference_or_goal_nodes
                .into_iter()
                .map(|line| format!("- {line}")),
        );
    }

    let general_memories = response
        .nodes
        .iter()
        .filter(|node| !is_preference_or_goal(node))
        .filter_map(|node| {
            let text = truncate_line(&format_memory_line(&node.title, &node.summary), 150);
            dedupe_text(&mut seen, text)
        })
        .take(config.max_general_memories)
        .collect::<Vec<_>>();
    if !general_memories.is_empty() {
        lines.push(String::new());
        lines.push("Other verified context:".to_owned());
        lines.extend(general_memories.into_iter().map(|line| format!("- {line}")));
    }

    if config.include_checkpoint {
        if let Some(checkpoint) = response
            .checkpoint_summary
            .as_deref()
            .map(|value| truncate_line(value, 180))
            .and_then(|value| dedupe_text(&mut seen, value))
        {
            lines.push(String::new());
            lines.push(format!("Checkpoint: {checkpoint}"));
        }
    }

    let traits = response
        .trait_snapshot
        .iter()
        .filter_map(|trait_state| {
            let text = truncate_line(
                &format!("{} ({:.2})", trait_state.label.trim(), trait_state.strength),
                80,
            );
            dedupe_text(&mut seen, text)
        })
        .take(config.max_traits)
        .collect::<Vec<_>>();
    if !traits.is_empty() {
        lines.push(String::new());
        lines.push(format!("Trait snapshot: {}", traits.join(", ")));
    }

    if lines.len() == 1 {
        lines.push(String::new());
        lines.push("No verified memory available.".to_owned());
    }

    trim_to_configured_length(lines, config.max_chars)
}

fn trim_to_configured_length(mut lines: Vec<String>, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let mut output = lines.join("\n");
    if output.chars().count() <= max_chars {
        return output;
    }

    while lines.len() > 3 {
        lines.pop();
        output = lines.join("\n");
        if output.chars().count() <= max_chars {
            return output;
        }
    }

    let mut shortened = output
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    while shortened.ends_with(char::is_whitespace) {
        shortened.pop();
    }
    if shortened.is_empty() {
        "## Verified Memory Context".to_owned()
    } else {
        shortened.push('…');
        shortened
    }
}

fn format_memory_line(title: &str, summary: &str) -> String {
    let title = title.trim();
    let summary = summary.trim();
    if title.is_empty() {
        summary.to_owned()
    } else if summary.is_empty() || normalize_for_dedupe(title) == normalize_for_dedupe(summary) {
        title.to_owned()
    } else {
        format!("{title}: {summary}")
    }
}

fn dedupe_text(seen: &mut HashSet<String>, text: String) -> Option<String> {
    let dedupe_keys = dedupe_keys(&text);
    if dedupe_keys.is_empty()
        || dedupe_keys.iter().any(|key| is_hypothetical_text(key))
        || dedupe_keys.iter().any(|key| seen.contains(key))
    {
        return None;
    }
    seen.extend(dedupe_keys);
    Some(text)
}

fn normalize_for_dedupe(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() {
                ch
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn dedupe_keys(text: &str) -> Vec<String> {
    let normalized = normalize_for_dedupe(text);
    if normalized.is_empty() {
        return Vec::new();
    }

    let mut keys = vec![normalized.clone()];
    if let Some((_, detail)) = text.split_once(':') {
        let detail_key = normalize_for_dedupe(detail);
        if !detail_key.is_empty() && detail_key != normalized {
            keys.push(detail_key);
        }
    }
    keys
}

fn truncate_line(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }
    let mut shortened = compact
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    while shortened.ends_with(char::is_whitespace) {
        shortened.pop();
    }
    shortened.push('…');
    shortened
}

fn is_preference_or_goal(node: &agent_api::adapters::openclaw_types::OpenClawNodeSummary) -> bool {
    let haystack =
        format!("{} {} {}", node.title, node.summary, node.tags.join(" ")).to_lowercase();
    [
        "preference",
        "prefers",
        "likes",
        "dislikes",
        "goal",
        "objective",
        "plan",
        "priority",
    ]
    .iter()
    .any(|keyword| haystack.contains(keyword))
}

fn is_hypothetical_text(normalized_text: &str) -> bool {
    [
        "hypothetical",
        "imagined",
        "what if",
        "possible scenario",
        "potential scenario",
        "counterfactual",
        "predicted outcome",
    ]
    .iter()
    .any(|phrase| normalized_text.contains(phrase))
}

pub fn should_propose_memory(user_text: &str, assistant_text: &str) -> bool {
    let user = user_text.trim();
    let assistant = assistant_text.trim();
    if user.is_empty() || assistant.is_empty() {
        return false;
    }
    assistant.len() >= 48 || user.contains("remember") || user.contains("preference")
}

pub async fn open_default_adapter(data_dir: PathBuf) -> Option<Arc<NodamemAdapter>> {
    let path = data_dir.join("nodamem").join("nodamem.db");
    match NodamemAdapter::open_at(path).await {
        Ok(adapter) => {
            info!(db_path = %data_dir.join("nodamem").join("nodamem.db").display(), "nodamem adapter enabled");
            Some(Arc::new(adapter))
        },
        Err(error) => {
            warn!(%error, "nodamem adapter unavailable; continuing without external memory");
            None
        },
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {
        super::*,
        agent_api::adapters::openclaw_types::{
            OpenClawLessonSummary, OpenClawNodeSummary, OpenClawRecallContextResponse,
            OpenClawTraitSummary,
        },
        memory_core::{LessonType, TraitType},
        tempfile::TempDir,
        uuid::Uuid,
    };

    async fn open_test_adapter() -> (NodamemAdapter, TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let adapter = NodamemAdapter::open_at(dir.path().join("nodamem.db"))
            .await
            .expect("adapter should open");
        (adapter, dir)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn read_path_returns_compact_context() {
        let (adapter, _dir) = open_test_adapter().await;
        let proposal = adapter
            .propose_memory(&MemoryProposalRequest {
                session_id: Some("session-1".to_owned()),
                user_text: "Remember that the user prefers concise release notes.".to_owned(),
                assistant_text: "I will keep future release notes concise and focused.".to_owned(),
                event_id: Uuid::new_v4().to_string(),
            })
            .await
            .expect("proposal should succeed");
        assert!(proposal.decision_count > 0);

        let context = adapter
            .recall_context(&RecallRequest {
                text: "release notes preference".to_owned(),
                session_id: Some("session-1".to_owned()),
                topic: Some("preferences".to_owned()),
            })
            .await
            .expect("recall should succeed")
            .expect("recall should return context");

        assert!(context.prompt_context.contains("Verified Memory Context"));
        assert!(!context.source_node_ids.is_empty());
    }

    #[tokio::test]
    async fn propose_memory_persists_validated_nodes() {
        let (adapter, _dir) = open_test_adapter().await;
        let result = adapter
            .propose_memory(&MemoryProposalRequest {
                session_id: Some("session-2".to_owned()),
                user_text: "Remember that the deployment window is Friday night.".to_owned(),
                assistant_text: "Noted. The deployment window is Friday night.".to_owned(),
                event_id: Uuid::new_v4().to_string(),
            })
            .await
            .expect("proposal should succeed");

        assert!(result.candidate_node_count > 0);
        assert!(result.decision_count > 0);
        assert!(result.stored_node_count > 0);
    }

    #[tokio::test]
    async fn superseded_preference_is_archived_and_replaced() {
        let (adapter, _dir) = open_test_adapter().await;
        adapter
            .propose_memory(&MemoryProposalRequest {
                session_id: Some("session-3".to_owned()),
                user_text: "Remember that the user prefers verbose release notes.".to_owned(),
                assistant_text: "Noted. I will keep release notes verbose.".to_owned(),
                event_id: Uuid::new_v4().to_string(),
            })
            .await
            .expect("initial proposal should succeed");

        let result = adapter
            .propose_memory(&MemoryProposalRequest {
                session_id: Some("session-3".to_owned()),
                user_text: "The user no longer prefers verbose release notes; keep them concise."
                    .to_owned(),
                assistant_text: "Understood. I will keep release notes concise.".to_owned(),
                event_id: Uuid::new_v4().to_string(),
            })
            .await
            .expect("replacement proposal should succeed");

        assert!(result.stored_node_count > 0);

        let runtime = adapter.runtime.lock().await;
        let nodes = runtime
            .repository()
            .list_nodes()
            .await
            .expect("nodes should load");
        let preference_nodes = nodes
            .into_iter()
            .filter(|node| node.node_type == memory_core::NodeType::Preference)
            .collect::<Vec<_>>();

        assert_eq!(
            preference_nodes
                .iter()
                .filter(|node| node.status == MemoryStatus::Archived)
                .count(),
            1
        );
        assert_eq!(
            preference_nodes
                .iter()
                .filter(|node| matches!(
                    node.status,
                    MemoryStatus::Active | MemoryStatus::Reinforced
                ))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn outcome_feedback_persists_trait_updates() {
        let (adapter, _dir) = open_test_adapter().await;
        let outcome = adapter
            .record_outcome(&OutcomeFeedbackRequest {
                outcome_id: Uuid::new_v4().to_string(),
                subject_node_id: None,
                success: true,
                usefulness: 0.9,
                prediction_correct: true,
                user_accepted: true,
                validated: true,
            })
            .await
            .expect("outcome should succeed");

        assert!(outcome.updated_trait_count > 0);
        assert!(outcome.update_count > 0);

        let runtime = adapter.runtime.lock().await;
        let traits = runtime
            .repository()
            .list_trait_states()
            .await
            .expect("traits should load");
        let events = runtime
            .repository()
            .load_trait_events(None, Some(50))
            .await
            .expect("trait events should load");

        assert!(!traits.is_empty());
        assert!(!events.is_empty());
        assert!(events.iter().all(|event| event.outcome_id.is_some()));
    }

    #[test]
    fn proposal_heuristic_requires_meaningful_exchange() {
        assert!(!should_propose_memory("", "assistant"));
        assert!(!should_propose_memory("short", "tiny"));
        assert!(should_propose_memory(
            "please remember this preference",
            "I will remember the user's preference for later turns"
        ));
    }

    #[test]
    fn prompt_memory_format_is_compact_and_bounded() {
        let response = OpenClawRecallContextResponse {
            summary: "Use verified memory sparingly and only when it helps the current task."
                .to_owned(),
            nodes: vec![
                OpenClawNodeSummary {
                    node_id: NodeId(Uuid::new_v4()),
                    title: "User preference".to_owned(),
                    summary: "Prefers concise release notes with bullet points.".to_owned(),
                    tags: vec!["preference".to_owned()],
                    confidence: 0.9,
                    importance: 0.8,
                },
                OpenClawNodeSummary {
                    node_id: NodeId(Uuid::new_v4()),
                    title: "Project goal".to_owned(),
                    summary: "Ship the integration incrementally without a rewrite.".to_owned(),
                    tags: vec!["goal".to_owned()],
                    confidence: 0.92,
                    importance: 0.85,
                },
            ],
            lessons: vec![OpenClawLessonSummary {
                lesson_id: memory_core::LessonId(Uuid::new_v4()),
                lesson_type: LessonType::Strategy,
                title: "Integration approach".to_owned(),
                statement: "Prefer adapter-backed changes over runtime-wide memory rewrites."
                    .to_owned(),
                confidence: 0.95,
            }],
            checkpoint_summary: Some(
                "Current milestone: read path is integrated and under active verification."
                    .to_owned(),
            ),
            trait_snapshot: vec![OpenClawTraitSummary {
                trait_type: TraitType::Practicality,
                label: "Practicality".to_owned(),
                strength: 0.81,
                confidence: 0.9,
            }],
        };

        let formatted = format_prompt_memory_context(&response, PromptMemoryFormatConfig {
            max_chars: 420,
            ..PromptMemoryFormatConfig::default()
        });

        assert!(formatted.contains("## Verified Memory Context"));
        assert!(formatted.contains("Validated lessons:"));
        assert!(formatted.contains("Relevant preferences and goals:"));
        assert!(formatted.chars().count() <= 420);
    }

    #[test]
    fn prompt_memory_format_deduplicates_repeated_items() {
        let response = OpenClawRecallContextResponse {
            summary: "The user prefers concise release notes.".to_owned(),
            nodes: vec![
                OpenClawNodeSummary {
                    node_id: NodeId(Uuid::new_v4()),
                    title: "Release notes preference".to_owned(),
                    summary: "The user prefers concise release notes.".to_owned(),
                    tags: vec!["preference".to_owned()],
                    confidence: 0.9,
                    importance: 0.8,
                },
                OpenClawNodeSummary {
                    node_id: NodeId(Uuid::new_v4()),
                    title: "Release notes preference".to_owned(),
                    summary: "The user prefers concise release notes.".to_owned(),
                    tags: vec!["preference".to_owned()],
                    confidence: 0.88,
                    importance: 0.79,
                },
            ],
            lessons: vec![OpenClawLessonSummary {
                lesson_id: memory_core::LessonId(Uuid::new_v4()),
                lesson_type: LessonType::User,
                title: "Release notes preference".to_owned(),
                statement: "The user prefers concise release notes.".to_owned(),
                confidence: 0.95,
            }],
            checkpoint_summary: None,
            trait_snapshot: vec![],
        };

        let formatted =
            format_prompt_memory_context(&response, PromptMemoryFormatConfig::default());

        assert_eq!(
            formatted
                .matches("The user prefers concise release notes.")
                .count(),
            1
        );
    }

    #[test]
    fn prompt_memory_format_excludes_hypothetical_content() {
        let response = OpenClawRecallContextResponse {
            summary: "Verified memory available for the current task.".to_owned(),
            nodes: vec![
                OpenClawNodeSummary {
                    node_id: NodeId(Uuid::new_v4()),
                    title: "Possible scenario".to_owned(),
                    summary: "Hypothetical launch plan if the user changes priorities.".to_owned(),
                    tags: vec!["scenario".to_owned()],
                    confidence: 0.5,
                    importance: 0.4,
                },
                OpenClawNodeSummary {
                    node_id: NodeId(Uuid::new_v4()),
                    title: "User goal".to_owned(),
                    summary: "Finish the rollout with minimal disruption.".to_owned(),
                    tags: vec!["goal".to_owned()],
                    confidence: 0.91,
                    importance: 0.84,
                },
            ],
            lessons: vec![],
            checkpoint_summary: Some(
                "Imagined paths are not injected as verified memory.".to_owned(),
            ),
            trait_snapshot: vec![],
        };

        let formatted =
            format_prompt_memory_context(&response, PromptMemoryFormatConfig::default());

        assert!(!formatted.to_lowercase().contains("hypothetical"));
        assert!(!formatted.to_lowercase().contains("possible scenario"));
        assert!(formatted.contains("Finish the rollout with minimal disruption."));
    }

    #[test]
    fn prompt_memory_format_has_stable_empty_state() {
        let response = OpenClawRecallContextResponse {
            summary: String::new(),
            nodes: vec![],
            lessons: vec![],
            checkpoint_summary: None,
            trait_snapshot: vec![],
        };

        let formatted =
            format_prompt_memory_context(&response, PromptMemoryFormatConfig::default());

        assert_eq!(
            formatted,
            "## Verified Memory Context\n\nNo verified memory available."
        );
    }
}
