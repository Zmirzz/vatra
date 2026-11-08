import { Folder, ShieldCheck } from "lucide-react";
import { useRef, useState } from "react";
import { copy } from "../copy";
import { useClient } from "../rpc/context";
import type { PermissionMode, Project, TurnOptions } from "../rpc/types";
import { openSession, refreshSessions, useAppStore } from "../state/store";
import { modeOptions } from "./Composer";
import { Mark } from "./Mark";
import { Menu } from "./Menu";
import { ModelPicker } from "./ModelPicker";
import { PromptBox } from "./PromptBox";
import { hasAcceptedSubscriptionNotice, SubscriptionNotice } from "./SubscriptionNotice";

function rememberProject(project: Project): void {
  useAppStore.setState((state) => ({
    projects: [...state.projects.filter((candidate) => candidate.id !== project.id), project],
  }));
}

function NewProjectDialog({ onCreated, onCancel }: { onCreated: (project: Project) => void; onCancel: () => void }) {
  const client = useClient();
  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const [error, setError] = useState<string | null>(null);

  const create = async () => {
    if (!name.trim()) {
      setError(copy.nameRequired);
      return;
    }
    try {
      const project = await client.call<Project>("projects.create", { name: name.trim(), path: path.trim() || null });
      rememberProject(project);
      onCreated(project);
    } catch (caught) {
      setError((caught as Error).message);
    }
  };

  return (
    <div className="dialog-backdrop">
      <div role="dialog" aria-modal="true" aria-labelledby="project-title" className="dialog">
        <h2 id="project-title">{copy.newProjectTitle}</h2>
        <label>
          {copy.projectName}
          <input type="text" autoFocus value={name} onChange={(event) => setName(event.target.value)} />
        </label>
        <label>
          {copy.projectPath}
          <input type="text" value={path} onChange={(event) => setPath(event.target.value)} />
        </label>
        <p className="help">{copy.projectPathHelp}</p>
        {error ? <div role="alert">{error}</div> : null}
        <div className="dialog-actions">
          <button type="button" className="quiet" onClick={onCancel}>
            {copy.cancel}
          </button>
          <button type="button" className="primary" onClick={create}>
            {copy.create}
          </button>
        </div>
      </div>
    </div>
  );
}

