import { useEffect, useState } from "react";
import { isTauri, invoke } from "@tauri-apps/api/core";
import { copy } from "../copy";

export function UpdateStatus() {
  const [status, setStatus] = useState<{ configured: boolean; available: string | null; error: string | null } | null>(null);
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [installed, setInstalled] = useState(false);
  const [checking, setChecking] = useState(false);
  const check = async () => {
    setChecking(true);
    try { setStatus(await invoke<NonNullable<typeof status>>("update_status")); }
    catch (error) { setStatus({ configured: true, available: null, error: String(error) }); }
    finally { setChecking(false); }
  };
  useEffect(() => { if (isTauri()) void check(); }, []);
  const install = async () => {
    setConfirm(false); setBusy(true);
    try { await invoke("install_update"); setInstalled(true); }
    catch (error) { setStatus((old) => old ? { ...old, error: String(error) } : null); }
    finally { setBusy(false); }
  };
  return <div>
    <p className="help">{status?.error ?? (installed ? copy.updateAction.installed : status?.available ? `${copy.updates.available} ${status.available}` : status?.configured ? copy.updates.current : copy.updates.unconfigured)}</p>
    {isTauri() ? <button type="button" disabled={checking || busy} onClick={() => void check()}>{checking ? copy.updates.checking : copy.updates.check}</button> : null}
    {status?.available && !installed ? <button type="button" disabled={busy} onClick={() => setConfirm(true)}>{busy ? copy.updateAction.installing : copy.updateAction.install}</button> : null}
    {confirm ? <div role="alertdialog" aria-label={copy.updateAction.install} className="confirm">
      <p>{copy.updateAction.confirm}</p>
      <button type="button" onClick={() => void install()}>{copy.models.confirm}</button>
      <button type="button" onClick={() => setConfirm(false)}>{copy.models.cancel}</button>
    </div> : null}
  </div>;
}
