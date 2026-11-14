import { useEffect, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { copy } from "../copy";
import { checkForUpdates, installUpdate, useUpdates } from "../updates";

export function UpdateStatus() {
  const { status, installing: busy, installed, checking, installError } = useUpdates();
  const [confirm, setConfirm] = useState(false);
  useEffect(() => { if (!useUpdates.getState().status) void checkForUpdates(); }, []);
  return <div>
    <p className="help">{installError ?? status?.error ?? (installed ? copy.updateAction.installed : status?.available ? `${copy.updates.available} ${status.available}` : status?.configured ? copy.updates.current : copy.updates.unconfigured)}</p>
    {isTauri() ? <button type="button" disabled={checking || busy} onClick={() => void checkForUpdates()}>{checking ? copy.updates.checking : copy.updates.check}</button> : null}
    {status?.available && !installed ? <button type="button" disabled={busy} onClick={() => setConfirm(true)}>{busy ? copy.updateAction.installing : copy.updateAction.install}</button> : null}
    {confirm ? <div role="alertdialog" aria-label={copy.updateAction.install} className="confirm">
      <p>{copy.updateAction.confirm}</p>
      <button type="button" disabled={busy} onClick={() => { setConfirm(false); void installUpdate(); }}>{copy.models.confirm}</button>
      <button type="button" onClick={() => setConfirm(false)}>{copy.models.cancel}</button>
    </div> : null}
  </div>;
}
