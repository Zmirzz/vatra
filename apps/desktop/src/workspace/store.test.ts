import {expect,it} from "vitest";
import {WorkspaceCache} from "./store";
import {markWorkspaceBlocked,hasUnsavedWorkspaceDrafts,flushWorkspace,resetWorkspace} from "./store";
import type {WorkspaceItem} from "./types";
const item=(project:number,id:number,body:string,revision=1):WorkspaceItem=>({project_id:project,id,kind:"note",body,title:body,revision,fields:{},deleted_at:null,created_at:1,updated_at:1});
it("keeps project responses and drafts separate and ignores older server revisions",()=>{
 const cache=new WorkspaceCache();cache.select({projectId:2,area:"documents",itemId:2});cache.merge([item(2,2,"B",2)]);cache.setDraft(2,2,{title:"B",body:"my B",fields:{}});cache.merge([item(1,1,"late A"),item(2,2,"old B",1)]);
 expect(cache.selection.projectId).toBe(2);expect(cache.get(2,2)?.body).toBe("B");expect(cache.draft(2,2)?.body).toBe("my B");expect(cache.get(1,1)?.body).toBe("late A");
});
it("does not replace a loaded source with a truncated list preview of the same revision",()=>{
 const cache=new WorkspaceCache();cache.merge([item(1,1,"complete source",2)]);cache.merge([{...item(1,1,"preview",2),body_truncated:true}]);expect(cache.get(1,1)?.body).toBe("complete source");
});
it("blocks navigation and closing while a reminder form has an unresolved local time",async()=>{resetWorkspace();markWorkspaceBlocked(1,2,true);expect(hasUnsavedWorkspaceDrafts()).toBe(true);await expect(flushWorkspace()).rejects.toThrow();markWorkspaceBlocked(1,2,false);await expect(flushWorkspace()).resolves.toBeUndefined();resetWorkspace();});
