import { useEffect, useMemo, useState } from "react";

import { t } from "../i18n/strings";
import { presentation } from "../model/presentation";
import type { Generation, Id, Message } from "../ipc/types";
import { threadControls } from "../state/selectors";
import { buildThread, type AnswerItem } from "../state/tree";
import { useStore } from "../state/store";
import { Icon } from "./Icon";
import { SelectableMarkdown } from "./Markdown";

const COLLAPSE_HEIGHT = 320;

export function MessageThread() {
  const { state, actions, dispatch } = useStore();
  const conversationId = state.currentConversationId;
  const messages = conversationId ? (state.messages[conversationId] ?? []) : [];
  const generations = conversationId
    ? (state.generations[conversationId] ?? [])
    : [];

  const conversation = state.conversations.find(
    (entry) => entry.id === conversationId,
  );

  const turns = useMemo(
    () => buildThread(messages, generations, conversation?.main_leaf_id ?? null),
    [messages, generations, conversation?.main_leaf_id],
  );

  /**
   * What the controls may show.
   *
   * A single-model conversation without branches gets neither an answer tab
   * strip nor branch buttons: there is nothing to compare and nothing to
   * branch from.
   */
  const controls = useMemo(
    () => threadControls(messages, generations),
    [messages, generations],
  );

  const split = conversationId ? (state.splitView[conversationId] ?? false) : false;

  if (!conversationId) {
    return (
      <div className="empty">
        <div className="empty__title">{t("empty.title")}</div>
        <p>{t("empty.subtitle")}</p>
        <button
          type="button"
          className="btn btn--accent"
          onClick={() => actions.openModal("models")}
        >
          {t("empty.configure")}
        </button>
      </div>
    );
  }

  if (turns.length === 0) {
    return (
      <div className="empty">
        <div className="empty__title">{t("empty.title")}</div>
        <p>{t("empty.subtitle")}</p>
      </div>
    );
  }

  return (
    <div
      className="thread"
      role="log"
      aria-live="polite"
      aria-label={t("a11y.messages")}
      data-testid="thread"
    >
      {turns.map((turn) => (
        <TurnView
          key={turn.question.id}
          question={turn.question}
          answers={turn.answers}
          generations={generations}
          messages={messages}
          split={split}
          showBranchControls={controls.showBranchControls}
          branchSelection={state.branchSelection}
          showAttempts={state.showAttempts[conversationId] ?? false}
          storedAnswerId={state.activeAnswer[turn.question.id] ?? null}
          onSelectAnswer={(messageId) =>
            dispatch({
              type: "answer/selected",
              questionId: turn.question.id,
              messageId,
            })
          }
          onToggleSplit={() => dispatch({ type: "split/toggle", conversationId })}
          onToggleAttempts={() =>
            dispatch({ type: "attempts/toggle", conversationId })
          }
        />
      ))}
    </div>
  );
}

interface TurnViewProps {
  question: Message;
  answers: AnswerItem[];
  generations: Generation[];
  messages: Message[];
  split: boolean;
  showBranchControls: boolean;
  branchSelection: Id[];
  showAttempts: boolean;
  /** The answer the user picked for this question (raw store value). */
  storedAnswerId: string | null;
  onSelectAnswer(messageId: string): void;
  onToggleSplit(): void;
  onToggleAttempts(): void;
}

