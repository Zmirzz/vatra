import type {RpcClient} from "../rpc/client";
import {useAppStore} from "../state/store";
import {useFiles,type SessionFiles,type FileEdit} from "./store";
type UiState={drafts:Record<number,string>;sessions:Record<number,SessionFiles>;edits:Record<string,FileEdit>;width:number};
const object=(value:unknown):Record<string,unknown>=>value!==null&&typeof value==="object"&&!Array.isArray(value)?value as Record<string,unknown>:{};
const id=(value:unknown):value is number=>typeof value==="number"&&Number.isSafeInteger(value)&&value>0;
export function sanitizeConversationUi(value:unknown):UiState{
  const data=object(value);const drafts:UiState["drafts"]={},sessions:UiState["sessions"]={},edits:UiState["edits"]={};
  for(const [key,text] of Object.entries(object(data.drafts)))if(id(Number(key))&&typeof text==="string"&&text.length<=500_000)drafts[Number(key)]=text;
  for(const [key,value] of Object.entries(object(data.sessions))){
    if(!id(Number(key)))continue;const state=object(value);
    const draft=(Array.isArray(state.draft)?state.draft:[]).map(object).filter(s=>id(s.id)&&[s.start,s.end].every(n=>n===undefined||typeof n==="number"&&Number.isSafeInteger(n)&&n>=0)).slice(0,20).map(s=>({id:s.id as number,...(s.start===undefined?{}:{start:s.start as number}),...(s.end===undefined?{}:{end:s.end as number})}));
    const tabs=(Array.isArray(state.tabs)?state.tabs:[]).filter(id).slice(0,30);
    sessions[Number(key)]={draft,tabs,active:id(state.active)&&tabs.includes(state.active)?state.active:null,open:state.open===true};
  }
  for(const [key,value] of Object.entries(object(data.edits))){const edit=object(value);if(/^\d+\/\d+$/.test(key)&&typeof edit.text==="string"&&edit.text.length<=500_000&&id(edit.revision))edits[key]={text:edit.text,revision:edit.revision};}
  return {drafts,sessions,edits,width:typeof data.width==="number"&&Number.isFinite(data.width)?Math.max(320,Math.min(900,data.width)):460};
}
let client:RpcClient|null=null,ready=false,dirty=false,timer:ReturnType<typeof setTimeout>|undefined;
let pending:Promise<void>=Promise.resolve();
function snapshot():UiState{const app=useAppStore.getState(),files=useFiles.getState();return {drafts:app.drafts,sessions:files.sessions,edits:files.edits,width:files.width};}
export function hasUnsavedConversationState(){return dirty;}
export async function flushConversationState():Promise<void>{
  if(timer)clearTimeout(timer);if(!ready||!client)return;
  const target=client,save=snapshot();const serialized=JSON.stringify(save);
  pending=pending.catch(()=>undefined).then(async()=>{await target.call("conversations.ui.set",save);if(JSON.stringify(snapshot())===serialized)dirty=false;});
  return pending;
}
function changed(){
  dirty=true;if(!ready)return;if(timer)clearTimeout(timer);
  timer=setTimeout(()=>{void flushConversationState().catch(error=>{
    const session=useAppStore.getState().activeSessionId;if(session!==null)useFiles.setState(s=>({errors:{...s.errors,[session]:`Draft could not be saved: ${(error as Error).message}`}}));
  });},250);
}
export function attachConversationPersistence(target:RpcClient){
  client=target;
  target.onStatus(status=>{if(status!=="connected")return;
    if(ready){if(dirty)void flushConversationState().catch(()=>undefined);return;}
    void target.call<unknown>("conversations.ui.get",{}).then(value=>{
      const saved=sanitizeConversationUi(value);const local=snapshot();
      useAppStore.setState({drafts:{...saved.drafts,...local.drafts}});
      useFiles.setState({sessions:{...saved.sessions,...local.sessions},edits:{...saved.edits,...local.edits},width:saved.width});
      ready=true;if(dirty)changed();
    }).catch(()=>{ready=true;changed();});
  });
  useAppStore.subscribe((state,old)=>{if(state.drafts!==old.drafts)changed();});
  useFiles.subscribe((state,old)=>{if(state.sessions!==old.sessions||state.edits!==old.edits||state.width!==old.width)changed();});
}
