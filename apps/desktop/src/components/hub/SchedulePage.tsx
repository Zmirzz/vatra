import {CalendarClock, Play, Plus, Trash2} from "lucide-react";
import {agentLabel} from "../../conversation/labels";
import {useCallback, useEffect, useState, type FormEvent} from "react";
import {useHubCopy} from "../../hub/copy";
import {useHubFocus} from "../../hub/focus";
import {useMore} from "../../hub/moreCopy";
import {fromInput, formatWhen, toInputDate, toInputTime, tzOffsetMin} from "../../hub/time";
import type {Schedule, ScheduledRun, ScheduledTask} from "../../hub/types";
import {useRefresh} from "../../hub/useRefresh";
import {useClient} from "../../rpc/context";
import {usePreferences} from "../../settings/preferences";
import {useAppStore} from "../../state/store";
import {navigateWorkspace} from "../../workspace/store";

type Kind = Schedule["kind"];

function describe(schedule: Schedule, t: ReturnType<typeof useHubCopy>): string {
  const hh = (h: number, m: number) => `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}`;
  if (schedule.kind === "once") return t.once;
  if (schedule.kind === "interval") return `${t.every} · ${Math.round(schedule.every_secs / 60)} min`;
  if (schedule.kind === "daily") return `${t.daily} · ${hh(schedule.hour, schedule.minute)}`;
  return `${schedule.days.map((d) => t.weekdays[d]).join(", ")} · ${hh(schedule.hour, schedule.minute)}`;
}

