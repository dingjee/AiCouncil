import { useEffect, useMemo, useRef } from "react";

import { t } from "../i18n/strings";
import {
  enabledModels,
  filterModelGroups,
  groupEnabledModels,
  resolveSelection,
} from "../state/selectors";
import { useStore } from "../state/store";
import { usePopupDismiss } from "../util/popup";
import { Icon } from "./Icon";

export interface ModelPickerProps {
  onClose(): void;
  /** Opens the Models panel (the management surface). */
  onManageModels?(): void;
  /** The control that opened the picker (the focus returns there on Escape). */
  trigger?: () => HTMLElement | null;
}

/**
 * Model picker.
 *
 * The picker is a **selection** surface: it only offers the models the user
 * enabled in the Models panel ("My models"). It never enables a model of the
 * full catalogue — that happens in the Models panel only.
 *
 * One list for both modes: every entry carries a checkbox, and the checked
 * entries are the selection. One checked model sends to that model, several
 * checked models ask the same question in parallel, and no checked model
 * disables the send. The star marks the primary model that owns the main
 * branch; it always belongs to the selection.
 *
 * Checking a model never closes the picker — the user picks several models
 * in one go.
 */
export function ModelPicker({ onClose, onManageModels, trigger }: ModelPickerProps) {
  const { state, actions, dispatch } = useStore();
  const searchRef = useRef<HTMLInputElement | null>(null);
  const panelRef = useRef<HTMLDivElement | null>(null);

  // The picker only ever shows the enabled models.
  const groups = useMemo(
    () => groupEnabledModels(state.preferences, state.catalog),
    [state.preferences, state.catalog],
  );
  const filtered = useMemo(
    () => filterModelGroups(groups, state.pickerSearch),
    [groups, state.pickerSearch],
  );
  const enabled = useMemo(() => enabledModels(state), [state]);
  const selection = useMemo(
    () => resolveSelection(enabled, [], state.modelSelection),
    [enabled, state.modelSelection],
  );

  // The picker closes as soon as it is not the active surface any more.
  usePopupDismiss({
    open: true,
    onDismiss: onClose,
    popupRef: panelRef,
    trigger,
    restoreFocus: true,
  });

  useEffect(() => {
    searchRef.current?.focus();
    // A new picker starts with an empty query.
    dispatch({ type: "picker/search", query: "" });
  }, [dispatch]);

  /** The label of every enabled model (the id when the catalogue lost it). */
  const labels = useMemo(() => {
    const map = new Map<string, string>();
    for (const group of groups) {
      for (const item of group.items) map.set(item.raw_id, item.full_label);
    }
    return map;
  }, [groups]);

  const primaryName = selection.primary
    ? (labels.get(selection.primary) ?? selection.primary)
    : "";
  const checkedCount = state.modelSelection.checked.length;

  return (
    <div className="picker" role="dialog" aria-label={t("selector.open")} ref={panelRef}>
      <input
        ref={searchRef}
        className="input picker__search"
        type="search"
        value={state.pickerSearch}
        placeholder={t("selector.search_placeholder")}
        aria-label={t("selector.search_placeholder")}
        onChange={(event) =>
          dispatch({ type: "picker/search", query: event.target.value })
        }
      />

      {enabled.length === 0 ? (
        <div className="picker__empty">
          <p className="hint" style={{ margin: 0 }}>
            {t("selector.empty")}
          </p>
          <p className="hint" style={{ margin: 0 }}>
            {t("selector.empty_hint")}
          </p>
          {onManageModels ? (
            <button
              type="button"
              className="btn btn--accent"
              onClick={() => {
                onClose();
                onManageModels();
              }}
            >
              <Icon name="models" size={14} />
              {t("selector.manage_models")}
            </button>
          ) : null}
        </div>
      ) : null}

      {enabled.length > 0 && filtered.length === 0 ? (
        <p className="hint" style={{ padding: "var(--space-s)" }}>
          {t("selector.no_results").replace("{query}", state.pickerSearch)}
        </p>
      ) : null}

      <div
        role="listbox"
        aria-multiselectable
        aria-label={t("a11y.model_list")}
      >
        {filtered.map((group) => (
          <div className="picker__group" key={group.label}>
            <div className="picker__group-title">{group.label}</div>
            {group.items.map((item) => {
              const isChecked = state.modelSelection.checked.includes(item.raw_id);
              const isPrimary = selection.primary === item.raw_id;
              return (
                <div
                  key={item.raw_id}
                  className="picker__row"
                  data-selected={isChecked}
                  role="option"
                  aria-selected={isChecked}
                  tabIndex={0}
                  onClick={() => actions.toggleChecked(item.raw_id, !isChecked)}
                  onKeyDown={(event) => {
                    // The star and the checkbox are children of the row:
                    // their own key handling must not toggle the model a
                    // second time.
                    if (event.target !== event.currentTarget) return;
                    if (event.key === "Enter" || event.key === " ") {
                      event.preventDefault();
                      actions.toggleChecked(item.raw_id, !isChecked);
                    }
                  }}
                >
                  <input
                    type="checkbox"
                    checked={isChecked}
                    tabIndex={-1}
                    aria-label={item.full_label}
                    onClick={(event) => event.stopPropagation()}
                    onChange={(event) =>
                      actions.toggleChecked(item.raw_id, event.target.checked)
                    }
                  />

                  <span className="picker__name" title={item.full_label}>
                    {item.full_label}
                  </span>
                  <span className="picker__id">{item.raw_id}</span>

                  <button
                    type="button"
                    className="btn btn--icon"
                    title={t("composer.primary_tooltip")}
                    aria-label={`${t("composer.primary_badge")}: ${item.full_label}`}
                    aria-pressed={isPrimary}
                    onClick={(event) => {
                      event.stopPropagation();
                      void actions.setPrimaryModel(item.raw_id);
                    }}
                  >
                    <Icon name={isPrimary ? "starFilled" : "star"} size={13} />
                  </button>
                </div>
              );
            })}
          </div>
        ))}
      </div>

      <div className="picker__footer">
        <span className="small muted">{t("selector.multi_hint")}</span>
        <span className="small" style={{ marginLeft: "auto" }}>
          {t("selector.selected_count").replace("{n}", String(checkedCount))}
          {primaryName ? ` · ${primaryName}` : ""}
        </span>
        <button
          type="button"
          className="btn"
          onClick={() => actions.setChecked(enabled)}
        >
          {t("selector.select_all")}
        </button>
      </div>
    </div>
  );
}