export function Home() {
  const client = useClient();
  const agents = useAppStore((state) => state.agents);
  const subscriptionAgents = useAppStore((state)=>state.subscriptionAgents);
  const agentsLoaded = useAppStore((state) => state.agentsLoaded);
  const preferredAgent = useAppStore((state) => state.preferredAgent);
  const projects = useAppStore((state) => state.projects);
  const [text, setText] = useState("");
  const [chosenProject, setChosenProject] = useState<number | null>(null);
  const [options, setOptions] = useState<TurnOptions>({});
  const [chosenAgent, setChosenAgent] = useState<string | null>(null);
  const [mode, setMode] = useState<PermissionMode>("supervised");
  const [error, setError] = useState<string | null>(null);
  const [noticeOpen, setNoticeOpen] = useState(false);
  const [projectDialogOpen, setProjectDialogOpen] = useState(false);
  const starting = useRef(false);
  // A session survives a failed first turn, so trying again continues it instead of adding another.
  const createdSession = useRef<number | null>(null);

  if (agents.length === 0 && !agentsLoaded) {
    return (
      <div className="home">
        <Mark size={36} />
        <p role="status" className="help">
          {copy.connecting}
        </p>
      </div>
    );
  }

  if (agents.length === 0) {
    return (
      <div className="home">
        <Mark size={36} />
        <h1>{copy.noAgentsTitle}</h1>
        <p className="help">{copy.noAgentsBody}</p>
        <button type="button" onClick={() => useAppStore.setState({ setupRequested: true })}>{copy.setUp}</button>
        <button type="button" className="primary" onClick={() => useAppStore.setState({ settingsPage: "models" })}>{copy.findLocalModel}</button>
      </div>
    );
  }

  const agent = chosenAgent ?? (preferredAgent && agents.includes(preferredAgent) ? preferredAgent : agents[0]);
  const project = chosenProject !== null ? (projects.find((candidate) => candidate.id === chosenProject) ?? null) : null;

  const start = async () => {
    if (starting.current) {
      return;
    }
    starting.current = true;
    setError(null);
    try {
      if (createdSession.current === null) {
        const projectId = project?.id ?? null;
        const { id } = await client.call<{ id: number }>("sessions.create", { project_id: projectId, agent });
        createdSession.current = id;
      }
      const id = createdSession.current;
      useAppStore.setState((state) => ({ modes: { ...state.modes, [id]: mode } }));
      await client.call("turn.start", { session_id: id, prompt: text, mode, ...options });
      useAppStore.setState((state) => ({ turnOptions: { ...state.turnOptions, [id]: options } }));
      createdSession.current = null;
      setText("");
      // The work has started: a refresh that fails now is only a stale list, and resync repairs it.
      await refreshSessions(client).catch(() => undefined);
      openSession(client, id);
    } catch (caught) {
      setError((caught as Error).message);
    } finally {
      starting.current = false;
    }
  };

  const submit = () => {
    if (!text.trim()) {
      return;
    }
    if (subscriptionAgents.includes(agent) && !hasAcceptedSubscriptionNotice(agent)) {
      setNoticeOpen(true);
    } else {
      void start();
    }
  };
  const attach=async(files:File[])=>{
    if(starting.current)return;
    starting.current=true;setError(null);
    try{
      let id=createdSession.current;
      if(id===null){id=(await client.call<{id:number}>("sessions.create",{project_id:project?.id??null,agent})).id;createdSession.current=id;}
      useAppStore.setState(state=>({drafts:{...state.drafts,[id]:text},modes:{...state.modes,[id]:mode},turnOptions:{...state.turnOptions,[id]:options}}));
      const {importFiles}=await import("../files/store");
      await importFiles(client,id,files);
      await refreshSessions(client);openSession(client,id);
    }catch(e){setError((e as Error).message);}finally{starting.current=false;}
  };

  return (
    <div className="home">
      <Mark size={36} />
      <h1>{copy.greeting}</h1>
      <div className="home-box">
        {error ? <div role="alert">{error}</div> : null}
        <PromptBox
          onFiles={files=>{void attach(files);}}
          value={text}
          onChange={setText}
          onSubmit={submit}
          placeholder={copy.homePlaceholder}
          projectId={project?.id ?? 0}
          rows={3}
          left={
            <>
              <Menu
                label={copy.project}
                value={project ? String(project.id) : "none"}
                options={[
                  { value: "none", label: copy.noProject },
                  ...projects.map((candidate) => ({ value: String(candidate.id), label: candidate.name })),
                ]}
                onSelect={(value) => setChosenProject(value === "none" ? null : Number(value))}
                icon={<Folder size={14} aria-hidden="true" />}
                actions={[{ label: copy.newProject, onSelect: () => setProjectDialogOpen(true) }]}
              />
              <Menu
                label={copy.permissions}
                value={mode}
                options={modeOptions()}
                onSelect={(value) => setMode(value as PermissionMode)}
                icon={<ShieldCheck size={14} aria-hidden="true" />}
              />
            </>
          }
          right={
            <ModelPicker value={agent} options={options} projectId={project?.id} onOptionsChange={setOptions} onSelect={(next, choice = {}) => {
              setChosenAgent(next); setOptions(choice); createdSession.current = null;
            }} />
          }
        />
      </div>
      {projectDialogOpen ? (
        <NewProjectDialog
          onCancel={() => setProjectDialogOpen(false)}
          onCreated={(created) => {
            setChosenProject(created.id);
            setProjectDialogOpen(false);
          }}
        />
      ) : null}
      {noticeOpen ? (
        <SubscriptionNotice
          agent={agent}
          onCancel={() => setNoticeOpen(false)}
          onAccept={() => {
            setNoticeOpen(false);
            void start();
          }}
        />
      ) : null}
    </div>
  );
}
