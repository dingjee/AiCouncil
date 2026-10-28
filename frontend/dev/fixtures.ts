/**
 * Browser fixtures for the mock harness (development only).
 *
 * The `long` fixture is a **synthetic imported-style chat**: a long linear
 * conversation with one model plus a parallel conversation with two models.
 * It exists for the geometry harness (`dev/geometry.ts`): the real user
 * data of the macOS app must never be touched by a test.
 */

import type {
  ConversationSummary,
  Generation,
  Id,
  Message,
  UsageRecord,
} from "../src/ipc/types";

export type FixtureName = "default" | "long";

export interface FixtureData {
  conversations: ConversationSummary[];
  messages: Map<Id, Message[]>;
  generations: Map<Id, Generation[]>;
  usage: Map<Id, UsageRecord[]>;
}

const MODEL_A = "anthropic/claude-sonnet-4.5";
const MODEL_B = "openai/gpt-4.1-mini";

function uuid(seed: string): Id {
  let hash = 0;
  for (let i = 0; i < seed.length; i += 1) {
    hash = (hash * 31 + seed.charCodeAt(i)) >>> 0;
  }
  const hex = hash.toString(16).padStart(8, "0");
  return `${hex}-0000-4000-8000-000000000000`;
}

const LONG_QUESTION = [
  "Please compare the three candidate architectures for the offline evaluation",
  "pipeline and explain, step by step, why the streaming variant was rejected",
  "in the last review round.",
  "",
  "Context: the pipeline ingests roughly 40k documents per night, splits them",
  "into semantic chunks, embeds the chunks with two different encoders and",
  "stores the vectors in a local index. The retrieval step mixes lexical and",
  "dense search, the reranker runs on CPU only, and the evaluation harness",
  "must stay reproducible on a laptop without a GPU.",
  "",
  "Questions to answer:",
  "1. Which bottleneck dominates the total wall clock time?",
  "2. What changes if the reranker moves to a distilled cross-encoder?",
  "3. How much of the pipeline can be evaluated without labelled data?",
  "4. Which parts of the result would survive a hardware change?",
  "",
  "Please be concrete, cite the measured numbers we discussed and mark the",
  "assumptions that still need verification.",
].join("\n");

const LONG_ANSWER = [
  "The streaming variant was rejected for three concrete reasons.",
  "",
  "**1. Wall clock time.** The dense retrieval stage dominates: with the",
  "current index the 40k document corpus needs about 38 minutes for the",
  "embedding pass, while the lexical pass needs 6. The reranker adds 21",
  "minutes on CPU. Streaming does not help, because the index build is",
  "inherently sequential with respect to the encoder batch size.",
  "",
  "**2. Distilled cross-encoder.** Moving the reranker to a distilled model",
  "of 4 layers cuts the reranking stage to roughly 9 minutes at a measured",
  "recall loss of 1.3 points on the held-out set. The saving is real, but it",
  "only pays off if the evaluation harness can run the same model on CPU.",
  "",
  "**3. Unlabelled evaluation.** Without labels the following proxies are",
  "usable: answer groundedness (does the answer cite the retrieved chunk?),",
  "citation precision, and stability across two paraphrases of the question.",
  "None of them replaces a human judgement, so a small labelled subset of",
  "about 200 questions is still required.",
  "",
  "```python",
  "def groundedness(answer: str, chunk: str) -> float:",
  "    return 1.0 if chunk.strip() and chunk.strip() in answer else 0.0",
  "```",
  "",
  "$$R = \\frac{1}{N}\\sum_{i=1}^{N} \\mathbb{1}[c_i \\in a_i]$$",
  "",
  "**4. Hardware independence.** The chunking, the lexical index and the",
  "evaluation protocol survive a hardware change; the encoder timings do not.",
  "That is the part of the result I would keep.",
].join("\n");

const SHORT_ANSWER = [
  "Short answer: the corpus is small enough to stay in memory, the encoder",
  "batch is the knob that matters, and the harness should pin the seed.",
].join("\n");

interface TurnSpec {
  question: string;
  answer: string;
  model: string;
}

function linearTurns(count: number): TurnSpec[] {
  const turns: TurnSpec[] = [];
  for (let index = 0; index < count; index += 1) {
    const long = index % 3 === 1;
    turns.push({
      question: long
        ? LONG_QUESTION
        : `Question ${index + 1} of the imported chat: what does the review say about step ${index + 1}?`,
      answer: long ? LONG_ANSWER : SHORT_ANSWER,
      model: MODEL_A,
    });
  }
  return turns;
}