function TurnView({
  question,
  answers,
  generations,
  messages,
  split,
  showBranchControls,
  branchSelection,
  showAttempts,
  storedAnswerId,
  onSelectAnswer,
  onToggleSplit,
  onToggleAttempts,
}: TurnViewProps) {
  const { state, actions } = useStore();
  const [expanded, setExpanded] = useState(false);
  const long = question.content.length > COLLAPSE_HEIGHT;

  /**
   * What the tabs show.
   *
   * Default: one slot per model lineage — a retry replaced its failed
   * attempt in place, so the replaced ones are hidden (their content lives
   * on in the store/history, never re-rendered here unless asked for).
   * History mode: every attempt, oldest first, grouped at its slot.
   */
  const visible = showAttempts
    ? answers.flatMap((item) => [...item.older_attempts, item])
    : answers;
  const hiddenCount = answers.reduce(
    (count, item) => count + item.older_attempts.length,
    0,
  );

  /**
   * Resolve the answer the user is looking at.
   *
   * A stored selection may name a replaced (hidden) attempt — the typical
   * case is "the failed tab was open, the user hit retry": the selection
   * then resolves to its lineage slot so the new attempt takes over the
   * very box the user was reading instead of streaming somewhere else.
   */
  const activeAnswerId = (() => {
    // Exactly what the user picked (the history view lists every attempt).
    if (storedAnswerId && visible.some((item) => item.message_id === storedAnswerId)) {
      return storedAnswerId;
    }
    if (storedAnswerId) {
      const slot = answers.find(
        (item) =>
          item.older_attempts.some((entry) => entry.message_id === storedAnswerId),
      );
      if (slot) return slot.message_id;
    }
    return answers[0]?.message_id ?? null;
  })();

  const activeIds = split
    ? [
        activeAnswerId,
        visible.find((item) => item.message_id !== activeAnswerId)?.message_id,
      ].filter((id): id is string => Boolean(id))
    : activeAnswerId
      ? [activeAnswerId]
      : visible.slice(0, 1).map((item) => item.message_id);

  // The backend tells us which question may be edited (`edit_latest`).
  const editableQuestion = state.editLatestEligible;

  // The tab strip only makes sense when the question really has several
  // answers (a parallel comparison); a single answer needs no tabs.
  const showTabs = visible.length > 1;

  return (
    <section className="turn" data-testid="turn">
      <div
        className="turn__question"
        data-message-id={question.id}
        data-testid="question"
      >
        <div
          className="turn__question-body"
          data-collapsed={long && !expanded ? "true" : "false"}
          data-testid="question-body"
        >
          <div>{question.content}</div>
          {question.attachments.length > 0 ? (
            <div className="composer__chips" style={{ marginTop: "var(--space-xs)" }}>
              {question.attachments.map((attachment, index) => (
                <span className="chip" key={`${attachment.name}-${index}`}>
                  <Icon name="paperclip" size={11} />
                  <span className="chip__name">{attachment.name}</span>
                </span>
              ))}
            </div>
          ) : null}
        </div>

        {/*
          The footer is always visible: collapsing the body must never hide
          the controls that expand it again.
        */}
        <div className="turn__question-footer" data-testid="question-actions">
          {long ? (
            <button
              type="button"
              className="btn"
              aria-expanded={expanded}
              data-testid="question-toggle"
              onClick={() => setExpanded((value) => !value)}
            >
              {expanded ? t("message.collapse") : t("message.expand")}
            </button>
          ) : null}
          {editableQuestion === question.id ? (
            <button
              type="button"
              className="btn"
              title={t("message.edit_question_tooltip")}
              aria-label={t("message.edit_question_tooltip")}
              onClick={() => actions.startEdit(question.id)}
            >
              <Icon name="pencil" size={12} />
              {t("message.edit_question")}
            </button>
          ) : null}
          <button
            type="button"
            className="btn btn--icon"
            title={t("message.copy_question_tooltip")}
            aria-label={t("message.copy_question_tooltip")}
            data-testid="question-copy"
            onClick={() => {
              void navigator.clipboard?.writeText(question.content);
            }}
          >
            <Icon name="copy" size={12} />
          </button>
          <button
            type="button"
            className="btn btn--danger"
            title={t("message.delete_tooltip")}
            aria-label={t("message.delete_tooltip")}
            data-testid="question-delete"
            onClick={() => void actions.deleteMessage(question.id)}
          >
            <Icon name="trash" size={12} />
          </button>
        </div>
      </div>

      {answers.length > 0 ? (
        <>
          {showTabs ? (
            <div
              className="answer-tabs"
              role="tablist"
              aria-label={t("a11y.answer_tabs")}
              data-testid="answer-tabs"
            >
              {visible.map((item) => {
                const selected = activeIds.includes(item.message_id);
                const label = modelLabel(state.catalog, item.model_id);
                return (
                  <button
                    key={item.message_id}
                    type="button"
                    role="tab"
                    className="answer-tab"
                    aria-selected={selected}
                    tabIndex={selected ? 0 : -1}
                    onClick={() => onSelectAnswer(item.message_id)}
                    onKeyDown={(event) => {
                      if (event.key === "ArrowRight" || event.key === "ArrowLeft") {
                        event.preventDefault();
                        const index = visible.findIndex(
                          (entry) => entry.message_id === item.message_id,
                        );
                        const delta = event.key === "ArrowRight" ? 1 : -1;
                        const next =
                          visible[(index + delta + visible.length) % visible.length];
                        if (next) onSelectAnswer(next.message_id);
                      }
                    }}
                    title={label}
                  >
                    <span
                      className="answer-tab__dot"
                      data-status={item.status ?? "unknown"}
                      aria-hidden="true"
                    />
                    <span>{label}</span>
                    {/* Attempt numbers only in the explicit history view;
                        a retried tab shows no marker in normal use. */}
                    {showAttempts && item.attempt > 1 ? (
                      <span className="muted small">
                        {t("answers.attempt").replace("{n}", String(item.attempt))}
                      </span>
                    ) : null}
                  </button>
                );
              })}

              {visible.length > 1 ? (
                <button
                  type="button"
                  className="btn btn--icon"
                  style={{ marginLeft: "auto" }}
                  title={t("answers.split_tooltip")}
                  aria-label={t("answers.split_tooltip")}
                  aria-pressed={split}
                  data-testid="answer-split"
                  onClick={onToggleSplit}
                >
                  <Icon name="split" size={13} />
                </button>
              ) : null}
            </div>
          ) : null}

          {hiddenCount > 0 ? (
            <button
              type="button"
              className="btn small"
              data-testid="answer-attempts"
              onClick={onToggleAttempts}
            >
              {showAttempts
                ? t("message.hide_attempts")
                : t("message.show_attempts").replace("{n}", String(hiddenCount))}
            </button>
          ) : null}

          <div className={split ? "answers answers--split" : "answers"}>
            {activeIds.map((messageId) => {
              // History mode also renders replaced attempts, so look the
              // item up in the visible set (slots + their older attempts).
              const item = visible.find((entry) => entry.message_id === messageId);
              const message = messages.find((entry) => entry.id === messageId);
              if (!item || !message) return null;
              return (
                <AnswerCard
                  key={messageId}
                  message={message}
                  item={item}
                  history={showAttempts && item.superseded}
                  generations={generations}
                  branchSelected={branchSelection.includes(messageId)}
                  showBranchControls={showBranchControls}
                />
              );
            })}
          </div>
        </>
      ) : null}
    </section>
  );
}

