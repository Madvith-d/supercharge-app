import type { RefObject } from "react";
import { createT, type Locale } from "@/i18n";
import * as api from "@/lib/api";
import { type InlineMcpConfigDraft, type InlineMcpDraft } from "@/lib/pluginApiInline";
import { IconPlus, IconTrash } from "@/components/icons";
import { UiCheck } from "@/components/settings/shared";
import { SegmentedControl } from "@/components/ui/SegmentedControl";

interface Props {
  locale: Locale;
  draft: InlineMcpDraft;
  busy: boolean;
  busyStep: string | null;
  validated: api.PluginApiManifest | null;
  reviewApproved: boolean;
  importRef: RefObject<HTMLTextAreaElement | null>;
  importDefinition: () => void;
  updateDraft: (update: (current: InlineMcpDraft) => InlineMcpDraft) => void;
  addConfig: () => void;
  setConfig: (index: number, next: InlineMcpConfigDraft) => void;
  validate: () => Promise<void>;
  install: () => Promise<void>;
  setReviewApproved: (approved: boolean) => void;
}

export function PluginApiDefinitionEditor({
  locale,
  draft,
  busy,
  busyStep,
  validated,
  reviewApproved,
  importRef,
  importDefinition,
  updateDraft,
  addConfig,
  setConfig,
  validate,
  install,
  setReviewApproved,
}: Props) {
  const tr = createT(locale);
  return (
  <section className="ext-ref-block">
            <div className="ext-ref-block__head">
              <IconPlus size={16} />
              <h2 className="ext-ref-block__title">
                {tr("pluginApi.editor.title")}
              </h2>
            </div>
            <div className="plugin-api-import">
              <label className="field">
                <span>{tr("pluginApi.editor.importLabel")}</span>
                <textarea
                  ref={importRef}
                  className="settings-input plugin-api-textarea"
                  rows={4}
                  autoComplete="off"
                  spellCheck={false}
                  disabled={!!busy}
                />
                <span className="ext-field-hint">
                  {tr("pluginApi.editor.importHint")}
                </span>
              </label>
              <button
                type="button"
                className="btn btn--ghost btn--sm"
                disabled={!!busy}
                onClick={importDefinition}
              >
                {tr("pluginApi.editor.import")}
              </button>
            </div>
            <div className="plugin-api-grid">
              <label className="field">
                <span>{tr("pluginApi.editor.pluginId")}</span>
                <input
                  className="settings-input"
                  value={draft.id}
                  onChange={(event) =>
                    updateDraft((current) => ({ ...current, id: event.target.value }))
                  }
                  placeholder={tr("pluginApi.editor.pluginIdPlaceholder")}
                  spellCheck={false}
                />
              </label>
              <label className="field">
                <span>{tr("ext.mcp.name")}</span>
                <input
                  className="settings-input"
                  value={draft.name}
                  onChange={(event) =>
                    updateDraft((current) => ({ ...current, name: event.target.value }))
                  }
                />
              </label>
              <label className="field">
                <span>{tr("pluginApi.editor.version")}</span>
                <input
                  className="settings-input"
                  value={draft.version}
                  onChange={(event) =>
                    updateDraft((current) => ({ ...current, version: event.target.value }))
                  }
                  spellCheck={false}
                />
              </label>
              <label className="field">
                <span>{tr("pluginApi.editor.serverId")}</span>
                <input
                  className="settings-input"
                  value={draft.serverId}
                  onChange={(event) =>
                    updateDraft((current) => ({ ...current, serverId: event.target.value }))
                  }
                  spellCheck={false}
                />
              </label>
            </div>
            <label className="field">
              <span>{tr("pluginApi.editor.description")}</span>
              <input
                className="settings-input"
                value={draft.description}
                onChange={(event) =>
                  updateDraft((current) => ({ ...current, description: event.target.value }))
                }
              />
            </label>
            <div className="field">
              <span>{tr("pluginApi.editor.transport")}</span>
              <SegmentedControl
                value={draft.transport}
                ariaLabel={tr("pluginApi.editor.transport")}
                options={[
                  { value: "stdio", label: tr("pluginApi.editor.stdio") },
                  { value: "http", label: tr("pluginApi.editor.http") },
                ]}
                onChange={(transport) =>
                  updateDraft((current) => ({
                    ...current,
                    transport,
                    config: [],
                  }))
                }
              />
            </div>
            {draft.transport === "stdio" ? (
              <>
                <label className="field">
                  <span>{tr("ext.mcp.command")}</span>
                  <input
                    className="settings-input"
                    value={draft.command}
                    onChange={(event) =>
                      updateDraft((current) => ({
                        ...current,
                        command: event.target.value,
                      }))
                    }
                    spellCheck={false}
                  />
                </label>
                <label className="field">
                  <span>{tr("ext.mcp.args")}</span>
                  <textarea
                    className="settings-input plugin-api-textarea"
                    rows={3}
                    value={draft.argsText}
                    onChange={(event) =>
                      updateDraft((current) => ({
                        ...current,
                        argsText: event.target.value,
                      }))
                    }
                    placeholder={'["--flag", "value"]'}
                    spellCheck={false}
                  />
                  <span className="ext-field-hint">
                    {tr("pluginApi.editor.argsHint")}
                  </span>
                </label>
              </>
            ) : (
              <label className="field">
                <span>{tr("pluginApi.editor.url")}</span>
                <input
                  className="settings-input"
                  type="url"
                  value={draft.url}
                  onChange={(event) =>
                    updateDraft((current) => ({ ...current, url: event.target.value }))
                  }
                  placeholder="https://mcp.example.com/rpc"
                  spellCheck={false}
                />
              </label>
            )}

            <div className="plugin-api-secrets">
              <div className="ext-ref-block__head">
                <div>
                  <div className="ext-ref-section-label">
                    {tr("pluginApi.secrets.title")}
                  </div>
                  <p className="ext-ref-block__lead">
                    {tr("pluginApi.secrets.hint")}
                  </p>
                </div>
                <button
                  type="button"
                  className="btn btn--ghost btn--sm"
                  onClick={addConfig}
                >
                  <IconPlus size={13} />
                  {tr("pluginApi.secrets.add")}
                </button>
              </div>
              {draft.config.map((field, index) => (
                <div className="plugin-api-secret-row" key={`${index}:${field.key}`}>
                  <input
                    className="settings-input"
                    aria-label={tr("pluginApi.secrets.key")}
                    value={field.key}
                    onChange={(event) =>
                      setConfig(index, { ...field, key: event.target.value })
                    }
                    placeholder="token"
                    spellCheck={false}
                  />
                  <input
                    className="settings-input"
                    aria-label={tr("pluginApi.secrets.target")}
                    value={field.targetName}
                    onChange={(event) =>
                      setConfig(index, { ...field, targetName: event.target.value })
                    }
                    placeholder={
                      draft.transport === "stdio" ? "API_TOKEN" : "Authorization"
                    }
                    spellCheck={false}
                  />
                  <UiCheck
                    checked={field.required}
                    onChange={(required) => setConfig(index, { ...field, required })}
                    label={tr("pluginApi.secrets.required")}
                  />
                  <button
                    type="button"
                    className="ext-ref-gear"
                    aria-label={tr("ext.mcp.remove")}
                    onClick={() =>
                      updateDraft((current) => ({
                        ...current,
                        config: current.config.filter((_, item) => item !== index),
                      }))
                    }
                  >
                    <IconTrash size={14} />
                  </button>
                </div>
              ))}
            </div>

            <div className="settings-row__actions">
              <button
                type="button"
                className="btn btn--ghost btn--sm"
                disabled={!!busy}
                onClick={() => void validate()}
              >
                {busyStep === "validate"
                  ? tr("pluginApi.validating")
                  : tr("pluginApi.validate")}
              </button>
            </div>
            {validated ? (
              <div className="plugin-api-review">
                <div className="ext-ref-section-label">
                  {tr("pluginApi.review.title")}
                </div>
                <pre className="ext-details-pre">
                  {JSON.stringify(validated, null, 2)}
                </pre>
                <UiCheck
                  checked={reviewApproved}
                  disabled={!!busy}
                  onChange={setReviewApproved}
                  label={tr("pluginApi.review.approve")}
                />
                <div className="settings-row__actions">
                  <button
                    type="button"
                    className="btn btn--solid btn--sm"
                    disabled={!!busy || !reviewApproved}
                    onClick={() => void install()}
                  >
                    {busyStep === "install"
                      ? tr("pluginApi.installing")
                      : tr("pluginApi.install")}
                  </button>
                </div>
              </div>
            ) : null}
          </section>
  );
}
