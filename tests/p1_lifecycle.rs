//! P1 出口条件集成测试(临时数据库,无 UI):
//! 创建会话 → 并行分支 → 多选追问 → 保存 → 重新加载;
//! 树结构、模型/请求快照、费用记录一致;启动恢复标记 Interrupted。

use aicouncil::domain::plan::{ContinuationSelection, ModelTarget, Planner};
use aicouncil::domain::{
    BatchId, Conversation, Generation, GenerationId, GenerationStatus, Message, MessageId,
    MessageTree, ProviderId, Role, UsageRecord,
};
use aicouncil::storage::{open_in_memory, Db, SendBatchRecord};
use chrono::Utc;

fn target(model: &str) -> ModelTarget {
    ModelTarget {
        provider_id: ProviderId::new(),
        model_id: model.to_string(),
        params: None,
        price: None,
    }
}

/// 把计划落库并构建内存树(模拟 ChatService.submit 的持久化部分)。
fn persist_plan(db: &Db, plan: &aicouncil::domain::SendPlan) -> BatchId {
    let batch = BatchId::new();
    let now = Utc::now();
    let mut users = Vec::new();
    let mut assistants = Vec::new();
    let mut generations = Vec::new();
    let mut usage = Vec::new();

    for b in &plan.branches {
        let user = Message {
            id: b.user_message_id,
            conversation_id: plan.conversation_id,
            parent_id: b.parent_leaf,
            role: Role::User,
            content: plan.prompt.clone(),
            attachments: plan.attachments.clone(),
            created_at: now,
        };
        users.push(user);
        for req in &b.requests {
            let assistant_id = MessageId::new();
            assistants.push(Message {
                id: assistant_id,
                conversation_id: plan.conversation_id,
                parent_id: Some(b.user_message_id),
                role: Role::Assistant,
                content: String::new(),
                attachments: Vec::new(),
                created_at: now,
            });
            generations.push(Generation {
                request_id: req.request_id,
                batch_id: batch,
                user_message_id: b.user_message_id,
                assistant_message_id: assistant_id,
                provider_id: req.provider_id,
                model_id: req.model_id.clone(),
                status: GenerationStatus::Queued,
                params: req.params.clone(),
                system_prompt: plan.system_prompt.clone(),
                context_snapshot: Vec::new(),
                price_snapshot: req.price_snapshot,
                remote_generation_id: None,
                error: None,
                retry_of: None,
                retry_root_id: Some(req.request_id),
                failure: None,
                request_snapshot: None,
                retry_not_before_ms: None,
                dispatch_phase: aicouncil::domain::DispatchPhase::NotDispatched,
                started_at: None,
                finished_at: None,
            });
            usage.push(UsageRecord::pending(req.request_id));
        }
    }

    let primary = &plan.branches[plan.primary.0].requests[plan.primary.1];
    db.save_send_batch(&SendBatchRecord {
        batch_id: batch,
        conversation_id: plan.conversation_id,
        primary_request_id: primary.request_id,
        user_messages: users,
        assistant_messages: assistants,
        generations,
        usage,
        intended_main_leaf: None,
        replacement: None,
        draft_consumption: None,
    })
    .expect("批次落库");
    batch
}

fn load_tree(db: &Db, conv: aicouncil::domain::ConversationId) -> MessageTree {
    let (tree, orphans) = MessageTree::from_messages(db.messages_of(conv).unwrap());
    assert_eq!(orphans, 0, "重载后不应有孤儿消息");
    tree
}

