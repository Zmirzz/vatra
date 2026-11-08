import {useEffect,useState} from "react";
import {FileText,Image as ImageIcon,X} from "lucide-react";
import {useClient} from "../rpc/context";
import {useAppStore} from "../state/store";
import {usePreferences} from "../settings/preferences";
import {emptyFiles,openFile,refreshFiles,removeDraftFile,useFiles} from "./store";
import {readFileBytes,type FileRecord,type InputCapabilities} from "./client";
import {fileCopy} from "./copy";

function ImageThumbnail({file}:{file:FileRecord}){
  const client=useClient();const [url,setUrl]=useState<string|null>(null);
  useEffect(()=>{
    let active=true,objectUrl:string|undefined;
    if(typeof createImageBitmap!=="function")return;
    void readFileBytes(client,file.session_id,file).then(async bytes=>{
      const bitmap=await createImageBitmap(new Blob([bytes],{type:file.media_type}),{resizeWidth:80,resizeHeight:80});
      try{const canvas=document.createElement("canvas");canvas.width=80;canvas.height=80;canvas.getContext("2d")?.drawImage(bitmap,0,0,80,80);const blob=await new Promise<Blob|null>(resolve=>canvas.toBlob(resolve));if(blob&&active){objectUrl=URL.createObjectURL(blob);setUrl(objectUrl);}}finally{bitmap.close();}
    }).catch(()=>undefined);
    return()=>{active=false;if(objectUrl)URL.revokeObjectURL(objectUrl);};
  },[client,file.id,file.revision,file.session_id]);
  return url?<img className="attachment-thumbnail" src={url} alt=""/>:<ImageIcon size={18}/>;
}

export function AttachmentTray({sessionId,model}:{sessionId:number;model?:string}){
  const client=useClient();usePreferences(s=>s.preferences.language);const c=fileCopy();
  const state=useFiles(s=>s.sessions[sessionId]);const draft=state?.draft??[];
  const records=useFiles(s=>s.records[sessionId]);const busy=useFiles(s=>s.busy[sessionId]);
  const error=useFiles(s=>s.errors[sessionId]);const caps=useFiles(s=>s.capabilities[sessionId]);
  const links=useFiles(s=>s.links[sessionId]);
  const eventCount=useAppStore(s=>s.eventsBySession[sessionId]?.length??0);
  const running=useAppStore(s=>s.sessions.find(session=>session.id===sessionId)?.running);
  useEffect(()=>{void refreshFiles(client,sessionId).catch(()=>undefined);},[client,sessionId,eventCount,running]);
  useEffect(()=>{let active=true;client.call<InputCapabilities>("files.capabilities",{session_id:sessionId,model}).then(caps=>{if(active&&Number.isFinite(caps?.context_tokens))useFiles.setState(s=>({capabilities:{...s.capabilities,[sessionId]:caps}}));}).catch(()=>undefined);return()=>{active=false;};},[client,sessionId,model]);
  const selected=(records??[]).filter(f=>draft.some(d=>d.id===f.id));
  const historyImages=(records??[]).some(file=>file.media_type.startsWith("image/")&&(links??[]).some(([,id])=>id===file.id));
  const selectedImage=selected.some(file=>file.media_type.startsWith("image/"));
  return <div className="attachment-area">
    {selected.length>0?<div className="attachment-tray" aria-label={c.attached}>{selected.map(file=><div className="attachment-chip" key={file.id}>
      <button type="button" onClick={()=>openFile(sessionId,file.id)} title={file.name}>{file.media_type.startsWith("image/")?<ImageThumbnail file={file}/>:<FileText size={18}/>}<span>{file.name}<small>{Math.ceil(file.size/1024)} KiB{draft.find(d=>d.id===file.id)?.start!==undefined?` · ${c.selection}`:""}</small></span></button>
      <button type="button" aria-label={`${c.remove}: ${file.name}`} onClick={()=>removeDraftFile(sessionId,file.id)}><X size={13}/></button>
    </div>)}</div>:null}
    {busy?<p className="help" role="status">{c.preparing}: {busy}</p>:null}
    {error?<p className="notice notice-warning" role="alert">{error}</p>:null}
    {selectedImage&&caps?.images!=="supported"?<p className="notice notice-warning" role="status">{c.vision}</p>:null}
    {!selectedImage&&historyImages&&caps?.images!=="supported"?<p className="notice notice-warning" role="status">{c.visionHistory}</p>:null}
    {draft.length>0&&caps?<p className="context-budget">{c.budget}: {Math.min(16000,Math.floor(Math.max(0,caps.context_tokens-4096)/4)).toLocaleString()} tokens</p>:null}
  </div>;
}
export function selectedAttachments(sessionId:number){return (useFiles.getState().sessions[sessionId]??emptyFiles()).draft;}
