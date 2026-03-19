use std::{
    collections::{BTreeSet, HashSet},
    path::PathBuf,
    sync::Arc,
};

use {
    agent_api::{
        AgentApiService, GenerateImaginedScenariosRequest, OutcomeRecordDto, ProposeMemoryRequest,
        RecallContextRequest, RecordOutcomeRequest, adapters::openclaw::compact_recall_context,
    },
    chrono::Utc,
    memory_core::{Edge, MemoryPacket, MemoryStatus, Node, NodeId, NodeType},
    memory_imagination::{ImaginationService, ScenarioReviewDecision},
    memory_ingest::{AdmissionContext, IngestEvent, MessageEvent},
    memory_store::{StoreConfig, StoreRuntime},
    tokio::sync::Mutex,
    tracing::{debug, info, warn},
    uuid::Uuid,
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
    pub include_hypothetical: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecallResult {
    pub prompt_context: String,
    pub verified_prompt_context: String,
    pub hypothetical_prompt_context: Option<String>,
    pub packet: MemoryPacket,
    pub source_node_ids: Vec<NodeId>,
    pub imagined_scenario_ids: Vec<String>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImaginedScenarioFeedbackRequest {
    pub scenario_ids: Vec<String>,
    pub accepted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImaginedScenarioFeedbackResult {
    pub reviewed_count: usize,
    pub missing_count: usize,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptImaginationFormatConfig {
    pub max_chars: usize,
    pub max_scenarios: usize,
    pub max_outcomes_per_scenario: usize,
    pub include_strategy_continuity: bool,
}

impl Default for PromptImaginationFormatConfig {
    fn default() -> Self {
        Self {
            max_chars: 700,
            max_scenarios: 2,
            max_outcomes_per_scenario: 2,
            include_strategy_continuity: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InspectionConfig {
    pub max_nodes: usize,
    pub max_lessons: usize,
    pub max_scenarios: usize,
    pub max_trait_events: usize,
    pub max_history_nodes: usize,
}

impl Default for InspectionConfig {
    fn default() -> Self {
        Self {
            max_nodes: 4,
            max_lessons: 3,
            max_scenarios: 2,
            max_trait_events: 4,
            max_history_nodes: 6,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryInspectionReport {
    pub verified_packet_view: String,
    pub hypothetical_scenarios_view: String,
    pub self_model_continuity_view: String,
    pub trait_update_reasons_view: String,
    pub lesson_reasons_view: String,
    pub superseded_history_view: String,
}

impl MemoryInspectionReport {
    #[must_use]
    pub fn render(&self) -> String {
        [
            self.verified_packet_view.as_str(),
            self.hypothetical_scenarios_view.as_str(),
            self.self_model_continuity_view.as_str(),
            self.trait_update_reasons_view.as_str(),
            self.lesson_reasons_view.as_str(),
            self.superseded_history_view.as_str(),
        ]
        .join("\n\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationScenarioResult {
    pub name: &'static str,
    pub passed: bool,
    pub details: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationHarnessReport {
    pub scenarios: Vec<EvaluationScenarioResult>,
}

impl EvaluationHarnessReport {
    #[must_use]
    pub fn render(&self) -> String {
        let mut lines = vec!["## Nodamem Evaluation".to_owned()];
        for scenario in &self.scenarios {
            lines.push(format!(
                "- {} [{}]: {}",
                scenario.name,
                if scenario.passed {
                    "pass"
                } else {
                    "fail"
                },
                scenario.details
            ));
        }
        lines.join("\n")
    }
}

#[derive(Debug, Clone)]
struct StoreSnapshot {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    lessons: Vec<memory_core::Lesson>,
    checkpoints: Vec<memory_core::Checkpoint>,
    traits: Vec<memory_core::TraitState>,
    self_model_snapshot: Option<memory_core::SelfModel>,
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
            self_model_snapshot: snapshot.self_model_snapshot,
        })?;

        let compact = compact_recall_context(response.clone());
        let verified_prompt_context =
            format_prompt_memory_context(&compact, PromptMemoryFormatConfig::default());
        let mut hypothetical_prompt_context = None;
        let mut imagined_scenario_ids = Vec::new();

        if request.include_hypothetical {
            match self
                .service
                .generate_imagined_scenarios(&GenerateImaginedScenariosRequest {
                    planning_task: request.text.clone(),
                    desired_scenarios: PromptImaginationFormatConfig::default().max_scenarios,
                    context_packet: response.packet.clone(),
                    active_goal_node_ids: response
                        .packet
                        .nodes
                        .iter()
                        .filter(|node| node.node_type == NodeType::Goal)
                        .map(|node| node.id)
                        .collect(),
                }) {
                Ok(imagined_response) => {
                    if !imagined_response.scenarios.is_empty() {
                        let runtime = self.runtime.lock().await;
                        for scenario in &imagined_response.scenarios {
                            if let Err(error) = runtime
                                .repository()
                                .upsert_imagined_scenario(scenario)
                                .await
                            {
                                warn!(
                                    session_id = request.session_id.as_deref().unwrap_or(""),
                                    %error,
                                    scenario_id = %scenario.id.0,
                                    "nodamem imagined scenario persistence failed"
                                );
                                continue;
                            }
                            imagined_scenario_ids.push(scenario.id.0.to_string());
                        }
                        drop(runtime);
                        hypothetical_prompt_context = format_prompt_imagination_context(
                            &imagined_response.scenarios,
                            response.packet.self_model_snapshot.as_ref(),
                            PromptImaginationFormatConfig::default(),
                        );
                    }
                },
                Err(error) => {
                    warn!(
                        session_id = request.session_id.as_deref().unwrap_or(""),
                        %error,
                        "nodamem imagined scenario generation failed; continuing with verified context only"
                    );
                },
            }
        }

        let prompt_context = match hypothetical_prompt_context.as_deref() {
            Some(hypothetical) => format!("{verified_prompt_context}\n\n{hypothetical}"),
            None => verified_prompt_context.clone(),
        };
        let source_node_ids = response.packet.nodes.iter().map(|node| node.id).collect();
        debug!(
            session_id = request.session_id.as_deref().unwrap_or(""),
            node_count = response.packet.nodes.len(),
            lesson_count = response.packet.lessons.len(),
            checkpoint_count = response.packet.checkpoints.len(),
            trait_count = response.packet.traits.len(),
            imagined_scenario_count = imagined_scenario_ids.len(),
            hypothetical_included = hypothetical_prompt_context.is_some(),
            prompt_chars = prompt_context.len(),
            "nodamem recall_context produced prompt context"
        );

        Ok(Some(RecallResult {
            prompt_context,
            verified_prompt_context,
            hypothetical_prompt_context,
            packet: response.packet,
            source_node_ids,
            imagined_scenario_ids,
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

    pub async fn review_imagined_scenarios(
        &self,
        request: &ImaginedScenarioFeedbackRequest,
    ) -> anyhow::Result<ImaginedScenarioFeedbackResult> {
        if request.scenario_ids.is_empty() {
            return Ok(ImaginedScenarioFeedbackResult {
                reviewed_count: 0,
                missing_count: 0,
            });
        }

        let decision = if request.accepted {
            ScenarioReviewDecision::AcceptAsHypothesis
        } else {
            ScenarioReviewDecision::Reject
        };
        let reviewer = ImaginationService::default();
        let runtime = self.runtime.lock().await;
        let mut reviewed_count = 0;
        let mut missing_count = 0;

        for scenario_id in &request.scenario_ids {
            let Ok(parsed) = Uuid::parse_str(scenario_id) else {
                missing_count += 1;
                continue;
            };
            let Some(existing) = runtime
                .repository()
                .load_imagined_scenario(memory_core::ScenarioId(parsed))
                .await?
            else {
                missing_count += 1;
                continue;
            };

            let reviewed = reviewer.review_scenario(&existing, decision);
            runtime
                .repository()
                .upsert_imagined_scenario(&reviewed)
                .await?;
            reviewed_count += 1;
        }

        debug!(
            accepted = request.accepted,
            reviewed_count, missing_count, "nodamem imagined scenario review completed"
        );

        Ok(ImaginedScenarioFeedbackResult {
            reviewed_count,
            missing_count,
        })
    }

    pub async fn inspect_memory_flow(
        &self,
        request: &RecallRequest,
        config: InspectionConfig,
    ) -> anyhow::Result<MemoryInspectionReport> {
        let recall = self.recall_context(request).await?;
        let runtime = self.runtime.lock().await;
        let repository = runtime.repository();
        let self_model = repository.load_latest_self_model().await?;
        let trait_events = repository
            .load_trait_events(None, Some(config.max_trait_events))
            .await?;
        let lessons = repository.list_lessons().await?;
        let nodes = repository.list_nodes().await?;

        let report = MemoryInspectionReport {
            verified_packet_view: render_verified_packet_view(recall.as_ref(), config),
            hypothetical_scenarios_view: render_hypothetical_scenarios_view(
                recall.as_ref(),
                &repository,
                config,
            )
            .await?,
            self_model_continuity_view: render_self_model_continuity_view(self_model.as_ref()),
            trait_update_reasons_view: render_trait_update_reasons_view(&trait_events),
            lesson_reasons_view: render_lesson_reasons_view(&repository, &lessons, config).await?,
            superseded_history_view: render_superseded_history_view(&nodes, config),
        };

        debug!(
            has_verified_packet = recall.is_some(),
            trait_event_count = trait_events.len(),
            lesson_count = lessons.len(),
            node_count = nodes.len(),
            "nodamem inspection report generated"
        );

        Ok(report)
    }

    pub async fn run_evaluation_harness_at(
        path: PathBuf,
    ) -> anyhow::Result<EvaluationHarnessReport> {
        let adapter = NodamemAdapter::open_at(path).await?;
        let mut scenarios = Vec::new();

        scenarios.push(
            evaluate_stable_preference_recall(&adapter)
                .await
                .unwrap_or_else(evaluation_failure("stable preference recall")),
        );
        scenarios.push(
            evaluate_contradiction_handling(&adapter)
                .await
                .unwrap_or_else(evaluation_failure("contradiction handling")),
        );
        scenarios.push(
            evaluate_duplicate_suppression(&adapter)
                .await
                .unwrap_or_else(evaluation_failure("duplicate suppression")),
        );
        scenarios.push(
            evaluate_grounded_imagination(&adapter)
                .await
                .unwrap_or_else(evaluation_failure("grounded imagination")),
        );
        scenarios.push(
            evaluate_scenario_feedback_review(&adapter)
                .await
                .unwrap_or_else(evaluation_failure("scenario feedback review")),
        );

        debug!(
            scenario_count = scenarios.len(),
            "nodamem evaluation harness completed"
        );
        Ok(EvaluationHarnessReport { scenarios })
    }
}

async fn load_snapshot(runtime: &StoreRuntime) -> anyhow::Result<StoreSnapshot> {
    Ok(StoreSnapshot {
        nodes: runtime.repository().list_nodes().await?,
        edges: runtime.repository().list_edges().await?,
        lessons: runtime.repository().list_lessons().await?,
        checkpoints: runtime.repository().load_recent_checkpoints(3).await?,
        traits: runtime.repository().list_trait_states().await?,
        self_model_snapshot: runtime.repository().load_latest_self_model().await?,
    })
}

fn render_verified_packet_view(recall: Option<&RecallResult>, config: InspectionConfig) -> String {
    let mut lines = vec!["## Verified Packet".to_owned()];
    let Some(recall) = recall else {
        lines.push("No verified packet available.".to_owned());
        return lines.join("\n");
    };

    lines.push(format!(
        "nodes={} lessons={} checkpoints={} traits={}",
        recall.packet.nodes.len(),
        recall.packet.lessons.len(),
        recall.packet.checkpoints.len(),
        recall.packet.traits.len()
    ));
    for node in recall.packet.nodes.iter().take(config.max_nodes) {
        lines.push(format!(
            "- {:?}: {}",
            node.node_type,
            truncate_line(&format_memory_line(&node.title, &node.summary), 96)
        ));
    }
    for lesson in recall.packet.lessons.iter().take(config.max_lessons) {
        lines.push(format!(
            "- lesson: {}",
            truncate_line(&format!("{}: {}", lesson.title, lesson.statement), 96)
        ));
    }
    lines.join("\n")
}

async fn render_hypothetical_scenarios_view(
    recall: Option<&RecallResult>,
    repository: &memory_store::StoreRepository<'_>,
    config: InspectionConfig,
) -> anyhow::Result<String> {
    let mut lines = vec!["## Hypothetical Scenarios".to_owned()];
    let Some(recall) = recall else {
        lines.push("No scenarios requested.".to_owned());
        return Ok(lines.join("\n"));
    };
    if recall.imagined_scenario_ids.is_empty() {
        lines.push("No hypothetical scenarios attached.".to_owned());
        return Ok(lines.join("\n"));
    }

    for scenario_id in recall
        .imagined_scenario_ids
        .iter()
        .take(config.max_scenarios)
    {
        let parsed = Uuid::parse_str(scenario_id)?;
        if let Some(scenario) = repository
            .load_imagined_scenario(memory_core::ScenarioId(parsed))
            .await?
        {
            lines.push(format!(
                "- {} [{} | {:?}]",
                truncate_line(&scenario.title, 60),
                imagination_kind_label(scenario.kind),
                scenario.status
            ));
            lines.push(format!(
                "  basis={} outcomes={}",
                scenario.basis_source_node_ids.len(),
                scenario.predicted_outcomes.len()
            ));
        }
    }
    Ok(lines.join("\n"))
}

fn render_self_model_continuity_view(self_model: Option<&memory_core::SelfModel>) -> String {
    let mut lines = vec!["## Self-Model Continuity".to_owned()];
    let Some(self_model) = self_model else {
        lines.push("No self-model snapshot available.".to_owned());
        return lines.join("\n");
    };

    let continuity = summarize_self_model_for_strategy(Some(self_model))
        .unwrap_or_else(|| "No compact continuity line available.".to_owned());
    lines.push(format!("line: {continuity}"));
    lines.push(format!(
        "source: strengths={} preferences={} tendencies={} domains={}",
        self_model.recurring_strengths.len(),
        self_model.user_interaction_preferences.len(),
        self_model.behavioral_tendencies.len(),
        self_model.active_domains.len()
    ));
    lines.join("\n")
}

fn render_trait_update_reasons_view(trait_events: &[memory_core::TraitEvent]) -> String {
    let mut lines = vec!["## Trait Update Reasons".to_owned()];
    if trait_events.is_empty() {
        lines.push("No trait updates recorded.".to_owned());
        return lines.join("\n");
    }
    for event in trait_events {
        lines.push(format!(
            "- {} {:?} {:.2}->{:.2}: {}",
            trait_type_label(event.trait_type),
            event.change_kind,
            event.previous_strength,
            event.updated_strength,
            truncate_line(&event.reason, 84)
        ));
    }
    lines.join("\n")
}

async fn render_lesson_reasons_view(
    repository: &memory_store::StoreRepository<'_>,
    lessons: &[memory_core::Lesson],
    config: InspectionConfig,
) -> anyhow::Result<String> {
    let mut lines = vec!["## Lesson Reasons".to_owned()];
    if lessons.is_empty() {
        lines.push("No lessons available.".to_owned());
        return Ok(lines.join("\n"));
    }
    for lesson in lessons.iter().take(config.max_lessons) {
        if let Some(audit) = repository.inspect_lesson_audit(lesson.id).await? {
            lines.push(format!(
                "- {}: {}",
                truncate_line(&audit.lesson.title, 40),
                audit
                    .reasons
                    .iter()
                    .take(2)
                    .map(|value| truncate_line(value, 60))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
    }
    Ok(lines.join("\n"))
}

fn render_superseded_history_view(nodes: &[Node], config: InspectionConfig) -> String {
    let mut lines = vec!["## Superseded Preferences And Goals".to_owned()];
    let relevant = nodes
        .iter()
        .filter(|node| {
            matches!(node.node_type, NodeType::Preference | NodeType::Goal)
                && matches!(
                    node.status,
                    MemoryStatus::Archived | MemoryStatus::Active | MemoryStatus::Reinforced
                )
        })
        .take(config.max_history_nodes)
        .collect::<Vec<_>>();
    if relevant.is_empty() {
        lines.push("No preference or goal history available.".to_owned());
        return lines.join("\n");
    }
    for node in relevant {
        lines.push(format!(
            "- {:?} {:?}: {}",
            node.node_type,
            node.status,
            truncate_line(&format_memory_line(&node.title, &node.summary), 84)
        ));
    }
    lines.join("\n")
}

fn evaluation_failure(
    name: &'static str,
) -> impl FnOnce(anyhow::Error) -> EvaluationScenarioResult {
    move |error| EvaluationScenarioResult {
        name,
        passed: false,
        details: truncate_line(&format!("error: {error}"), 120),
    }
}

async fn evaluate_stable_preference_recall(
    adapter: &NodamemAdapter,
) -> anyhow::Result<EvaluationScenarioResult> {
    adapter
        .propose_memory(&MemoryProposalRequest {
            session_id: Some("eval-pref".to_owned()),
            user_text: "Remember the user prefers concise answers.".to_owned(),
            assistant_text: "I will keep future answers concise.".to_owned(),
            event_id: Uuid::new_v4().to_string(),
        })
        .await?;
    let recall = adapter
        .recall_context(&RecallRequest {
            text: "What tone should I use based on the user's preference?".to_owned(),
            session_id: Some("eval-pref".to_owned()),
            topic: Some("preference".to_owned()),
            include_hypothetical: false,
        })
        .await?;
    let passed = recall
        .as_ref()
        .is_some_and(|value| value.prompt_context.to_lowercase().contains("concise"));
    Ok(EvaluationScenarioResult {
        name: "stable preference recall",
        passed,
        details: if passed {
            "verified preference was recalled".to_owned()
        } else {
            "verified preference was not surfaced".to_owned()
        },
    })
}

async fn evaluate_contradiction_handling(
    adapter: &NodamemAdapter,
) -> anyhow::Result<EvaluationScenarioResult> {
    adapter
        .propose_memory(&MemoryProposalRequest {
            session_id: Some("eval-contradiction".to_owned()),
            user_text: "Remember the user prefers detailed updates.".to_owned(),
            assistant_text: "I will use detailed updates.".to_owned(),
            event_id: Uuid::new_v4().to_string(),
        })
        .await?;
    adapter
        .propose_memory(&MemoryProposalRequest {
            session_id: Some("eval-contradiction".to_owned()),
            user_text: "The user now prefers concise updates instead.".to_owned(),
            assistant_text: "I will use concise updates instead.".to_owned(),
            event_id: Uuid::new_v4().to_string(),
        })
        .await?;
    let runtime = adapter.runtime.lock().await;
    let nodes = runtime.repository().list_nodes().await?;
    let archived_preferences = nodes
        .iter()
        .filter(|node| {
            node.node_type == NodeType::Preference && node.status == MemoryStatus::Archived
        })
        .count();
    Ok(EvaluationScenarioResult {
        name: "contradiction handling",
        passed: archived_preferences >= 1,
        details: format!("archived_preferences={archived_preferences}"),
    })
}

async fn evaluate_duplicate_suppression(
    adapter: &NodamemAdapter,
) -> anyhow::Result<EvaluationScenarioResult> {
    let first = adapter
        .propose_memory(&MemoryProposalRequest {
            session_id: Some("eval-duplicate".to_owned()),
            user_text: "Remember the deployment window is Friday night.".to_owned(),
            assistant_text: "The deployment window is Friday night.".to_owned(),
            event_id: Uuid::new_v4().to_string(),
        })
        .await?;
    let second = adapter
        .propose_memory(&MemoryProposalRequest {
            session_id: Some("eval-duplicate".to_owned()),
            user_text: "Remember again that the deployment window is Friday night.".to_owned(),
            assistant_text: "Still noted: Friday night deployment window.".to_owned(),
            event_id: Uuid::new_v4().to_string(),
        })
        .await?;
    Ok(EvaluationScenarioResult {
        name: "duplicate suppression",
        passed: second.stored_node_count <= first.stored_node_count,
        details: format!(
            "first_stored={} second_stored={}",
            first.stored_node_count, second.stored_node_count
        ),
    })
}

async fn evaluate_grounded_imagination(
    adapter: &NodamemAdapter,
) -> anyhow::Result<EvaluationScenarioResult> {
    adapter
        .propose_memory(&MemoryProposalRequest {
            session_id: Some("eval-imagination".to_owned()),
            user_text: "Remember the project goal is to stage the rollout carefully.".to_owned(),
            assistant_text: "I will plan around a careful staged rollout.".to_owned(),
            event_id: Uuid::new_v4().to_string(),
        })
        .await?;
    let recall = adapter
        .recall_context(&RecallRequest {
            text: "Brainstorm staged rollout options for next week.".to_owned(),
            session_id: Some("eval-imagination".to_owned()),
            topic: Some("planning".to_owned()),
            include_hypothetical: true,
        })
        .await?;
    let passed = recall.as_ref().is_some_and(|value| {
        value
            .hypothetical_prompt_context
            .as_deref()
            .is_some_and(|block| block.contains("## Hypothetical Planning Scenarios"))
    });
    Ok(EvaluationScenarioResult {
        name: "grounded imagination",
        passed,
        details: if passed {
            "planning prompt produced hypothetical scenarios".to_owned()
        } else {
            "no hypothetical planning block was produced".to_owned()
        },
    })
}

async fn evaluate_scenario_feedback_review(
    adapter: &NodamemAdapter,
) -> anyhow::Result<EvaluationScenarioResult> {
    adapter
        .propose_memory(&MemoryProposalRequest {
            session_id: Some("eval-review".to_owned()),
            user_text: "Remember the current goal is to ship a staged rollout safely.".to_owned(),
            assistant_text: "I will plan for a safe staged rollout.".to_owned(),
            event_id: Uuid::new_v4().to_string(),
        })
        .await?;
    let recall = adapter
        .recall_context(&RecallRequest {
            text: "Plan a rollout fallback for next week.".to_owned(),
            session_id: Some("eval-review".to_owned()),
            topic: Some("planning".to_owned()),
            include_hypothetical: true,
        })
        .await?;
    let Some(recall) = recall else {
        return Ok(EvaluationScenarioResult {
            name: "scenario acceptance/rejection feedback",
            passed: false,
            details: "no planning scenarios were generated".to_owned(),
        });
    };
    adapter
        .review_imagined_scenarios(&ImaginedScenarioFeedbackRequest {
            scenario_ids: recall.imagined_scenario_ids.clone(),
            accepted: false,
        })
        .await?;
    let runtime = adapter.runtime.lock().await;
    let mut rejected = 0;
    for scenario_id in &recall.imagined_scenario_ids {
        let scenario = runtime
            .repository()
            .load_imagined_scenario(memory_core::ScenarioId(Uuid::parse_str(scenario_id)?))
            .await?;
        if let Some(scenario) = scenario
            && scenario.status == memory_core::ImaginationStatus::Rejected
        {
            rejected += 1;
        }
    }
    Ok(EvaluationScenarioResult {
        name: "scenario acceptance/rejection feedback",
        passed: rejected == recall.imagined_scenario_ids.len() && rejected > 0,
        details: format!("rejected={rejected}"),
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

fn format_prompt_imagination_context(
    scenarios: &[memory_core::ImaginedScenario],
    self_model: Option<&memory_core::SelfModel>,
    config: PromptImaginationFormatConfig,
) -> Option<String> {
    if scenarios.is_empty() || config.max_chars == 0 || config.max_scenarios == 0 {
        return None;
    }

    let mut seen = HashSet::new();
    let mut lines = vec![
        "## Hypothetical Planning Scenarios".to_owned(),
        String::new(),
        "Use only for planning, brainstorming, or future-oriented reasoning. These are hypotheses, not verified facts.".to_owned(),
    ];

    if config.include_strategy_continuity {
        if let Some(strategy_line) = summarize_self_model_for_strategy(self_model) {
            lines.push(String::new());
            lines.push(format!("Strategy continuity: {strategy_line}"));
        }
    }

    for scenario in scenarios.iter().take(config.max_scenarios) {
        let summary = truncate_line(
            &format!(
                "{} [{} | plausible {:.2} | useful {:.2}]: {}",
                scenario.title.trim(),
                imagination_kind_label(scenario.kind),
                scenario.plausibility_score,
                scenario.usefulness_score,
                scenario.premise.trim()
            ),
            220,
        );
        if let Some(summary) = dedupe_text_allowing_hypothetical(&mut seen, summary) {
            lines.push(String::new());
            lines.push(format!("- {summary}"));
        }

        let outcome_lines = scenario
            .predicted_outcomes
            .iter()
            .filter_map(|outcome| {
                let line = truncate_line(outcome, 140);
                dedupe_text_allowing_hypothetical(&mut seen, line)
            })
            .take(config.max_outcomes_per_scenario)
            .collect::<Vec<_>>();
        for outcome in outcome_lines {
            lines.push(format!("  Outcome: {outcome}"));
        }
    }

    Some(trim_to_configured_length(lines, config.max_chars))
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

fn summarize_self_model_for_strategy(
    self_model: Option<&memory_core::SelfModel>,
) -> Option<String> {
    let self_model = self_model?;
    let strengths = self_model
        .recurring_strengths
        .iter()
        .filter_map(|value| summarize_phrase(value, 5))
        .take(1);
    let preferences = self_model
        .user_interaction_preferences
        .iter()
        .filter_map(|value| summarize_phrase(value, 6))
        .take(1);
    let tendencies = self_model
        .behavioral_tendencies
        .iter()
        .filter_map(|value| summarize_phrase(value, 6))
        .take(1);
    let domains = self_model
        .active_domains
        .iter()
        .filter_map(|value| summarize_phrase(value, 4))
        .take(1);

    let phrases = strengths
        .chain(preferences)
        .chain(tendencies)
        .chain(domains)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(3)
        .collect::<Vec<_>>();
    if phrases.is_empty() {
        None
    } else {
        Some(phrases.join("; "))
    }
}

fn summarize_phrase(value: &str, max_words: usize) -> Option<String> {
    let compact = value
        .trim()
        .split_whitespace()
        .take(max_words)
        .collect::<Vec<_>>()
        .join(" ");
    if compact.is_empty() {
        None
    } else {
        Some(compact)
    }
}

fn imagination_kind_label(kind: memory_core::ImaginedScenarioKind) -> &'static str {
    match kind {
        memory_core::ImaginedScenarioKind::FutureNeedPrediction => "future need",
        memory_core::ImaginedScenarioKind::AlternativePlan => "alternative plan",
        memory_core::ImaginedScenarioKind::Counterfactual => "counterfactual",
    }
}

fn trait_type_label(trait_type: memory_core::TraitType) -> &'static str {
    match trait_type {
        memory_core::TraitType::Curiosity => "curiosity",
        memory_core::TraitType::Caution => "caution",
        memory_core::TraitType::Verbosity => "verbosity",
        memory_core::TraitType::NoveltySeeking => "novelty_seeking",
        memory_core::TraitType::EvidenceReliance => "evidence_reliance",
        memory_core::TraitType::Reliability => "reliability",
        memory_core::TraitType::Practicality => "practicality",
        memory_core::TraitType::Proactivity => "proactivity",
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

fn dedupe_text_allowing_hypothetical(seen: &mut HashSet<String>, text: String) -> Option<String> {
    let dedupe_keys = dedupe_keys(&text);
    if dedupe_keys.is_empty() || dedupe_keys.iter().any(|key| seen.contains(key)) {
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
        memory_core::{
            ImaginationStatus, ImaginedScenario, ImaginedScenarioKind, LessonType, SelfModel,
            TraitType,
        },
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
                include_hypothetical: false,
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
            .filter(|node| node.node_type == NodeType::Preference)
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

    #[test]
    fn prompt_context_keeps_verified_and_hypothetical_sections_separate() {
        let hypothetical = format_prompt_imagination_context(
            &[sample_imagined_scenario()],
            Some(&sample_self_model()),
            PromptImaginationFormatConfig::default(),
        )
        .expect("hypothetical section should exist");

        assert!(hypothetical.contains("## Hypothetical Planning Scenarios"));
        assert!(hypothetical.contains("not verified facts"));
        assert!(!hypothetical.contains("## Verified Memory Context"));
    }

    #[test]
    fn hypothetical_formatter_uses_self_model_without_raw_dump() {
        let formatted = format_prompt_imagination_context(
            &[sample_imagined_scenario()],
            Some(&sample_self_model()),
            PromptImaginationFormatConfig::default(),
        )
        .expect("hypothetical section should exist");

        assert!(formatted.contains("Strategy continuity:"));
        assert!(!formatted.contains("supporting_lesson_ids"));
        assert!(!formatted.contains("version"));
        assert!(!formatted.contains("00000000"));
    }

    #[test]
    fn hypothetical_formatter_does_not_present_scenarios_as_verified_memory() {
        let formatted = format_prompt_imagination_context(
            &[sample_imagined_scenario()],
            None,
            PromptImaginationFormatConfig::default(),
        )
        .expect("hypothetical section should exist");

        assert!(formatted.contains("hypotheses, not verified facts"));
        assert!(!formatted.contains("Validated lessons:"));
        assert!(!formatted.contains("Other verified context:"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn imagined_scenarios_can_be_reviewed_after_prompt_use() {
        let (adapter, _dir) = open_test_adapter().await;
        adapter
            .propose_memory(&MemoryProposalRequest {
                session_id: Some("session-4".to_owned()),
                user_text: "Remember the current goal is to plan a staged release.".to_owned(),
                assistant_text: "Noted. The current goal is a staged release plan.".to_owned(),
                event_id: Uuid::new_v4().to_string(),
            })
            .await
            .expect("goal proposal should succeed");

        let recall = adapter
            .recall_context(&RecallRequest {
                text: "brainstorm options for the next staged release".to_owned(),
                session_id: Some("session-4".to_owned()),
                topic: Some("planning".to_owned()),
                include_hypothetical: true,
            })
            .await
            .expect("recall should succeed")
            .expect("recall should return context");

        assert!(recall.hypothetical_prompt_context.is_some());
        assert!(!recall.imagined_scenario_ids.is_empty());

        let review = adapter
            .review_imagined_scenarios(&ImaginedScenarioFeedbackRequest {
                scenario_ids: recall.imagined_scenario_ids.clone(),
                accepted: true,
            })
            .await
            .expect("review should succeed");
        assert_eq!(review.reviewed_count, recall.imagined_scenario_ids.len());

        let runtime = adapter.runtime.lock().await;
        let scenarios = runtime
            .repository()
            .list_imagined_scenarios(10)
            .await
            .expect("imagined scenarios should load");
        assert!(scenarios.iter().all(|scenario| {
            scenario.status == ImaginationStatus::AcceptedAsHypothesis
                || scenario.status == ImaginationStatus::Simulated
        }));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn recall_context_keeps_hypothetical_block_disabled_when_not_requested() {
        let (adapter, _dir) = open_test_adapter().await;
        adapter
            .propose_memory(&MemoryProposalRequest {
                session_id: Some("session-5".to_owned()),
                user_text: "Remember the goal is to plan a careful migration.".to_owned(),
                assistant_text: "Noted. The goal is a careful migration plan.".to_owned(),
                event_id: Uuid::new_v4().to_string(),
            })
            .await
            .expect("goal proposal should succeed");

        let recall = adapter
            .recall_context(&RecallRequest {
                text: "plan a careful migration".to_owned(),
                session_id: Some("session-5".to_owned()),
                topic: Some("planning".to_owned()),
                include_hypothetical: false,
            })
            .await
            .expect("recall should succeed")
            .expect("recall should return context");

        assert!(
            recall
                .verified_prompt_context
                .contains("## Verified Memory Context")
        );
        assert!(recall.hypothetical_prompt_context.is_none());
        assert!(recall.imagined_scenario_ids.is_empty());
        assert!(
            !recall
                .prompt_context
                .contains("## Hypothetical Planning Scenarios")
        );
    }

    #[test]
    fn inspection_report_render_is_stable_and_compact() {
        let report = MemoryInspectionReport {
            verified_packet_view: "## Verified Packet\nnodes=1 lessons=0 checkpoints=0 traits=0"
                .to_owned(),
            hypothetical_scenarios_view:
                "## Hypothetical Scenarios\nNo hypothetical scenarios attached.".to_owned(),
            self_model_continuity_view:
                "## Self-Model Continuity\nline: practical release planning".to_owned(),
            trait_update_reasons_view: "## Trait Update Reasons\nNo trait updates recorded."
                .to_owned(),
            lesson_reasons_view: "## Lesson Reasons\nNo lessons available.".to_owned(),
            superseded_history_view:
                "## Superseded Preferences And Goals\nNo preference or goal history available."
                    .to_owned(),
        };

        let rendered = report.render();
        assert!(rendered.contains("## Verified Packet"));
        assert!(rendered.contains("## Self-Model Continuity"));
        assert!(rendered.contains("## Superseded Preferences And Goals"));
        assert!(rendered.lines().count() <= 20);
    }

    #[test]
    fn evaluation_report_render_is_stable() {
        let report = EvaluationHarnessReport {
            scenarios: vec![
                EvaluationScenarioResult {
                    name: "stable preference recall",
                    passed: true,
                    details: "verified preference was recalled".to_owned(),
                },
                EvaluationScenarioResult {
                    name: "grounded imagination",
                    passed: false,
                    details: "no hypothetical planning block was produced".to_owned(),
                },
            ],
        };

        let rendered = report.render();
        assert!(rendered.starts_with("## Nodamem Evaluation"));
        assert!(rendered.contains("- stable preference recall [pass]:"));
        assert!(rendered.contains("- grounded imagination [fail]:"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn inspection_report_captures_memory_and_reason_views() {
        let (adapter, _dir) = open_test_adapter().await;
        adapter
            .propose_memory(&MemoryProposalRequest {
                session_id: Some("inspect-session".to_owned()),
                user_text: "Remember the user prefers concise deployment updates.".to_owned(),
                assistant_text: "I will keep deployment updates concise.".to_owned(),
                event_id: Uuid::new_v4().to_string(),
            })
            .await
            .expect("proposal should succeed");
        let _ = adapter
            .record_outcome(&OutcomeFeedbackRequest {
                outcome_id: Uuid::new_v4().to_string(),
                subject_node_id: None,
                success: true,
                usefulness: 0.8,
                prediction_correct: true,
                user_accepted: true,
                validated: true,
            })
            .await
            .expect("outcome should succeed");

        let report = adapter
            .inspect_memory_flow(
                &RecallRequest {
                    text: "What preference should guide the update?".to_owned(),
                    session_id: Some("inspect-session".to_owned()),
                    topic: Some("preference".to_owned()),
                    include_hypothetical: false,
                },
                InspectionConfig::default(),
            )
            .await
            .expect("inspection should succeed");

        let rendered = report.render();
        assert!(rendered.contains("## Verified Packet"));
        assert!(rendered.contains("## Trait Update Reasons"));
        assert!(rendered.contains("## Self-Model Continuity"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn evaluation_harness_runs_repeatable_scenarios() {
        let dir = tempfile::tempdir().expect("tempdir should exist");
        let report = NodamemAdapter::run_evaluation_harness_at(dir.path().join("eval.db"))
            .await
            .expect("evaluation harness should succeed");

        assert_eq!(report.scenarios.len(), 5);
        assert!(
            report
                .scenarios
                .iter()
                .any(|scenario| scenario.name == "stable preference recall")
        );
        assert!(report.render().contains("## Nodamem Evaluation"));
    }

    fn sample_imagined_scenario() -> ImaginedScenario {
        ImaginedScenario {
            id: memory_core::ScenarioId(Uuid::new_v4()),
            kind: ImaginedScenarioKind::AlternativePlan,
            status: ImaginationStatus::Simulated,
            title: "Alternative rollout path".to_owned(),
            premise: "If the release is staged by tenant tier, support load likely stays lower."
                .to_owned(),
            narrative: "A simulated option grounded in prior release coordination memory."
                .to_owned(),
            basis_source_node_ids: vec![NodeId(Uuid::new_v4())],
            basis_lesson_ids: vec![],
            active_goal_node_ids: vec![NodeId(Uuid::new_v4())],
            trait_snapshot: vec![],
            self_model_snapshot: None,
            predicted_outcomes: vec![
                "Risk falls if high-touch customers move last.".to_owned(),
                "Coordination cost rises slightly because rollout checkpoints increase.".to_owned(),
            ],
            plausibility_score: 0.79,
            novelty_score: 0.52,
            usefulness_score: 0.83,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn sample_self_model() -> SelfModel {
        SelfModel {
            id: memory_core::SelfModelId(Uuid::nil()),
            version: 7,
            recurring_strengths: vec!["practical release planning".to_owned()],
            user_interaction_preferences: vec!["prefer concise direct collaboration".to_owned()],
            behavioral_tendencies: vec!["stay evidence-led when weighing options".to_owned()],
            active_domains: vec!["release planning".to_owned()],
            supporting_lesson_ids: vec![],
            supporting_trait_ids: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }
}
