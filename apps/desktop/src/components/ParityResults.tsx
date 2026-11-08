import { useEffect, useState } from "react";
import { copy } from "../copy";
import { useClient } from "../rpc/context";
import type { ParityRun } from "../rpc/types";

const fill = (text: string, values: Record<string, string | number>) => Object.entries(values).reduce((out, [key, value]) => out.replace(`{${key}}`, String(value)), text);
export function ParityResults({ agent, engine = true }: { agent: string; engine?: boolean }) {
  const client = useClient();
  const [runs, setRuns] = useState<ParityRun[]>([]);
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [matrix, setMatrix] = useState<string | null>(null);
  const [native, setNative] = useState<"claude" | "codex">("claude");
  const [nativeModel, setNativeModel] = useState("");
  const [confirmNative, setConfirmNative] = useState(false);
  useEffect(() => {
    let live = true;
    client.call<ParityRun[]>("parity.results", { agent }).then((values) => { if (live) setRuns(values); }).catch((caught) => { if (live) setError(caught.message); });
    const off = client.on((notice) => { if (notice.method === "parity_progress" && notice.params.agent === agent) setProgress(fill(copy.parity.progress, notice.params)); });
    return () => { live = false; off(); };
  }, [client, agent]);
  const start = async () => {
    setConfirm(false); setBusy(true); setError(null); setProgress(null);
    try {
      const result = await client.call<ParityRun>("parity.run", { agent, consentCost: !agent.startsWith("local:") });
      if (result?.results) setRuns((old) => [result, ...old]);
    } catch (caught) { setError((caught as Error).message); }
    finally { setBusy(false); setProgress(null); }
  };
  const latest = runs.filter((run) => run.agent === agent).sort((a, b) => b.startedAt - a.startedAt)[0];
  const startNative = async () => {
    setConfirmNative(false); setBusy(true); setError(null);
    try { const result = await client.call<ParityRun>("parity.native", { kind: native, model: nativeModel.trim(), consentCost: true }); if (result?.results) { setRuns((old) => [result, ...old]); setMatrix(await client.call<string>("parity.export", {})); } }
    catch (caught) { setError((caught as Error).message); }
    finally { setBusy(false); setProgress(null); }
  };
  return <section className="parity-results" aria-label={copy.parity.title}>
    <h5>{copy.parity.title}</h5>
    <p className="help">{copy.parity.limits}</p>
    {latest ? <>
      <p>{fill(copy.parity.result, { passed: latest.results.filter((item) => item.outcome.passed).length, total: latest.results.length, model: latest.model, date: new Date(latest.startedAt * 1000).toLocaleString() })}</p>
      <details><summary>{copy.parity.detail}</summary>
        <p>{latest.build}</p>
        <ul>{latest.results.map((item) => <li key={item.taskId}>{item.taskId}: {item.outcome.passed ? "✓" : "✗"}{item.outcome.failures.map((failure) => <p key={failure}>{failure}</p>)}</li>)}</ul>
      </details>
    </> : <p className="help">{copy.parity.none}</p>}
    {error ? <p role="alert">{error}</p> : null}
    {busy ? <p role="status">{progress ?? copy.parity.run}</p> : null}
    {engine ? <button type="button" disabled={busy} onClick={() => agent.startsWith("local:") ? void start() : setConfirm(true)}>{copy.parity.run}</button> : <p className="help">{copy.taskAvailability.engineOnly}</p>}
    {latest ? <button type="button" disabled={busy} onClick={() => { client.call<string>("parity.export", {}).then(setMatrix).catch((caught) => setError(caught.message)); }}>{copy.parity.export}</button> : null}
    {matrix ? <pre className="parity-export" tabIndex={0}>{matrix}</pre> : null}
    <details><summary>{copy.nativeParity.title}</summary>
      <p className="help">{copy.nativeParity.help}</p>
      <label>{copy.agents.program}<select value={native} onChange={(event) => setNative(event.target.value as "claude" | "codex")}><option value="claude">Claude Code</option><option value="codex" disabled>Codex</option></select></label>
      <p className="help">{copy.taskAvailability.codexNative}</p>
      <label>{copy.agents.model}<input value={nativeModel} onChange={(event) => setNativeModel(event.target.value)} placeholder={latest?.model} /></label>
      <button type="button" disabled={busy || !nativeModel.trim()} onClick={() => setConfirmNative(true)}>{copy.nativeParity.run}</button>
    </details>
    {confirmNative ? <div role="alertdialog" aria-label={copy.parity.cost} className="confirm">
      <b>{copy.parity.cost}</b><p>{fill(copy.parity.costBody, { agent: `${native} (${nativeModel.trim()})` })}</p>
      <button type="button" onClick={() => void startNative()}>{copy.models.confirm}</button>
      <button type="button" onClick={() => setConfirmNative(false)}>{copy.models.cancel}</button>
    </div> : null}
    {confirm ? <div role="alertdialog" aria-label={copy.parity.cost} className="confirm">
      <b>{copy.parity.cost}</b><p>{fill(copy.parity.costBody, { agent })}</p>
      <button type="button" onClick={() => void start()}>{copy.models.confirm}</button>
      <button type="button" onClick={() => setConfirm(false)}>{copy.models.cancel}</button>
    </div> : null}
  </section>;
}
