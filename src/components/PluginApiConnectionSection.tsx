import type { FormEvent, RefObject } from "react";
import * as api from "@/lib/api";
import { createT, type Locale } from "@/i18n";
import { IconKey, IconRefresh, IconUser } from "@/components/icons";

interface Props {
  locale: Locale;
  account: api.AccountStatus | null;
  accountReady: boolean;
  onOpenAccount?: () => void;
  connection: api.PluginApiConnectionStatus | null;
  scopes: string;
  workspaces: string;
  busy: boolean;
  connecting: boolean;
  endpoint: string;
  setEndpoint: (endpoint: string) => void;
  apiKeyRef: RefObject<HTMLInputElement | null>;
  connect: (event: FormEvent) => Promise<void>;
  disconnect: () => Promise<void>;
  refresh: () => Promise<void>;
}

export function PluginApiConnectionSection({
  locale, account, accountReady, onOpenAccount, connection, scopes,
  workspaces, busy, connecting, endpoint, setEndpoint, apiKeyRef,
  connect, disconnect, refresh,
}: Props) {
  const tr = createT(locale);
  return (
    <>
      <section className="ext-ref-block plugin-api-account">
        <div className="ext-ref-block__head">
          <IconUser size={16} />
          <h2 className="ext-ref-block__title">{tr("pluginApi.account.title")}</h2>
          <span
            className={`ext-badge ext-badge--${accountReady ? "ok" : "muted"}`}
          >
            {accountReady ? tr("account.signedIn") : tr("account.signedOut")}
          </span>
        </div>
        <p className="ext-ref-block__lead">{tr("pluginApi.account.separate")}</p>
        {account?.profile.email ? (
          <p className="plugin-api-meta">{account.profile.email}</p>
        ) : null}
        {onOpenAccount ? (
          <div className="settings-row__actions">
            <button
              type="button"
              className="btn btn--ghost btn--sm"
              onClick={onOpenAccount}
            >
              {tr("managedSetup.openAccount")}
            </button>
          </div>
        ) : null}
      </section>

      <section className="ext-ref-block">
        <div className="ext-ref-block__head">
          <IconKey size={16} />
          <h2 className="ext-ref-block__title">
            {tr("pluginApi.connection.title")}
          </h2>
          <span
            className={`ext-badge ext-badge--${connection?.connected ? "ok" : "muted"}`}
          >
            {connection?.connected
              ? tr("pluginApi.connection.connected")
              : tr("pluginApi.connection.disconnected")}
          </span>
        </div>
        {connection?.connected ? (
          <div className="plugin-api-connection">
            <code className="plugin-api-endpoint">{connection.endpoint}</code>
            <p className="plugin-api-meta">
              {tr("pluginApi.connection.scopes", { value: scopes })}
            </p>
            <p className="plugin-api-meta">
              {tr("pluginApi.connection.workspaces", { value: workspaces })}
            </p>
            <div className="settings-row__actions">
              <button
                type="button"
                className="btn btn--ghost btn--sm"
                disabled={!!busy}
                onClick={() => void refresh()}
              >
                <IconRefresh size={14} />
                {tr("ext.refresh")}
              </button>
              <button
                type="button"
                className="btn btn--ghost btn--sm"
                disabled={!!busy}
                onClick={() => void disconnect()}
              >
                {tr("pluginApi.connection.disconnect")}
              </button>
            </div>
          </div>
        ) : (
          <form className="plugin-api-form" onSubmit={(event) => void connect(event)}>
            <label className="field">
              <span>{tr("pluginApi.connection.endpoint")}</span>
              <input
                className="settings-input"
                value={endpoint}
                onChange={(event) => setEndpoint(event.target.value)}
                placeholder={tr("pluginApi.connection.endpointPlaceholder")}
                autoComplete="url"
                spellCheck={false}
                disabled={!!busy}
              />
            </label>
            <label className="field">
              <span>{tr("pluginApi.connection.apiKey")}</span>
              <input
                ref={apiKeyRef}
                className="settings-input"
                type="password"
                aria-label={tr("pluginApi.connection.apiKey")}
                placeholder={tr("pluginApi.connection.apiKeyPlaceholder")}
                autoComplete="off"
                spellCheck={false}
                minLength={32}
                maxLength={256}
                disabled={!!busy}
              />
              <span className="ext-field-hint">
                {tr("pluginApi.connection.apiKeyHint")}
              </span>
            </label>
            <div className="settings-row__actions">
              <button
                type="submit"
                className="btn btn--solid btn--sm"
                disabled={!!busy}
              >
                {connecting
                  ? tr("pluginApi.connection.connecting")
                  : tr("pluginApi.connection.connect")}
              </button>
            </div>
          </form>
        )}
      </section>

    </>
  );
}
