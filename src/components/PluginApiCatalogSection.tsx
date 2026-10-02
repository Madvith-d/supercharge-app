import * as api from "@/lib/api";
import { createT, type Locale } from "@/i18n";
import { IconPlug } from "@/components/icons";

export function PluginApiCatalogSection({
  locale,
  catalog,
  unavailable,
  installed,
  busy,
  connected,
  onInstall,
}: {
  locale: Locale;
  catalog: api.PluginApiCatalogEntry[];
  unavailable: boolean;
  installed: api.PluginApiPublicPlugin[];
  busy: boolean;
  connected: boolean;
  onInstall: (entry: api.PluginApiCatalogEntry) => void;
}) {
  const tr = createT(locale);
  return <section className="ext-ref-block">
    <div className="ext-ref-block__head">
      <IconPlug size={16} />
      <h2 className="ext-ref-block__title">{tr("pluginApi.catalog.title")}</h2>
      <span className="ext-ref-block__meta">{catalog.length}</span>
    </div>
    <p className="ext-ref-empty">{tr("pluginApi.catalog.description")}</p>
    {unavailable && <p className="ext-ref-empty" role="status">{tr("pluginApi.catalog.unavailable")}</p>}
    {!unavailable && catalog.length === 0 && <p className="ext-ref-empty">{tr("pluginApi.catalog.empty")}</p>}
    <ul className="ext-ref-list">
      {catalog.map((entry) => {
        const existing = installed.find((plugin) => plugin.id === entry.id);
        return <li key={entry.id} className="ext-ref-row">
          <div className="ext-ref-row__main">
            <div className="ext-ref-row__icon" aria-hidden><IconPlug size={16} /></div>
            <div className="ext-ref-row__body">
              <div className="ext-ref-row__title">{entry.name} · {entry.version}</div>
              <div className="ext-ref-row__desc">{entry.description}</div>
              <div className="ext-ref-row__meta">{entry.manifest.permissions.join(", ")} · SHA-256 {entry.source.sha256.slice(0, 16)}…</div>
            </div>
            <div className="ext-ref-row__end">
              <button type="button" className="btn btn--ghost btn--sm" disabled={busy || !connected || !!existing} onClick={() => onInstall(entry)}>
                {existing ? tr("pluginApi.catalog.installed") : tr("pluginApi.catalog.install")}
              </button>
            </div>
          </div>
        </li>;
      })}
    </ul>
  </section>;
}
