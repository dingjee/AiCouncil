import { describe, expect, test } from "bun:test";

import {
  answerSet,
  branchableAnswers,
  buildThread,
  buildTree,
  childrenOf,
  leaves,
  pathFromRoot,
} from "../src/state/tree";
import type { Generation, Message } from "../src/ipc/types";

function message(
  id: string,
  parent: string | null,
  role: "user" | "assistant",
  at = 0,
): Message {
  return {
    id,
    conversation_id: "c1",
    parent_id: parent,
    role,
    content: id,
    attachments: [],
    created_at_ms: at,
  };
}

function generation(
  requestId: string,
  assistantId: string,
  userId: string,
  model: string,
  root: string | null,
  at: number,
): Generation {
  return {
    request_id: requestId,
    batch_id: "b1",
    user_message_id: userId,
    assistant_message_id: assistantId,
    provider_id: "p1",
    model_id: model,
    status: "completed",
    system_prompt: null,
    price_prompt: null,
    price_completion: null,
    remote_generation_id: null,
    error: null,
    retry_of: null,
    retry_root_id: root,
    retry_not_before_ms: null,
    dispatch_phase: "remote_accepted",
    failure_kind: null,
    failure_message: null,
    started_at_ms: at,
    finished_at_ms: at,
  };
}

describe("message tree", () => {
  const messages = [
    message("root", null, "user", 1),
    message("a1", "root", "assistant", 2),
    message("a2", "root", "assistant", 3),
    message("a1b", "a1", "assistant", 4),
  ];

  test("builds the ancestor chain root → leaf", () => {
    const tree = buildTree(messages);
    expect(pathFromRoot(tree, "a1b")?.map((m) => m.id)).toEqual(["root", "a1", "a1b"]);
    expect(pathFromRoot(tree, null)).toEqual([]);
  });

  test("detects a broken chain", () => {
    const tree = buildTree([message("orphan", "missing", "user")]);
    expect(pathFromRoot(tree, "orphan")).toBeNull();
  });

  test("lists the children of a node in creation order", () => {
    const tree = buildTree(messages);
    expect(childrenOf(tree, "root").map((m) => m.id)).toEqual(["a1", "a2"]);
    expect(childrenOf(tree, null).map((m) => m.id)).toEqual(["root"]);
  });

  test("lists the leaves", () => {
    const tree = buildTree(messages);
    expect(leaves(tree).map((m) => m.id)).toEqual(["a2", "a1b"]);
  });
});

describe("answer sets", () => {
  test("folds retry attempts of the same lineage", () => {
    const messages = [
      message("q", null, "user", 1),
      message("first", "q", "assistant", 2),
      message("retry", "q", "assistant", 3),
    ];
    const root = "root-req";
    const generations = [
      generation(root, "first", "q", "vendor/alpha", root, 2),
      generation("req-2", "retry", "q", "vendor/alpha", root, 3),
    ];
    const tree = buildTree(messages);
    const items = answerSet(tree, generations, "q");
    expect(items).toHaveLength(2);
    expect(items[0].superseded).toBe(true);
    expect(items[0].attempt).toBe(1);
    expect(items[0].attempts_total).toBe(2);
    expect(items[1].superseded).toBe(false);
    expect(items[1].attempt).toBe(2);
  });

  test("keeps different models separate", () => {
    const messages = [
      message("q", null, "user", 1),
      message("a", "q", "assistant", 2),
      message("b", "q", "assistant", 3),
    ];
    const generations = [
      generation("r1", "a", "q", "vendor/alpha", null, 2),
      generation("r2", "b", "q", "vendor/beta", null, 3),
    ];
    const items = answerSet(buildTree(messages), generations, "q");
    expect(items).toHaveLength(2);
    expect(items.every((item) => !item.superseded)).toBe(true);
  });
});

describe("thread view", () => {
  test("follows the main branch", () => {
    const messages = [
      message("q1", null, "user", 1),
      message("a1", "q1", "assistant", 2),
      message("q2", "a1", "user", 3),
      message("a2", "q2", "assistant", 4),
      message("other", "q1", "assistant", 5),
    ];
    const turns = buildThread(messages, [], "a2");
    expect(turns.map((turn) => turn.question.id)).toEqual(["q1", "q2"]);
  });

  test("lists only completed answers as branchable", () => {
    const messages = [
      message("q", null, "user", 1),
      message("done", "q", "assistant", 2),
      message("live", "q", "assistant", 3),
    ];
    const generations = [
      { ...generation("r1", "done", "q", "m", null, 2), status: "completed" as const },
      { ...generation("r2", "live", "q", "m", null, 3), status: "streaming" as const },
    ];
    expect(branchableAnswers(messages, generations).map((m) => m.id)).toEqual(["done"]);
  });
});
