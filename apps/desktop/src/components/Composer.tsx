import { ShieldCheck } from "lucide-react";
import { useRef, useState } from "react";
import { copy } from "../copy";
import { useClient } from "../rpc/context";
import type { PermissionMode, TurnOptions } from "../rpc/types";
import { useAppStore } from "../state/store";
import { ApprovalCard } from "./ApprovalCard";
import { HandoffDialog } from "./HandoffDialog";
import { Menu } from "./Menu";
import { ModelPicker } from "./ModelPicker";
import { PromptBox } from "./PromptBox";
import { QuestionCard } from "./QuestionCard";
import {AttachmentTray,selectedAttachments} from "../files/AttachmentTray";
import {importFiles,updateFiles,useFiles} from "../files/store";
import {ContextStatus} from "../files/ContextStatus";

export const MODE_IDS: PermissionMode[] = ["supervised", "accept_edits", "full_access"];

export const modeOptions = () => MODE_IDS.map((value) => ({ value, label: copy.modes[value] }));

export function Composer({ sessionId }: { sessionId: number }) {
  const client = useClient();
  const session = useAppStore((state) => state.sessions.find((candidate) => candidate.id === sessionId));
  const turnOptions = useAppStore((state) => state.turnOptions[sessionId]);
  const options = turnOptions ?? session?.options ?? {};
  const [handoffOptions, setHandoffOptions] = useState<TurnOptions>({});
  const draft = useAppStore((state) => state.drafts[sessionId] ?? "");
  const mode = useAppStore((state) => state.modes[sessionId] ?? "supervised");
  const approvals = useAppStore((state) => state.approvals).filter((approval) => approval.session_id === sessionId);
  const questions = useAppStore((state) => state.questions).filter((request) => request.session_id === sessionId);
  const [error, setError] = useState<string | null>(null);
  const [handoffTo, setHandoffTo] = useState<string | null>(null);
  // A second Enter before the first send has answered must not send the same text again.
  const sending = useRef(false);
  const running = session?.running ?? false;
  const preparing=useFiles(state=>state.busy[sessionId]);
  const attachmentDraft=useFiles(state=>state.sessions[sessionId]?.draft);
  const agent = session?.agent ?? "";

  const setDraft = (text: string) =>
    useAppStore.setState((state) => ({ drafts: { ...state.drafts, [sessionId]: text } }));
  const setMode = (next: string) =>
    useAppStore.setState((state) => ({ modes: { ...state.modes, [sessionId]: next as PermissionMode } }));

  const send = async () => {
    const attachments=selectedAttachments(sessionId);
    if (sending.current || running || preparing || (!draft.trim()&&attachments.length===0)) {
      return;
    }
    sending.current = true;
    setError(null);
    try {
      await client.call("turn.start", { session_id: sessionId, prompt: draft, mode, ...options,...(attachments.length?{attachments}:{}) });
      setDraft("");
      updateFiles(sessionId,{draft:[]});
    } catch (caught) {
      setError((caught as Error).message);
    } finally {
      sending.current = false;
    }
  };

  const stop = async () => {
    try {
      await client.call("turn.stop", { session_id: sessionId });
    } catch (caught) {
      setError((caught as Error).message);
    }
  };

  return (
    <div className="dock">
      {approvals.map((request) => (
        <ApprovalCard key={request.id} request={request} agent={agent} />
      ))}
      {questions.map((request) => (
        <QuestionCard key={request.id} request={request} />
      ))}
      {error ? <div role="alert">{error}</div> : null}
      <PromptBox
        value={draft}
        onChange={setDraft}
        onSubmit={send}
        placeholder={copy.sessionPlaceholder}
        projectId={session?.project_id ?? 0}
        running={running}
        onStop={stop}
        sendDisabled={Boolean(preparing)||(!draft.trim()&&!attachmentDraft?.length)}
        onFiles={files=>{void importFiles(client,sessionId,files).catch(e=>setError(e.message));}}
        attachments={<AttachmentTray sessionId={sessionId} model={options.model}/>}
        left={<Menu label={copy.permissions} value={mode} options={modeOptions()} onSelect={setMode} icon={<ShieldCheck size={14} aria-hidden="true" />} />}
        right={
          <ModelPicker
            value={agent}
            options={options}
            projectId={session?.project_id ?? 0}
            disabled={running}
            onOptionsChange={(next) => {
              useAppStore.setState((state) => ({ turnOptions: { ...state.turnOptions, [sessionId]: next } }));
              client.call("sessions.options", { session_id: sessionId, ...next }).catch((caught) => setError((caught as Error).message));
            }}
            onSelect={(next, choice = {}) => {
              if (next !== agent) { setHandoffOptions(choice); setHandoffTo(next); }
            }}
          />
        }
      />
      <ContextStatus sessionId={sessionId}/>
      {handoffTo !== null ? <HandoffDialog sessionId={sessionId} to={handoffTo} mode={mode} options={handoffOptions} onClose={() => setHandoffTo(null)} /> : null}
    </div>
  );
}
