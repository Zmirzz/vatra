import {afterEach,describe,expect,it,vi} from "vitest";
import {AutosaveController,WorkspaceConflict} from "./autosave";
import type {WorkspaceItem,ItemDraft} from "./types";
const item:WorkspaceItem={id:1,project_id:1,kind:"note",title:"Note",body:"original",fields:{},revision:1,deleted_at:null,created_at:1,updated_at:1};
const draft=(body:string):ItemDraft=>({title:"Note",body,fields:{}});
const deferred=<T>()=>{let resolve!:(value:T)=>void;let reject!:(error:unknown)=>void;const promise=new Promise<T>((a,b)=>{resolve=a;reject=b;});return {promise,resolve,reject};};
afterEach(()=>vi.useRealTimers());
describe("autosave",()=>{
 it("serializes A/B edits and uses the returned revision without replacing the newer draft",async()=>{
  vi.useFakeTimers();const first=deferred<WorkspaceItem>();const second=deferred<WorkspaceItem>();const save=vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);const controller=new AutosaveController(item,save);
  controller.edit(draft("A"));const flush=controller.flush();expect(save).toHaveBeenCalledTimes(1);controller.edit(draft("B"));first.resolve({...item,body:"A",revision:2});await Promise.resolve();await Promise.resolve();
  expect(controller.snapshot().draft.body).toBe("B");expect(save).toHaveBeenNthCalledWith(2,draft("B"),2);second.resolve({...item,body:"B",revision:3});await flush;expect(controller.snapshot().status).toBe("saved");expect(controller.snapshot().revision).toBe(3);controller.dispose();
 });
 it("keeps the draft and server version on conflict and never retries on its own",async()=>{
  vi.useFakeTimers();const current={...item,body:"remote",revision:2};const save=vi.fn().mockRejectedValue(new WorkspaceConflict(current));const controller=new AutosaveController(item,save);controller.edit(draft("mine"));await expect(controller.flush()).rejects.toBeInstanceOf(WorkspaceConflict);await vi.advanceTimersByTimeAsync(5000);
  expect(save).toHaveBeenCalledTimes(1);expect(controller.snapshot()).toMatchObject({draft:{body:"mine"},current:{body:"remote"},status:"conflict"});controller.receive({...current,body:"reconnected"});expect(controller.snapshot().draft.body).toBe("mine");controller.dispose();
 });
 it("retains unsaved text through errors and retries only when asked",async()=>{
  vi.useFakeTimers();const save=vi.fn().mockRejectedValueOnce(Error("offline")).mockResolvedValue({...item,body:"mine",revision:2});const controller=new AutosaveController(item,save);controller.edit(draft("mine"));await expect(controller.flush()).rejects.toThrow("offline");expect(controller.snapshot().draft.body).toBe("mine");await controller.retry();expect(controller.snapshot().status).toBe("saved");controller.dispose();
 });
 it("refreshes a clean item, but dispose cancels a pending debounce without pretending it was saved",async()=>{
  vi.useFakeTimers();const save=vi.fn();const controller=new AutosaveController(item,save);controller.receive({...item,body:"remote",revision:2});expect(controller.snapshot().draft.body).toBe("remote");controller.edit(draft("unsaved"));controller.dispose();await vi.advanceTimersByTimeAsync(1000);expect(save).not.toHaveBeenCalled();expect(controller.snapshot().status).toBe("dirty");
 });
});
