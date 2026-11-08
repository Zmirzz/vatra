import { useEffect, useState } from "react";
import { copy } from "../copy";
import { useClient } from "../rpc/context";
import type { DepPackage } from "../rpc/types";

const fill = (text: string, values: Record<string, string | number>) =>
  Object.entries(values).reduce((out, [key, value]) => out.replace(`{${key}}`, String(value)), text);

type Phase = "ask" | "installing" | "done" | "failed";

/** Asks before installing a program through winget, then shows how it went. Nothing starts before "Install". */
export function DependencyDialog({ id, onClose }: { id: string; onClose: (installed: boolean) => void }) {
  const client = useClient();
  const [item, setItem] = useState<DepPackage | null>(null);
  const [winget, setWinget] = useState(true);
  const [phase, setPhase] = useState<Phase>("ask");
  const [line, setLine] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    client.call<{ winget: boolean; packages: DepPackage[] }>("deps.status", {}).then((status) => {
      if (!live) return;
      setWinget(status.winget);
      setItem(status.packages.find((candidate) => candidate.id === id) ?? null);
    }).catch((caught) => { if (live) setError(caught.message); });
    const off = client.on((notice) => {
      if (notice.method === "deps_install" && notice.params.id === id && notice.params.state === "progress") setLine(notice.params.line);
    });
    return () => { live = false; off(); };
  }, [client, id]);

  const install = async () => {
    setPhase("installing"); setError(null);
    try { await client.call("deps.install", { id, consent: true }); setPhase("done"); }
    catch (caught) { setError((caught as Error).message); setPhase("failed"); }
  };
  const name = item?.name ?? id;

  return <div className="dialog-backdrop">
    <div role="dialog" aria-modal="true" aria-labelledby="dep-title" className="dialog dep-dialog">
      <h2 id="dep-title">{phase === "done" ? fill(copy.deps.done, { name }) : phase === "failed" ? fill(copy.deps.failed, { name }) : fill(copy.deps.title, { name })}</h2>
      {phase === "ask" ? <>
        {item ? <p>{copy.deps.purpose[item.purpose]}</p> : null}
        <p>{winget ? copy.deps.source : fill(copy.deps.noWinget, { name })}</p>
        {item ? <details><summary>{copy.deps.command}</summary><pre>{item.command}</pre></details> : null}
      </> : null}
      {phase === "installing" ? <p role="status"><span className="spinner" aria-hidden="true" /> {fill(copy.deps.installing, { name })}{line ? <small>{line}</small> : null}</p> : null}
      {error ? <pre role="alert">{error}</pre> : null}
      <div className="dialog-actions">
        {phase === "ask" ? <>
          <button type="button" onClick={() => onClose(false)}>{copy.agents.cancel}</button>
          <button type="button" className="primary" disabled={!winget || !item} onClick={() => void install()}>{copy.deps.install}</button>
        </> : phase === "installing" ? null : <button type="button" className="primary" onClick={() => onClose(phase === "done")}>{copy.deps.close}</button>}
      </div>
    </div>
  </div>;
}
