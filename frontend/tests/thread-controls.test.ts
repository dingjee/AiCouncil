import { describe, expect, test } from "bun:test";

import type { Generation, Message } from "../src/ipc/types";
import { threadControls } from "../src/state/selectors";

/**
 * The thread controls decide what the message thread shows.
 *
 * The user asked for a simple surface: a single-model chat without branches
 * must not look like a multi-model comparison. These tests pin the rules
 * that produce that behaviour.
 */

function user(id: string, parentId: string | null): Message {
  return {
    id,
    conversation_id: "c1",
    parent_id: parentId,
    role: "user",
    content: "question",
    attachments: [],
    created_at_ms: 1,
  };
}

function assistant(id: string, parentId: string): Message {
  return {
    id,
    conversation_id: "c1",
    parent_id: parentId,
    role: "assistant",
    content: "answer",
    attachments: [],
    created_at_ms: 2,
  };
}

function gen(
  requestId: string,
  assistantId: string,
  modelId: string,
  retryRootId: string | null = null,
): Generation {
  return {
    request_id: requestId,
    batch_id: "b1",
    user_message_id: "q1",
    assistant_message_id: assistantId,
    provider_id: "p1",
    model_id: modelId,
    status: "completed",
    system_prompt: null,
    price_prompt: null,
    price_completion: null,
    remote_generation_id: null,
    error: null,
    retry_of: null,
    retry_root_id: retryRootId,
    retry_not_before_ms: null,
    dispatch_phase: "remote_accepted",
    failure_kind: null,
    failure_message: null,
    started_at_ms: 1,
    finished_at_ms: 2,
  };
}

describe("thread controls", () => {
  test("an empty conversation shows nothing", () => {
    const controls = threadControls([], []);
    expect(controls.showBranchControls).toBe(false);
    expect(controls.multiModel).toBe(false);
    expect(controls.branched).toBe(false);
    expect(controls.candidates).toBe(0);
  });

  test("a linear single-model chat has no branch controls", () => {
    const messages = [
      user("q1", null),
      assistant("a1", "q1"),
      user("q2", "a1"),
      assistant("a2", "q2"),
    ];
    const generations = [gen("g1", "a1", "vendor/alpha"), gen("g2", "a2", "vendor/alpha")];
    const controls = threadControls(messages, generations);
    expect(controls.multiModel).toBe(false);
    expect(controls.branched).toBe(false);
    expect(controls.showBranchControls).toBe(false);
    expect(controls.modelIds).toEqual(["vendor/alpha"]);
  });

  test("two models answering the same question are a comparison", () => {
    const messages = [user("q1", null), assistant("a1", "q1"), assistant("a2", "q1")];
    const generations = [
      gen("g1", "a1", "vendor/alpha"),
      gen("g2", "a2", "vendor/beta"),
    ];
    const controls = threadControls(messages, generations);
    expect(controls.multiModel).toBe(true);
    expect(controls.showBranchControls).toBe(true);
    expect(controls.modelIds).toEqual(["vendor/alpha", "vendor/beta"]);
  });

  test("a retry of the same model is neither a comparison nor a branch", () => {
    const messages = [user("q1", null), assistant("a1", "q1"), assistant("a2", "q1")];
    const generations = [
      gen("g1", "a1", "vendor/alpha", "g1"),
      gen("g2", "a2", "vendor/alpha", "g1"),
    ];
    const controls = threadControls(messages, generations);
    expect(controls.multiModel).toBe(false);
    expect(controls.branched).toBe(false);
    expect(controls.showBranchControls).toBe(false);
    // The older attempt does not count as a candidate either.
    expect(controls.candidates).toBe(1);
  });

  test("continuing from the same answer twice is a branch", () => {
    const messages = [
      user("q1", null),
      assistant("a1", "q1"),
      user("q2", "a1"),
      user("q3", "a1"),
    ];
    const generations = [gen("g1", "a1", "vendor/alpha")];
    const controls = threadControls(messages, generations);
    expect(controls.branched).toBe(true);
    expect(controls.showBranchControls).toBe(true);
  });

  test("a branch in a side path is still a branch", () => {
    const messages = [
      user("q1", null),
      assistant("a1", "q1"),
      user("q2", "a1"),
      assistant("a2", "q2"),
      user("q3", "a1"),
    ];
    const generations = [
      gen("g1", "a1", "vendor/alpha"),
      gen("g2", "a2", "vendor/alpha"),
    ];
    const controls = threadControls(messages, generations);
    expect(controls.branched).toBe(true);
    expect(controls.showBranchControls).toBe(true);
  });
});
