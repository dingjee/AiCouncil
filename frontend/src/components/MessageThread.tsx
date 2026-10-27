import { useMemo, useState } from "react";

import { t } from "../i18n/strings";
import { presentation } from "../model/presentation";
import type { Generation, Id, Message } from "../ipc/types";
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

  const split = conversationId ? (state.splitView[conversationId] ?? false) : false;

  if (!conversationId) {
    return (
      <div className="empty">
        <div className="empty__title">{t("empty.title")}</div>
        <p>{t("empty.subtitle")}</p>
        <button
          type="button"
          className="btn btn--accent"
          onClick={() => actions.setPanel("models")}
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
    >
      {turns.map((turn) => (
        <TurnView
          key={turn.question.id}
          question={turn.question}
          answers={turn.answers}
          generations={generations}
          messages={messages}
          split={split}
          branchSelection={state.branchSelection}
          showAttempts={state.showAttempts[conversationId] ?? false}
          activeAnswerId={
            state.activeAnswer[turn.question.id] ??
            turn.answers.find((item) => !item.superseded)?.message_id ??
            turn.answers[0]?.message_id ??
            null
          }
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
  branchSelection: Id[];
  showAttempts: boolean;
  activeAnswerId: string | null;
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
  branchSelection,
  showAttempts,
  activeAnswerId,
  onSelectAnswer,
  onToggleSplit,
  onToggleAttempts,
}: TurnViewProps) {
  const { state, actions } = useStore();
  const [expanded, setExpanded] = useState(false);
  const long = question.content.length > COLLAPSE_HEIGHT;

  const visible = answers.filter((item) => showAttempts || !item.superseded);
  const hiddenCount = answers.length - visible.length;

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

  return (
    <section className="turn">
      <div
        className={`turn__question${long && !expanded ? " turn__question--collapsed" : ""}`}
        data-message-id={question.id}
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
        <div className="turn__question-actions">
          {long ? (
            <button
              type="button"
              className="btn"
              aria-expanded={expanded}
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
            className="btn btn--danger"
            title={t("message.delete_tooltip")}
            aria-label={t("message.delete_tooltip")}
            onClick={() => void actions.deleteMessage(question.id)}
          >
            <Icon name="trash" size={12} />
          </button>
        </div>
      </div>

      {answers.length > 0 ? (
        <>
          <div
            className="answer-tabs"
            role="tablist"
            aria-label={t("a11y.answer_tabs")}
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
                  {item.attempt > 1 ? (
                    <span className="muted small">
                      {t("answers.attempt").replace("{n}", String(item.attempt))}
                    </span>
                  ) : null}
                </button>
              );
            })}

            {answers.length > 1 ? (
              <button
                type="button"
                className="btn btn--icon"
                style={{ marginLeft: "auto" }}
                title={t("answers.split_tooltip")}
                aria-label={t("answers.split_tooltip")}
                aria-pressed={split}
                onClick={onToggleSplit}
              >
                <Icon name="split" size={13} />
              </button>
            ) : null}
          </div>

          {hiddenCount > 0 ? (
            <button
              type="button"
              className="btn small"
              onClick={onToggleAttempts}
            >
              {showAttempts
                ? t("message.hide_attempts")
                : t("message.show_attempts").replace("{n}", String(hiddenCount))}
            </button>
          ) : null}

          <div className={split ? "answers answers--split" : "answers"}>
            {activeIds.map((messageId) => {
              const item = answers.find((entry) => entry.message_id === messageId);
              const message = messages.find((entry) => entry.id === messageId);
              if (!item || !message) return null;
              return (
                <AnswerCard
                  key={messageId}
                  message={message}
                  item={item}
                  generations={generations}
                  branchSelected={branchSelection.includes(messageId)}
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
  generations: Generation[];
  branchSelected: boolean;
}

function AnswerCard({ message, item, generations, branchSelected }: AnswerCardProps) {
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

  return (
    <article className="card" aria-label={`${t("message.role.assistant")}: ${label}`}>
      <header className="card__header">
        <span className="card__model">{label}</span>
        {item.attempt > 1 ? (
          <span>{t("answers.attempt").replace("{n}", String(item.attempt))}</span>
        ) : null}
        {item.status ? <span>{statusLabel(item.status)}</span> : null}
        <div className="card__actions">
          <button
            type="button"
            className="btn btn--icon"
            title={t("message.copy_tooltip")}
            aria-label={t("message.copy_tooltip")}
            onClick={() => {
              void navigator.clipboard?.writeText(message.content);
            }}
          >
            <Icon name="copy" size={13} />
          </button>
          <button
            type="button"
            className="btn btn--icon"
            title={t("message.select_branch_tooltip")}
            aria-label={t("message.select_branch_tooltip")}
            aria-pressed={branchSelected}
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
            onClick={() => void actions.continueFrom(message.id)}
          >
            <Icon name="branch" size={13} />
          </button>
          {live && generation ? (
            <button
              type="button"
              className="btn btn--icon"
              title={t("message.stop")}
              aria-label={t("message.stop")}
              onClick={() => void actions.cancel(generation.request_id)}
            >
              <Icon name="stop" size={13} />
            </button>
          ) : null}
          {failed && generation ? (
            <button
              type="button"
              className="btn btn--icon"
              title={t("message.retry")}
              aria-label={t("message.retry")}
              onClick={() => void actions.retry(generation.request_id)}
            >
              <Icon name="retry" size={13} />
            </button>
          ) : null}
        </div>
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
