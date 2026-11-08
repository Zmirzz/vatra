import { ChevronRight, CircleAlert, Command, Moon, PanelLeft, Search, Settings2, SquarePen, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { answerApproval } from "../conversation/answer";
import { agentLabel, approvalHeading, commandFitsInline, exactCommand, projectTint } from "../conversation/labels";
import { copy } from "../copy";
import { useClient } from "../rpc/context";
import type { ApprovalRequest, HistoryHit, ImportedConversation, Session } from "../rpc/types";
import { displayCombo } from "../settings/keybindings";
import { toggleMode, usePreferences } from "../settings/preferences";
import { openSession, refreshSessions, sessionGroups, useAppStore } from "../state/store";
import { ProjectRemover } from "./ProjectRemover";
import { useMore } from "../hub/moreCopy";
import { Mark } from "./Mark";
import {WorkspaceNav} from "./workspace/WorkspaceNav";
import {useWorkspace,navigateWorkspace} from "../workspace/store";

const normalize = (text: string) =>
  text
    .normalize("NFD")
    .replace(/\p{Diacritic}/gu, "")
    .toLowerCase();

function NeedsYou({ session, requests }: { session: Session; requests: ApprovalRequest[] }) {
  const client = useClient();
  const [error, setError] = useState<string | null>(null);
  const request = requests[0];
  const command = exactCommand(request.arguments);

  const answer = async (allow: boolean) => {
    setError(null);
    try {
      await answerApproval(client, request.id, allow);
    } catch (caught) {
      setError((caught as Error).message);
    }
  };

  return (
    <div className="needs">
      <button type="button" className="needs-title" onClick={() => openSession(client, session.id)}>
        {session.title}
      </button>
      <small>{approvalHeading(session.agent, request.tool)}</small>
      {commandFitsInline(request.tool, request.arguments) ? (
        <>
          <code>{command}</code>
          <div className="approval-actions">
            <button type="button" className="approve" onClick={() => answer(true)}>
              {copy.approvals.allow}
            </button>
            <button type="button" className="refuse" onClick={() => answer(false)}>
              {copy.approvals.refuse}
            </button>
          </div>
        </>
      ) : (
        <div className="approval-actions">
          <button type="button" className="approve" onClick={() => openSession(client, session.id)}>
            {copy.approvals.review}
          </button>
        </div>
      )}
      {requests.length > 1 ? <small className="needs-more">{copy.approvals.more(requests.length - 1)}</small> : null}
      {error ? <div role="alert">{error}</div> : null}
    </div>
  );
}

function SessionLine({ session, live = false }: { session: Session; live?: boolean }) {
  const client = useClient();
  const active = useAppStore((state) => state.activeSessionId === session.id);
  const [asking, setAsking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const remove = async () => {
    setError(null);
    try {
      await client.call("sessions.delete", { session_id: session.id });
      if (useAppStore.getState().activeSessionId === session.id) useAppStore.setState({ activeSessionId: null });
      await refreshSessions(client);
    } catch (caught) { setError(`${copy.chat.failed} ${(caught as Error).message}`); setAsking(false); }
  };
  return (
    <div className="line-row">
      <button type="button" className="line" aria-current={active} title={`${session.title}
${agentLabel(session.agent)}`} onClick={() => openSession(client, session.id)}>
        {live ? <Mark size={13} stitching /> : null}
        <span className="line-name">{session.title}</span>
        {session.last_turn === "failed" ? (
          <span className="line-failed" role="img" aria-label={copy.turnFailed} title={copy.turnFailed}>
            <CircleAlert size={13} aria-hidden="true" />
          </span>
        ) : null}
      </button>
      {!live ? (
        <button type="button" className="line-delete" aria-label={copy.chat.delete} title={copy.chat.delete} onClick={() => setAsking(true)}>
          <Trash2 size={13} aria-hidden="true" />
        </button>
      ) : null}
      {asking ? (
        <div className="line-confirm" role="alertdialog" aria-label={copy.chat.delete}>
          <span>{copy.chat.confirm}</span>
          <div className="row">
            <button type="button" onClick={() => setAsking(false)}>{copy.chat.keep}</button>
            <button type="button" className="danger" onClick={() => void remove()}>{copy.chat.deleteNow}</button>
          </div>
        </div>
      ) : null}
      {error ? <div role="alert" className="help">{error}</div> : null}
    </div>
  );
}

function Area({ id, name, sessions }: { id: number; name: string; sessions: Session[] }) {
  const [open, setOpen] = useState(true);
  return (
    <div className="area">
      {(() => {
        const head = (
          <button type="button" className="area-head" aria-expanded={open} onClick={() => setOpen(!open)}>
            <ChevronRight className="area-chevron" size={12} aria-hidden="true" />
            <span className="tile" style={{ background: projectTint(id) }} aria-hidden="true">
              {[...name][0]?.toUpperCase()}
            </span>
            {name}
            <span className="area-count">{sessions.length}</span>
          </button>
        );
        return id > 0 ? <ProjectRemover project={{ id, name }} conversations={sessions.length}>{head}</ProjectRemover> : head;
      })()}
      {open ? (
        <div role="group" aria-label={name} className="area-items">
          {sessions.map((session) => (
            <SessionLine key={session.id} session={session} />
          ))}
        </div>
      ) : null}
    </div>
  );
}

type Props = { onOpenSettings?: () => void; onCollapse?: () => void; onOpenPalette?: () => void };

export function Sidebar({ onOpenSettings, onCollapse, onOpenPalette }: Props) {
  const more = useMore();
  const {cache}=useWorkspace();
  const client = useClient();
  const allSessions = useAppStore((state) => state.sessions);
  const approvals = useAppStore((state) => state.approvals);
  const questions = useAppStore((state) => state.questions);
  const projects = useAppStore((state) => state.projects);
  const status = useAppStore((state) => state.status);
  const newSessionKeys = usePreferences((state) => state.preferences.keybindings["session.new"]);
  const [query, setQuery] = useState("");
  const [history, setHistory] = useState<HistoryHit[]>([]);
  const [historyError, setHistoryError] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    setHistory([]); setHistoryError(null);
    if (!query.trim()) return;
    const timer = setTimeout(() => {
      client.call<HistoryHit[]>("history.search", { query, limit: 40 }).then((hits) => { if (current) setHistory(hits); }).catch((caught) => { if (current) setHistoryError(`${copy.transfer.searchFailed} ${caught.message}`); });
    }, 180);
    return () => { current = false; clearTimeout(timer); };
  }, [client, query]);
  const openImported = async (id: number) => {
    useAppStore.setState({ activeSessionId: null, importedSession: null, openingImportedId: id }); setHistoryError(null);
    try {
      const importedSession = await client.call<ImportedConversation>("history.open_imported", { id });
      if (useAppStore.getState().openingImportedId === id) useAppStore.setState({ importedSession, openingImportedId: null });
    } catch (caught) { if (useAppStore.getState().openingImportedId === id) { useAppStore.setState({ openingImportedId: null }); setHistoryError((caught as Error).message); } }
  };

  const needle = normalize(query).replace(/[^\p{L}\p{N} ]/gu, "");
  const sessions = needle ? allSessions.filter((session) => normalize(session.title).includes(needle)) : allSessions;
  const groups = sessionGroups({ sessions, approvals, questions });

  const directChats = groups.done.filter((session) => session.project_id == null);
  const areas = projects
    .map((project) => ({ project, items: groups.done.filter((session) => session.project_id === project.id) }))
    .filter((area) => area.items.length > 0);
  const known = new Set(projects.map((project) => project.id));
  const orphans = groups.done.filter((session) => session.project_id != null && !known.has(session.project_id));

  return (
    <nav className="side">
      <header className="side-head">
        <Mark size={17} />
        <span className="brand">Vatra</span>
        {onCollapse ? (
          <button type="button" className="icon" aria-label={copy.collapseSidebar} title={copy.collapseSidebar} onClick={onCollapse}>
            <PanelLeft size={15} aria-hidden="true" />
          </button>
        ) : null}
      </header>
      <WorkspaceNav/>
      {cache.selection.area==="conversations"?<>
      <button type="button" className="new-session" onClick={() => useAppStore.setState({ activeSessionId: null, importedSession: null, openingImportedId: null })}>
        <SquarePen size={15} aria-hidden="true" />
        {copy.newSession}
        <kbd>{displayCombo(newSessionKeys)}</kbd>
      </button>
      <label className="search">
        <Search size={15} aria-hidden="true" />
        <input type="search" aria-label={copy.search} placeholder={copy.search} value={query} onChange={(event) => setQuery(event.target.value)} />
      </label>
      {status !== "connected" ? (
        <div role="status" className="connection">
          {status === "reconnecting" ? copy.reconnecting : copy.connecting}
        </div>
      ) : null}
      <div className="side-list">
        {historyError ? <p role="alert">{historyError}</p> : null}
        {query.trim() ? history.filter((hit) => hit.session.kind === "imported" || !sessions.some((session) => session.id === hit.session.id)).map((hit) => <button type="button" className="line history-hit" key={`${hit.session.kind}-${hit.session.id}`} title={hit.snippet} onClick={() => hit.session.kind === "imported" ? void openImported(hit.session.id) : openSession(client, hit.session.id)}><span className="line-name">{hit.title}</span>{hit.session.kind === "imported" ? <small>{copy.transfer.imported}</small> : null}<small className="history-snippet">{hit.snippet}</small></button>) : null}
        {groups.waiting.map((session) => (
          <NeedsYou key={approvals.find((request) => request.session_id === session.id)?.id} session={session} requests={approvals.filter((request) => request.session_id === session.id)} />
        ))}
        {groups.working.length > 0 ? (
          <div role="group" aria-label={copy.working}>
            <div className="side-label">{copy.working}</div>
            {groups.working.map((session) => (
              <SessionLine key={session.id} session={session} live />
            ))}
          </div>
        ) : null}
        {directChats.length > 0 ? (
          <div role="group" aria-label={copy.chatsTitle}>
            <div className="side-label">{copy.chatsTitle}</div>
            {directChats.map((session) => (
              <SessionLine key={session.id} session={session} />
            ))}
          </div>
        ) : null}
        {areas.length > 0 ? (
          <div role="group" aria-label={copy.projectsTitle}>
            <div className="side-label">{copy.projectsTitle}</div>
            {areas.map(({ project, items }) => (
              <Area key={project.id} id={project.id} name={project.name} sessions={items} />
            ))}
          </div>
        ) : null}
        {orphans.length > 0 ? <Area id={0} name={copy.defaultProject} sessions={orphans} /> : null}
      </div>
      </>:<div className="side-list workspace-projects"><div className="side-label">{copy.workspace.project}</div>{projects.map(project=><ProjectRemover key={project.id} project={project} conversations={allSessions.filter(s=>s.project_id===project.id).length}><button type="button" className="line" aria-current={cache.selection.projectId===project.id?"true":undefined} onClick={()=>void navigateWorkspace({...cache.selection,projectId:project.id,itemId:null})}>{project.name}</button></ProjectRemover>)}{groups.waiting.length?<button type="button" className="workspace-notice" onClick={()=>void navigateWorkspace({...cache.selection,area:"conversations",itemId:null})}>{copy.approvals.review} ({groups.waiting.length})</button>:null}</div>}
      <footer className="side-foot">
        <button type="button" className="icon" aria-label={copy.settings} title={copy.settings} onClick={onOpenSettings}>
          <Settings2 size={15} aria-hidden="true" />
        </button>
        {onOpenPalette ? (
          <button type="button" className="icon" aria-label={more.paletteOpen} title={`${more.paletteOpen} (Ctrl+Shift+P)`} onClick={onOpenPalette}>
            <Command size={15} aria-hidden="true" />
          </button>
        ) : null}
        <span className="spacer" />
        <button type="button" className="icon" aria-label={copy.switchMode} title={copy.switchMode} onClick={toggleMode}>
          <Moon size={15} aria-hidden="true" />
        </button>
      </footer>
    </nav>
  );
}
