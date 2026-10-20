use aicouncil::domain::plan::{ContinuationSelection, Planner};
use aicouncil::domain::{
    Conversation, Generation, GenerationId, GenerationStatus, Message, MessageId, MessageTree,
    ProviderId, Role, UsageRecord,
};
use aicouncil::storage::{open_in_memory, Db, SendBatchRecord};
use chrono::Utc;

fn terminal_request(
    db: &Db,
    with_remote_id: bool,
) -> (aicouncil::domain::ConversationId, GenerationId) {
    let conversation = aicouncil::domain::ConversationId::new();
    db.upsert_conversation(&Conversation::new(conversation, Utc::now()))
        .unwrap();
    let plan = Planner::prepare(
        conversation,
        &ContinuationSelection::Single {
            leaf: None,
            provider_id: ProviderId::new(),
            model_id: "test-model".into(),
        },
        "question",
        &[],
        None,
        &MessageTree::new(),
        Utc::now(),
    )
    .unwrap();
    let branch = &plan.branches[0];
    let request = branch.requests[0].request_id;
    let batch = aicouncil::domain::BatchId::new();
    let assistant = MessageId::new();
    let now = Utc::now();
    let mut generation = Generation {
        request_id: request,
        batch_id: batch,
        user_message_id: branch.user_message_id,
        assistant_message_id: assistant,
        provider_id: branch.requests[0].provider_id,
        model_id: branch.requests[0].model_id.clone(),
        status: GenerationStatus::Completed,
        params: None,
        system_prompt: None,
        context_snapshot: Vec::new(),
        price_snapshot: None,
        remote_generation_id: None,
        error: None,
        retry_of: None,
        retry_root_id: Some(request),
        failure: None,
        request_snapshot: None,
        retry_not_before_ms: None,
        // Legacy migrations default this field to not_dispatched even if the
        // terminal generation retained a usable remote ID.
        dispatch_phase: aicouncil::domain::DispatchPhase::NotDispatched,
        started_at: Some(now),
        finished_at: Some(now),
    };
    if with_remote_id {
        generation.remote_generation_id = Some("gen-existing".into());
    }
    db.save_send_batch(&SendBatchRecord {
        batch_id: batch,
        conversation_id: conversation,
        primary_request_id: request,
        user_messages: vec![Message {
            id: branch.user_message_id,
            conversation_id: conversation,
            parent_id: None,
            role: Role::User,
            content: "question".into(),
            attachments: Vec::new(),
            created_at: now,
        }],
        assistant_messages: vec![Message {
            id: assistant,
            conversation_id: conversation,
            parent_id: Some(branch.user_message_id),
            role: Role::Assistant,
            content: String::new(),
            attachments: Vec::new(),
            created_at: now,
        }],
        generations: vec![generation],
        usage: vec![UsageRecord::pending(request)],
        intended_main_leaf: None,
        replacement: None,
        draft_consumption: None,
    })
    .unwrap();
    (conversation, request)
}

#[test]
fn terminal_history_with_remote_id_is_repairable_and_without_id_is_unknown() {
    let db = open_in_memory().unwrap();
    let (known_conv, known_request) = terminal_request(&db, true);
    let (unknown_conv, unknown_request) = terminal_request(&db, false);

    let candidates = db.terminal_pending_usage(20).unwrap();
    let known = candidates
        .iter()
        .find(|record| record.request_id == known_request)
        .unwrap();
    assert_eq!(known.remote_generation_id.as_deref(), Some("gen-existing"));

    let unknown = candidates
        .iter()
        .find(|record| record.request_id == unknown_request)
        .unwrap();
    assert!(unknown.remote_generation_id.is_none());
    db.mark_usage_unavailable(unknown_request, "no persisted remote id")
        .unwrap();
    let record = db
        .usage_of(unknown_conv)
        .unwrap()
        .into_iter()
        .find(|record| record.request_id == unknown_request)
        .unwrap();
    assert_eq!(record.status, aicouncil::domain::UsageStatus::Unavailable);
    assert!(record.cost.is_none());

    assert_eq!(db.usage_of(known_conv).unwrap().len(), 1);
}

#[test]
fn query_cost_cannot_replace_api_cost_but_api_can_replace_query_cost() {
    let db = open_in_memory().unwrap();
    let (conversation, request) = terminal_request(&db, true);
    let mut query = UsageRecord::pending(request);
    query.merge_known(
        Some(10),
        Some(3),
        Some(rust_decimal::Decimal::new(5, 3)),
        aicouncil::domain::CostSource::GenerationQuery,
        Some("gen-existing".into()),
        Utc::now(),
    );
    db.upsert_usage(&query).unwrap();
    let mut api = UsageRecord::pending(request);
    api.merge_known(
        None,
        None,
        Some(rust_decimal::Decimal::new(4, 3)),
        aicouncil::domain::CostSource::ApiReported,
        None,
        Utc::now(),
    );
    db.upsert_usage(&api).unwrap();
    let mut stale_query = UsageRecord::pending(request);
    stale_query.merge_known(
        None,
        None,
        Some(rust_decimal::Decimal::new(6, 3)),
        aicouncil::domain::CostSource::GenerationQuery,
        None,
        Utc::now(),
    );
    db.upsert_usage(&stale_query).unwrap();

    let row = db.usage_of(conversation).unwrap().remove(0);
    assert_eq!(row.cost, Some(rust_decimal::Decimal::new(4, 3)));
    assert_eq!(
        row.cost_source,
        Some(aicouncil::domain::CostSource::ApiReported)
    );
}
