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
        Ok(Self {
            runtime: Mutex::new(runtime),
            service: AgentApiService::new(),
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
        let prompt_context = format_compact_context(&compact);
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
                memory_core::AdmissionAction::CreateNewNode => Some(decision.candidate_node_id),
                _ => None,
            })
            .collect();

        let now = Utc::now();
        let mut stored_node_count = 0;
        let mut stored_edge_count = 0;

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
        let response = self.service.record_outcome(&RecordOutcomeRequest {
            existing_traits,
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

        let result = OutcomeFeedbackResult {
            updated_trait_count: response.updated_traits.len(),
            update_count: response.updates.len(),
        };
        debug!(
            outcome_id = %request.outcome_id,
            success = request.success,
            updated_trait_count = result.updated_trait_count,
            update_count = result.update_count,
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

fn format_compact_context(
    response: &agent_api::adapters::openclaw_types::OpenClawRecallContextResponse,
) -> String {
    let mut lines = vec![
        "## External Memory Context".to_owned(),
        String::new(),
        response.summary.clone(),
    ];

    if !response.nodes.is_empty() {
        lines.push(String::new());
        lines.push("Relevant memories:".to_owned());
        lines.extend(
            response
                .nodes
                .iter()
                .take(4)
                .map(|node| format!("- {}: {}", node.title, node.summary)),
        );
    }

    if !response.lessons.is_empty() {
        lines.push(String::new());
        lines.push("Lessons:".to_owned());
        lines.extend(
            response
                .lessons
                .iter()
                .take(3)
                .map(|lesson| format!("- {}: {}", lesson.title, lesson.statement)),
        );
    }

    if let Some(checkpoint) = &response.checkpoint_summary {
        lines.push(String::new());
        lines.push(format!("Checkpoint: {checkpoint}"));
    }

    if !response.trait_snapshot.is_empty() {
        let traits = response
            .trait_snapshot
            .iter()
            .take(3)
            .map(|trait_state| format!("{} ({:.2})", trait_state.label, trait_state.strength))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(String::new());
        lines.push(format!("Trait snapshot: {traits}"));
    }

    lines.join("\n")
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
    use {super::*, tempfile::TempDir, uuid::Uuid};

    async fn open_test_adapter() -> (NodamemAdapter, TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let adapter = NodamemAdapter::open_at(dir.path().join("nodamem.db"))
            .await
            .expect("adapter should open");
        (adapter, dir)
    }

    #[tokio::test]
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

        assert!(context.prompt_context.contains("External Memory Context"));
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
}
