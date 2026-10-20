use aicouncil::domain::{
    BatchId, Conversation, ConversationId, DispatchPhase, Generation, GenerationId,
    GenerationStatus, Message, MessageId, Planner, ProviderId, Role, UsageRecord,
};
use aicouncil::storage::{open_in_memory, SendBatchRecord};
use chrono::Utc;

fn message(conv: ConversationId, parent: Option<MessageId>, role: Role, content: &str) -> Message {
    Message {
        id: MessageId::new(),
        conversation_id: conv,
        parent_id: parent,
        role,
        content: content.into(),
        attachments: Vec::new(),
        created_at: Utc::now(),
    }
}

fn seed_old_turn(
    db: &aicouncil::storage::Db,
    conv: ConversationId,
) -> (MessageId, MessageId, MessageId, GenerationId) {
    db.upsert_conversation(&Conversation::new(conv, Utc::now()))
        .unwrap();
    let user = message(conv, None, Role::User, "old question");
    let assistant_a = message(conv, Some(user.id), Role::Assistant, "old answer A");
    let assistant_b = message(conv, Some(user.id), Role::Assistant, "old answer B");
    let request = GenerationId::new();
    let batch = BatchId::new();
    {
        let conn = db.lock();
        conn.execute(
            "INSERT INTO messages(id,conversation_id,parent_id,role,content,attachments_json,created_at) VALUES (?1,?2,NULL,'user',?3,'[]',?4)",
            rusqlite::params![user.id.as_str(), conv.as_str(), user.content, user.created_at.timestamp_millis()],
        ).unwrap();
        for assistant in [&assistant_a, &assistant_b] {
            conn.execute(
                "INSERT INTO messages(id,conversation_id,parent_id,role,content,attachments_json,created_at) VALUES (?1,?2,?3,'assistant',?4,'[]',?5)",
                rusqlite::params![assistant.id.as_str(), conv.as_str(), user.id.as_str(), assistant.content, assistant.created_at.timestamp_millis()],
            ).unwrap();
        }
        conn.execute(
            "INSERT INTO send_batches(id,conversation_id,primary_request_id,sent_at) VALUES (?1,?2,?3,?4)",
            rusqlite::params![batch.as_str(), conv.as_str(), request.as_str(), Utc::now().timestamp_millis()],
        ).unwrap();
        conn.execute(
            "INSERT INTO generations(request_id,batch_id,user_message_id,assistant_message_id,provider_id,model_id,status,context_json,dispatch_phase,started_at) VALUES (?1,?2,?3,?4,?5,'test/model','completed','[]','accepted',?6)",
            rusqlite::params![request.as_str(), batch.as_str(), user.id.as_str(), assistant_a.id.as_str(), ProviderId::new().as_str(), Utc::now().timestamp_millis()],
        ).unwrap();
        conn.execute(
            "INSERT INTO usage_records(request_id,cost_text,cost_source,status) VALUES (?1,'0.0123','api','known')",
            [request.as_str()],
        ).unwrap();
    }
    db.set_conversation_main_leaf(conv, Some(assistant_a.id))
        .unwrap();
    (user.id, assistant_a.id, assistant_b.id, request)
}

fn replacement_record(
    conv: ConversationId,
    old_root: MessageId,
    expected_leaf: MessageId,
    new_parent: Option<MessageId>,
) -> SendBatchRecord {
    let user = message(conv, new_parent, Role::User, "edited question");
    let assistant_a = message(conv, Some(user.id), Role::Assistant, "");
    let assistant_b = message(conv, Some(user.id), Role::Assistant, "");
    let batch = BatchId::new();
    let provider = ProviderId::new();
    let generations: Vec<_> = [&assistant_a, &assistant_b]
        .into_iter()
        .map(|assistant| {
            let request_id = GenerationId::new();
            Generation {
                request_id,
                batch_id: batch,
                user_message_id: user.id,
                assistant_message_id: assistant.id,
                provider_id: provider,
                model_id: "test/model".into(),
                status: GenerationStatus::Queued,
                params: None,
                system_prompt: None,
                context_snapshot: Vec::new(),
                price_snapshot: None,
                remote_generation_id: None,
                error: None,
                retry_of: None,
                retry_root_id: Some(request_id),
                failure: None,
                request_snapshot: None,
                retry_not_before_ms: None,
                dispatch_phase: DispatchPhase::NotDispatched,
                started_at: None,
                finished_at: None,
            }
        })
        .collect();
    SendBatchRecord {
        batch_id: batch,
        conversation_id: conv,
        primary_request_id: generations[0].request_id,
        user_messages: vec![user.clone()],
        assistant_messages: vec![assistant_a.clone(), assistant_b],
        usage: generations
            .iter()
            .map(|generation| UsageRecord::pending(generation.request_id))
            .collect(),
        generations,
        intended_main_leaf: Some((Some(expected_leaf), assistant_a.id)),
        replacement: Some((old_root, Some(expected_leaf))),
        draft_consumption: None,
    }
}

