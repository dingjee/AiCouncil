import { useMemo, useState } from "react";

import { t } from "../i18n/strings";
import { presentation } from "../model/presentation";
import { groupModels, orderedPreferences } from "../state/selectors";
import { useStore } from "../state/store";
import { Icon } from "./Icon";

/**
 * Models panel.
 *
 * Shows the models the user enabled ("My models"), the full catalog grouped
 * by family, the primary model, the per-model parameter overrides and the
 * catalog refresh control.
 */
export function ModelsPanel() {
  const { state, actions, dispatch } = useStore();
  const [overridesFor, setOverridesFor] = useState<string | null>(null);
  const [overridesDraft, setOverridesDraft] = useState("");
  const [overridesError, setOverridesError] = useState<string | null>(null);

  const groups = useMemo(
    () => groupModels(state.catalog, state.preferences),
    [state.catalog, state.preferences],
  );
  const preferences = useMemo(
    () => orderedPreferences(state.preferences),
    [state.preferences],
  );
  const enabledIds = useMemo(
    () => new Set(preferences.filter((item) => item.enabled).map((item) => item.model_id)),
    [preferences],
  );

  const openOverrides = (modelId: string) => {
    const preference = preferences.find((item) => item.model_id === modelId);
    setOverridesFor(modelId);
    setOverridesDraft(
      preference?.overrides ? JSON.stringify(preference.overrides, null, 2) : "",
    );
    setOverridesError(null);
  };

  const saveOverrides = () => {
    if (!overridesFor) return;
    const trimmed = overridesDraft.trim();
    if (!trimmed) {
      void actions.setModelOverrides(overridesFor, null);
      setOverridesFor(null);
      return;
    }
    try {
      const parsed: unknown = JSON.parse(trimmed);
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
        setOverridesError(t("models.overrides_invalid"));
        return;
      }
      void actions.setModelOverrides(overridesFor, parsed as Record<string, unknown>);
      setOverridesFor(null);
    } catch {
      setOverridesError(t("models.overrides_invalid"));
    }
  };

  const move = (modelId: string, delta: number) => {
    const ids = preferences.map((item) => item.model_id);
    const index = ids.indexOf(modelId);
    const target = index + delta;
    if (index < 0 || target < 0 || target >= ids.length) return;
    const next = ids.slice();
    const [moved] = next.splice(index, 1);
    next.splice(target, 0, moved);
    void actions.reorderModels(next);
  };

  return (
    <div className="panel-section">
      <div style={{ display: "flex", alignItems: "center", gap: "var(--space-s)" }}>
        <h2 className="panel-section__title" style={{ margin: 0, flex: 1 }}>
          {t("panel.models")}
        </h2>
        <button
          type="button"
          className="btn"
          disabled={state.catalogRefreshing}
          onClick={() => void actions.refreshCatalog()}
        >
          <Icon name="retry" size={13} />
          {state.catalogRefreshing ? t("models.refreshing") : t("models.refresh_catalog")}
        </button>
      </div>

      <input
        className="input"
        type="search"
        value={state.modelSearch}
        placeholder={t("models.search_placeholder")}
        aria-label={t("models.search_placeholder")}
        onChange={(event) =>
          dispatch({ type: "model/search", query: event.target.value })
        }
      />

      <section>
        <h3 className="panel-section__title">{t("models.mine")}</h3>
        {preferences.filter((item) => item.enabled).length === 0 ? (
          <p className="hint">{t("models.none_enabled")}</p>
        ) : null}
        {preferences
          .filter((item) => item.enabled)
          .map((preference) => {
            const entry = state.catalog.find(
              (item) => item.id === preference.model_id,
            );
            const label = entry
              ? presentation(entry.id, entry.name).full_label
              : preference.model_id;
            return (
              <div className="model-row" key={preference.model_id}>
                <span className="model-row__name" title={preference.model_id}>
                  {label}
                </span>
                {preference.is_default ? (
                  <span className="badge">{t("composer.primary_badge")}</span>
                ) : (
                  <button
                    type="button"
                    className="btn btn--icon"
                    title={t("models.set_default")}
                    aria-label={`${t("models.set_default")}: ${label}`}
                    onClick={() => void actions.setPrimaryModel(preference.model_id)}
                  >
                    <Icon name="star" size={13} />
                  </button>
                )}
                <button
                  type="button"
                  className="btn btn--icon"
                  title={t("models.overrides")}
                  aria-label={`${t("models.overrides")}: ${label}`}
                  aria-pressed={overridesFor === preference.model_id}
                  onClick={() => openOverrides(preference.model_id)}
                >
                  {t("models.overrides")}
                </button>
                <button
                  type="button"
                  className="btn btn--icon"
                  title={t("models.move_up_tooltip")}
                  aria-label={t("models.move_up_tooltip")}
                  onClick={() => move(preference.model_id, -1)}
                >
                  ↑
                </button>
                <button
                  type="button"
                  className="btn btn--icon"
                  title={t("models.move_down_tooltip")}
                  aria-label={t("models.move_down_tooltip")}
                  onClick={() => move(preference.model_id, 1)}
                >
                  ↓
                </button>
                <button
                  type="button"
                  className="btn btn--icon btn--danger"
                  title={t("models.enabled")}
                  aria-label={`${t("models.enabled")}: ${label}`}
                  onClick={() =>
                    void actions.setModelEnabled(preference.model_id, false)
                  }
                >
                  <Icon name="close" size={12} />
                </button>
              </div>
            );
          })}
      </section>

      {overridesFor ? (
        <section className="card">
          <label className="field__label" htmlFor="model-overrides">
            {t("models.overrides")}
          </label>
          <textarea
            id="model-overrides"
            className="input"
            rows={5}
            value={overridesDraft}
            placeholder='{"temperature": 0.2}'
            aria-describedby="model-overrides-hint"
            onChange={(event) => setOverridesDraft(event.target.value)}
          />
          <p className="hint" id="model-overrides-hint">
            {t("models.overrides_hint")}
          </p>
          {overridesError ? <p className="card__error">{overridesError}</p> : null}
          <div style={{ display: "flex", gap: "var(--space-s)" }}>
            <button type="button" className="btn btn--accent" onClick={saveOverrides}>
              {t("title.save")}
            </button>
            <button
              type="button"
              className="btn"
              onClick={() => setOverridesFor(null)}
            >
              {t("egress.cancel")}
            </button>
          </div>
        </section>
      ) : null}

      <section>
        <h3 className="panel-section__title">
          {t("models.catalog").replace("{n}", String(state.catalog.length))}
        </h3>
        {groups.map((group) => (
          <div key={group.label} style={{ marginBottom: "var(--space-s)" }}>
            <div className="picker__group-title">{group.label}</div>
            {group.items.map((item) => (
              <div className="model-row" key={item.raw_id}>
                <span className="model-row__name" title={item.raw_id}>
                  {item.full_label}
                </span>
                <span className="model-row__price">
                  {priceLabel(state.catalog, item.raw_id)}
                </span>
                <input
                  type="checkbox"
                  checked={enabledIds.has(item.raw_id)}
                  aria-label={`${t("models.enabled")}: ${item.full_label}`}
                  onChange={(event) =>
                    void actions.setModelEnabled(item.raw_id, event.target.checked)
                  }
                />
              </div>
            ))}
          </div>
        ))}
      </section>
    </div>
  );
}

function priceLabel(
  catalog: { id: string; prompt_price: string | null }[],
  modelId: string,
): string {
  const entry = catalog.find((item) => item.id === modelId);
  if (!entry) return t("models.price_unknown");
  const price = entry.prompt_price;
  if (!price) return t("models.price_unknown");
  if (price === "0" || price === "0.00") return t("models.price_free");
  return `$${price}/M`;
}
