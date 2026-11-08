import {Copy, Download, FileText, MessageSquare, Search, Square, Trash2} from "lucide-react";
import {agentLabel} from "../../conversation/labels";
import {useCallback, useEffect, useState, type FormEvent} from "react";
import {useHubCopy} from "../../hub/copy";
import {useMore} from "../../hub/moreCopy";
import {formatWhen} from "../../hub/time";
import {toast} from "../../hub/toast";
import type {ResearchProfile, ResearchRun, ResearchSummary} from "../../hub/types";
import {useRefresh} from "../../hub/useRefresh";
import {useClient} from "../../rpc/context";
import {usePreferences} from "../../settings/preferences";
import {refreshSessions, useAppStore} from "../../state/store";
import {navigateWorkspace} from "../../workspace/store";

const WORKING = ["planning", "searching", "reading", "writing"];
type Panel = null | "export" | "save" | "discuss";

/** The report as paragraphs, with each [n] turned into a link to the source below. */
function ReportText({text, sources}: {text: string; sources: ResearchRun["sources"]}) {
  const body = text.split("\n\nSources\n")[0];
  return (
    <div className="research-report">
      {body.split(/\n{2,}/).map((paragraph, i) => (
        <p key={i}>
          {paragraph.split(/(\[\d+\])/g).map((part, j) => {
            const match = /^\[(\d+)\]$/.exec(part);
            const source = match ? sources.find((s) => s.n === Number(match[1])) : undefined;
            return source ? <a key={j} href={`#source-${source.n}`} className="research-cite" title={source.title}>{part}</a> : <span key={j}>{part}</span>;
          })}
        </p>
      ))}
    </div>
  );
}

function download(filename: string, text: string, mime: string) {
  const url = URL.createObjectURL(new Blob([text], {type: `${mime};charset=utf-8`}));
  const link = document.createElement("a");
  link.href = url; link.download = filename; link.click();
  URL.revokeObjectURL(url);
}

