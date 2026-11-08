import {Trash2} from "lucide-react";
import {useState, type ReactNode} from "react";
import {useHubCopy} from "../hub/copy";
import {useClient} from "../rpc/context";
import type {Project} from "../rpc/types";
import {refreshSessions, useAppStore} from "../state/store";
import {navigateWorkspace, useWorkspace} from "../workspace/store";

/**
 * A project's row with a delete button that appears on hover. Deleting asks first, says what goes with the project
 * and what stays, and then clears everything the app still shows about it.
 */
export function ProjectRemover({project, conversations, children}: {project: Pick<Project, "id" | "name">; conversations: number; children: ReactNode}) {
  const t = useHubCopy();
  const client = useClient();
  const {cache} = useWorkspace();
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const remove = async () => {
    setBusy(true); setError(null);
    try {
      await client.call("projects.delete", {id: project.id});
      const state = useAppStore.getState();
      const active = state.sessions.find((s) => s.id === state.activeSessionId);
      useAppStore.setState((s) => ({
        projects: s.projects.filter((p) => p.id !== project.id),
        ...(active?.project_id === project.id ? {activeSessionId: null} : {}),
      }));
      if (cache.selection.projectId === project.id) await navigateWorkspace({...cache.selection, projectId: null, itemId: null});
      await refreshSessions(client);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : t.saveError);
      setAsking(false);
    } finally { setBusy(false); }
  };

  return (
    <div className="line-row project-row">
      {children}
      <button type="button" className="line-delete" aria-label={`${t.projectDelete}: ${project.name}`} title={t.projectDelete} onClick={() => setAsking(true)}>
        <Trash2 size={13} aria-hidden="true" />
      </button>
      {asking ? (
        <div className="line-confirm" role="alertdialog" aria-label={`${t.projectDelete}: ${project.name}`}>
          <span>{t.projectDeleteBody(project.name, conversations)}</span>
          <div className="row">
            <button type="button" onClick={() => setAsking(false)} disabled={busy}>{t.cancel}</button>
            <button type="button" className="danger" onClick={() => void remove()} disabled={busy}>{t.projectDeleteNow}</button>
          </div>
        </div>
      ) : null}
      {error ? <div role="alert" className="help">{error}</div> : null}
    </div>
  );
}
