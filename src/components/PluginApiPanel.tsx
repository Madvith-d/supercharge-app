import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
} from "react";
import * as api from "@/lib/api";
import { createT, type Locale } from "@/i18n";
import {
  EMPTY_INLINE_MCP_DRAFT,
  buildInlineMcpManifest,
  importInlineMcpDraft,
  type InlineMcpConfigDraft,
  type InlineMcpDraft,
} from "@/lib/pluginApiInline";
import { PluginApiRemoveDialog } from "@/components/PluginApiRemoveDialog";
import {
  IconPlug,
  IconShieldCheck,
  IconTrash,
} from "@/components/icons";
import { UiCheck, UiSwitch } from "@/components/settings/shared";
import { PluginApiDefinitionEditor } from "@/components/PluginApiDefinitionEditor";
import { PluginApiConnectionSection } from "@/components/PluginApiConnectionSection";
import { Select } from "@/components/Select";

type BusyStep =
  | "connect"
  | "disconnect"
  | "refresh"
  | "validate"
  | "install"
  | "configure"
  | "trust"
  | "enable"
  | "disable"
  | "remove"
  | "apply"
  | "retry"
  | "verify"
  | "unbind";

export interface PluginApiPanelProps {
  locale: Locale;
  appSessionId?: string | null;
  agentSessionId?: string | null;
  onOpenAccount?: () => void;
}

function requiredConfigurationComplete(plugin: api.PluginApiPublicPlugin): boolean {
  const configured = new Set(plugin.configuredKeys);
  return Object.entries(plugin.manifest.userConfig).every(
    ([key, field]) => !field.required || configured.has(key),
  );
}