export function ResearchPage({onOpenSettings, focus}: {onOpenSettings: () => void; focus?: number | null}) {
  const t = useHubCopy();
  const m = useMore();
  const rpc = useClient();
  const language = usePreferences((s) => s.preferences.language);
  const agents = useAppStore((s) => s.agents);
  const projects = useAppStore((s) => s.projects);
  const [ready, setReady] = useState<{ready: boolean; providers: string[]} | null>(null);
  const [profile, setProfile] = useState<ResearchProfile | null>(null);
  const [runs, setRuns] = useState<ResearchSummary[] | null>(null);
  const [current, setCurrent] = useState<ResearchRun | null>(null);
  const [question, setQuestion] = useState("");
  const [depth, setDepth] = useState("standard");
  const [agent, setAgent] = useState("");
  const [light, setLight] = useState<boolean | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const [clarify, setClarify] = useState<{questions: string[]; answers: string[]} | null>(null);
  const [followup, setFollowup] = useState("");
  const [asking, setAsking] = useState(false);
  const [panel, setPanel] = useState<Panel>(null);
  const [project, setProject] = useState("");
  const [discussAgent, setDiscussAgent] = useState("");

  const lightOn = light ?? profile?.small_model ?? false;

  const loadList = useCallback(async () => {
    try { setRuns(await rpc.call<ResearchSummary[]>("research.list", {})); } catch { setError(t.loadError); setRuns([]); }
  }, [rpc, t]);
  const loadCurrent = useCallback(async (id: number) => {
    try { setCurrent(await rpc.call<ResearchRun>("research.get", {id})); setPanel(null); } catch { setCurrent(null); }
  }, [rpc]);
  useEffect(() => {
    void loadList();
    void rpc.call<{ready: boolean; providers: string[]}>("research.ready", {}).then(setReady).catch(() => setReady({ready: false, providers: []}));
    void rpc.call<ResearchProfile>("research.profile", {}).then(setProfile).catch(() => undefined);
  }, [rpc, loadList]);
  useEffect(() => { if (focus) void loadCurrent(focus); }, [focus, loadCurrent]);
  useRefresh(rpc, ["research_changed"], () => { void loadList(); if (current) void loadCurrent(current.id); });

  const begin = async (context: string) => {
    try {
      const result = await rpc.call<{id: number}>("research.start", {question: question.trim(), depth, agent: agent || undefined, small_model: lightOn, context});
      setQuestion(""); setClarify(null); setError(null);
      await loadList(); await loadCurrent(result.id);
    } catch (e) { setError(e instanceof Error ? e.message : t.saveError); }
  };
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!question.trim() || checking) return;
    setChecking(true); setError(null);
    try {
      const {questions} = await rpc.call<{questions: string[]}>("research.clarify", {question: question.trim(), agent: agent || undefined});
      if (questions.length) setClarify({questions, answers: questions.map(() => "")});
      else await begin("");
    } catch { await begin(""); } finally { setChecking(false); }
  };
  const startWithAnswers = () => {
    if (!clarify) return;
    const context = clarify.questions.map((q, i) => (clarify.answers[i].trim() ? `${q} ${clarify.answers[i].trim()}` : "")).filter(Boolean).join("\n");
    void begin(context);
  };
  const ask = async (event: FormEvent) => {
    event.preventDefault();
    if (!current || !followup.trim()) return;
    setAsking(true);
    try { await rpc.call("research.followup", {id: current.id, question: followup.trim(), agent: agent || undefined}); setFollowup(""); await loadCurrent(current.id); setError(null); } catch (e) { setError(e instanceof Error ? e.message : t.saveError); } finally { setAsking(false); }
  };

  const exportAs = async (format: "markdown" | "html") => {
    if (!current) return;
    try {
      const file = await rpc.call<{text: string; filename: string; mime: string}>("research.export", {id: current.id, format});
      download(file.filename, file.text, file.mime); toast(m.exported); setPanel(null);
    } catch (e) { toast(e instanceof Error ? e.message : t.saveError, "error"); }
  };
  const copyReport = async () => {
    if (!current?.report) return;
    try { await navigator.clipboard.writeText(current.report); toast(m.copied); } catch { toast(t.saveError, "error"); }
  };
  const saveDocument = async () => {
    if (!current || !project) return;
    try { await rpc.call("research.save_document", {id: current.id, project_id: Number(project)}); toast(m.savedDocument); setPanel(null); } catch (e) { toast(e instanceof Error ? e.message : t.saveError, "error"); }
  };
  const discuss = async () => {
    const chosen = discussAgent || agents[0];
    if (!current || !chosen) return;
    try {
      const {session_id} = await rpc.call<{session_id: number}>("research.discuss", {id: current.id, agent: chosen});
      await refreshSessions(rpc);
      const ok = await navigateWorkspace({projectId: null, area: "conversations", itemId: null});
      if (ok) useAppStore.setState({activeSessionId: session_id, importedSession: null, openingImportedId: null});
      toast(m.startedConversation);
    } catch (e) { toast(e instanceof Error ? e.message : t.saveError, "error"); }
  };

  const working = current ? WORKING.includes(current.state) : false;
  const stateText = (state: string) => (WORKING.includes(state) ? t.stateWorking : state === "done" ? t.stateDone : state === "cancelled" ? t.cancel : t.stateFailed);
  const sourceText = {read: t.sourceRead, "not relevant": t.sourceSkipped, unreadable: t.sourceUnreadable};
  const stats = current?.stats ?? {};
  const rounds = stats.rounds ?? 0;

  return (
    <div className={`hub-page research-layout${runs && runs.length === 0 ? " is-empty" : ""}`}>
      <aside className="research-history" aria-label={t.researchTitle}>
        <h1>{t.researchTitle}</h1>
        {runs && runs.length === 0 ? <p className="hub-meta">{t.researchEmpty}</p> : null}
        <ul>
          {runs?.map((r) => (
            <li key={r.id}>
              <button type="button" className="research-item" aria-current={current?.id === r.id ? "true" : undefined} onClick={() => void loadCurrent(r.id)}>
                <span>{r.question}</span><small>{stateText(r.state)} · {formatWhen(r.created_at, language)}</small>
              </button>
            </li>
          ))}
        </ul>
      </aside>
      <section className="research-main">
        {ready && !ready.ready ? <div className="hub-note"><p>{m.noSearch}</p><button type="button" onClick={onOpenSettings}>{t.openSettings}</button></div> : null}
        <form className="hub-form" onSubmit={(e) => void submit(e)} aria-label={t.startResearch}>
          <label>{t.researchQuestion}<textarea rows={3} maxLength={2000} value={question} onChange={(e) => { setQuestion(e.target.value); setClarify(null); }} /></label>
          <div className="hub-row">
            <fieldset className="hub-depth"><legend>{t.depth}</legend>
              {([["quick", t.quick, t.quickHelp], ["standard", t.standard, t.standardHelp], ["deep", t.deep, t.deepHelp]] as const).map(([id, label, help]) => (
                <label key={id} className="hub-choice"><input type="radio" name="depth" checked={depth === id} onChange={() => setDepth(id)} /><span>{label}<small>{help}</small></span></label>
              ))}
            </fieldset>
            <label>{t.taskAgent}<select value={agent} onChange={(e) => setAgent(e.target.value)}><option value="">{m.agentAuto}</option>{agents.map((a) => <option key={a} value={a}>{agentLabel(a)}</option>)}</select></label>
          </div>
          <label className="hub-check research-light">
            <input type="checkbox" checked={lightOn} onChange={(e) => setLight(e.target.checked)} />
            <span>{m.lightMode}<small>{profile?.small_model && light === null ? m.lightAuto(profile.reason) : m.lightHint}</small></span>
          </label>
          {ready?.ready ? <small className="hub-meta">{m.searchingWith((ready.providers ?? []).map((p) => (m.providers[p] ?? (p === "duckduckgo" ? "DuckDuckGo" : p))).join(", "))}</small> : null}
          {clarify ? (
            <fieldset className="research-clarify">
              <legend>{m.clarifyTitle}</legend>
              <p className="hub-meta">{m.clarifyHelp}</p>
              {clarify.questions.map((q, i) => (
                <label key={q}>{q}
                  <input value={clarify.answers[i]} maxLength={300} onChange={(e) => setClarify({...clarify, answers: clarify.answers.map((a, j) => (j === i ? e.target.value : a))})} />
                </label>
              ))}
              <div className="hub-actions">
                <button type="button" onClick={() => void begin("")}>{m.clarifySkip}</button>
                <button type="button" className="primary" onClick={startWithAnswers}>{m.clarifyStart}</button>
              </div>
            </fieldset>
          ) : (
            <div className="hub-actions"><button type="submit" className="primary" disabled={!question.trim() || ready?.ready === false || checking}><Search size={14} aria-hidden="true" />{checking ? m.clarifying : t.startResearch}</button></div>
          )}
        </form>
        {error ? <p role="alert" className="hub-error">{error}</p> : null}
        {!current && runs && runs.length === 0 ? <div className="hub-empty"><Search size={28} aria-hidden="true" /><h2>{t.researchEmpty}</h2><p>{t.researchEmptyHelp}</p></div> : null}
        {current ? (
          <article className="research-run" aria-label={current.question}>
            <header>
              <h2>{current.question}</h2>
              <div className="hub-actions">
                {working ? <button type="button" onClick={() => void rpc.call("research.cancel", {id: current.id})}><Square size={13} aria-hidden="true" />{t.stop}</button> : null}
                <button type="button" className="icon" aria-label={`${t.delete}: ${current.question}`} onClick={() => void rpc.call("research.delete", {id: current.id}).then(() => { setCurrent(null); return loadList(); })}><Trash2 size={14} aria-hidden="true" /></button>
              </div>
            </header>
            {working ? <p role="status" className="research-step"><span className="research-pulse" aria-hidden="true" />{current.step || t.working}</p> : null}
            {current.state === "failed" || current.state === "cancelled" ? <p role="alert" className="hub-error">{current.error}</p> : null}
            {rounds > 0 ? (
              <ul className="research-stats" aria-label={m.statsTitle}>
                <li>{m.statRounds(rounds)}</li>
                {stats.sources ? <li>{m.statRead(stats.sources.read)}</li> : null}
                {stats.hosts ? <li>{m.statSites(stats.hosts)}</li> : null}
                {Object.entries(stats.kinds ?? {}).filter(([k, n]) => k !== "web" && n > 0).map(([k, n]) => <li key={k}>{n} {m.kinds[k] ?? k}</li>)}
                {stats.small_model ? <li>{m.statLight}</li> : null}
              </ul>
            ) : null}
            {stats.partial ? <p className="hub-error research-warning"><b>{m.partialTitle}.</b> {m.partialHelp}</p> : null}
            {current.plan.length ? <details><summary>{t.plan}</summary><ul>{current.plan.map((q) => <li key={q}>{q}</li>)}</ul></details> : null}
            {stats.search_errors?.length ? <details><summary>{m.whySearchFailed}</summary><ul>{stats.search_errors.map((e) => <li key={e}>{e}</li>)}</ul></details> : null}
            {current.report ? (
              <section aria-label={t.report}>
                <div className="research-toolbar">
                  <h3>{t.report}</h3>
                  <div className="hub-actions">
                    <button type="button" onClick={() => void copyReport()}><Copy size={13} aria-hidden="true" />{m.copy}</button>
                    <button type="button" aria-expanded={panel === "export"} onClick={() => setPanel(panel === "export" ? null : "export")}><Download size={13} aria-hidden="true" />{m.export}</button>
                    <button type="button" aria-expanded={panel === "save"} onClick={() => { setPanel(panel === "save" ? null : "save"); if (!project && projects[0]) setProject(String(projects[0].id)); }}><FileText size={13} aria-hidden="true" />{m.saveDocument}</button>
                    <button type="button" aria-expanded={panel === "discuss"} onClick={() => setPanel(panel === "discuss" ? null : "discuss")}><MessageSquare size={13} aria-hidden="true" />{m.discuss}</button>
                  </div>
                </div>
                {panel === "export" ? <div className="research-panel"><button type="button" onClick={() => void exportAs("markdown")}>{m.exportMarkdown}</button><button type="button" onClick={() => void exportAs("html")}>{m.exportHtml}</button></div> : null}
                {panel === "save" ? (
                  <div className="research-panel">
                    <label>{t.project}<select value={project} onChange={(e) => setProject(e.target.value)}>{projects.length === 0 ? <option value="">{m.chooseProject}</option> : null}{projects.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}</select></label>
                    <button type="button" className="primary" disabled={!project} onClick={() => void saveDocument()}>{t.save}</button>
                  </div>
                ) : null}
                {panel === "discuss" ? (
                  <div className="research-panel">
                    <label>{m.discussWith}<select value={discussAgent || agents[0] || ""} onChange={(e) => setDiscussAgent(e.target.value)}>{agents.map((a) => <option key={a} value={a}>{agentLabel(a)}</option>)}</select></label>
                    <button type="button" className="primary" disabled={agents.length === 0} onClick={() => void discuss()}>{m.discuss}</button>
                  </div>
                ) : null}
                {current.error && current.state === "done" && !stats.partial ? <p className="hub-error research-warning">{t.unverified}: {current.error}</p> : null}
                <ReportText text={current.report} sources={current.sources} />
              </section>
            ) : null}
            {current.sources.length ? (
              <section aria-label={t.sources}>
                <h3>{t.sources}</h3>
                <ol className="research-sources">
                  {current.sources.map((s) => (
                    <li key={s.n} id={`source-${s.n}`} data-status={s.status}>
                      <a href={s.url} target="_blank" rel="noreferrer noopener">{s.title || s.url}</a>
                      <small>{sourceText[s.status]}{s.kind && s.kind !== "web" ? ` · ${m.kinds[s.kind]}` : ""}{s.round ? ` · ${m.roundN(s.round)}` : ""}</small>
                      {s.status === "unreadable" && s.reason ? <small className="research-reason">{s.reason}</small> : null}
                    </li>
                  ))}
                </ol>
              </section>
            ) : null}
            {current.state === "done" ? (
              <section>
                <h3>{t.followUp}</h3>
                <p className="hub-meta">{t.followUpHelp}</p>
                {current.followups.map((f, i) => <div key={i} className="research-qa"><b>{f.question}</b><p>{f.answer}</p></div>)}
                <form className="hub-row" onSubmit={(e) => void ask(e)}>
                  <input aria-label={t.followUp} value={followup} maxLength={2000} onChange={(e) => setFollowup(e.target.value)} />
                  <button type="submit" disabled={asking || !followup.trim()}>{asking ? t.working : t.ask}</button>
                </form>
              </section>
            ) : null}
          </article>
        ) : null}
      </section>
    </div>
  );
}
