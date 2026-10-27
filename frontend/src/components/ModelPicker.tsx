import { useEffect, useMemo, useRef } from "react";

import { t } from "../i18n/strings";
import { presentation } from "../model/presentation";
import {
  enabledModels,
  filterModelGroups,
  groupModels,
  resolveSelection,
} from "../state/selectors";
import { useStore } from "../state/store";
import { Icon } from "./Icon";

export interface ModelPickerProps {
  onClose(): void;
}

/**
 * Model picker.
 *
 * One entry for both modes: clicking a model selects it (single mode),
 * checking models selects several (parallel mode) and the star marks the
 * primary model that owns the main branch.
 */
export function ModelPicker({ onClose }: ModelPickerProps) {
  const { state, actions, dispatch } = useStore();
  const searchRef = useRef<HTMLInputElement | null>(null);

  const groups = useMemo(
    () => groupModels(state.catalog, state.preferences),
    [state.catalog, state.preferences],
  );
  const filtered = useMemo(
    () => filterModelGroups(groups, state.modelSearch),
    [groups, state.modelSearch],
  );
  const enabled = useMemo(() => enabledModels(state), [state]);
  const selection = useMemo(
    () => resolveSelection(enabled, [], state.modelSelection),
    [enabled, state.modelSelection],
  );
  const parallel = state.modelSelection.parallel;

  useEffect(() => {
    searchRef.current?.focus();
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  const primaryName = useMemo(() => {
    if (!selection.primary) return "";
    const entry = state.catalog.find((item) => item.id === selection.primary);
    return entry ? presentation(entry.id, entry.name).full_label : selection.primary;
  }, [selection.primary, state.catalog]);

  return (
    <div className="picker" role="dialog" aria-label={t("selector.open")}>
      <input
        ref={searchRef}
        className="input picker__search"
        type="search"
        value={state.modelSearch}
        placeholder={t("selector.search_placeholder")}
        aria-label={t("selector.search_placeholder")}
        onChange={(event) =>
          dispatch({ type: "model/search", query: event.target.value })
        }
      />

      {filtered.length === 0 ? (
        <p className="hint" style={{ padding: "var(--space-s)" }}>
          {t("selector.no_results").replace("{query}", state.modelSearch)}
        </p>
      ) : null}

      <div role="listbox" aria-multiselectable={parallel} aria-label={t("a11y.model_list")}>
        {filtered.map((group) => (
          <div className="picker__group" key={group.label}>
            <div className="picker__group-title">{group.label}</div>
            {group.items.map((item) => {
              const isEnabled = enabled.includes(item.raw_id);
              const isChecked = state.modelSelection.checked.includes(item.raw_id);
              const isPrimary = selection.primary === item.raw_id;
              const selected = parallel ? isChecked : isPrimary;
              return (
                <div
                  key={item.raw_id}
                  className="picker__row"
                  data-selected={selected}
                  role="option"
                  aria-selected={selected}
                  tabIndex={0}
                  onClick={() => {
                    if (parallel) {
                      actions.toggleChecked(item.raw_id, !isChecked);
                    } else {
                      void actions.setPrimaryModel(item.raw_id);
                      onClose();
                    }
                  }}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" || event.key === " ") {
                      event.preventDefault();
                      if (parallel) {
                        actions.toggleChecked(item.raw_id, !isChecked);
                      } else {
                        void actions.setPrimaryModel(item.raw_id);
                        onClose();
                      }
                    }
                  }}
                >
                  {parallel ? (
                    <input
                      type="checkbox"
                      checked={isChecked}
                      readOnly
                      tabIndex={-1}
                      aria-label={item.full_label}
                      onClick={(event) => event.stopPropagation()}
                    />
                  ) : null}

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

                  <label className="btn btn--icon" title={t("models.enabled")}>
                    <input
                      type="checkbox"
                      checked={isEnabled}
                      aria-label={`${t("models.enabled")}: ${item.full_label}`}
                      onClick={(event) => event.stopPropagation()}
                      onChange={(event) =>
                        void actions.setModelEnabled(item.raw_id, event.target.checked)
                      }
                    />
                  </label>
                </div>
              );
            })}
          </div>
        ))}
      </div>

      <div className="picker__footer">
        <span className="small muted">
          {parallel ? t("selector.multi_hint") : t("selector.single_hint")}
        </span>
        <span className="small" style={{ marginLeft: "auto" }}>
          {primaryName
            ? parallel && selection.targets.length > 1
              ? t("selector.primary_plus")
                  .replace("{primary}", primaryName)
                  .replace("{n}", String(selection.targets.length - 1))
              : primaryName
            : t("models.none_enabled")}
        </span>
        {parallel ? (
          <button
            type="button"
            className="btn"
            onClick={() => actions.setChecked(enabled)}
          >
            {t("selector.select_all")}
          </button>
        ) : null}
      </div>
    </div>
  );
}