function buildConversation(
  id: Id,
  title: string,
  turns: TurnSpec[],
  createdAt: number,
  models: string[],
): {
  conversation: ConversationSummary;
  messages: Message[];
  generations: Generation[];
} {
  const messages: Message[] = [];
  const generations: Generation[] = [];
  let parentId: Id | null = null;
  let clock = createdAt;

  turns.forEach((turn, index) => {
    const questionId = uuid(`${id}-q${index}`);
    messages.push({
      id: questionId,
      conversation_id: id,
      parent_id: parentId,
      role: "user",
      content: turn.question,
      attachments: [],
      created_at_ms: clock,
    });
    clock += 1000;

    const answerIds: string[] = [];
    models.forEach((model, modelIndex) => {
      const assistantId = uuid(`${id}-a${index}-${modelIndex}`);
      messages.push({
        id: assistantId,
        conversation_id: id,
        parent_id: questionId,
        role: "assistant",
        content:
          modelIndex === 0
            ? turn.answer
            : `${turn.answer}\n\n_Second opinion from ${model}: the same conclusion, reached with a shorter argument._`,
        attachments: [],
        created_at_ms: clock,
      });
      clock += 1000;
      answerIds.push(assistantId);
      generations.push({
        request_id: uuid(`${id}-g${index}-${modelIndex}`),
        batch_id: uuid(`${id}-b${index}`),
        user_message_id: questionId,
        assistant_message_id: assistantId,
        provider_id: uuid("provider-openrouter"),
        model_id: model,
        status: "completed",
        system_prompt: null,
        price_prompt: "3.00",
        price_completion: "15.00",
        remote_generation_id: null,
        error: null,
        retry_of: null,
        retry_root_id: null,
        retry_not_before_ms: null,
        dispatch_phase: "remote_accepted",
        failure_kind: null,
        failure_message: null,
        started_at_ms: clock - 900,
        finished_at_ms: clock,
      });
    });

    // The main path continues from the first answer of the turn.
    parentId = answerIds[0];
  });

  const conversation: ConversationSummary = {
    id,
    title,
    group_id: null,
    system_prompt: null,
    title_source: "manual",
    title_status: "ready",
    title_revision: 1,
    main_leaf_id: parentId,
    draft_revision: 0,
    created_at_ms: createdAt,
    updated_at_ms: clock,
  };

  return { conversation, messages, generations };
}

/**
 * The long synthetic chat.
 *
 * - `Imported research chat` — nine linear turns with a **single** model:
 *   no answer tab strip, no branch controls.
 * - `Parallel model comparison` — two turns answered by **two** models:
 *   the tab strip and the branch controls are meaningful there.
 */
export function longFixture(): FixtureData {
  const singleId = uuid("fixture-long-single");
  const parallelId = uuid("fixture-long-parallel");
  const base = Date.now() - 86_400_000;

  const single = buildConversation(
    singleId,
    "Imported research chat",
    linearTurns(9),
    base,
    [MODEL_A],
  );
  const parallel = buildConversation(
    parallelId,
    "Parallel model comparison",
    [
      {
        question:
          "Two models, one question: which retrieval design should the nightly pipeline use?",
        answer: LONG_ANSWER,
        model: MODEL_A,
      },
      {
        question:
          "Follow-up: how does the second opinion change the evaluation plan?",
        answer: LONG_ANSWER,
        model: MODEL_A,
      },
    ],
    base + 60_000,
    [MODEL_A, MODEL_B],
  );

  const usage: UsageRecord[] = [
    {
      request_id: uuid("fixture-usage"),
      prompt_tokens: 4200,
      completion_tokens: 1800,
      total_tokens: 6000,
      cost_text: "0.0396",
      cost_source: "api",
      status: "known",
    },
  ];

  return {
    conversations: [single.conversation, parallel.conversation],
    messages: new Map([
      [singleId, single.messages],
      [parallelId, parallel.messages],
    ]),
    generations: new Map([
      [singleId, single.generations],
      [parallelId, parallel.generations],
    ]),
    usage: new Map([
      [singleId, usage],
      [parallelId, usage],
    ]),
  };
}
