import * as api from "@/lib/api";
import { createT, type Locale } from "@/i18n";
import { GlassModal } from "@/components/GlassModal";

interface Props {
  locale: Locale;
  removeTarget: api.PluginApiPublicPlugin | null;
  busy: string | null;
  setRemoveTarget: (plugin: api.PluginApiPublicPlugin | null) => void;
  remove: (plugin: api.PluginApiPublicPlugin) => Promise<boolean>;
}

export function PluginApiRemoveDialog({
  locale, removeTarget, busy, setRemoveTarget, remove,
}: Props) {
  const tr = createT(locale);
  return (
      <GlassModal
        open={!!removeTarget}
        onClose={() => {
          if (!busy) setRemoveTarget(null);
        }}
        title={tr("pluginApi.removeTitle")}
        size="sm"
        closeLabel={tr("common.close")}
        footer={
          <>
            <button
              type="button"
              className="btn btn--ghost"
              disabled={!!busy}
              onClick={() => setRemoveTarget(null)}
            >
              {tr("common.cancel")}
            </button>
            <button
              type="button"
              className="btn btn--danger"
              disabled={!!busy || !removeTarget}
              onClick={() => {
                if (!removeTarget) return;
                void remove(removeTarget).then((removed) => {
                  if (removed) setRemoveTarget(null);
                });
              }}
            >
              {busy === "remove"
                ? tr("pluginApi.removing")
                : tr("ext.plugins.uninstall")}
            </button>
          </>
        }
      >
        <p className="app-dialog__msg">
          {tr("pluginApi.removeConfirm", {
            name: removeTarget?.manifest.name ?? "",
          })}
        </p>
      </GlassModal>
  );
}
