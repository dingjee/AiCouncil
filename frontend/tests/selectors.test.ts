import { describe, expect, test } from "bun:test";

import {
  conversationsByGroup,
  enabledModels,
  filterModelGroups,
  groupModels,
  resolveSelection,
  usageSummary,
} from "../src/state/selectors";
import { initialState, type AppState } from "../src/state/types";
import type { CatalogEntry, ModelPreferenceEntry } from "../src/ipc/types";

function stateWith(overrides: Partial<AppState>): AppState {
  return { ...initialState, ...overrides };
}

const preferences: ModelPreferenceEntry[] = [
  {
    provider_id: "p1",
    model_id: "vendor/alpha",
    enabled: true,
    sort_order: 0,
    is_default: true,
    overrides: null,
  },
  {
    provider_id: "p1",
    model_id: "vendor/beta",
    enabled: true,
    sort_order: 1,
    is_default: false,
    overrides: null,
  },
  {
    provider_id: "p1",
    model_id: "vendor/gamma",
    enabled: false,
    sort_order: 2,
    is_default: false,
    overrides: null,
  },
];

describe("resolveSelection", () => {
  test("disables the send when no model is enabled", () => {
    const result = resolveSelection([], [], {
      parallel: false,
      checked: [],
      primary: null,
    });
    expect(result.invalidReason).toBe("composer.send_disabled_no_model");
    expect(result.targets).toEqual([]);
  });

  test("an empty selection disables the send (no silent fallback)", () => {
    const result = resolveSelection(["a", "b"], [], {
      parallel: false,
      checked: [],
      primary: "a",
    });
    expect(result.invalidReason).toBe("composer.send_disabled_no_targets");
    expect(result.targets).toEqual([]);
    expect(result.primary).toBeNull();
  });

  test("one checked model is the whole selection", () => {
    const result = resolveSelection(["a", "b"], [], {
      parallel: false,
      checked: ["b"],
      primary: "b",
    });
    expect(result.targets).toEqual(["b"]);
    expect(result.primary).toBe("b");
    expect(result.invalidReason).toBeNull();
  });

  test("several checked models are the parallel selection", () => {
    const result = resolveSelection(["a", "b", "c"], [], {
      parallel: true,
      checked: ["a", "b"],
      primary: "b",
    });
    expect(result.targets).toEqual(["b", "a"]);
    expect(result.primary).toBe("b");
  });

  test("checked models that are not enabled are ignored", () => {
    const result = resolveSelection(["a"], [], {
      parallel: true,
      checked: ["a", "disabled"],
      primary: "disabled",
    });
    expect(result.targets).toEqual(["a"]);
    expect(result.primary).toBe("a");
  });

  test("the primary falls back to a checked model when it is not checked", () => {
    const result = resolveSelection(["a", "b"], [], {
      parallel: true,
      checked: ["a", "b"],
      primary: null,
    });
    expect(result.primary).toBe("a");
  });

  test("a ranked model takes over when the primary is not checked", () => {
    const result = resolveSelection(["a", "b"], ["b"], {
      parallel: true,
      checked: ["a", "b"],
      primary: null,
    });
    expect(result.primary).toBe("b");
    expect(result.targets[0]).toBe("b");
  });
});

describe("enabledModels", () => {
  test("returns the enabled models in panel order", () => {
    const state = stateWith({ preferences });
    expect(enabledModels(state)).toEqual(["vendor/alpha", "vendor/beta"]);
  });
});

describe("groupModels", () => {
  const catalog: CatalogEntry[] = [
    {
      id: "anthropic/claude-opus-4.1",
      provider_id: "p1",
      name: "Anthropic: Claude Opus 4.1",
      family: "claude",
      context_length: null,
      prompt_price: null,
      completion_price: null,
      supports_streaming: true,
      supports_temperature: true,
    },
    {
      id: "openai/gpt-4.1-mini",
      provider_id: "p1",
      name: "OpenAI: GPT-4.1 mini",
      family: "gpt",
      context_length: null,
      prompt_price: null,
      completion_price: null,
      supports_streaming: true,
      supports_temperature: true,
    },
  ];

  test("groups by family", () => {
    const groups = groupModels(catalog, preferences);
    expect(groups.map((group) => group.label)).toEqual(["Claude", "GPT"]);
    expect(groups[0].items[0].full_label).toBe("Claude Opus 4.1");
  });

  test("filters by name, id and family", () => {
    const groups = groupModels(catalog, preferences);
    expect(filterModelGroups(groups, "gpt").length).toBe(1);
    expect(filterModelGroups(groups, "anthropic/").length).toBe(1);
    expect(filterModelGroups(groups, "nothing").length).toBe(0);
  });
});

