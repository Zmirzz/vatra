import {itemDraft,type ItemDraft,type WorkspaceItem} from "./types";
export class WorkspaceConflict extends Error {constructor(readonly current:WorkspaceItem){super("workspace_conflict");}}
export type SaveStatus="saved"|"dirty"|"saving"|"error"|"conflict";
export type SaveSnapshot={draft:ItemDraft;item:WorkspaceItem;revision:number;status:SaveStatus;dirty:boolean;error?:Error;current?:WorkspaceItem};
type Save=(draft:ItemDraft,revision:number)=>Promise<WorkspaceItem>;
export class AutosaveController {
 private draft:ItemDraft;private item:WorkspaceItem;private version=0;private savedVersion=0;private status:SaveStatus="saved";private error?:Error;private current?:WorkspaceItem;private timer?:ReturnType<typeof setTimeout>;private inflight?:Promise<void>;private disposed=false;
 constructor(item:WorkspaceItem,private save:Save,private changed:(snapshot:SaveSnapshot)=>void=()=>{}){this.item=item;this.draft=itemDraft(item);}
 snapshot():SaveSnapshot {return {draft:this.draft,item:this.item,revision:this.item.revision,status:this.status,dirty:this.version!==this.savedVersion,error:this.error,current:this.current};}
 private emit(){if(!this.disposed)this.changed(this.snapshot());}
 edit(draft:ItemDraft){if(this.disposed)throw Error("Autosave is disposed");if(JSON.stringify(draft)===JSON.stringify(this.draft))return;this.draft={...draft,fields:{...draft.fields}};this.version++;if(!this.error)this.status="dirty";clearTimeout(this.timer);if(!this.error)this.timer=setTimeout(()=>void this.flush().catch(()=>undefined),600);this.emit();}
 receive(item:WorkspaceItem){if(item.revision<=this.item.revision||this.version!==this.savedVersion||this.inflight)return;this.item=item;this.draft=itemDraft(item);this.emit();}
 async flush():Promise<void>{clearTimeout(this.timer);if(this.error)throw this.error;if(this.inflight)return this.inflight;if(this.version===this.savedVersion)return;
  const drain=async()=>{while(this.version!==this.savedVersion){const version=this.version;const draft={...this.draft,fields:{...this.draft.fields}};this.status="saving";this.emit();try{const item=await this.save(draft,this.item.revision);this.item=item;this.savedVersion=version;if(this.version===version)this.draft=itemDraft(item);this.status=this.version===this.savedVersion?"saved":"dirty";this.emit();}catch(error){this.error=error instanceof Error?error:Error(String(error));this.current=error instanceof WorkspaceConflict?error.current:undefined;this.status=this.current?"conflict":"error";this.emit();throw this.error;}}};
  this.inflight=drain();try{await this.inflight;}finally{this.inflight=undefined;}
 }
 async retry(){if(this.current)throw this.error;this.error=undefined;this.status="dirty";this.emit();return this.flush();}
 discardChanges(){if(this.inflight)throw Error("A save is still in progress");clearTimeout(this.timer);this.draft=itemDraft(this.item);this.version++;this.savedVersion=this.version;this.error=undefined;this.current=undefined;this.status="saved";this.emit();}
 useServer(){if(!this.current)return;this.item=this.current;this.draft=itemDraft(this.current);this.version++;this.savedVersion=this.version;this.error=undefined;this.current=undefined;this.status="saved";this.emit();}
 async keepDraft(){if(!this.current)return;this.item=this.current;this.current=undefined;this.error=undefined;this.status="dirty";this.emit();return this.flush();}
 dispose(){clearTimeout(this.timer);this.disposed=true;}
}
