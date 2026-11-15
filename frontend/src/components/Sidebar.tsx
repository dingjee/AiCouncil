import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { t } from "../i18n/strings";
import { conversationsByGroup } from "../state/selectors";
import { useStore } from "../state/store";
import type { ConversationSummary, Group, Id } from "../ipc/types";
import { usePopupDismiss } from "../util/popup";
import { Icon } from "./Icon";

export function Sidebar() {
  const { state, actions, dispatch } = useStore();
  const [renamingId, setRenamingId] = useState<Id | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [movingId, setMovingId] = useState<Id | null>(null);
  const [newGroupOpen, setNewGroupOpen] = useState(false);
  const [newGroupName, setNewGroupName] = useState("");
  const [renamingGroupId, setRenamingGroupId] = useState<Id | null>(null);
  const [groupNameValue, setGroupNameValue] = useState("");
  const [pendingDelete, setPendingDelete] = useState<ConversationSummary | null>(null);
  const renameRef = useRef<HTMLInputElement | null>(null);
  const newGroupRef = useRef<HTMLDivElement | null>(null);
  const newGroupButtonRef = useRef<HTMLButtonElement | null>(null);
  const deleteDialogRef = useRef<HTMLDivElement | null>(null);
  const deleteCancelRef = useRef<HTMLButtonElement | null>(null);
  /** The buttons that opened a transient surface (focus returns to them). */
  const moveButtonRefs = useRef(new Map<Id, HTMLButtonElement | null>());
  const deleteButtonRefs = useRef(new Map<Id, HTMLButtonElement | null>());

  const buckets = useMemo(() => conversationsByGroup(state), [state]);

  useEffect(() => {
    if (renamingId && renameRef.current) renameRef.current.focus();
  }, [renamingId]);

  const closeNewGroup = useCallback(() => {
    setNewGroupOpen(false);
    setNewGroupName("");
  }, []);

  // The new group form is a transient surface: it closes on a press outside,
  // when the focus leaves it, when the window loses the focus and on Escape.
  usePopupDismiss({
    open: newGroupOpen,
    onDismiss: closeNewGroup,
    popupRef: newGroupRef,
    trigger: () => newGroupButtonRef.current,
    restoreFocus: true,
  });

  // The delete confirmation is a transient surface as well.
  usePopupDismiss({
    open: pendingDelete !== null,
    onDismiss: () => setPendingDelete(null),
    popupRef: deleteDialogRef,
    trigger: () =>
      pendingDelete ? (deleteButtonRefs.current.get(pendingDelete.id) ?? null) : null,
    restoreFocus: true,
  });

  // The confirmation takes the focus, so the keyboard reaches it.
  useEffect(() => {
    if (pendingDelete) deleteCancelRef.current?.focus();
  }, [pendingDelete]);

  const startRename = (conversation: ConversationSummary) => {
    setRenamingId(conversation.id);
    setRenameValue(conversation.title);
    setMovingId(null);
  };

  const commitRename = () => {
    if (!renamingId) return;
    const value = renameValue.trim();
    setRenamingId(null);
    if (!value) return;
    void actions.renameConversation(renamingId, value);
  };

  const commitNewGroup = () => {
    const value = newGroupName.trim();
    if (!value) {
      closeNewGroup();
      return;
    }
    void actions.createGroup(value);
    closeNewGroup();
  };

  return (
    <aside className="sidebar" aria-label={t("a11y.sidebar")}>
      <div className="sidebar__header">
        <h1 className="sidebar__title">{t("app.title")}</h1>
        <button
          type="button"
          className="btn btn--icon"
          style={{ marginLeft: "auto" }}
          title={t("panel.sidebar_collapse_tooltip")}
          aria-label={t("a11y.toggle_sidebar")}
          onClick={() => actions.toggleSidebar(true)}
        >
          <Icon name="sidebar" />
        </button>
      </div>

      <div className="sidebar__toolbar">
        <button
          type="button"
          className="btn btn--accent"
          onClick={() => void actions.createConversation()}
        >
          <Icon name="plus" />
          {t("conversation.new_button")}
        </button>
        <button
          ref={newGroupButtonRef}
          type="button"
          className="btn btn--icon"
          title={t("group.new")}
          aria-label={t("group.new")}
          aria-expanded={newGroupOpen}
          onClick={() => setNewGroupOpen((open) => !open)}
        >
          <Icon name="folderPlus" />
        </button>
      </div>

      {newGroupOpen ? (
        <div className="sidebar__search" ref={newGroupRef}>
          <input
            className="input"
            value={newGroupName}
            placeholder={t("group.new_placeholder")}
            aria-label={t("group.new_placeholder")}
            onChange={(event) => setNewGroupName(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                commitNewGroup();
              } else if (event.key === "Escape") {
                setNewGroupOpen(false);
                setNewGroupName("");
              }
            }}
          />
          <button
            type="button"
            className="btn btn--accent"
            style={{ marginTop: "var(--space-xs)" }}
            onClick={commitNewGroup}
          >
            {t("group.create")}
          </button>
        </div>
      ) : null}

      <div className="sidebar__search">
        <input
          className="input"
          type="search"
          value={state.conversationSearch}
          placeholder={t("conversation.search_placeholder")}
          aria-label={t("conversation.search_placeholder")}
          onChange={(event) =>
            dispatch({ type: "conversation/search", query: event.target.value })
          }
        />
      </div>

      <nav className="sidebar__list" aria-label={t("a11y.groups")}>
        {buckets.map((bucket) => (
          <section className="group" key={bucket.groupId ?? "__ungrouped"}>
            <header className="group__header">
              {renamingGroupId !== null && renamingGroupId === bucket.groupId ? (
                <>
                  <input
                    className="input chat-item__rename"
                    value={groupNameValue}
                    aria-label={t("group.rename")}
                    onChange={(event) => setGroupNameValue(event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") {
                        event.preventDefault();
                        if (bucket.groupId && groupNameValue.trim()) {
                          void actions.renameGroup(bucket.groupId, groupNameValue);
                        }
                        setRenamingGroupId(null);
                      } else if (event.key === "Escape") {
                        setRenamingGroupId(null);
                      }
                    }}
                  />
                  <button
                    type="button"
                    className="btn btn--icon"
                    aria-label={t("group.save")}
                    onClick={() => {
                      if (bucket.groupId && groupNameValue.trim()) {
                        void actions.renameGroup(bucket.groupId, groupNameValue);
                      }
                      setRenamingGroupId(null);
                    }}
                  >
                    <Icon name="check" />
                  </button>
                </>
              ) : (
                <>
                  <span style={{ flex: 1 }}>{bucket.name}</span>
                  {bucket.groupId ? (
                    <>
                      <button
                        type="button"
                        className="btn btn--icon"
                        title={t("group.rename")}
                        aria-label={`${t("group.rename")} ${bucket.name}`}
                        onClick={() => {
                          setRenamingGroupId(bucket.groupId);
                          setGroupNameValue(bucket.name);
                        }}
                      >
                        <Icon name="pencil" size={12} />
                      </button>
                      <button
                        type="button"
                        className="btn btn--icon btn--danger"
                        title={t("group.delete")}
                        aria-label={`${t("group.delete")} ${bucket.name}`}
                        onClick={() => {
                          if (bucket.groupId) void actions.deleteGroup(bucket.groupId);
                        }}
                      >
                        <Icon name="trash" size={12} />
                      </button>
                    </>
                  ) : null}
                </>
              )}
            </header>

            <ul style={{ listStyle: "none", margin: 0, padding: 0 }}>
              {bucket.conversations.map((conversation) => (
                <li key={conversation.id}>
                  <div
                    className="chat-item"
                    data-active={conversation.id === state.currentConversationId}
                    role="button"
                    tabIndex={0}
                    aria-current={
                      conversation.id === state.currentConversationId
                        ? "true"
                        : undefined
                    }
                    onClick={() => {
                      if (renamingId === conversation.id) return;
                      void actions.selectConversation(conversation.id);
                    }}
                    onKeyDown={(event) => {
                      if (event.key === "Enter" || event.key === " ") {
                        event.preventDefault();
                        void actions.selectConversation(conversation.id);
                      }
                    }}
                  >
                    {renamingId === conversation.id ? (
                      <input
                        ref={renameRef}
                        className="input chat-item__rename"
                        value={renameValue}
                        aria-label={t("conversation.rename")}
                        onClick={(event) => event.stopPropagation()}
                        onChange={(event) => setRenameValue(event.target.value)}
                        onKeyDown={(event) => {
                          if (event.key === "Enter") {
                            event.preventDefault();
                            commitRename();
                          } else if (event.key === "Escape") {
                            setRenamingId(null);
                          }
                        }}
                      />
                    ) : (
                      <>
                        <span className="chat-item__title">
                          {conversation.title.trim() || t("conversation.untitled")}
                        </span>
                        <span className="chat-item__actions">
                          <button
                            type="button"
                            className="btn btn--icon"
                            title={t("conversation.rename")}
                            aria-label={`${t("conversation.rename")}: ${conversation.title}`}
                            onClick={(event) => {
                              event.stopPropagation();
                              startRename(conversation);
                            }}
                          >
                            <Icon name="pencil" size={12} />
                          </button>
                          <button
                            type="button"
                            className="btn btn--icon"
                            title={t("conversation.move_to")}
                            aria-label={`${t("conversation.move_to")} ${conversation.title}`}
                            aria-expanded={movingId === conversation.id}
                            ref={(element) => {
                              moveButtonRefs.current.set(conversation.id, element);
                            }}
                            onClick={(event) => {
                              event.stopPropagation();
                              setMovingId(
                                movingId === conversation.id ? null : conversation.id,
                              );
                            }}
                          >
                            <Icon name="folder" size={12} />
                          </button>
                          <button
                            type="button"
                            className="btn btn--icon btn--danger"
                            title={t("conversation.delete")}
                            aria-label={`${t("conversation.delete")}: ${conversation.title}`}
                            ref={(element) => {
                              deleteButtonRefs.current.set(conversation.id, element);
                            }}
                            onClick={(event) => {
                              event.stopPropagation();
                              setPendingDelete(conversation);
                            }}
                          >
                            <Icon name="trash" size={12} />
                          </button>
                        </span>
                      </>
                    )}
                  </div>

                  {movingId === conversation.id ? (
                    <MoveMenu
                      groups={state.groups}
                      conversation={conversation}
                      trigger={() => moveButtonRefs.current.get(conversation.id) ?? null}
                      onPick={(groupId) => {
                        setMovingId(null);
                        void actions.moveConversation(conversation.id, groupId);
                      }}
                      onClose={() => setMovingId(null)}
                    />
                  ) : null}
                </li>
              ))}
            </ul>
          </section>
        ))}

        {buckets.length === 0 ? (
          <p className="hint" style={{ padding: "var(--space-s)" }}>
            {t("empty.title")}
          </p>
        ) : null}
      </nav>

      {pendingDelete ? (
        <div
          className="scrim"
          role="presentation"
          onClick={() => setPendingDelete(null)}
        >
          <div
            ref={deleteDialogRef}
            className="card"
            role="dialog"
            aria-modal="true"
            aria-label={t("conversation.delete")}
            style={{
              position: "absolute",
              top: "50%",
              left: "50%",
              transform: "translate(-50%, -50%)",
              maxWidth: 380,
              zIndex: 30,
            }}
          >
            <p>{`Delete "${pendingDelete.title}"?`}</p>
            <div style={{ display: "flex", gap: "var(--space-s)", justifyContent: "flex-end" }}>
              <button
                ref={deleteCancelRef}
                type="button"
                className="btn"
                onClick={() => setPendingDelete(null)}
              >
                {t("egress.cancel")}
              </button>
              <button
                type="button"
                className="btn btn--danger"
                onClick={() => {
                  void actions.deleteConversation(pendingDelete.id);
                  setPendingDelete(null);
                }}
              >
                {t("conversation.delete")}
              </button>
            </div>
          </div>
        </div>
      ) : null}

      <div className="sidebar__footer">
        <button
          type="button"
          className="btn"
          aria-haspopup="dialog"
          aria-expanded={state.modal === "models"}
          onClick={() => actions.openModal("models")}
        >
          <Icon name="models" />
          {t("panel.models")}
        </button>
        <button
          type="button"
          className="btn"
          aria-haspopup="dialog"
          aria-expanded={state.modal === "settings"}
          onClick={() => actions.openModal("settings")}
        >
          <Icon name="settings" />
          {t("panel.settings")}
        </button>
      </div>
    </aside>
  );
}

