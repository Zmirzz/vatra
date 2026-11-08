import { PanelLeft,Files,Pencil } from "lucide-react";
import { useEffect, useState } from "react";
import { Composer } from "./components/Composer";
import { Conversation } from "./components/Conversation";
import { Home } from "./components/Home";
import { SettingsPage } from "./components/SettingsPage";
import { Onboarding } from "./components/Onboarding";
import { ImportedSession } from "./components/ImportedSession";
import { Sidebar } from "./components/Sidebar";
import { projectTint } from "./conversation/labels";
import { copy } from "./copy";
import { useClient } from "./rpc/context";
import { matchCommand } from "./settings/keybindings";
import { toggleMode, usePreferences } from "./settings/preferences";
import { useAppStore,refreshSessions } from "./state/store";
import {WorkspaceShell} from "./components/workspace/WorkspaceShell";
import {CommandPalette} from "./components/CommandPalette";
import {ToastHost} from "./hub/toast";
import {WorkspaceNav} from "./components/workspace/WorkspaceNav";
import {flushWorkspace,hasUnsavedWorkspaceDrafts,navigateWorkspace,useWorkspace} from "./workspace/store";
import {FileInspector} from "./files/FileInspector";
import {updateFiles,useFiles} from "./files/store";
import {fileCopy} from "./files/copy";
import "./files/files.css";
import {flushConversationState,hasUnsavedConversationState} from "./files/persistence";

function ExpandButton({ waiting, onClick }: { waiting: number; onClick: () => void }) {
  const label = waiting > 0 ? copy.expandSidebarWaiting(waiting) : copy.expandSidebar;
  return (
    <button type="button" className="icon expand" data-waiting={waiting > 0} aria-label={label} title={label} onClick={onClick}>
      <PanelLeft size={15} aria-hidden="true" />
    </button>
  );
}

