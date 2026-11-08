import { useEffect, useState } from "react";
import { copy } from "../copy";
import { useClient } from "../rpc/context";
import type { AgentView, AgentForm as Form, SaveResult } from "../rpc/types";
import { useAppStore } from "../state/store";
import { AgentForm, RestartNotice } from "./AgentForm";

const fill = (text: string, values: Record<string, string | number>) =>
  Object.entries(values).reduce((out, [key, value]) => out.replace(`{${key}}`, String(value)), text);

function describe(agent: AgentView): string {
  if (agent.kind !== "model") return copy.agents.kinds[agent.kind];
  return [agent.model, agent.provider].filter(Boolean).join(", ");
}

export function AgentsPage() {
  const client = useClient();
  const allAgents = useAppStore((state) => state.agents);
  const localAgents = allAgents.filter((name) => name.startsWith("local:"));
  const [agents, setAgents] = useState<AgentView[]>([]);
  const [summarizer, setSummarizer] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<Form | null>(null);
  const [restart, setRestart] = useState<SaveResult | null>(null);
  const [removing, setRemoving] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    const result = await client.call<{ agents: AgentView[]; summarizer: string | null }>("agents.configured", {});
    setAgents(result.agents); setSummarizer(result.summarizer);
  };
  useEffect(() => { refresh().catch((caught) => setError(caught.message)); }, [client]);
  const changed = (result: SaveResult) => { setAdding(false); setEditing(null); setRestart(result); void refresh().catch(() => undefined); };
  const summaryChoices = [...agents.filter((agent) => agent.kind === "model").map((agent) => agent.name), ...localAgents];

  return <section className="transfer-page agents-page">
    <h3>{copy.agents.tab}</h3>
    <p className="help">{copy.agents.intro}</p>
    {error ? <p role="alert">{error}</p> : null}
    {restart ? <RestartNotice runningTurns={restart.runningTurns} onLater={() => setRestart(null)} /> : null}

    {agents.length === 0 && !adding ? <p className="help">{copy.agents.empty}</p> : null}
    <ul className="agent-list">
      {agents.map((agent) => <li key={agent.name}>
        <div>
          <b>{agent.name}</b>
          <span className="help">{describe(agent)}</span>
          {agent.source === "file" ? <span className="tag" title={copy.agents.fromFileHelp}>{copy.agents.fromFile}</span> : null}
        </div>
        {agent.source === "app" ? removing === agent.name
          ? <div className="confirm-inline" role="alertdialog" aria-label={copy.agents.remove}>
              <span>{fill(copy.agents.removeConfirm, { name: agent.name })}</span>
              <button type="button" onClick={() => setRemoving(null)}>{copy.agents.cancel}</button>
              <button type="button" className="danger" onClick={() => {
                setRemoving(null);
                client.call<SaveResult>("agents.remove", { name: agent.name }).then(changed).catch((caught) => setError(caught.message));
              }}>{copy.agents.remove}</button>
            </div>
          : <div className="row"><button type="button" onClick={() => client.call<Form>("agents.form", { name: agent.name }).then(setEditing).catch((caught) => setError(caught.message))}>{copy.agents.edit}</button><button type="button" onClick={() => setRemoving(agent.name)}>{copy.agents.remove}</button></div> : null}
      </li>)}
    </ul>

    {editing ? <div className="agent-add"><AgentForm key={"name" in editing ? editing.name : "claude"} initial={editing} kinds={[editing.kind]} onSaved={changed} onCancel={() => setEditing(null)} /></div> : adding
      ? <div className="agent-add"><AgentForm onSaved={changed} onCancel={() => setAdding(false)} /></div>
      : <button type="button" className="primary" onClick={() => setAdding(true)}>{copy.agents.add}</button>}

    {summaryChoices.length ? <label className="setting">
      <span>{copy.agents.summarizer}</span>
      <select value={summarizer ?? ""} onChange={(event) => {
        const name = event.target.value || null;
        client.call<SaveResult>("agents.summarizer", { name }).then((result) => { setSummarizer(name); setRestart(result); }).catch((caught) => setError(caught.message));
      }}>
        <option value="">{copy.agents.summarizerNone}</option>
        {summaryChoices.map((name) => <option key={name} value={name}>{name}</option>)}
      </select>
    </label> : null}
  </section>;
}
