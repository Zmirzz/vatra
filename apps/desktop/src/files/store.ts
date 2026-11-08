import {create} from "zustand";
import type {FileRecord,FileSelection,InputCapabilities,FileClient} from "./client";
import {uploadPrepared} from "./client";
import {prepareFile,MAX_FILE_BYTES} from "./prepare";

export type SessionFiles={draft:FileSelection[];tabs:number[];active:number|null;open:boolean};
export type FileEdit={text:string;revision:number};
type FileState={sessions:Record<number,SessionFiles>;edits:Record<string,FileEdit>;width:number;records:Record<number,FileRecord[]>;links:Record<number,[number,number,number][]>;busy:Record<number,string|null>;errors:Record<number,string|null>;capabilities:Record<number,InputCapabilities>};
export const emptyFiles=():SessionFiles=>({draft:[],tabs:[],active:null,open:false});
export const useFiles=create<FileState>(()=>({sessions:{},edits:{},width:460,records:{},links:{},busy:{},errors:{},capabilities:{}}));
export function setFileEdit(session:number,id:number,edit:FileEdit|null){useFiles.setState(s=>{const edits={...s.edits};if(edit)edits[`${session}/${id}`]=edit;else delete edits[`${session}/${id}`];return {edits};});}
export function updateFiles(session:number,change:Partial<SessionFiles>){useFiles.setState(s=>({sessions:{...s.sessions,[session]:{...(s.sessions[session]??emptyFiles()),...change}}}));}
export function openFile(session:number,id:number){const current=useFiles.getState().sessions[session]??emptyFiles();updateFiles(session,{open:true,active:id,tabs:current.tabs.includes(id)?current.tabs:[...current.tabs,id]});}
export function selectFile(session:number,selection:FileSelection){
  const current=useFiles.getState().sessions[session]??emptyFiles();
  updateFiles(session,{draft:[...current.draft.filter(f=>f.id!==selection.id),selection]});
}
export function removeDraftFile(session:number,id:number){const current=useFiles.getState().sessions[session]??emptyFiles();updateFiles(session,{draft:current.draft.filter(f=>f.id!==id)});}
export async function refreshFiles(client:FileClient,session:number){
  const [records,links]=await Promise.all([client.call<FileRecord[]>("files.list",{session_id:session}),client.call<[number,number,number][]>("files.links",{session_id:session})]);
  useFiles.setState(s=>({records:{...s.records,[session]:records??[]},links:{...s.links,[session]:links??[]}}));
}
export async function importFiles(client:FileClient,session:number,files:File[]){
  if(useFiles.getState().busy[session])throw new Error("Files are still being prepared.");
  const current=useFiles.getState().sessions[session]??emptyFiles();
  if(current.draft.length+files.length>20)throw new Error("Attach up to 20 files per message.");
  const selected=useFiles.getState().records[session]??[];
  const oldSize=selected.filter(r=>current.draft.some(f=>f.id===r.id)).reduce((n,r)=>n+r.size,0);
  if(oldSize+files.reduce((n,f)=>n+f.size,0)>100*1024*1024)throw new Error("Attachments exceed 100 MiB per message.");
  const errors:string[]=[];
  useFiles.setState(s=>({errors:{...s.errors,[session]:null}}));
  try{for(const file of files){
    try{
      if(file.size>MAX_FILE_BYTES)throw new Error("File exceeds 25 MiB.");
      useFiles.setState(s=>({busy:{...s.busy,[session]:file.name}}));
      const prepared=await prepareFile(file);
      const record=await uploadPrepared(client,session,prepared);
      useFiles.setState(s=>({records:{...s.records,[session]:[record,...(s.records[session]??[])]}}));
      selectFile(session,{id:record.id});
      if(prepared.warning)errors.push(`${file.name}: ${prepared.warning}`);
    }catch(error){errors.push(`${file.name}: ${(error as Error).message}`);}
  }}finally{
    useFiles.setState(s=>({busy:{...s.busy,[session]:null},errors:{...s.errors,[session]:errors.length?errors.join("\n"):null}}));
  }
}
