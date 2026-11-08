import {Scale} from "lucide-react";
import {agentLabel} from "../../conversation/labels";
import {useCallback, useEffect, useState, type FormEvent} from "react";
import {useHubCopy} from "../../hub/copy";
import type {CompareRun} from "../../hub/types";
import {useClient} from "../../rpc/context";
import {useAppStore} from "../../state/store";

export function ComparePage() {
  const t = useHubCopy();
  const rpc = useClient();
  const agents = useAppStore((s) => s.agents);
  const [prompt, setPrompt] = useState("");
  const [chosen, setChosen] = useState<string[]>([]);
  const [run, setRun] = useState<CompareRun | null>(null);
  const [history, setHistory] = useState<CompareRun[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadHistory = useCallback(async () => {
    try { setHistory(await rpc.call<CompareRun[]>("compare.list", {})); } catch { /* the page still works without the list */ }
  }, [rpc]);
  useEffect(() => { void loadHistory(); }, [loadHistory]);

  const toggle = (name: string) => setChosen((old) => (old.includes(name) ? old.filter((n) => n !== name) : old.length < 4 ? [...old, name] : old));
  const start = async (event: FormEvent) => {
    event.preventDefault();
    if (chosen.length < 2 || !prompt.trim()) return;
    setBusy(true); setError(null); setRun(null);
    try {
      const result = await rpc.call<{id: number; answers: CompareRun["answers"]}>("compare.start", {prompt: prompt.trim(), agents: chosen});
      setRun({id: result.id, prompt: prompt.trim(), answers: result.answers, vote: null, created_at: Math.floor(Date.now() / 1000)});
    } catch (e) { setError(e instanceof Error ? e.message : t.saveError); } finally { setBusy(false); }
  };
  const vote = async (choice: string) => {
    if (!run) return;
    try { setRun(await rpc.call<CompareRun>("compare.vote", {id: run.id, choice})); setError(null); await loadHistory(); } catch (e) { setError(e instanceof Error ? e.message : t.saveError); }
  };
  const revealed = run?.vote != null;

  return (
    <div className="hub-page compare-page">
      <header className="hub-head"><h1>{t.compareTitle}</h1></header>
      <form className="hub-form" onSubmit={(e) => void start(e)} aria-label={t.compareTitle}>
        <label>{t.comparePrompt}<textarea rows={4} maxLength={8000} value={prompt} onChange={(e) => setPrompt(e.target.value)} /></label>
        <fieldset className="hub-agents"><legend>{t.compareAgents}</legend>
          {agents.map((name) => (
            <label key={name} className="hub-choice"><input type="checkbox" checked={chosen.includes(name)} disabled={!chosen.includes(name) && chosen.length >= 4} onChange={() => toggle(name)} /><span>{agentLabel(name)}</span></label>
          ))}
        </fieldset>
        <div className="hub-actions"><button type="submit" className="primary" disabled={busy || chosen.length < 2 || !prompt.trim()}><Scale size={14} aria-hidden="true" />{busy ? t.working : t.runCompare}</button></div>
      </form>
      {error ? <p role="alert" className="hub-error">{error}</p> : null}
      {!run && history.length === 0 ? <p className="hub-meta">{t.compareEmpty}</p> : null}
      {run ? (
        <section aria-label={t.compareTitle}>
          <div className="compare-answers" data-count={run.answers.length}>
            {run.answers.map((answer) => (
              <article key={answer.label} className="compare-answer" data-picked={run.vote === answer.label}>
                <h3>{revealed && answer.agent ? `${t.answerLabel(answer.label)} · ${answer.agent}` : t.answerLabel(answer.label)}</h3>
                {answer.text ? <div className="compare-text">{answer.text}</div> : <p className="hub-error">{t.couldntAnswer}</p>}
              </article>
            ))}
          </div>
          {!revealed ? (
            <div className="compare-vote" role="group" aria-label={t.pickBest}>
              <b>{t.pickBest}</b>
              {run.answers.map((a) => <button key={a.label} type="button" onClick={() => void vote(a.label)}>{t.answerLabel(a.label)}</button>)}
              <button type="button" onClick={() => void vote("tie")}>{t.tie}</button>
              <button type="button" onClick={() => void vote("none")}>{t.noneGood}</button>
            </div>
          ) : <p role="status" className="hub-note">{t.revealed} {run.vote !== "tie" && run.vote !== "none" ? `${t.voted}: ${t.answerLabel(String(run.vote))}` : run.vote === "tie" ? t.tie : t.noneGood}</p>}
        </section>
      ) : null}
      {history.filter((h) => h.vote !== null && h.id !== run?.id).length ? (
        <section aria-label={t.history}>
          <h2 className="hub-sub">{t.history}</h2>
          <ul className="hub-list">
            {history.filter((h) => h.vote !== null && h.id !== run?.id).map((h) => (
              <li key={h.id} className="hub-card"><div className="hub-card-main"><b>{h.prompt}</b><span className="hub-meta">{h.answers.map((a) => `${a.label}: ${a.agent}${h.vote === a.label ? " ✓" : ""}`).join(" · ")}</span></div></li>
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
}