describe("conversationsByGroup", () => {
  test("buckets chats by group and keeps ungrouped last", () => {
    const state = stateWith({
      groups: [{ id: "g1", name: "Research", sort_order: 0 }],
      conversations: [
        {
          id: "c1",
          title: "In group",
          group_id: "g1",
          system_prompt: null,
          title_source: "empty",
          title_status: "needs_generation",
          title_revision: 0,
          title_attempt_count: 0,
          main_leaf_id: null,
          draft_revision: 0,
          created_at_ms: 0,
          updated_at_ms: 0,
        },
        {
          id: "c2",
          title: "Loose",
          group_id: null,
          system_prompt: null,
          title_source: "empty",
          title_status: "needs_generation",
          title_revision: 0,
          title_attempt_count: 0,
          main_leaf_id: null,
          draft_revision: 0,
          created_at_ms: 0,
          updated_at_ms: 0,
        },
      ],
    });
    const buckets = conversationsByGroup(state);
    expect(buckets[0].name).toBe("Research");
    expect(buckets[0].conversations.map((c) => c.id)).toEqual(["c1"]);
    expect(buckets[buckets.length - 1].name).toBe("Ungrouped");
    expect(buckets[buckets.length - 1].conversations.map((c) => c.id)).toEqual(["c2"]);
  });

  test("the search box filters by title and group name", () => {
    const state = stateWith({
      groups: [{ id: "g1", name: "Research", sort_order: 0 }],
      conversations: [
        {
          id: "c1",
          title: "Pricing",
          group_id: "g1",
          system_prompt: null,
          title_source: "empty",
          title_status: "needs_generation",
          title_revision: 0,
          title_attempt_count: 0,
          main_leaf_id: null,
          draft_revision: 0,
          created_at_ms: 0,
          updated_at_ms: 0,
        },
        {
          id: "c2",
          title: "Other",
          group_id: null,
          system_prompt: null,
          title_source: "empty",
          title_status: "needs_generation",
          title_revision: 0,
          title_attempt_count: 0,
          main_leaf_id: null,
          draft_revision: 0,
          created_at_ms: 0,
          updated_at_ms: 0,
        },
      ],
      conversationSearch: "research",
    });
    const buckets = conversationsByGroup(state);
    expect(buckets[0].conversations.map((c) => c.id)).toEqual(["c1"]);
  });
});

describe("usageSummary", () => {
  test("sums the known costs and counts the unknown ones", () => {
    const summary = usageSummary([
      { cost_text: "0.0108", status: "known" },
      { cost_text: "0.0045", status: "known" },
      { cost_text: null, status: "pending" },
      { cost_text: null, status: "unavailable" },
    ]);
    expect(summary.knownTotal).toBe("0.02");
    expect(summary.pending).toBe(1);
    expect(summary.unavailable).toBe(1);
  });

  test("reports unknown when nothing is known", () => {
    const summary = usageSummary([{ cost_text: null, status: "unavailable" }]);
    expect(summary.knownTotal).toBeNull();
  });

  test("handles a missing record list", () => {
    expect(usageSummary(undefined).knownTotal).toBeNull();
  });
});

describe("unknown groups stay visible", () => {
  test("a chat in a group the frontend does not know is not dropped", () => {
    const state = stateWith({
      groups: [{ id: "g1", name: "Research", sort_order: 0 }],
      conversations: [
        {
          id: "c1",
          title: "In a known group",
          group_id: "g1",
          system_prompt: null,
          title_source: "empty",
          title_status: "needs_generation",
          title_revision: 0,
          title_attempt_count: 0,
          main_leaf_id: null,
          draft_revision: 0,
          created_at_ms: 0,
          updated_at_ms: 0,
        },
        {
          id: "c2",
          title: "In an unknown group",
          group_id: "group-created-elsewhere",
          system_prompt: null,
          title_source: "empty",
          title_status: "needs_generation",
          title_revision: 0,
          title_attempt_count: 0,
          main_leaf_id: null,
          draft_revision: 0,
          created_at_ms: 0,
          updated_at_ms: 0,
        },
      ],
    });
    const buckets = conversationsByGroup(state);
    const rendered = buckets.flatMap((bucket) =>
      bucket.conversations.map((conversation) => conversation.id),
    );
    // Every chat must appear exactly once.
    expect(rendered.sort()).toEqual(["c1", "c2"]);
    const ungrouped = buckets[buckets.length - 1];
    expect(ungrouped.name).toBe("Ungrouped");
    expect(ungrouped.conversations.map((c) => c.id)).toEqual(["c2"]);
  });
});