export function SchedulePage() {
  const t = useHubCopy();
  const m = useMore();
  const rpc = useClient();
  const language = usePreferences((s) => s.preferences.language);
  const agents = useAppStore((s) => s.agents);
  const projects = useAppStore((s) => s.projects);
  const [tasks, setTasks] = useState<ScheduledTask[] | null>(null);
  const [runs, setRuns] = useState<ScheduledRun[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<ScheduledTask | "new" | null>(null);

  const load = useCallback(async () => {
    try {
      const data = await rpc.call<{tasks: ScheduledTask[]; runs: ScheduledRun[]}>("schedule.list", {});
      setTasks(data.tasks);
      setRuns(data.runs);
      setError(null);
    } catch {
      setError(t.loadError);
    }
  }, [rpc, t]);
  useEffect(() => { void load(); }, [load]);
  useRefresh(rpc, ["schedule_changed"], () => void load());

  const act = async (method: string, params: Record<string, unknown>) => {
    try { await rpc.call(method, params); setError(null); await load(); } catch (e) { setError(e instanceof Error ? e.message : t.saveError); }
  };
  const openRun = (run: ScheduledRun) => {
    if (run.session_id === null) return;
    void navigateWorkspace({projectId: null, area: "conversations", itemId: null}).then((ok) => {
      if (ok) useAppStore.setState({activeSessionId: run.session_id, importedSession: null, openingImportedId: null});
    });
  };
  const openReport = (id: number) => {
    useHubFocus.setState({researchId: id});
    void navigateWorkspace({projectId: null, area: "research", itemId: null});
  };
  const taskName = (id: number) => tasks?.find((x) => x.id === id)?.name ?? "";
  const stateLabel = {done: t.stateDone, failed: t.stateFailed, running: t.stateRunning, interrupted: t.stateInterrupted};

  return (
    <div className="hub-page">
      <header className="hub-head">
        <h1>{t.scheduleTitle}</h1>
        <button type="button" className="primary" onClick={() => setEditing("new")}><Plus size={15} aria-hidden="true" />{t.newTask}</button>
      </header>
      <p className="hub-note">{t.scheduleNote}</p>
      {error ? <p role="alert" className="hub-error">{error}</p> : null}
      {editing ? (
        <TaskForm key={editing === "new" ? "new" : editing.id} initial={editing === "new" ? null : editing} agents={agents} projects={projects}
          onCancel={() => setEditing(null)}
          onSave={async (body) => { await act("schedule.save", body); setEditing(null); }} />
      ) : null}
      {tasks === null && !error ? <p role="status">{t.loading}</p> : null}
      {tasks && tasks.length === 0 && !editing ? (
        <div className="hub-empty"><CalendarClock size={28} aria-hidden="true" /><h2>{t.scheduleEmpty}</h2><p>{t.scheduleEmptyHelp}</p></div>
      ) : null}
      <ul className="hub-list" aria-label={t.scheduleTitle}>
        {tasks?.map((task) => (
          <li key={task.id} className="hub-card" data-off={!task.enabled}>
            <div className="hub-card-main">
              <b>{task.name}</b>
              <span className="hub-meta">{describe(task.schedule, t)} · {task.kind === "research" ? `${m.researchTask} · ${task.options.depth ?? "standard"}` : task.agent}</span>
              <span className="hub-meta">{t.nextRun}: {task.next_run_at ? formatWhen(task.next_run_at, language) : t.never} · {t.lastRun}: {task.last_run_at ? formatWhen(task.last_run_at, language) : t.never}</span>
            </div>
            <div className="hub-actions">
              <button type="button" onClick={() => void act("schedule.run_now", {id: task.id})}><Play size={14} aria-hidden="true" />{t.runNow}</button>
              <button type="button" onClick={() => setEditing(task)}>{t.edit}</button>
              <button type="button" aria-pressed={task.enabled} onClick={() => void act("schedule.save", {id: task.id, name: task.name, prompt: task.prompt, agent: task.agent, project_id: task.project_id, schedule: task.schedule, kind: task.kind, options: task.options, enabled: !task.enabled})}>{task.enabled ? t.enable : t.disable}</button>
              <button type="button" className="icon" aria-label={`${t.delete}: ${task.name}`} onClick={() => void act("schedule.delete", {id: task.id})}><Trash2 size={14} aria-hidden="true" /></button>
            </div>
          </li>
        ))}
      </ul>
      {runs.length > 0 ? (
        <section aria-label={t.runs}>
          <h2 className="hub-sub">{t.runs}</h2>
          <ul className="hub-list">
            {runs.slice(0, 20).map((run) => (
              <li key={run.id} className="hub-card" data-state={run.state}>
                <div className="hub-card-main">
                  <b>{taskName(run.task_id)} <span className="hub-badge" data-state={run.state}>{stateLabel[run.state]}</span></b>
                  <span className="hub-meta">{formatWhen(run.started_at, language)}</span>
                  {run.summary ? <p className="hub-summary">{run.summary}</p> : null}
                </div>
                {run.research_id !== null ? <div className="hub-actions"><button type="button" onClick={() => openReport(run.research_id as number)}>{m.openReport}</button></div> : null}
                {run.session_id !== null ? <div className="hub-actions"><button type="button" onClick={() => openRun(run)}>{t.openConversation}</button></div> : null}
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
}

function TaskForm({initial, agents, projects, onCancel, onSave}: {
  initial: ScheduledTask | null; agents: string[]; projects: {id: number; name: string}[];
  onCancel: () => void; onSave: (body: Record<string, unknown>) => Promise<void>;
}) {
  const t = useHubCopy();
  const m = useMore();
  const first = initial?.schedule;
  const soon = Math.floor(Date.now() / 1000) + 3600;
  const [name, setName] = useState(initial?.name ?? "");
  const [prompt, setPrompt] = useState(initial?.prompt ?? "");
  const [agent, setAgent] = useState(initial?.agent ?? agents[0] ?? "");
  const [type, setType] = useState<"agent" | "research">(initial?.kind ?? "agent");
  const [depth, setDepth] = useState(initial?.options.depth ?? "standard");
  const [light, setLight] = useState(initial?.options.small_model ?? false);
  const [project, setProject] = useState<string>(initial?.project_id ? String(initial.project_id) : "");
  const [kind, setKind] = useState<Kind>(first?.kind ?? "daily");
  const [date, setDate] = useState(first?.kind === "once" ? toInputDate(first.at) : toInputDate(soon));
  const [time, setTime] = useState(first?.kind === "once" ? toInputTime(first.at) : first && "hour" in first ? `${String(first.hour).padStart(2, "0")}:${String(first.minute).padStart(2, "0")}` : "09:00");
  const [minutes, setMinutes] = useState(first?.kind === "interval" ? Math.round(first.every_secs / 60) : 60);
  const [days, setDays] = useState<number[]>(first?.kind === "weekly" ? first.days : [1, 2, 3, 4, 5]);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const build = (): Schedule | null => {
    const [h, m] = time.split(":").map(Number);
    if (kind === "once") { const at = fromInput(date, time); return Number.isFinite(at) ? {kind, at} : null; }
    if (kind === "interval") return minutes >= 1 ? {kind, every_secs: Math.round(minutes) * 60} : null;
    if (!Number.isFinite(h) || !Number.isFinite(m)) return null;
    if (kind === "daily") return {kind, hour: h, minute: m, tz_offset_min: tzOffsetMin()};
    return days.length ? {kind, days, hour: h, minute: m, tz_offset_min: tzOffsetMin()} : null;
  };
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const schedule = build();
    if (!schedule || !name.trim() || !prompt.trim() || (type === "agent" && !agent)) { setProblem(t.saveError); return; }
    setBusy(true);
    try {
      await onSave({
        id: initial?.id, name: name.trim(), prompt: prompt.trim(), agent: type === "research" ? agent : agent, project_id: project ? Number(project) : null, schedule, enabled: initial?.enabled ?? true,
        kind: type, options: type === "research" ? {depth, small_model: light} : {},
      });
    } catch (e) { setProblem(e instanceof Error ? e.message : t.saveError); } finally { setBusy(false); }
  };

  return (
    <form className="hub-form" onSubmit={(e) => void submit(e)} aria-label={t.newTask}>
      <div className="hub-tabs" role="radiogroup" aria-label={m.taskType}>
        <button type="button" role="radio" aria-checked={type === "agent"} aria-pressed={type === "agent"} onClick={() => setType("agent")}>{m.typeAgent}</button>
        <button type="button" role="radio" aria-checked={type === "research"} aria-pressed={type === "research"} onClick={() => setType("research")}>{m.typeResearch}</button>
      </div>
      <label>{t.taskName}<input value={name} maxLength={120} onChange={(e) => setName(e.target.value)} required /></label>
      <label>{type === "research" ? m.researchQuestion : t.taskPrompt}<textarea value={prompt} rows={type === "research" ? 3 : 4} onChange={(e) => setPrompt(e.target.value)} required /></label>
      {type === "research" ? (
        <div className="hub-row">
          <label>{m.researchDepth}<select value={depth} onChange={(e) => setDepth(e.target.value)}><option value="quick">{t.quick}</option><option value="standard">{t.standard}</option><option value="deep">{t.deep}</option></select></label>
          <label className="hub-check"><input type="checkbox" checked={light} onChange={(e) => setLight(e.target.checked)} />{m.lightMode}</label>
        </div>
      ) : null}
      <div className="hub-row">
        <label>{t.taskAgent}<select value={agent} onChange={(e) => setAgent(e.target.value)}>{type === "research" ? <option value="">{m.agentAuto}</option> : null}{agents.map((a) => <option key={a} value={a}>{agentLabel(a)}</option>)}</select></label>
        <label>{t.taskProject}<select value={project} onChange={(e) => setProject(e.target.value)}><option value="">{t.noProject}</option>{projects.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}</select></label>
      </div>
      <label>{t.repeat}
        <select value={kind} onChange={(e) => setKind(e.target.value as Kind)}>
          <option value="once">{t.once}</option><option value="interval">{t.every}</option><option value="daily">{t.daily}</option><option value="weekly">{t.weekly}</option>
        </select>
      </label>
      {kind === "once" ? <div className="hub-row"><label>{t.date}<input type="date" value={date} onChange={(e) => setDate(e.target.value)} /></label><label>{t.time}<input type="time" value={time} onChange={(e) => setTime(e.target.value)} /></label></div> : null}
      {kind === "interval" ? <label>{t.minutes}<input type="number" min={1} max={527040} value={minutes} onChange={(e) => setMinutes(Number(e.target.value))} /></label> : null}
      {kind === "daily" || kind === "weekly" ? <label>{t.time}<input type="time" value={time} onChange={(e) => setTime(e.target.value)} /></label> : null}
      {kind === "weekly" ? (
        <fieldset className="hub-days"><legend>{t.days}</legend>
          {t.weekdays.map((label, index) => (
            <button type="button" key={label} aria-pressed={days.includes(index)} onClick={() => setDays((old) => old.includes(index) ? old.filter((d) => d !== index) : [...old, index].sort())}>{label}</button>
          ))}
        </fieldset>
      ) : null}
      {problem ? <p role="alert" className="hub-error">{problem}</p> : null}
      <div className="hub-actions">
        <button type="button" onClick={onCancel}>{t.cancel}</button>
        <button type="submit" className="primary" disabled={busy}>{t.save}</button>
      </div>
    </form>
  );
}
