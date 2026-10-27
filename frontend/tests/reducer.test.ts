import { describe, expect, test } from "bun:test";

import { reducer, type Action } from "../src/state/reducer";
import { initialState, type AppState } from "../src/state/types";
import type { Generation, Message } from "../src/ipc/types";

function apply(state: AppState, ...actions: Action[]): AppState {
  return actions.reduce(reducer, state);
}

function message(id: string, parent: string | null, role: "user" | "assistant"): Message {
  return {
    id,
    conversation_id: "c1",
    parent_id: parent,
    role,
    content: "",
    attachments: [],
    created_at_ms: 0,
  };
}

describe("conversation state", () => {
  test("loading a conversation stores its snapshot", () => {
    const state = apply(initialState, {
      type: "conversation/loaded",
      conversationId: "c1",
      conversation: {
        id: "c1",
        title: "Chat",
        group_id: null,
        system_prompt: null,
        title_source: "empty",
        title_status: "needs_generation",
        title_revision: 0,
        main_leaf_id: null,
        draft_revision: 4,
        created_at_ms: 0,
        updated_at_ms: 0,
      },
      messages: [message("m1", null, "user")],
      generations: [],
      usage: [],
      draftRevision: 4,
      editTargetId: null,
      editText: "",
      editAttachments: [],
      editLatestEligible: null,
    });
    expect(state.messages.c1).toHaveLength(1);
    expect(state.draftRevision).toBe(4);
    expect(state.conversations[0].id).toBe("c1");
  });

  test("removing a conversation clears its snapshot", () => {
    const state = apply(
      { ...initialState, messages: { c1: [message("m1", null, "user")] } },
      { type: "conversation/removed", conversationId: "c1" },
    );
    expect(state.messages.c1).toBeUndefined();
    expect(state.conversations).toHaveLength(0);
  });

  test("removing a message re-parents its children", () => {
    const state = apply(
      {
        ...initialState,
        messages: {
          c1: [
            message("root", null, "user"),
            message("mid", "root", "assistant"),
            message("leaf", "mid", "assistant"),
          ],
        },
      },
      { type: "message/removed", conversationId: "c1", messageId: "mid" },
    );
    const leaf = state.messages.c1.find((entry) => entry.id === "leaf");
    expect(leaf?.parent_id).toBe("root");
  });

  test("removing the edited question clears the edit buffer", () => {
    const state = apply(
      { ...initialState, editingLatest: "m1", editText: "x", editAttachments: [] },
      { type: "message/removed", conversationId: "c1", messageId: "m1" },
    );
    expect(state.editingLatest).toBeNull();
    expect(state.editText).toBe("");
  });

  test("conversation upsert keeps the newest chat first", () => {
    const base: AppState = {
      ...initialState,
      conversations: [
        {
          id: "a",
          title: "A",
          group_id: null,
          system_prompt: null,
          title_source: "empty",
          title_status: "needs_generation",
          title_revision: 0,
          main_leaf_id: null,
          draft_revision: 0,
          created_at_ms: 0,
          updated_at_ms: 10,
        },
      ],
    };
    const state = apply(base, {
      type: "conversation/upserted",
      conversation: {
        id: "b",
        title: "B",
        group_id: null,
        system_prompt: null,
        title_source: "empty",
        title_status: "needs_generation",
        title_revision: 0,
        main_leaf_id: null,
        draft_revision: 0,
        created_at_ms: 0,
        updated_at_ms: 20,
      },
    });
    expect(state.conversations.map((entry) => entry.id)).toEqual(["b", "a"]);
  });
});

describe("streaming", () => {
  test("appends a delta to the assistant message", () => {
    const state = apply(
      { ...initialState, messages: { c1: [message("m1", null, "assistant")] } },
      { type: "stream/chunk", conversationId: "c1", messageId: "m1", delta: "Hello" },
      { type: "stream/chunk", conversationId: "c1", messageId: "m1", delta: " world" },
    );
    expect(state.messages.c1[0].content).toBe("Hello world");
  });

  test("ignores a delta for an unknown message", () => {
    const state = apply(initialState, {
      type: "stream/chunk",
      conversationId: "c1",
      messageId: "missing",
      delta: "x",
    });
    expect(state.messages.c1).toBeUndefined();
  });

  test("a generation upsert replaces the same request", () => {
    const generation: Generation = {
      request_id: "r1",
      batch_id: "b1",
      user_message_id: "u1",
      assistant_message_id: "a1",
      provider_id: "p1",
      model_id: "m",
      status: "streaming",
      system_prompt: null,
      price_prompt: null,
      price_completion: null,
      remote_generation_id: null,
      error: null,
      retry_of: null,
      retry_root_id: null,
      retry_not_before_ms: null,
      dispatch_phase: "attempted",
      failure_kind: null,
      failure_message: null,
      started_at_ms: 1,
      finished_at_ms: null,
    };
    const state = apply(
      initialState,
      { type: "generation/upserted", conversationId: "c1", generation },
      {
        type: "generation/upserted",
        conversationId: "c1",
        generation: { ...generation, status: "completed" },
      },
    );
    expect(state.generations.c1).toHaveLength(1);
    expect(state.generations.c1[0].status).toBe("completed");
  });
});

describe("model selection", () => {
  test("checking a model adds it to the checked set", () => {
    const state = apply(
      initialState,
      { type: "model/checked", modelId: "a", checked: true },
      { type: "model/checked", modelId: "b", checked: true },
      { type: "model/checked", modelId: "a", checked: false },
    );
    expect(state.modelSelection.checked).toEqual(["b"]);
  });

  test("setting the primary model marks it as the default", () => {
    const state = apply(
      {
        ...initialState,
        preferences: [
          {
            provider_id: "p",
            model_id: "a",
            enabled: true,
            sort_order: 0,
            is_default: true,
            overrides: null,
          },
          {
            provider_id: "p",
            model_id: "b",
            enabled: true,
            sort_order: 1,
            is_default: false,
            overrides: null,
          },
        ],
      },
      { type: "model/primary", modelId: "b" },
    );
    expect(state.preferences.find((p) => p.model_id === "a")?.is_default).toBe(false);
    expect(state.preferences.find((p) => p.model_id === "b")?.is_default).toBe(true);
    expect(state.modelSelection.primary).toBe("b");
  });
});

describe("composer", () => {
  test("clearing the composer also clears the edit buffer", () => {
    const state = apply(
      { ...initialState, composerText: "x", editingLatest: "m1", editText: "y" },
      { type: "composer/clear" },
    );
    expect(state.composerText).toBe("");
    expect(state.editingLatest).toBeNull();
    expect(state.editText).toBe("");
  });

  test("the draft revision is updated from the backend reply", () => {
    const state = apply(initialState, { type: "draft/revision", revision: 7 });
    expect(state.draftRevision).toBe(7);
  });
});

describe("panels and toasts", () => {
  test("the toast is replaced and can be dismissed", () => {
    let state = apply(initialState, { type: "toast/show", text: "one", kind: "info" });
    expect(state.toast?.text).toBe("one");
    state = apply(state, { type: "toast/show", text: "two", kind: "error" });
    expect(state.toast?.text).toBe("two");
    expect(state.toast?.kind).toBe("error");
    state = apply(state, { type: "toast/hide" });
    expect(state.toast).toBeNull();
  });

  test("the sidebar collapse can be forced", () => {
    const state = apply(initialState, { type: "sidebar/toggle", collapsed: true });
    expect(state.sidebarCollapsed).toBe(true);
    const toggled = apply(state, { type: "sidebar/toggle" });
    expect(toggled.sidebarCollapsed).toBe(false);
  });
});
