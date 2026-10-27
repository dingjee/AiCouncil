/**
 * Message tree helpers — a port of `src/domain/tree.rs`.
 *
 * Messages form a tree (one parent per node). The UI needs the ancestor
 * chain of a leaf to render a branch, the children of a question to build the
 * answer tabs, and the leaves to offer branch targets.
 */

import type { Generation, Id, Message, Role } from "../ipc/types";

export type { Role };

export interface MessageTree {
  messages: Map<string, Message>;
}

export function buildTree(messages: Message[]): MessageTree {
  const map = new Map<string, Message>();
  for (const message of messages) map.set(message.id, message);
  return { messages: map };
}

/** Root → leaf order, or `null` when the chain is broken/cyclic. */
export function pathFromRoot(
  tree: MessageTree,
  leaf: Id | null,
): Message[] | null {
  if (!leaf) return [];
  const chain: Message[] = [];
  const seen = new Set<string>();
  let current: Id | null = leaf;
  while (current) {
    if (seen.has(current)) return null;
    seen.add(current);
    const message = tree.messages.get(current);
    if (!message) return null;
    chain.push(message);
    current = message.parent_id;
  }
  chain.reverse();
  return chain;
}

/** Direct children of a node, ordered by creation time then id. */
export function childrenOf(tree: MessageTree, parentId: Id | null): Message[] {
  const out: Message[] = [];
  for (const message of tree.messages.values()) {
    if (message.parent_id === parentId) out.push(message);
  }
  out.sort(compareMessages);
  return out;
}

/** Every leaf (a message without children), ordered deterministically. */
export function leaves(tree: MessageTree): Message[] {
  const parents = new Set<string>();
  for (const message of tree.messages.values()) {
    if (message.parent_id) parents.add(message.parent_id);
  }
  const out: Message[] = [];
  for (const message of tree.messages.values()) {
    if (!parents.has(message.id)) out.push(message);
  }
  out.sort(compareMessages);
  return out;
}

export function compareMessages(a: Message, b: Message): number {
  if (a.created_at_ms !== b.created_at_ms) return a.created_at_ms - b.created_at_ms;
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}

export interface AnswerItem {
  message_id: Id;
  model_id: string;
  request_id: Id | null;
  attempt: number;
  attempts_total: number;
  status: string | null;
  superseded: boolean;
}

/** Latest generation for an assistant message. */
export function latestGeneration(
  generations: Generation[] | undefined,
  assistantId: Id,
): Generation | null {
  if (!generations) return null;
  let best: Generation | null = null;
  for (const generation of generations) {
    if (generation.assistant_message_id !== assistantId) continue;
    if (!best || (generation.started_at_ms ?? 0) >= (best.started_at_ms ?? 0)) {
      best = generation;
    }
  }
  return best;
}

/**
 * Build the answer set of a question.
 *
 * Retry attempts of the same lineage are folded: the newest attempt stays
 * visible, older ones are marked `superseded` but keep their attempt number.
 */
export function answerSet(
  tree: MessageTree,
  generations: Generation[] | undefined,
  questionId: Id,
): AnswerItem[] {
  const ids = childrenOf(tree, questionId).filter((message) => message.role === "assistant");
  const generationById = new Map<string, Generation | null>();
  for (const id of ids) {
    generationById.set(id.id, latestGeneration(generations, id.id));
  }

  const rootOf = (id: string) => {
    const generation = generationById.get(id);
    if (!generation) return null;
    return generation.retry_root_id ?? generation.request_id;
  };

  return ids.map((message, index) => {
    const generation = generationById.get(message.id);
    const root = rootOf(message.id);
    let attempt = 1;
    let attemptsTotal = 1;
    let superseded = false;
    if (root) {
      attempt =
        ids.slice(0, index).filter((other) => rootOf(other.id) === root).length + 1;
      attemptsTotal = Math.max(
        1,
        ids.filter((other) => rootOf(other.id) === root).length,
      );
      superseded = ids
        .slice(index + 1)
        .some((other) => rootOf(other.id) === root);
    }
    return {
      message_id: message.id,
      model_id: generation?.model_id ?? "",
      request_id: generation?.request_id ?? null,
      attempt,
      attempts_total: attemptsTotal,
      status: generation?.status ?? null,
      superseded,
    };
  });
}

export interface ThreadTurn {
  question: Message;
  answers: AnswerItem[];
}

/** The visible thread: main branch turns with their answer sets. */
export function buildThread(
  messages: Message[],
  generations: Generation[] | undefined,
  mainLeafId: Id | null,
): ThreadTurn[] {
  const tree = buildTree(messages);
  const chain = pathFromRoot(tree, mainLeafId) ?? [];
  const turns: ThreadTurn[] = [];
  for (const message of chain) {
    if (message.role !== "user") continue;
    turns.push({ question: message, answers: answerSet(tree, generations, message.id) });
  }
  return turns;
}

/** Assistant answers that can be continued from (completed only). */
export function branchableAnswers(
  messages: Message[],
  generations: Generation[] | undefined,
): Message[] {
  const tree = buildTree(messages);
  const out: Message[] = [];
  for (const message of messages) {
    if (message.role !== "assistant") continue;
    const generation = latestGeneration(generations, message.id);
    if (!generation || generation.status !== "completed") continue;
    if (!pathFromRoot(tree, message.id)) continue;
    out.push(message);
  }
  out.sort(compareMessages);
  return out;
}