#[test]
fn full_branch_lifecycle_roundtrip() {
    let db = open_in_memory().unwrap();
    let conv_id = aicouncil::domain::ConversationId::new();
    db.upsert_conversation(&Conversation::new(conv_id, Utc::now()))
        .unwrap();

    // 1) 根问题 Q
    let plan1 = Planner::prepare(
        conv_id,
        &ContinuationSelection::Single {
            leaf: None,
            provider_id: ProviderId::new(),
            model_id: "openrouter/auto".into(),
        },
        "Q",
        &[],
        Some("你是助手".into()),
        &MessageTree::new(),
        Utc::now(),
    )
    .unwrap();
    persist_plan(&db, &plan1);
    let q = plan1.branches[0].user_message_id;
    let a1_placeholder = db
        .generations_of(conv_id)
        .unwrap()
        .iter()
        .find(|g| g.user_message_id == q)
        .map(|g| g.assistant_message_id)
        .unwrap();
    db.update_message_content(a1_placeholder, "A1 回答")
        .unwrap();
    db.update_generation_status(
        plan1.branches[0].requests[0].request_id,
        GenerationStatus::Completed,
        None,
        Some("gen-remote-1"),
    )
    .unwrap();

    // 2) 同题双模型:Q2 -> B1(模型A)/B2(模型C),主=A
    let mut tree = load_tree(&db, conv_id);
    let plan2 = Planner::prepare(
        conv_id,
        &ContinuationSelection::SameQuestionMulti {
            leaf: Some(a1_placeholder),
            targets: vec![target("modelA"), target("modelC")],
            primary: 0,
        },
        "Q2",
        &[],
        None,
        &tree,
        Utc::now(),
    )
    .unwrap();
    // 模拟占位落库并补全树,便于下一步多选
    persist_plan(&db, &plan2);
    let gens2 = db.generations_of(conv_id).unwrap();
    let b_ans: Vec<MessageId> = gens2
        .iter()
        .filter(|g| g.user_message_id == plan2.branches[0].user_message_id)
        .map(|g| g.assistant_message_id)
        .collect();
    assert_eq!(b_ans.len(), 2, "同题双模型应有 2 个 assistant 占位");
    for (i, g) in gens2
        .iter()
        .filter(|g| g.user_message_id == plan2.branches[0].user_message_id)
        .enumerate()
    {
        db.update_message_content(g.assistant_message_id, &format!("B{}", i + 1))
            .unwrap();
        db.update_generation_status(g.request_id, GenerationStatus::Completed, None, None)
            .unwrap();
    }
    tree = load_tree(&db, conv_id);

    // 3) 多选两支追问:A2 只含 A1,B2 只含 B1
    let plan3 = Planner::prepare(
        conv_id,
        &ContinuationSelection::MultiBranch {
            branches: vec![(b_ans[0], target("A")), (b_ans[1], target("C"))],
            primary: 0,
        },
        "Q3",
        &[],
        None,
        &tree,
        Utc::now(),
    )
    .unwrap();
    persist_plan(&db, &plan3);

    // 4) 重新加载:树结构一致
    let reloaded = load_tree(&db, conv_id);
    assert_eq!(
        reloaded.messages.len(),
        db.messages_of(conv_id).unwrap().len()
    );
    // B2(第2支)的历史含 B1 不含 B2;第1支反之 —— 无串支
    let first = &plan3.branches[0].requests[0];
    let second = &plan3.branches[1].requests[0];
    assert!(first.history.iter().any(|m| m.content.contains("B1")));
    assert!(!first.history.iter().any(|m| m.content.contains("B2")));
    assert!(second.history.iter().any(|m| m.content.contains("B2")));
    assert!(!second.history.iter().any(|m| m.content.contains("B1")));
    // 两支都包含公共前缀(A1 与 Q2)
    for h in [&first.history, &second.history] {
        assert!(h.iter().any(|m| m.content.contains("A1")));
        assert!(h.iter().any(|m| m.content == "Q2"));
    }
    // 5) 请求/模型快照与费用一致
    let gens = db.generations_of(conv_id).unwrap();
    assert_eq!(gens.len(), 1 + 2 + 2, "Q(1) + 同题双模型(2) + 多支(2)");
    let models: Vec<&str> = gens.iter().map(|g| g.model_id.as_str()).collect();
    assert!(models.contains(&"modelA") && models.contains(&"modelC"));

    let usage = db.usage_of(conv_id).unwrap();
    assert_eq!(usage.len(), 5, "每请求一条 pending 用量");
    assert!(usage
        .iter()
        .all(|u| u.status == aicouncil::domain::UsageStatus::Pending));
}

