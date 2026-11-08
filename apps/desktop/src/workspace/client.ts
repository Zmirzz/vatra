import {RpcError,type RpcClient} from "../rpc/client";
import {WorkspaceConflict} from "./autosave";
import type {DocumentFormat,ItemDraft,WorkspaceItem,WorkspaceProposal,Reminder} from "./types";
export class WorkspaceClient {
 constructor(readonly rpc:Pick<RpcClient,"call">){}
 list(project_id:number,kinds:string[]=[],deleted=false,after_id?:number){return this.rpc.call<WorkspaceItem[]>("workspace.items.list",{project_id,kinds,deleted,after_id,limit:100});}
 tasks(project_id:number,task_view:"today"|"upcoming"|"completed",today:string,deleted=false,after_id?:number){return this.rpc.call<WorkspaceItem[]>("workspace.items.list",{project_id,kinds:["task"],task_view,today,deleted,after_id,limit:100});}
 get(project_id:number,id:number,include_deleted=false){return this.rpc.call<WorkspaceItem>("workspace.items.get",{project_id,id,include_deleted});}
 create(project_id:number,kind:string,draft:ItemDraft){return this.rpc.call<WorkspaceItem>("workspace.items.create",{project_id,kind,...draft});}
 async save(project_id:number,id:number,draft:ItemDraft,expected_revision:number){try{return await this.rpc.call<WorkspaceItem>("workspace.items.save",{project_id,id,...draft,expected_revision});}catch(error){this.conflict(error);throw error;}}
 private conflict(error:unknown):never|void {if(error instanceof RpcError&&error.code===1101){const data=error.data as {current?:WorkspaceItem}|undefined;if(data?.current)throw new WorkspaceConflict(data.current);}}
 async remove(project_id:number,id:number,expected_revision:number){try{return await this.rpc.call<WorkspaceItem>("workspace.items.delete",{project_id,id,expected_revision});}catch(e){this.conflict(e);throw e;}}
 async restore(project_id:number,id:number,expected_revision:number){try{return await this.rpc.call<WorkspaceItem>("workspace.items.restore",{project_id,id,expected_revision});}catch(e){this.conflict(e);throw e;}}
 search(project_id:number,query:string,kinds:string[]){return this.rpc.call<{id:number;kind:string;title:string;snippet:string}[]>("workspace.search",{project_id,query,kinds,limit:100});}
 proposals(project_id:number,target_id:number,after_id?:number){return this.rpc.call<WorkspaceProposal[]>("workspace.proposals.list",{project_id,target_id,after_id,limit:100});}
 async accept(project_id:number,id:number){try{return await this.rpc.call<{proposal_id:number;items:WorkspaceItem[]}>("workspace.proposals.accept",{project_id,id});}catch(e){this.conflict(e);throw e;}}
 reject(project_id:number,id:number){return this.rpc.call("workspace.proposals.reject",{project_id,id});}
 links(project_id:number,id:number){return this.rpc.call<[string,WorkspaceItem][]>("workspace.links.list",{project_id,id});}
 link(project_id:number,from_id:number,to_id:number,rel="related"){return this.rpc.call("workspace.links.add",{project_id,from_id,to_id,rel});}
 unlink(project_id:number,from_id:number,to_id:number,rel="related"){return this.rpc.call("workspace.links.remove",{project_id,from_id,to_id,rel});}
 import(project_id:number,path:string,format:DocumentFormat){return this.rpc.call<WorkspaceItem>("workspace.files.import",{project_id,path,format});}
 export(project_id:number,id:number,expected_revision:number,path:string,mode:"safe"|"exact"="safe"){return this.rpc.call("workspace.files.export",{project_id,id,expected_revision,path,mode});}
 reminders(project_id:number){return this.rpc.call<Reminder[]>("reminders.list",{project_id,limit:100});}
 ack(project_id:number,id:number,generation:number){return this.rpc.call("reminders.ack",{project_id,id,generation});}
}