export function App() {
  const workspace=useWorkspace();
  const client = useClient();
  const importedSession = useAppStore((state) => state.importedSession);
  const openingImportedId = useAppStore((state) => state.openingImportedId);
  const session = useAppStore((state) => state.sessions.find((candidate) => candidate.id === state.activeSessionId));
  const project = useAppStore((state) => state.projects.find((candidate) => candidate.id === session?.project_id));
  const bindings = usePreferences((state) => state.preferences.keybindings);
  // Every label reads the chosen language, so a change just needs this tree to render again.
  const language=usePreferences((state) => state.preferences.language);
  const connectionStatus=useAppStore(state=>state.status);
  useEffect(()=>{if(connectionStatus==="connected")void client.call("reminders.preferences.set",{language}).catch(()=>undefined);},[client,language,connectionStatus]);
  const waitingCount = useAppStore((state) => state.approvals.length);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [renamingTitle,setRenamingTitle]=useState<string|null>(null);
  const [titleError,setTitleError]=useState<string|null>(null);
  useEffect(()=>{setRenamingTitle(null);setTitleError(null);},[session?.id]);
  const settingsPage = useAppStore((state) => state.settingsPage);
  const [collapsed, setCollapsed] = useState(()=>window.innerWidth<960);
  // Shrinking the window below the point where both fit folds the list away, so it never covers the page.
  useEffect(()=>{
    let narrow=window.innerWidth<960;
    const onResize=()=>{const now=window.innerWidth<960;if(now&&!narrow)setCollapsed(true);narrow=now;};
    window.addEventListener("resize",onResize);
    return ()=>window.removeEventListener("resize",onResize);
  },[]);
  const agentsLoaded = useAppStore((state) => state.agentsLoaded);
  const agents = useAppStore((state) => state.agents);
  const setupRequested = useAppStore((state) => state.setupRequested);
  const [setup, setSetup] = useState<{ sources: boolean } | null>(null);
  useEffect(() => {
    if (!setupRequested && (!agentsLoaded || agents.length > 0)) return;
    let live = true;
    client.call<{ done: boolean; sources: boolean }>("onboarding.state", {}).then((state) => {
      if (live && (setupRequested || state.done === false)) setSetup({ sources: state.sources });
    }).catch(() => undefined);
    return () => { live = false; };
  }, [client, agentsLoaded, agents.length, setupRequested]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      // Ctrl+Shift+P opens the search over everything, from anywhere, including from inside Settings.
      if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === "p") {
        event.preventDefault();
        setPaletteOpen((open) => !open);
        return;
      }
      if (settingsOpen || paletteOpen) {
        return;
      }
      const command = matchCommand(event, bindings);
      if (command === null) {
        return;
      }
      event.preventDefault();
      const active = useAppStore.getState().activeSessionId;
      if (command === "session.new") void navigateWorkspace({...workspace.cache.selection,area:"conversations",itemId:null}).then(ok=>{if(ok)useAppStore.setState({ activeSessionId: null, importedSession: null, openingImportedId: null });});
      if (command === "settings.open") setSettingsOpen(true);
      if (command === "theme.cycleMode") toggleMode();
      if (command === "session.search") document.querySelector<HTMLInputElement>(".search input")?.focus();
      if (command === "composer.focus") document.querySelector<HTMLTextAreaElement>(".prompt textarea")?.focus();
      if (command === "turn.stop" && active !== null) {
        client.call("turn.stop", { session_id: active }).catch(() => undefined);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [bindings, client, settingsOpen, paletteOpen]);

  useEffect(()=>{
    const before=(event:BeforeUnloadEvent)=>{if(hasUnsavedWorkspaceDrafts()||hasUnsavedConversationState()){event.preventDefault();event.returnValue="";}};
    window.addEventListener("beforeunload",before);
    let active=true;let unlisten:(()=>void)|undefined;
    if("__TAURI_INTERNALS__" in window)void import("@tauri-apps/api/window").then(async({getCurrentWindow})=>{
      const win=getCurrentWindow();
      // Closing hides the window; the service keeps running so reminders can fire.
      const stop=await win.onCloseRequested(async event=>{
        event.preventDefault();
        try{await flushConversationState();}catch{await win.show().catch(()=>undefined);return;}
        if(hasUnsavedWorkspaceDrafts()){
          try{await flushWorkspace();}
          catch{useWorkspace.setState({navigationError:true});await win.show().catch(()=>undefined);return;}
        }
        await win.hide();
      });
      if(active)unlisten=stop;else stop();
    }).catch(()=>undefined);
    return()=>{active=false;window.removeEventListener("beforeunload",before);unlisten?.();};
  },[]);

  useEffect(()=>{
    if(!("__TAURI_INTERNALS__" in window))return;
    void import("@tauri-apps/api/core").then(({invoke})=>invoke("set_tray_labels",{open:copy.trayOpen,quit:copy.trayQuit,tooltip:copy.trayTooltip})).catch(()=>undefined);
  },[language]);

  useEffect(()=>{
    if(!("__TAURI_INTERNALS__" in window))return;
    let active=true;let unlisten:(()=>void)|undefined;
    void import("@tauri-apps/api/event").then(async({listen})=>{
      // The tray asks for a quit only after the window is visible, so a draft is never lost silently.
      const stop=await listen("vatra://quit-requested",async()=>{
        try{await flushConversationState();}catch{return;}
        if(hasUnsavedWorkspaceDrafts()){
          try{await flushWorkspace();}
          catch{useWorkspace.setState({navigationError:true});return;}
        }
        const {invoke}=await import("@tauri-apps/api/core");
        await invoke("quit_app").catch(()=>undefined);
      });
      if(active)unlisten=stop;else stop();
    }).catch(()=>undefined);
    return()=>{active=false;unlisten?.();};
  },[]);

  return (
    <div className="app" data-collapsed={collapsed}>
      {collapsed ? null : <Sidebar onOpenSettings={() => setSettingsOpen(true)} onCollapse={() => setCollapsed(true)} onOpenPalette={() => setPaletteOpen(true)} />}
      <main className="main">
        {collapsed&&workspace.cache.selection.area!=="conversations"?<div className="workspace-compact-nav"><ExpandButton waiting={waitingCount} onClick={()=>setCollapsed(false)}/><WorkspaceNav/></div>:null}
        {workspace.cache.selection.area!=="conversations"?<WorkspaceShell/>:importedSession || openingImportedId !== null ? <>
          <header className="crumb">{collapsed ? <ExpandButton waiting={waitingCount} onClick={() => setCollapsed(false)} /> : null}{importedSession ? <b>{importedSession.title}</b> : null}</header>
          {importedSession ? <ImportedSession session={importedSession} /> : <p role="status">{copy.transfer.opening}</p>}
        </> : session === undefined ? (
          <>
            <header className="crumb">
              {collapsed ? (
                <ExpandButton waiting={waitingCount} onClick={() => setCollapsed(false)} />
              ) : null}
            </header>
            <Home />
          </>
        ) : (
          <>
            <header className="crumb">
              {collapsed ? (
                <ExpandButton waiting={waitingCount} onClick={() => setCollapsed(false)} />
              ) : null}
              {project ? (
                <>
                  <span className="tile" style={{ background: projectTint(project.id) }} aria-hidden="true">
                    {[...project.name][0]?.toUpperCase()}
                  </span>
                  {project.name}
                  <span aria-hidden="true">/</span>
                </>
              ) : null}
              {renamingTitle===null?<button className="session-title" type="button" title={language==="ro"?"Redenumește conversația":"Rename conversation"} onClick={()=>setRenamingTitle(session.title)}><b>{session.title}</b><Pencil size={12}/></button>:<form className="rename-conversation" onSubmit={event=>{event.preventDefault();void client.call("sessions.rename",{session_id:session.id,title:renamingTitle}).then(()=>refreshSessions(client)).then(()=>setRenamingTitle(null)).catch(error=>setTitleError(error.message));}}><input autoFocus aria-label={language==="ro"?"Titlul conversației":"Conversation title"} maxLength={120} value={renamingTitle} onChange={e=>setRenamingTitle(e.target.value)} onKeyDown={e=>{if(e.key==="Escape")setRenamingTitle(null);}}/><button type="submit">{language==="ro"?"Salvează":"Save"}</button></form>}
              {titleError?<span role="alert">{titleError}</span>:null}
              <span className="spacer"/>
              <button type="button" className="files-toggle" onClick={()=>updateFiles(session.id,{open:!(useFiles.getState().sessions[session.id]?.open??false)})}><Files size={16}/>{fileCopy().files}</button>
            </header>
            <Conversation />
            <Composer sessionId={session.id} />
          </>
        )}
      </main>
      {session&&workspace.cache.selection.area==="conversations"?<FileInspector key={session.id} sessionId={session.id}/>:null}
      {settingsOpen || settingsPage ? (
        <SettingsPage
          onClose={() => { setSettingsOpen(false); useAppStore.setState({ settingsPage: null }); }}
          onRestartService={() => {
            setSettingsOpen(false);
            client.call("service.shutdown", {}).catch(() => undefined);
          }}
        />
      ) : null}
      {paletteOpen ? <CommandPalette onClose={() => setPaletteOpen(false)} /> : null}
      <ToastHost />
      {setup ? <Onboarding sources={setup.sources} onDone={() => { setSetup(null); useAppStore.setState({ setupRequested: false }); }} /> : null}
    </div>
  );
}
