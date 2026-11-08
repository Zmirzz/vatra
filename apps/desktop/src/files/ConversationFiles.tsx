import {FileText,GitBranch,RotateCcw,Pencil} from "lucide-react";
import {useState} from "react";
import {useAppStore,openSession,refreshSessions} from "../state/store";
import {useClient} from "../rpc/context";
import {usePreferences} from "../settings/preferences";
import type {FileSelection} from "./client";
import {openFile,updateFiles,useFiles} from "./store";

export function ConversationFiles({seq,toolIds=[]}:{seq:number;toolIds?:string[]}){
  const session=useAppStore(s=>s.activeSessionId);
  const records=useFiles(s=>session===null?undefined:s.records[session]);
  const links=useFiles(s=>session===null?undefined:s.links[session]);
  // Skip user-attached files linked by seq — MessageAttachments already renders them inline.
  // Only show agent-created/modified files from links, plus tool output files.
  const shown=(records??[]).filter(file=>{
    const linkedBySeq=(links??[]).some(([message,id])=>message===seq&&id===file.id);
    if(linkedBySeq&&file.origin==="attached")return false;
    return linkedBySeq||file.operation===`turn:${seq}`||Boolean(file.operation&&toolIds.includes(file.operation));
  });
  if(!shown.length||session===null)return null;
  return (
    <div className="conversation-files">
      {shown.map(file=>(
        <button key={file.id} type="button" onClick={()=>openFile(session,file.id)}>
          <FileText size={14}/>{file.name}
        </button>
      ))}
    </div>
  );
}

export function MessageActions({seq,text,user=false}:{seq:number;text:string;user?:boolean}){
  const client=useClient();
  const language=usePreferences(s=>s.preferences.language);
  const ro=language==="ro";
  const active=useAppStore(s=>s.activeSessionId);
  const session=useAppStore(s=>s.sessions.find(s=>s.id===active));
  const [error,setError]=useState<string|null>(null);
  const [busy,setBusy]=useState(false);

  const branch=async(action:"branch"|"edit"|"regenerate")=>{
    if(!session||busy||session.running)return;
    setBusy(true);setError(null);
    try{
      const state=useAppStore.getState();
      const events=state.eventsBySession[session.id]??[];
      const source=action==="regenerate"
        ?[...events].reverse().find(e=>e.seq<seq&&e.event.type==="user_message")
        :undefined;
      const messageSeq=source?.seq??seq;
      const prompt=source?.event.type==="user_message"?source.event.text:user?text:"";
      const {id,attachments}=await client.call<{id:number;attachments:FileSelection[]}>(
        "sessions.branch",
        {session_id:session.id,through_seq:action==="branch"?seq:messageSeq-1,...(action!=="branch"?{draft_from_seq:messageSeq}:{})}
      );
      const options=state.turnOptions[session.id]??session.options??{};
      const mode=state.modes[session.id]??"supervised";
      useAppStore.setState(s=>({
        drafts:{...s.drafts,[id]:action==="branch"?"":prompt},
        turnOptions:{...s.turnOptions,[id]:options},
        modes:{...s.modes,[id]:mode}
      }));
      updateFiles(id,{draft:attachments});
      await refreshSessions(client);
      openSession(client,id);
      if(action==="regenerate"){
        try{
          await client.call("turn.start",{session_id:id,prompt,mode,...options,...(attachments.length?{attachments}:{})});
          updateFiles(id,{draft:[]});
          useAppStore.setState(s=>({drafts:{...s.drafts,[id]:""}}));
        }catch(e){useFiles.setState(s=>({errors:{...s.errors,[id]:(e as Error).message}}));}
      }else{
        setTimeout(()=>document.querySelector<HTMLTextAreaElement>(".prompt textarea")?.focus(),0);
      }
    }catch(e){setError((e as Error).message);}
    finally{setBusy(false);}
  };

  return (
    <>
      <div className="message-actions">
        {user
          ?<button type="button" className="action-icon" title={ro?"Editează mesajul":"Edit message"} disabled={busy||session?.running} onClick={()=>void branch("edit")}><Pencil size={14}/></button>
          :<button type="button" className="action-icon" title={ro?"Regenerează":"Regenerate"} disabled={busy||session?.running} onClick={()=>void branch("regenerate")}><RotateCcw size={14}/></button>
        }
        <button type="button" className="action-icon" title={ro?"Continuă într-o ramură":"Branch from here"} disabled={busy||session?.running} onClick={()=>void branch("branch")}><GitBranch size={14}/></button>
      </div>
      {error?<p role="alert">{error}</p>:null}
    </>
  );
}
