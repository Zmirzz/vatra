import { useEffect, useId, useRef, type KeyboardEvent } from "react";
import { copy } from "../copy";
import { usePreferences } from "../settings/preferences";
import { checkForUpdates, dismissUpdate, installUpdate, useUpdates } from "../updates";

function UpdateDialog({ version }: { version: string }) {
  const { installing, installError } = useUpdates();
  const dialog = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const descriptionId = useId();
  const controls = () => Array.from(dialog.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []);
  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    controls()[0]?.focus();
    return () => { if (previous?.isConnected) previous.focus(); };
  }, []);
  useEffect(() => { if (installing) dialog.current?.focus(); }, [installing]);
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    // App shortcuts must not open another modal behind the update prompt.
    event.stopPropagation();
    if (event.key === "Escape") { event.preventDefault(); dismissUpdate(); }
    if (event.key === "Tab") {
      const buttons = controls();
      if (!buttons.length) { event.preventDefault(); return; }
      if (event.shiftKey && (document.activeElement === buttons[0] || document.activeElement === dialog.current)) {
        event.preventDefault(); buttons.at(-1)?.focus();
      } else if (!event.shiftKey && (document.activeElement === buttons.at(-1) || document.activeElement === dialog.current)) {
        event.preventDefault(); buttons[0]?.focus();
      }
    }
  };
  return <div className="dialog-backdrop">
    <div ref={dialog} className="dialog" role="alertdialog" aria-modal="true" aria-labelledby={titleId} aria-describedby={descriptionId} aria-busy={installing} tabIndex={-1} onKeyDown={onKeyDown}>
      <h2 id={titleId}>{copy.startupUpdate.title}</h2>
      <p id={descriptionId}>{copy.startupUpdate.available.replace("{version}", version)}</p>
      <p>{copy.updateAction.confirm}</p>
      {installError ? <p role="alert">{installError}</p> : null}
      {installing ? <p role="status">{copy.updateAction.installing}</p> : null}
      <div className="dialog-actions">
        <button type="button" disabled={installing} onClick={dismissUpdate}>{copy.startupUpdate.later}</button>
        <button type="button" className="primary" disabled={installing} onClick={() => void installUpdate()}>{installing ? copy.updateAction.installing : copy.updateAction.install}</button>
      </div>
    </div>
  </div>;
}

export function StartupUpdatePrompt({ suspended = false }: { suspended?: boolean }) {
  usePreferences(state => state.preferences.language);
  const { status, installed, dismissedVersion } = useUpdates();
  useEffect(() => { void checkForUpdates(true); }, []);
  if (suspended || installed || !status?.configured || status.error || !status.available || status.available === dismissedVersion) return null;
  return <UpdateDialog version={status.available} />;
}