#[test]
fn restart_recovery_marks_interrupted() {
    let db = open_in_memory().unwrap();
    let conv_id = aicouncil::domain::ConversationId::new();
    db.upsert_conversation(&Conversation::new(conv_id, Utc::now()))
        .unwrap();
    let plan = Planner::prepare(
        conv_id,
        &ContinuationSelection::Single {
            leaf: None,
            provider_id: ProviderId::new(),
            model_id: "m".into(),
        },
        "Q",
        &[],
        None,
        &MessageTree::new(),
        Utc::now(),
    )
    .unwrap();
    persist_plan(&db, &plan);
    let req = plan.branches[0].requests[0].request_id;
    db.update_generation_status(req, GenerationStatus::Streaming, None, None)
        .unwrap();

    // 模拟重启:打开新连接(同一库),恢复
    let n = db.recover_interrupted().unwrap();
    assert_eq!(n, 1);
    let g = db
        .generations_of(conv_id)
        .unwrap()
        .into_iter()
        .find(|g| g.request_id == req)
        .unwrap();
    assert_eq!(g.status, GenerationStatus::Interrupted);
    // 再跑一次:幂等
    assert_eq!(db.recover_interrupted().unwrap(), 0);
}

#[test]
fn usage_upsert_idempotent_and_no_regression() {
    let db = open_in_memory().unwrap();
    let conv_id = aicouncil::domain::ConversationId::new();
    db.upsert_conversation(&Conversation::new(conv_id, Utc::now()))
        .unwrap();
    let plan = Planner::prepare(
        conv_id,
        &ContinuationSelection::Single {
            leaf: None,
            provider_id: ProviderId::new(),
            model_id: "m".into(),
        },
        "Q",
        &[],
        None,
        &MessageTree::new(),
        Utc::now(),
    )
    .unwrap();
    persist_plan(&db, &plan);
    let req = plan.branches[0].requests[0].request_id;

    let mut rec = UsageRecord::pending(req);
    rec.merge_known(
        Some(100),
        Some(50),
        Some(rust_decimal::Decimal::new(123, 4)),
        aicouncil::domain::CostSource::ApiReported,
        Some("gen-1".into()),
        Utc::now(),
    );
    db.upsert_usage(&rec).unwrap();

    // 迟到的重复报告(无费用):不追加、不回退
    let mut dup = UsageRecord::pending(req);
    dup.remote_generation_id = Some("gen-1".into());
    db.upsert_usage(&dup).unwrap();

    let all = db.usage_of(conv_id).unwrap();
    assert_eq!(all.len(), 1, "同一请求只有一条用量");
    assert_eq!(all[0].cost, Some(rust_decimal::Decimal::new(123, 4)));
    assert_eq!(all[0].status, aicouncil::domain::UsageStatus::Known);
}

#[test]
fn delete_group_moves_conversations_to_ungrouped() {
    let db = open_in_memory().unwrap();
    let group = aicouncil::domain::Group {
        id: aicouncil::domain::GroupId::new(),
        name: "工作".into(),
        sort_order: 0,
    };
    db.upsert_group(&group).unwrap();
    let mut conv = Conversation::new(aicouncil::domain::ConversationId::new(), Utc::now());
    conv.group_id = Some(group.id);
    db.upsert_conversation(&conv).unwrap();

    db.delete_group(group.id).unwrap();
    let reloaded = db.conversations().unwrap();
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].group_id, None, "S2:删除组仅取消分组");
}

#[test]
fn transaction_rollback_on_bad_parent() {
    let db = open_in_memory().unwrap();
    let conv_id = aicouncil::domain::ConversationId::new();
    db.upsert_conversation(&Conversation::new(conv_id, Utc::now()))
        .unwrap();
    let now = Utc::now();
    let user = Message {
        id: MessageId::new(),
        conversation_id: conv_id,
        parent_id: Some(MessageId::new()), // 不存在的父
        role: Role::User,
        content: "Q".into(),
        attachments: Vec::new(),
        created_at: now,
    };
    let rec = SendBatchRecord {
        batch_id: BatchId::new(),
        conversation_id: conv_id,
        primary_request_id: GenerationId::new(),
        user_messages: vec![user],
        assistant_messages: vec![],
        generations: vec![],
        usage: vec![],
        intended_main_leaf: None,
        replacement: None,
        draft_consumption: None,
    };
    assert!(db.save_send_batch(&rec).is_err(), "外键应拒绝");
    // 整体回滚:批次未写入
    let n: i64 = db
        .lock()
        .query_row("SELECT COUNT(*) FROM send_batches", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "失败批次必须整体回滚");
}