interface MoveMenuProps {
  groups: Group[];
  conversation: ConversationSummary;
  onPick(groupId: Id | null): void;
  onClose(): void;
  /** The button that opened the menu (the focus returns to it). */
  trigger?: () => HTMLElement | null;
}

function MoveMenu({ groups, conversation, onPick, onClose, trigger }: MoveMenuProps) {
  const ref = useRef<HTMLDivElement | null>(null);

  // The menu closes as soon as it is not the active surface any more: a
  // press outside, the focus leaving it, the window losing the focus or
  // `Escape`.
  usePopupDismiss({
    open: true,
    onDismiss: onClose,
    popupRef: ref,
    trigger,
    restoreFocus: true,
  });

  return (
    <div
      ref={ref}
      className="card"
      role="menu"
      aria-label={t("conversation.move_to")}
      style={{
        margin: "var(--space-xs) 0 var(--space-xs) var(--space-s)",
        padding: "var(--space-xs)",
        boxShadow: "0 6px 18px rgb(0 0 0 / 10%)",
      }}
    >
      {groups.map((group) => (
        <button
          key={group.id}
          type="button"
          className="btn"
          role="menuitem"
          style={{ width: "100%", justifyContent: "flex-start" }}
          aria-current={conversation.group_id === group.id}
          onClick={() => onPick(group.id)}
        >
          {group.name}
          {conversation.group_id === group.id ? (
            <Icon name="check" size={12} />
          ) : null}
        </button>
      ))}
      <button
        type="button"
        className="btn"
        role="menuitem"
        style={{ width: "100%", justifyContent: "flex-start" }}
        onClick={() => onPick(null)}
      >
        {t("conversation.group_ungrouped")}
      </button>
    </div>
  );
}