export function PluginApiPanel({
  locale,
  appSessionId = null,
  agentSessionId = null,
  onOpenAccount,
}: PluginApiPanelProps) {
  const tr = useMemo(() => createT(locale), [locale]);
  const apiKeyRef = useRef<HTMLInputElement>(null);
  const bridgeKeyRef = useRef<HTMLInputElement>(null);
  const importRef = useRef<HTMLTextAreaElement>(null);
  const verifyArgumentsRef = useRef<HTMLTextAreaElement>(null);
  const configFormRef = useRef<HTMLFormElement>(null);
  const [account, setAccount] = useState<api.AccountStatus | null>(null);
  const [connection, setConnection] =
    useState<api.PluginApiConnectionStatus | null>(null);
  const [endpoint, setEndpoint] = useState("http://127.0.0.1:4319");
  const [plugins, setPlugins] = useState<api.PluginApiPublicPlugin[]>([]);
  const [catalog, setCatalog] = useState<api.PluginApiCatalogEntry[]>([]);
  const [catalogError, setCatalogError] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [draft, setDraft] = useState<InlineMcpDraft>(EMPTY_INLINE_MCP_DRAFT);
  const [validated, setValidated] = useState<api.PluginApiManifest | null>(null);
  const [reviewApproved, setReviewApproved] = useState(false);
  const [permissionApproved, setPermissionApproved] = useState(false);
  const [operation, setOperation] = useState<api.PluginApiOperation | null>(null);
  const [bridge, setBridge] = useState<api.PluginApiBridgeStatus | null>(null);
  const [workspaceId, setWorkspaceId] = useState("");
  const [serverId, setServerId] = useState("");
  const [verifyTool, setVerifyTool] = useState("");
  const [verifyConsent, setVerifyConsent] = useState(false);
  const [busy, setBusy] = useState<BusyStep | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [removeTarget, setRemoveTarget] =
    useState<api.PluginApiPublicPlugin | null>(null);

  const selected = useMemo(
    () => plugins.find((plugin) => plugin.id === selectedId) ?? null,
    [plugins, selectedId],
  );
  const selectedServerIds = selected
    ? Object.keys(selected.manifest.mcpServers)
    : [];
  const selectedServerId = selectedServerIds.includes(serverId)
    ? serverId
    : (selectedServerIds[0] ?? null);
  const workspaceIds = connection?.capabilities?.workspaceIds ?? [];
  const selectedWorkspaceId = workspaceIds.includes(workspaceId)
    ? workspaceId
    : (workspaceIds[0] ?? "");
  const bridgeTools = Array.isArray(bridge?.tools) ? bridge.tools : [];
  const selectedVerifyTool = bridgeTools.some((tool) => tool.name === verifyTool)
    ? verifyTool
    : (bridgeTools[0]?.name ?? "");

  const updateDraft = useCallback(
    (update: (current: InlineMcpDraft) => InlineMcpDraft) => {
      setDraft((current) => update(current));
      setValidated(null);
      setReviewApproved(false);
      setPermissionApproved(false);
      setNotice(null);
    },
    [],
  );

  useEffect(() => {
    if (!api.isDesktopHost()) return;
    let cancelled = false;
    void api.pluginApiCatalog()
      .then((result) => { if (!cancelled) { setCatalog(result.plugins); setCatalogError(false); } })
      .catch(() => { if (!cancelled) setCatalogError(true); });
    return () => { cancelled = true; };
  }, []);

  const loadPlugins = useCallback(
    async (
      status: api.PluginApiConnectionStatus,
      preferId?: string | null,
    ): Promise<api.PluginApiPublicPlugin[]> => {
      if (!status.connected || !status.connectionId) {
        setPlugins([]);
        setSelectedId(null);
        return [];
      }
      const result = await api.pluginApiList(status.connectionId);
      setPlugins(result.plugins);
      setSelectedId((current) => {
        const preferred = preferId ?? current;
        if (preferred && result.plugins.some((plugin) => plugin.id === preferred)) {
          return preferred;
        }
        return result.plugins[0]?.id ?? null;
      });
      return result.plugins;
    },
    [],
  );

  useEffect(() => {
    const ids = connection?.capabilities?.workspaceIds ?? [];
    setWorkspaceId((current) =>
      current && ids.includes(current) ? current : (ids[0] ?? ""),
    );
  }, [connection]);

  useEffect(() => {
    let cancelled = false;
    setBridge(null);
    setVerifyConsent(false);
    setVerifyTool("");
    if (!api.isDesktopHost() || !appSessionId) return;
    void api
      .pluginApiBridgeStatus(appSessionId)
      .then((status) => {
        if (!cancelled) setBridge(status);
      })
      .catch((cause) => {
        if (!cancelled) setError(String(cause));
      });
    return () => {
      cancelled = true;
    };
  }, [appSessionId]);

  useEffect(() => {
    if (!appSessionId || !bridge || !["waiting", "removing"].includes(bridge.status)) {
      return;
    }
    const timer = window.setInterval(() => {
      void api
        .pluginApiBridgeStatus(appSessionId)
        .then((status) => setBridge(status))
        .catch(() => undefined);
    }, 1_000);
    return () => window.clearInterval(timer);
  }, [appSessionId, bridge]);

  useEffect(() => {
    let cancelled = false;
    if (!api.isDesktopHost()) {
      setConnection({
        connected: false,
        endpoint: null,
        connectionId: null,
        capabilities: null,
      });
      return;
    }
    void Promise.all([
      api.pluginApiStatus(),
      api.accountStatus({ refreshBilling: false, includeLocalUsage: false }),
    ])
      .then(async ([status, accountStatus]) => {
        if (cancelled) return;
        setConnection(status);
        setAccount(accountStatus);
        if (status.endpoint) setEndpoint(status.endpoint);
        if (status.connected) await loadPlugins(status);
      })
      .catch((cause) => {
        if (!cancelled) setError(String(cause));
      });
    return () => {
      cancelled = true;
    };
  }, [loadPlugins]);

  const run = useCallback(
    async <T,>(step: BusyStep, task: () => Promise<T>): Promise<T | null> => {
      if (busy) return null;
      setBusy(step);
      setError(null);
      setNotice(null);
      try {
        return await task();
      } catch (cause) {
        setError(String(cause));
        return null;
      } finally {
        setBusy(null);
      }
    },
    [busy],
  );

  const connect = async (event: FormEvent) => {
    event.preventDefault();
    const input = apiKeyRef.current;
    const apiKey = input?.value ?? "";
    await run("connect", async () => {
      try {
        const status = await api.pluginApiConnect(endpoint, apiKey);
        setConnection(status);
        await loadPlugins(status);
      } finally {
        if (input) input.value = "";
      }
    });
  };

  const disconnect = async () => {
    await run("disconnect", async () => {
      const status = await api.pluginApiDisconnect();
      setConnection(status);
      setPlugins([]);
      setSelectedId(null);
      setOperation(null);
    });
  };

  const refresh = async (preferId?: string | null) => {
    if (!connection) return;
    await run("refresh", () => loadPlugins(connection, preferId));
  };

  const importDefinition = () => {
    const input = importRef.current;
    const raw = input?.value ?? "";
    setError(null);
    setNotice(null);
    try {
      const next = importInlineMcpDraft(JSON.parse(raw));
      setDraft(next);
      setValidated(null);
      setReviewApproved(false);
      setPermissionApproved(false);
      setNotice(tr("pluginApi.editor.imported"));
    } catch (cause) {
      setError(String(cause));
    } finally {
      if (input) input.value = "";
    }
  };

  const validate = async () => {
    if (!connection?.connectionId) return;
    await run("validate", async () => {
      const manifest = buildInlineMcpManifest(draft);
      const result = await api.pluginApiValidateInline(
        connection.connectionId!,
        manifest,
      );
      setValidated(result.manifest);
      setReviewApproved(false);
      setPermissionApproved(false);
      setNotice(tr("pluginApi.validated"));
    });
  };

  const wait = useCallback(
    async <T,>(initial: api.PluginApiOperation<T>) => {
      if (!connection?.connectionId) throw new Error("Plugin API disconnected");
      setOperation(initial);
      const terminal = await api.waitForPluginApiOperation(
        connection.connectionId,
        initial,
        { onUpdate: setOperation },
      );
      return terminal.result;
    },
    [connection?.connectionId],
  );

  const install = async () => {
    if (!connection?.connectionId || !validated || !reviewApproved) return;
    await run("install", async () => {
      const initial = await api.pluginApiInstall(
        connection.connectionId!,
        { type: "inline", manifest: validated },
        api.newPluginApiIdempotencyKey(`install-${validated.id}`),
      );
      const plugin = await wait(initial);
      await loadPlugins(connection, plugin?.id ?? validated.id);
      setNotice(tr("pluginApi.installed"));
    });
  };

  const applyAction = async (
    step: BusyStep,
    plugin: api.PluginApiPublicPlugin,
    action: api.PluginApiAction,
    success: string,
  ): Promise<boolean> => {
    if (!connection?.connectionId) return false;
    const result = await run(step, async () => {
      const initial = await api.pluginApiAction(
        connection.connectionId!,
        plugin.id,
        action,
        api.newPluginApiIdempotencyKey(`${step}-${plugin.id}`),
      );
      const updated = await wait(initial);
      await loadPlugins(connection, updated?.id ?? plugin.id);
      setNotice(success);
      return true;
    });
    return result === true;
  };

  const configure = async (event: FormEvent) => {
    event.preventDefault();
    if (!selected) return;
    const form = configFormRef.current;
    if (!form) return;
    const values: Record<string, string> = {};
    const data = new FormData(form);
    for (const [key, field] of Object.entries(selected.manifest.userConfig)) {
      const value = data.get(`config:${key}`);
      const text = typeof value === "string" ? value : "";
      if (text) values[key] = text;
      if (
        field.required &&
        !text &&
        !selected.configuredKeys.includes(key)
      ) {
        setError(`Required configuration is missing: ${key}`);
        return;
      }
    }
    const saved = await applyAction(
      "configure",
      selected,
      { action: "configure", expectedRevision: selected.revision, values },
      tr("pluginApi.configured"),
    );
    if (saved) form.reset();
  };

  const applyBridge = async (event: FormEvent) => {
    event.preventDefault();
    const input = bridgeKeyRef.current;
    const bridgeKey = input?.value ?? "";
    if (
      !connection?.connectionId ||
      !appSessionId ||
      !selected ||
      !selectedServerId ||
      !selectedWorkspaceId
    ) {
      return;
    }
    await run("apply", async () => {
      try {
        const status = await api.pluginApiBridgeApply({
          connectionId: connection.connectionId!,
          appSessionId,
          pluginId: selected.id,
          serverId: selectedServerId,
          workspaceId: selectedWorkspaceId,
          bridgeKey,
          expectedRevision: bridge?.revision ?? null,
        });
        setBridge(status);
        setVerifyConsent(false);
        setVerifyTool(status.tools[0]?.name ?? "");
      } finally {
        if (input) input.value = "";
      }
    });
  };

  const retryBridge = async () => {
    if (!appSessionId || bridge?.revision == null) return;
    await run("retry", async () => {
      const status = await api.pluginApiBridgeRetry(
        appSessionId,
        bridge.revision!,
      );
      setBridge(status);
      setVerifyTool(status.tools[0]?.name ?? "");
    });
  };

  const verifyBridge = async (event: FormEvent) => {
    event.preventDefault();
    if (
      !appSessionId ||
      bridge?.revision == null ||
      !selectedVerifyTool ||
      !verifyConsent
    ) {
      return;
    }
    const input = verifyArgumentsRef.current;
    const raw = input?.value.trim() || "{}";
    await run("verify", async () => {
      const argumentsValue: unknown = JSON.parse(raw);
      if (
        argumentsValue === null ||
        typeof argumentsValue !== "object" ||
        Array.isArray(argumentsValue)
      ) {
        throw new Error("Verification arguments must be a JSON object");
      }
      const status = await api.pluginApiBridgeVerify({
        appSessionId,
        expectedRevision: bridge.revision!,
        tool: selectedVerifyTool,
        arguments: argumentsValue as Record<string, unknown>,
        consent: true,
      });
      setBridge(status);
      setVerifyConsent(false);
      if (input) input.value = "";
    });
  };

  const removeBridge = async () => {
    if (!appSessionId || bridge?.revision == null) return;
    await run("unbind", async () => {
      const status = await api.pluginApiBridgeRemove(
        appSessionId,
        bridge.revision!,
      );
      setBridge(status);
      setVerifyConsent(false);
      setVerifyTool("");
    });
  };

  const addConfig = () => {
    updateDraft((current) => {
      const used = new Set(current.config.map((field) => field.key));
      let index = 1;
      let key = "secret";
      while (used.has(key)) key = `secret-${++index}`;
      return {
        ...current,
        config: [
          ...current.config,
          {
            key,
            targetName:
              current.transport === "stdio" ? "API_TOKEN" : "Authorization",
            target: current.transport === "stdio" ? "env" : "header",
            description: "",
            required: true,
          },
        ],
      };
    });
  };

  const setConfig = (index: number, next: InlineMcpConfigDraft) => {
    updateDraft((current) => ({
      ...current,
      config: current.config.map((field, fieldIndex) =>
        fieldIndex === index ? next : field,
      ),
    }));
  };

  const accountReady = Boolean(
    account?.profile.signedIn || account?.hasOfficialKey || account?.hasRelayKey,
  );
  const scopes = connection?.capabilities?.scopes?.join(", ") || "—";
  const workspaces =
    connection?.capabilities?.workspaceIds?.join(", ") || "—";
  const configComplete = selected
    ? requiredConfigurationComplete(selected)
    : false;

  if (!api.isDesktopHost()) {
    return <p className="ext-ref-empty">{tr("pluginApi.desktopOnly")}</p>;
  }

  return (
    <div
      className="ext-ref-stack plugin-api-panel"
      id="settings-anchor-ext-managed-mcp"
      data-testid="plugin-api-panel"
    >
      {error ? (
        <div className="ext-alert ext-alert--error" role="alert">
          <div className="ext-alert__title">{tr("pluginApi.error.title")}</div>
          <p className="ext-alert__body">{error}</p>
          <button
            type="button"
            className="btn btn--ghost ext-alert__cta"
            onClick={() => setError(null)}
          >
            {tr("common.dismiss")}
          </button>
        </div>
      ) : null}
      {notice ? (
        <div className="ext-alert" role="status">
          <p className="ext-alert__body">{notice}</p>
        </div>
      ) : null}

      <PluginApiConnectionSection
        locale={locale}
        account={account}
        accountReady={accountReady}
        onOpenAccount={onOpenAccount}
        connection={connection}
        scopes={scopes}
        workspaces={workspaces}
        busy={!!busy}
        connecting={busy === "connect"}
        endpoint={endpoint}
        setEndpoint={setEndpoint}
        apiKeyRef={apiKeyRef}
        connect={connect}
        disconnect={disconnect}
        refresh={() => refresh()}
      />

      <section className="ext-ref-block">
        <div className="ext-ref-block__head">
          <IconPlug size={16} />
          <h2 className="ext-ref-block__title">{tr("pluginApi.catalog.title")}</h2>
          <span className="ext-ref-block__meta">{catalog.length}</span>
        </div>
        <p className="ext-ref-empty">{tr("pluginApi.catalog.description")}</p>
        {catalogError && <p className="ext-ref-empty" role="status">{tr("pluginApi.catalog.unavailable")}</p>}
        {!catalogError && catalog.length === 0 && <p className="ext-ref-empty">{tr("pluginApi.catalog.empty")}</p>}
        <ul className="ext-ref-list">
          {catalog.map((entry) => {
            const installed = plugins.find((plugin) => plugin.id === entry.id);
            return <li key={entry.id} className="ext-ref-row">
              <div className="ext-ref-row__main">
                <div className="ext-ref-row__icon" aria-hidden><IconPlug size={16} /></div>
                <div className="ext-ref-row__body">
                  <div className="ext-ref-row__title">{entry.name} · {entry.version}</div>
                  <div className="ext-ref-row__desc">{entry.description}</div>
                  <div className="ext-ref-row__meta">{entry.manifest.permissions.join(", ")} · SHA-256 {entry.source.sha256.slice(0, 16)}…</div>
                </div>
                <div className="ext-ref-row__end">
                  <button type="button" className="btn btn--ghost btn--sm" disabled={!!busy || !connection?.connected || !!installed} onClick={() => { void run("install", async () => {
                    if (!connection?.connectionId) return;
                    const initial = await api.pluginApiInstall(connection.connectionId, entry.source, api.newPluginApiIdempotencyKey(`catalog-${entry.id}-${entry.version}`));
                    const installed = await wait(initial);
                    await loadPlugins(connection, installed?.id ?? entry.id);
                    setNotice(tr("pluginApi.installed"));
                  }); }}>
                    {installed ? tr("pluginApi.catalog.installed") : tr("pluginApi.catalog.install")}
                  </button>
                </div>
              </div>
            </li>;
          })}
        </ul>
      </section>

      {connection?.connected ? (
        <>
          <section className="ext-ref-block">
            <div className="ext-ref-block__head">
              <IconPlug size={16} />
              <h2 className="ext-ref-block__title">
                {tr("pluginApi.inventory.title")}
              </h2>
              <span className="ext-ref-block__meta">{plugins.length}</span>
            </div>
            {plugins.length === 0 ? (
              <p className="ext-ref-empty">{tr("pluginApi.inventory.empty")}</p>
            ) : (
              <ul className="ext-ref-list">
                {plugins.map((plugin) => (
                  <li
                    key={plugin.id}
                    className={
                      "ext-ref-row" +
                      (selectedId === plugin.id ? " plugin-api-row--selected" : "")
                    }
                  >
                    <div className="ext-ref-row__main">
                      <div className="ext-ref-row__icon" aria-hidden>
                        <IconPlug size={16} />
                      </div>
                      <div className="ext-ref-row__body">
                        <div className="ext-ref-row__title">
                          {plugin.manifest.name}
                        </div>
                        <div className="ext-ref-row__desc">
                          {plugin.id} · {plugin.manifest.version}
                        </div>
                        <div className="ext-ref-row__meta">
                          <span
                            className={`ext-badge ext-badge--${plugin.trusted ? "ok" : "warn"}`}
                          >
                            {plugin.trusted
                              ? tr("pluginApi.status.trusted")
                              : tr("pluginApi.status.untrusted")}
                          </span>
                          <span
                            className={`ext-badge ext-badge--${requiredConfigurationComplete(plugin) ? "ok" : "warn"}`}
                          >
                            {requiredConfigurationComplete(plugin)
                              ? tr("pluginApi.status.configured")
                              : tr("pluginApi.status.needsConfig")}
                          </span>
                        </div>
                      </div>
                      <div className="ext-ref-row__end">
                        <button
                          type="button"
                          className="btn btn--ghost btn--sm"
                          onClick={() => {
                            setSelectedId(plugin.id);
                            setPermissionApproved(false);
                          }}
                        >
                          {tr("pluginApi.inventory.select")}
                        </button>
                        <UiSwitch
                          checked={plugin.enabled}
                          disabled={!!busy || (!plugin.enabled && (!plugin.trusted || !requiredConfigurationComplete(plugin)))}
                          label={plugin.enabled ? tr("ext.enabled") : tr("ext.disabled")}
                          onChange={(next) => {
                            void applyAction(
                              next ? "enable" : "disable",
                              plugin,
                              {
                                action: next ? "enable" : "disable",
                                expectedRevision: plugin.revision,
                              },
                              next
                                ? tr("pluginApi.enable")
                                : tr("ext.disabled"),
                            );
                          }}
                        />
                      </div>
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </section>

          {selected ? (
            <section className="ext-ref-block plugin-api-selected">
              <div className="ext-ref-block__head">
                <IconShieldCheck size={16} />
                <h2 className="ext-ref-block__title">{selected.manifest.name}</h2>
                <span className="ext-ref-block__meta">rev {selected.revision}</span>
              </div>
              {Object.keys(selected.manifest.userConfig).length > 0 ? (
                <form
                  ref={configFormRef}
                  className="plugin-api-form"
                  onSubmit={(event) => void configure(event)}
                >
                  <div className="ext-ref-section-label">
                    {tr("pluginApi.secrets.title")}
                  </div>
                  {Object.entries(selected.manifest.userConfig).map(
                    ([key, field]) => (
                      <label className="field" key={key}>
                        <span>
                          {tr("pluginApi.secrets.value", { name: key })}
                          {field.required ? " *" : ""}
                        </span>
                        <input
                          className="settings-input"
                          type="password"
                          name={`config:${key}`}
                          autoComplete="off"
                          spellCheck={false}
                          maxLength={8192}
                          placeholder={
                            selected.configuredKeys.includes(key)
                              ? tr("pluginApi.secrets.keep")
                              : undefined
                          }
                          disabled={!!busy}
                        />
                      </label>
                    ),
                  )}
                  <div className="settings-row__actions">
                    <button
                      type="submit"
                      className="btn btn--ghost btn--sm"
                      disabled={!!busy}
                    >
                      {busy === "configure"
                        ? tr("pluginApi.configuring")
                        : tr("pluginApi.configure")}
                    </button>
                  </div>
                </form>
              ) : null}

              {!selected.trusted ? (
                <div className="plugin-api-review">
                  <div className="ext-ref-section-label">
                    {tr("pluginApi.review.permission")}
                  </div>
                  <div className="plugin-api-permissions">
                    {selected.manifest.permissions.map((permission) => (
                      <code key={permission}>{permission}</code>
                    ))}
                  </div>
                  <UiCheck
                    checked={permissionApproved}
                    disabled={!!busy}
                    onChange={setPermissionApproved}
                    label={tr("pluginApi.review.approve")}
                  />
                  <div className="settings-row__actions">
                    <button
                      type="button"
                      className="btn btn--solid btn--sm"
                      disabled={!!busy || !permissionApproved || !configComplete}
                      onClick={() =>
                        void applyAction(
                          "trust",
                          selected,
                          {
                            action: "trust",
                            expectedRevision: selected.revision,
                            permissions: selected.manifest.permissions,
                          },
                          tr("pluginApi.trust"),
                        )
                      }
                    >
                      {busy === "trust"
                        ? tr("pluginApi.trusting")
                        : tr("pluginApi.trust")}
                    </button>
                  </div>
                </div>
              ) : null}

              {selected.trusted && !selected.enabled ? (
                <div className="settings-row__actions">
                  <button
                    type="button"
                    className="btn btn--solid btn--sm"
                    disabled={!!busy || !configComplete}
                    onClick={() =>
                      void applyAction(
                        "enable",
                        selected,
                        {
                          action: "enable",
                          expectedRevision: selected.revision,
                        },
                        tr("pluginApi.enable"),
                      )
                    }
                  >
                    {busy === "enable"
                      ? tr("pluginApi.enabling")
                      : tr("pluginApi.enable")}
                  </button>
                </div>
              ) : null}

              {selected.enabled ? (
                <form className="plugin-api-form" onSubmit={(event) => void applyBridge(event)}>
                  <div className="ext-ref-section-label">
                    {tr("pluginApi.agent.title")}
                  </div>
                  {appSessionId ? (
                    <p className="plugin-api-meta">
                      {tr("pluginApi.agent.session", {
                        value: agentSessionId || appSessionId,
                      })}
                    </p>
                  ) : (
                    <p className="ext-ref-empty">{tr("pluginApi.agent.noSession")}</p>
                  )}
                  <div className="plugin-api-grid">
                    <div className="field">
                      <span>{tr("pluginApi.agent.workspace")}</span>
                      <Select
                        value={selectedWorkspaceId}
                        options={workspaceIds.map(
                          (id) => ({ value: id, label: id }),
                        )}
                        onChange={setWorkspaceId}
                        disabled={!!busy}
                        aria-label={tr("pluginApi.agent.workspace")}
                      />
                    </div>
                    <div className="field">
                      <span>{tr("pluginApi.editor.serverId")}</span>
                      <Select
                        value={selectedServerId ?? ""}
                        options={selectedServerIds.map((id) => ({ value: id, label: id }))}
                        onChange={setServerId}
                        disabled={!!busy}
                        aria-label={tr("pluginApi.editor.serverId")}
                      />
                    </div>
                  </div>
                  <label className="field">
                    <span>{tr("pluginApi.agent.bridgeKey")}</span>
                    <input
                      ref={bridgeKeyRef}
                      className="settings-input"
                      type="password"
                      aria-label={tr("pluginApi.agent.bridgeKey")}
                      placeholder={tr("pluginApi.agent.bridgeKeyPlaceholder")}
                      autoComplete="off"
                      spellCheck={false}
                      minLength={32}
                      maxLength={256}
                      disabled={!!busy || !appSessionId}
                    />
                    <span className="ext-field-hint">
                      {tr("pluginApi.agent.bridgeKeyHint")}
                    </span>
                  </label>
                  <div className="settings-row__actions">
                    <button
                      type="submit"
                      className="btn btn--solid btn--sm"
                      disabled={
                        !!busy ||
                        !appSessionId ||
                        !selectedWorkspaceId ||
                        !selectedServerId
                      }
                    >
                      {busy === "apply"
                        ? tr("pluginApi.agent.applying")
                        : tr("pluginApi.agent.apply")}
                    </button>
                    {bridge?.configured && bridge.revision != null ? (
                      <button
                        type="button"
                        className="btn btn--ghost btn--sm"
                        disabled={!!busy}
                        onClick={() => void retryBridge()}
                      >
                        {tr("pluginApi.agent.retry")}
                      </button>
                    ) : null}
                    {bridge?.configured && bridge.revision != null ? (
                      <button
                        type="button"
                        className="btn btn--ghost btn--sm ext-item__danger"
                        disabled={!!busy}
                        onClick={() => void removeBridge()}
                      >
                        {tr("pluginApi.agent.remove")}
                      </button>
                    ) : null}
                  </div>
                </form>
              ) : null}

              <div className="plugin-api-not-ready" role="status">
                <strong>
                  {bridge?.status === "ready"
                    ? tr("pluginApi.agent.ready")
                    : tr("pluginApi.agent.notApplied")}
                </strong>
                <span>
                  {bridge?.configured
                    ? tr("pluginApi.agent.status", { value: bridge.status })
                    : tr("pluginApi.agent.notReady")}
                </span>
                {bridge?.lastError ? <span>{bridge.lastError}</span> : null}
              </div>

              {bridge?.status === "applied" && bridgeTools.length > 0 ? (
                <form
                  className="plugin-api-form plugin-api-verify"
                  onSubmit={(event) => void verifyBridge(event)}
                >
                  <div className="ext-ref-section-label">
                    {tr("pluginApi.agent.verifyTitle")}
                  </div>
                  <p className="ext-ref-block__lead">
                    {tr("pluginApi.agent.verifyHint")}
                  </p>
                  <div className="field">
                    <span>{tr("pluginApi.agent.tool")}</span>
                    <Select
                      value={selectedVerifyTool}
                      options={bridgeTools.map((tool) => ({
                        value: tool.name,
                        label: tool.description
                          ? `${tool.name} · ${tool.description}`
                          : tool.name,
                      }))}
                      onChange={setVerifyTool}
                      disabled={!!busy}
                      aria-label={tr("pluginApi.agent.tool")}
                    />
                  </div>
                  <label className="field">
                    <span>{tr("pluginApi.agent.arguments")}</span>
                    <textarea
                      ref={verifyArgumentsRef}
                      className="settings-input plugin-api-textarea"
                      rows={3}
                      defaultValue="{}"
                      spellCheck={false}
                      disabled={!!busy}
                    />
                  </label>
                  <UiCheck
                    checked={verifyConsent}
                    disabled={!!busy}
                    onChange={setVerifyConsent}
                    label={tr("pluginApi.agent.verifyConsent")}
                  />
                  <div className="settings-row__actions">
                    <button
                      type="submit"
                      className="btn btn--solid btn--sm"
                      disabled={!!busy || !verifyConsent}
                    >
                      {busy === "verify"
                        ? tr("pluginApi.agent.verifying")
                        : tr("pluginApi.agent.verify")}
                    </button>
                  </div>
                </form>
              ) : null}

              <div className="settings-row__actions">
                <button
                  type="button"
                  className="btn btn--ghost btn--sm ext-item__danger"
                  disabled={!!busy}
                  onClick={() => setRemoveTarget(selected)}
                >
                  <IconTrash size={13} />
                  {tr("ext.plugins.uninstall")}
                </button>
              </div>
            </section>
          ) : null}

          <PluginApiDefinitionEditor
            locale={locale}
            draft={draft}
            busy={!!busy}
            busyStep={busy}
            validated={validated}
            reviewApproved={reviewApproved}
            importRef={importRef}
            importDefinition={importDefinition}
            updateDraft={updateDraft}
            addConfig={addConfig}
            setConfig={setConfig}
            validate={validate}
            install={install}
            setReviewApproved={setReviewApproved}
          />
        </>
      ) : null}

      {operation ? (
        <p className="plugin-api-operation" role="status">
          {tr("pluginApi.status.operation", {
            id: operation.id.slice(0, 8),
            status: operation.status,
          })}
        </p>
      ) : null}

      <PluginApiRemoveDialog
        locale={locale}
        removeTarget={removeTarget}
        busy={busy}
        setRemoveTarget={setRemoveTarget}
        remove={(plugin) => applyAction("remove", plugin, {
          action: "uninstall", expectedRevision: plugin.revision,
        }, tr("ext.plugins.uninstall"))}
      />
    </div>
  );
}
