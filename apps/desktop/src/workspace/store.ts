import {create} from "zustand";
import {AutosaveController,type SaveSnapshot} from "./autosave";
import {WorkspaceClient} from "./client";
import type {RpcClient} from "../rpc/client";
import type {ItemDraft,WorkspaceItem,WorkspaceSelection} from "./types";
const key=(p:number,id:number)=>`${p}/${id}`;
export class WorkspaceCache {
 selection:WorkspaceSelection={projectId:null,area:"conversations",itemId:null};
 private items=new Map<string,WorkspaceItem>();private drafts=new Map<string,ItemDraft>();
 constructor(private changed:()=>void=()=>{}){}
 select(selection:WorkspaceSelection){this.selection={...selection};this.changed();}
 merge(items:WorkspaceItem[]){for(const item of items){const old=this.get(item.project_id,item.id);if(!old||item.revision>old.revision||(item.revision===old.revision&&(!item.body_truncated||old.body_truncated)))this.items.set(key(item.project_id,item.id),item);}this.changed();}
 get(project:number,id:number){return this.items.get(key(project,id));}
 draft(project:number,id:number){return this.drafts.get(key(project,id));}
 setDraft(project:number,id:number,draft:ItemDraft){this.drafts.set(key(project,id),draft);this.changed();}
 clear(){this.items.clear();this.drafts.clear();this.changed();}
}
export const workspaceCache=new WorkspaceCache(()=>useWorkspace.setState(s=>({version:s.version+1})));
export const useWorkspace=create(()=>({cache:workspaceCache,version:0,navigationError:false}));
const controllers=new Map<string,AutosaveController>();
const blockedForms=new Set<string>();
export function markWorkspaceBlocked(project:number,id:number,blocked:boolean){const idKey=key(project,id);if(blocked===blockedForms.has(idKey))return;if(blocked)blockedForms.add(idKey);else blockedForms.delete(idKey);useWorkspace.setState(s=>({version:s.version+1}));}
export function hasWorkspaceBlock(project:number,id:number){return blockedForms.has(key(project,id));}
export function controllerFor(rpc:RpcClient,item:WorkspaceItem){const id=key(item.project_id,item.id);let controller=controllers.get(id);if(!controller){const client=new WorkspaceClient(rpc);controller=new AutosaveController(item,(draft,revision)=>client.save(item.project_id,item.id,draft,revision),(snapshot:SaveSnapshot)=>{workspaceCache.setDraft(item.project_id,item.id,snapshot.draft);workspaceCache.merge([snapshot.item]);});controllers.set(id,controller);}return controller;}
export function hasUnsavedWorkspaceDrafts(){return blockedForms.size>0||[...controllers.values()].some(c=>c.snapshot().dirty);}
export async function flushWorkspace(){if(blockedForms.size)throw Error("An unresolved workspace form must be corrected first.");for(const controller of controllers.values())await controller.flush();}
export async function navigateWorkspace(selection:WorkspaceSelection){try{await flushWorkspace();useWorkspace.setState({navigationError:false});workspaceCache.select(selection);return true;}catch{useWorkspace.setState({navigationError:true});return false;}}
export function resetWorkspace(){for(const c of controllers.values())c.dispose();controllers.clear();blockedForms.clear();workspaceCache.clear();workspaceCache.select({projectId:null,itemId:null,area:"conversations"});useWorkspace.setState({navigationError:false});}
