import { useMemo } from "react";

import { t } from "../i18n/strings";
import { formatUsd } from "../util/money";
import { useStore } from "../state/store";
import { Icon } from "./Icon";

/**
 * Settings panel.
 *
 * The provider keys are read by the application runtime from the local
 * configuration file. The UI therefore shows **only booleans** plus a
 * "Reload" control that asks the runtime to re-read them; no secret ever
 * enters the webview and nothing is stored in the browser.
 */
export function SettingsPanel() {
  const { state, actions } = useStore();
  const credential = state.credentials;

  const providerEntries = useMemo(
    () => Object.entries(credential ?? {}),
    [credential],
  );
  const missing = useMemo(
    () =>
      providerEntries.filter(([, configured]) => !configured).map(([name]) => name),
    [providerEntries],
  );
  const configured = missing.length === 0 && providerEntries.length > 0;

  const bodySize = state.settings.body_text_size ?? 15;

  return (
    <div className="panel-section">
      <h2 className="panel-section__title">{t("settings.title")}</h2>

      <section className="card">
        <h3 className="panel-section__title">{t("settings.provider_keys")}</h3>
        <p style={{ margin: "0 0 var(--space-s)" }}>
          <span
            className="status-dot"
            data-state={configured ? "ok" : "missing"}
            aria-hidden="true"
            style={{ display: "inline-block", marginRight: 6 }}
          />
          {configured
            ? t("settings.credentials_configured")
            : t("settings.credentials_not_configured")}
        </p>

        <ul style={{ listStyle: "none", margin: 0, padding: 0 }}>
          {providerEntries.map(([name, isConfigured]) => (
            <li
              key={name}
              style={{
                display: "flex",
                alignItems: "center",
                gap: 6,
                fontSize: "0.85em",
              }}
            >
              <span
                className="status-dot"
                data-state={isConfigured ? "ok" : "missing"}
                aria-hidden="true"
              />
              <span>{name}</span>
              <span className="muted" style={{ marginLeft: "auto" }}>
                {isConfigured
                  ? t("settings.credentials_configured")
                  : t("settings.credentials_not_configured")}
              </span>
            </li>
          ))}
        </ul>

        {missing.length > 0 ? (
          <p className="hint" style={{ marginTop: "var(--space-s)" }}>
            {t("settings.credentials_missing").replace(
              "{providers}",
              missing.join(", "),
            )}
          </p>
        ) : null}

        <p className="hint" style={{ marginTop: "var(--space-s)" }}>
          {t("settings.credentials_env_hint")}
        </p>

        <button
          type="button"
          className="btn"
          style={{ marginTop: "var(--space-s)" }}
          disabled={state.credentialsReloading}
          title={t("settings.credentials_reload_tooltip")}
          onClick={() => void actions.reloadCredentials()}
        >
          <Icon name="retry" size={13} />
          {state.credentialsReloading
            ? t("settings.credentials_reloading")
            : t("settings.credentials_reload_credentials")}
        </button>

        {state.keyStatus ? (
          <p className="hint" style={{ marginTop: "var(--space-s)" }}>
            {keyStatusLabel(state.keyStatus)}
          </p>
        ) : null}

        <button
          type="button"
          className="btn"
          style={{ marginTop: "var(--space-xs)" }}
          disabled={state.keyStatusLoading}
          title={t("models.test_connection")}
          onClick={() => void actions.refreshKeyStatus()}
        >
          <Icon name="models" size={13} />
          {state.keyStatusLoading ? t("models.testing") : t("models.test_connection")}
        </button>
      </section>

      <section className="card">
        <h3 className="panel-section__title">{t("settings.font_size")}</h3>
        <label className="field">
          <span className="field__label">
            {t("settings.font_size_value").replace("{n}", String(bodySize))}
          </span>
          <input
            type="range"
            min={12}
            max={22}
            step={1}
            value={bodySize}
            aria-label={t("settings.font_size")}
            onChange={(event) => {
              const value = Number(event.target.value);
              document.documentElement.style.setProperty("--body-size", `${value}px`);
              void actions.updateSettings({ body_text_size: value });
            }}
          />
        </label>
      </section>

      <section className="card">
        <h3 className="panel-section__title">{t("settings.about")}</h3>
        <p className="small" style={{ margin: 0, overflowWrap: "anywhere" }}>
          {t("app.title")} {state.appVersion}
        </p>
      </section>

      <section className="card">
        <p className="small" style={{ margin: "0 0 var(--space-xs)" }}>
          {t("settings.concurrency").replace(
            "{n}",
            String(state.settings.max_concurrent_requests),
          )}
        </p>
        <p className="small" style={{ margin: "0 0 var(--space-xs)" }}>
          {t("settings.assumed_output").replace(
            "{n}",
            String(state.settings.assumed_output_tokens),
          )}
        </p>
        <p className="small" style={{ margin: 0 }}>
          {t("settings.theme")}
        </p>
        <p className="small muted" style={{ margin: "var(--space-xs) 0 0" }}>
          {t("settings.export_note")}
        </p>
      </section>
    </div>
  );
}

function keyStatusLabel(status: {
  status: string;
  label: string | null;
  usage: string | null;
  limit: string | null;
  limit_remaining: string | null;
  error: string | null;
}): string {
  if (status.status === "ok") {
    const parts: string[] = [status.label ?? t("settings.credentials_configured")];
    if (status.usage) parts.push(`used $${formatUsd(status.usage)}`);
    if (status.limit_remaining) {
      parts.push(`remaining $${formatUsd(status.limit_remaining)}`);
    }
    return parts.join(" · ");
  }
  if (status.status === "unsupported") return t("models.connected_no_support");
  if (status.status === "permission_denied") return t("models.connected_no_quota");
  return status.error ?? t("models.connect_failed").replace("{error}", status.status);
}