interface AnswerCardProps {
  message: Message;
  item: AnswerItem;
  /** True when this card is a replaced attempt shown in the history view. */
  history: boolean;
  generations: Generation[];
  branchSelected: boolean;
  showBranchControls: boolean;
}

function AnswerCard({
  message,
  item,
  history,
  generations,
  branchSelected,
  showBranchControls,
}: AnswerCardProps) {
  const { state, actions } = useStore();
  const generation = generations.find(
    (entry) => entry.assistant_message_id === message.id,
  );
  const live =
    item.status === "streaming" ||
    item.status === "connecting" ||
    item.status === "queued";
  const failed =
    item.status === "failed" ||
    item.status === "cancelled" ||
    item.status === "interrupted";
  const label = modelLabel(state.catalog, item.model_id);

  /**
   * Retry cooldown (admission guard).
   *
   * The backend refuses a retry while `retry_not_before_ms` is in the
   * future (Retry-After / backoff). The button is disabled with a visible
   * countdown for that window, so a blocked retry is never even attempted —
   * no dead request, no surprise toast.
   */
  const retryNotBeforeMs = generation?.retry_not_before_ms ?? null;
  const [cooldownLeftMs, setCooldownLeftMs] = useState(() =>
    retryNotBeforeMs ? Math.max(0, retryNotBeforeMs - Date.now()) : 0,
  );
  useEffect(() => {
    if (!retryNotBeforeMs) {
      setCooldownLeftMs(0);
      return;
    }
    const tick = () =>
      setCooldownLeftMs(Math.max(0, retryNotBeforeMs - Date.now()));
    tick();
    const timer = setInterval(tick, 500);
    return () => clearInterval(timer);
  }, [retryNotBeforeMs]);
  const coolingDown = failed && cooldownLeftMs > 0;
  const cooldownSeconds = Math.ceil(cooldownLeftMs / 1000);

  return (
    <article
      className="card"
      aria-label={`${t("message.role.assistant")}: ${label}`}
      data-testid="answer"
    >
      <header className="card__header">
        <span className="card__model">{label}</span>
        {history && item.attempt > 1 ? (
          <span>{t("answers.attempt").replace("{n}", String(item.attempt))}</span>
        ) : null}
        {item.status ? <span>{statusLabel(item.status)}</span> : null}
      </header>

      <SelectableMarkdown
        messageId={message.id}
        source={message.content}
        label={`${t("message.role.assistant")}: ${label}`}
        streaming={live}
      />

      {live ? (
        <p className="card__status" role="status">
          {statusLabel(item.status ?? "streaming")}
        </p>
      ) : null}

      {generation?.failure_kind ? (
        <p className="card__error" role="alert">
          {failureLabel(generation.failure_kind)}
          {generation.failure_message ? ` — ${generation.failure_message}` : ""}
        </p>
      ) : null}
      {generation?.error && !generation.failure_kind ? (
        <p className="card__error" role="alert">
          {generation.error}
        </p>
      ) : null}

      {/*
        The actions follow the answer: they sit below the text, at the
        bottom right of the card, and never cover it.
      */}
      <footer className="card__footer" data-testid="answer-actions">
        <div className="card__actions">
          <button
            type="button"
            className="btn btn--icon"
            title={t("message.copy_tooltip")}
            aria-label={t("message.copy_tooltip")}
            data-testid="answer-copy"
            onClick={() => {
              void navigator.clipboard?.writeText(message.content);
            }}
          >
            <Icon name="copy" size={13} />
          </button>
          {showBranchControls ? (
            <>
              <button
                type="button"
                className="btn btn--icon"
                title={t("message.select_branch_tooltip")}
                aria-label={t("message.select_branch_tooltip")}
                aria-pressed={branchSelected}
                data-testid="answer-branch-select"
                onClick={() => actions.toggleBranch(message.id)}
              >
                <Icon name="branchSelect" size={13} />
              </button>
              <button
                type="button"
                className="btn btn--icon"
                title={t("message.continue_tooltip")}
                aria-label={t("message.continue_tooltip")}
                disabled={item.status !== "completed"}
                data-testid="answer-branch-continue"
                onClick={() => void actions.continueFrom(message.id)}
              >
                <Icon name="branch" size={13} />
              </button>
            </>
          ) : null}
          {live && generation ? (
            <button
              type="button"
              className="btn btn--icon"
              title={t("message.stop")}
              aria-label={t("message.stop")}
              data-testid="answer-stop"
              onClick={() => void actions.cancel(generation.request_id)}
            >
              <Icon name="stop" size={13} />
            </button>
          ) : null}
          {failed && generation ? (
            <>
              {coolingDown ? (
                <span className="muted small" role="status" data-testid="answer-retry-cooldown">
                  {t("retry.cooldown").replace("{s}", String(cooldownSeconds))}
                </span>
              ) : null}
              <button
                type="button"
                className="btn btn--icon"
                title={
                  coolingDown
                    ? t("retry.cooldown").replace("{s}", String(cooldownSeconds))
                    : t("message.retry")
                }
                aria-label={
                  coolingDown
                    ? t("retry.cooldown").replace("{s}", String(cooldownSeconds))
                    : t("message.retry")
                }
                disabled={coolingDown}
                aria-disabled={coolingDown}
                data-testid="answer-retry"
                onClick={() => void actions.retry(generation.request_id)}
              >
                <Icon name="retry" size={13} />
              </button>
            </>
          ) : null}
        </div>
      </footer>
    </article>
  );
}

function modelLabel(
  catalog: { id: string; name: string }[],
  modelId: string,
): string {
  if (!modelId) return t("message.role.assistant");
  const entry = catalog.find((item) => item.id === modelId);
  if (!entry) return modelId;
  return presentation(entry.id, entry.name).full_label;
}

function statusLabel(status: string): string {
  const known = `message.${status}`;
  const label = t(known);
  return label === known ? status : label;
}

function failureLabel(kind: string): string {
  const known = `failure.${kind}`;
  const label = t(known);
  return label === known ? kind : label;
}
