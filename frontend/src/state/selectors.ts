/**
 * Derived state.
 *
 * `resolveSelection` is a direct port of `resolve_selection` in
 * `src/app/mod.rs`: it decides which models a send uses, which one owns the
 * main branch, and why the send button is disabled.
 */

import type {
  CatalogEntry,
  ConversationSummary,
  Id,
  ModelPreferenceEntry,
} from "../ipc/types";
import {
  disambiguate,
  presentation,
  type ModelPresentation,
} from "../model/presentation";
import { sumCosts } from "../util/money";
import type { AppState, ModelSelectionState } from "./types";

export interface NormalizedSelection {
  /** Effective targets; the primary model is first. */
  targets: string[];
  primary: string | null;
  /** i18n key explaining why sending is disabled. */
  invalidReason: string | null;
}

/** Model ids the user enabled, in panel order. */
export function enabledModels(state: AppState): string[] {
  return orderedPreferences(state.preferences)
    .filter((preference) => preference.enabled)
    .map((preference) => preference.model_id);
}

/** Provider id of a model (needed for the send selection). */
export function providerOf(
  state: AppState,
  modelId: string,
): Id | null {
  const preference = state.preferences.find(
    (entry) => entry.model_id === modelId,
  );
  if (preference) return preference.provider_id;
  const entry = state.catalog.find((item) => item.id === modelId);
  return entry?.provider_id ?? null;
}

/** The default (primary) model id, if any. */
export function defaultModel(state: AppState): string | null {
  const preference = state.preferences.find((entry) => entry.is_default);
  return preference?.model_id ?? null;
}

/**
 * Resolve the effective model selection.
 *
 * Mirrors the native behaviour:
 *   - no enabled model at all → send disabled ("enable a model first");
 *   - single mode → the primary model (or the first enabled one);
 *   - parallel mode → exactly the checked models; the primary is promoted to
 *     the front and must belong to the checked set.
 */
export function resolveSelection(
  enabled: string[],
  usageRank: string[],
  selection: ModelSelectionState,
): NormalizedSelection {
  if (enabled.length === 0) {
    return {
      targets: [],
      primary: null,
      invalidReason: "composer.send_disabled_no_model",
    };
  }

  if (!selection.parallel) {
    const primary =
      selection.primary && enabled.includes(selection.primary)
        ? selection.primary
        : (enabled[0] ?? null);
    return {
      targets: primary ? [primary] : [],
      primary,
      invalidReason: null,
    };
  }

  const checked = enabled.filter((model) => selection.checked.includes(model));
  if (checked.length === 0) {
    return {
      targets: [],
      primary: null,
      invalidReason: "composer.send_disabled_no_targets",
    };
  }

  const primary =
    selection.primary && checked.includes(selection.primary)
      ? selection.primary
      : (usageRank.find((model) => checked.includes(model)) ?? checked[0] ?? null);

  const ordered = primary
    ? [primary, ...checked.filter((model) => model !== primary)]
    : checked;

  return { targets: ordered, primary, invalidReason: null };
}

/** The default pair of models for parallel mode (current + best ranked). */
export function defaultParallelPair(
  enabled: string[],
  usageRank: string[],
  current: string | null,
): string[] {
  const checked: string[] = [];
  if (current && enabled.includes(current)) checked.push(current);
  for (const model of usageRank) {
    if (enabled.includes(model) && !checked.includes(model)) {
      checked.push(model);
      break;
    }
  }
  if (checked.length < 2) {
    const fallback = enabled.find((model) => !checked.includes(model));
    if (fallback) checked.push(fallback);
  }
  if (checked.length === 0 && enabled[0]) checked.push(enabled[0]);
  return checked;
}

export interface ModelGroup {
  label: string;
  items: ModelPresentation[];
}

/** Group the catalog by family for the picker. */
export function groupModels(
  catalog: CatalogEntry[],
  preferences: ModelPreferenceEntry[],
): ModelGroup[] {
  const presentations = catalog.map((entry) => presentation(entry.id, entry.name));
  const disambiguated = disambiguate(presentations);
  const byId = new Map<string, ModelPresentation>();
  disambiguated.forEach((item) => byId.set(item.raw_id, item));

  const groups = new Map<string, ModelGroup>();
  const ordered: ModelGroup[] = [];

  for (const entry of orderedCatalog(catalog, preferences)) {
    const item = byId.get(entry.id);
    if (!item) continue;
    const label = item.group_label || entry.id;
    let group = groups.get(label);
    if (!group) {
      group = { label, items: [] };
      groups.set(label, group);
      ordered.push(group);
    }
    group.items.push(item);
  }
  return ordered;
}