#[test]
fn removing_message_reparents_visible_tree_and_excludes_it_from_planned_context() {
    let db = open_in_memory().unwrap();
    let conv = ConversationId::new();
    db.upsert_conversation(&Conversation::new(conv, Utc::now()))
        .unwrap();
    let user1 = message(conv, None, Role::User, "private removed text");
    let answer1 = message(conv, Some(user1.id), Role::Assistant, "first answer");
    let user2 = message(conv, Some(answer1.id), Role::User, "next question");
    let answer2 = message(conv, Some(user2.id), Role::Assistant, "last answer");
    for item in [&user1, &answer1, &user2, &answer2] {
        db.insert_message(item).unwrap();
    }
    db.set_conversation_main_leaf(conv, Some(answer2.id))
        .unwrap();
    db.remove_message(user1.id).unwrap();
    let visible = db.messages_of(conv).unwrap();
    assert!(visible
        .iter()
        .all(|m| !m.content.contains("private removed text")));
    let (tree, orphans) = aicouncil::domain::MessageTree::from_messages(visible);
    assert_eq!(orphans, 0);
    let selection = aicouncil::domain::ContinuationSelection::Single {
        leaf: Some(answer2.id),
        provider_id: ProviderId::new(),
        model_id: "test/model".into(),
    };
    let plan = Planner::prepare(conv, &selection, "new", &[], None, &tree, Utc::now()).unwrap();
    assert!(plan.branches[0].requests[0]
        .history
        .iter()
        .all(|entry| !entry.content.contains("private removed text")));
}

#[test]
fn replacement_is_atomic_and_hides_all_parallel_answers_but_keeps_usage() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conversation.db");
    let db = aicouncil::storage::open(&db_path).unwrap();
    let conv = ConversationId::new();
    let (old_user, old_a, old_b, request) = seed_old_turn(&db, conv);
    let record = replacement_record(conv, old_user, old_a, None);
    db.save_send_batch(&record).unwrap();
    drop(db);
    let db = aicouncil::storage::open(&db_path).unwrap();
    let visible = db.messages_of(conv).unwrap();
    assert!(visible
        .iter()
        .all(|m| m.id != old_user && m.id != old_a && m.id != old_b));
    assert!(visible.iter().any(|m| m.content == "edited question"));
    assert_eq!(
        db.usage_of(conv)
            .unwrap()
            .iter()
            .find(|u| u.request_id == request)
            .unwrap()
            .cost
            .unwrap()
            .to_string(),
        "0.0123"
    );
    let new_leaf = db
        .conversations()
        .unwrap()
        .into_iter()
        .find(|c| c.id == conv)
        .unwrap()
        .main_leaf_id
        .unwrap();
    assert!(visible.iter().any(|m| m.id == new_leaf));
}

#[test]
fn replacement_cas_failure_leaves_old_turn_visible_and_writes_nothing() {
    let db = open_in_memory().unwrap();
    let conv = ConversationId::new();
    let (old_user, old_a, _, _) = seed_old_turn(&db, conv);
    let record = replacement_record(conv, old_user, old_a, None);
    // Move the conversation after the replacement intent was captured.
    let other = message(conv, None, Role::User, "concurrent question");
    db.insert_message(&other).unwrap();
    db.set_conversation_main_leaf(conv, Some(other.id)).unwrap();
    assert!(db.save_send_batch(&record).is_err());
    let visible = db.messages_of(conv).unwrap();
    assert!(visible.iter().any(|m| m.id == old_user));
    assert!(visible.iter().any(|m| m.id == old_a));
    assert!(!visible.iter().any(|m| m.content == "edited question"));
}

#[test]
fn deleting_successive_leaf_ancestors_rewinds_to_nearest_visible_parent() {
    let db = open_in_memory().unwrap();
    let conv = ConversationId::new();
    db.upsert_conversation(&Conversation::new(conv, Utc::now()))
        .unwrap();
    let a = message(conv, None, Role::User, "a");
    let b = message(conv, Some(a.id), Role::Assistant, "b");
    let c = message(conv, Some(b.id), Role::User, "c");
    let d = message(conv, Some(c.id), Role::Assistant, "d");
    for item in [&a, &b, &c, &d] {
        db.insert_message(item).unwrap();
    }
    db.set_conversation_main_leaf(conv, Some(d.id)).unwrap();
    db.remove_message(c.id).unwrap();
    db.remove_message(b.id).unwrap();
    db.remove_message(d.id).unwrap();
    assert_eq!(
        db.conversations()
            .unwrap()
            .into_iter()
            .find(|v| v.id == conv)
            .unwrap()
            .main_leaf_id,
        Some(a.id)
    );
    assert!(db.messages_of(conv).unwrap().iter().all(|m| m.id != d.id));
}