/** Catalog entries ordered by the user's panel order, then by id. */
export function orderedCatalog(
  catalog: CatalogEntry[],
  preferences: ModelPreferenceEntry[],
): CatalogEntry[] {
  const order = new Map<string, number>();
  preferences.forEach((preference, index) => {
    order.set(preference.model_id, preference.sort_order ?? index);
  });
  return catalog.slice().sort((a, b) => {
    const sa = order.get(a.id) ?? Number.MAX_SAFE_INTEGER;
    const sb = order.get(b.id) ?? Number.MAX_SAFE_INTEGER;
    if (sa !== sb) return sa - sb;
    return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
  });
}

/** Filter the grouped catalog by a free-text query. */
export function filterModelGroups(
  groups: ModelGroup[],
  query: string,
): ModelGroup[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return groups;
  const out: ModelGroup[] = [];
  for (const group of groups) {
    const items = group.items.filter((item) => {
      const haystack = `${item.full_label} ${item.raw_id} ${item.catalog_name} ${group.label}`;
      return haystack.toLowerCase().includes(needle);
    });
    if (items.length > 0) out.push({ label: group.label, items });
  }
  return out;
}

/** Conversations filtered by the sidebar search box. */
export function filterConversations(state: AppState): ConversationSummary[] {
  const needle = state.conversationSearch.trim().toLowerCase();
  const groups = new Map(state.groups.map((group) => [group.id, group.name]));
  return state.conversations.filter((conversation) => {
    if (!needle) return true;
    if (conversation.title.toLowerCase().includes(needle)) return true;
    const groupName = conversation.group_id
      ? (groups.get(conversation.group_id) ?? "").toLowerCase()
      : "";
    return groupName.includes(needle);
  });
}

/**
 * Bucket conversations by group, keeping the "Ungrouped" bucket last.
 *
 * A conversation whose group is not (yet) known to the frontend (the group
 * list only comes from `bootstrap`; the IPC contract has no `list_groups`)
 * must never disappear from the sidebar — it falls into the "Ungrouped"
 * bucket instead.
 */
export function conversationsByGroup(state: AppState): {
  groupId: Id | null;
  name: string;
  conversations: ConversationSummary[];
}[] {
  const visible = filterConversations(state);
  const knownGroups = new Set(state.groups.map((group) => group.id));
  const buckets: {
    groupId: Id | null;
    name: string;
    conversations: ConversationSummary[];
  }[] = [];

  for (const group of state.groups) {
    buckets.push({
      groupId: group.id,
      name: group.name,
      conversations: visible.filter(
        (conversation) => conversation.group_id === group.id,
      ),
    });
  }
  buckets.push({
    groupId: null,
    name: "Ungrouped",
    conversations: visible.filter(
      (conversation) =>
        conversation.group_id === null || !knownGroups.has(conversation.group_id),
    ),
  });
  return buckets.filter(
    (bucket) => bucket.conversations.length > 0 || bucket.groupId !== null,
  );
}

/** Model preferences ordered by the user's panel order. */
export function orderedPreferences(
  preferences: ModelPreferenceEntry[],
): ModelPreferenceEntry[] {
  return preferences.slice().sort((a, b) => {
    const diff = (a.sort_order ?? 0) - (b.sort_order ?? 0);
    if (diff !== 0) return diff;
    return a.model_id < b.model_id ? -1 : a.model_id > b.model_id ? 1 : 0;
  });
}

/**
 * Sum the known costs of a conversation.
 *
 * The ledger stores decimal strings; the sum is computed with exact integer
 * arithmetic and formatted with the Rust rounding rule. Unknown (null)
 * records are never counted as zero — they are reported separately.
 */
export function usageSummary(
  records: { cost_text: string | null; status: string }[] | undefined,
): { knownTotal: string | null; pending: number; unavailable: number } {
  if (!records) {
    return { knownTotal: null, pending: 0, unavailable: 0 };
  }
  const known: string[] = [];
  let pending = 0;
  let unavailable = 0;
  for (const record of records) {
    if (record.status === "known" && record.cost_text !== null) {
      known.push(record.cost_text);
    } else if (record.status === "pending") {
      pending += 1;
    } else if (record.status === "unavailable") {
      unavailable += 1;
    }
  }
  return { knownTotal: sumCosts(known), pending, unavailable };
}
