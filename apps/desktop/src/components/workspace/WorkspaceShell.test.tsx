import {beforeEach,expect,it} from "vitest";
import {render,screen,waitFor} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {FakeService} from "../../rpc/fake";
import {ClientProvider} from "../../rpc/context";
import {initialState,useAppStore} from "../../state/store";
import {resetWorkspace,workspaceCache} from "../../workspace/store";
import {WorkspaceShell} from "./WorkspaceShell";
beforeEach(()=>{resetWorkspace();useAppStore.setState({...initialState(),projects:[{id:1,name:"Personal",path:null}],agentsLoaded:true});});
it("opens an empty project without making an agent request",async()=>{
 const fake=new FakeService();fake.results["workspace.items.list"]=[];fake.client.connect();fake.open();workspaceCache.select({projectId:1,area:"documents",itemId:null});render(<ClientProvider client={fake.client}><WorkspaceShell/></ClientProvider>);
 await screen.findByText("A place for your words");expect((screen.getByRole("button",{name:"New document"}) as HTMLButtonElement).disabled).toBe(false);expect(fake.sent.some(c=>c.method==="turn.start"||c.method==="workspace.ai.start")).toBe(false);
});
it("edits plain text and flushes before returning to the list without replacing a newer draft",async()=>{
 const fake=new FakeService();const item={id:11,project_id:1,kind:"note",title:"Trip",body:"old",fields:{format:"text"},revision:1,deleted_at:null,created_at:1,updated_at:1};fake.results["workspace.items.list"]=[item];fake.results["workspace.items.get"]=item;fake.results["workspace.items.save"]={...item,body:"new",revision:2};fake.open();workspaceCache.select({projectId:1,area:"documents",itemId:11});render(<ClientProvider client={fake.client}><WorkspaceShell/></ClientProvider>);
 const text=await screen.findByRole("textbox",{name:"Text"});await userEvent.clear(text);await userEvent.type(text,"new");await userEvent.click(screen.getByRole("button",{name:"Back to list"}));
 await waitFor(()=>expect(workspaceCache.selection.itemId).toBeNull());const calls=fake.sent.filter(c=>c.method==="workspace.items.save");expect(calls.at(-1)?.params.body).toBe("new");expect(calls[0].params.expected_revision).toBe(1);
});
it("searches using previews without downloading every full document",async()=>{
 const fake=new FakeService();fake.results["workspace.items.list"]=[];fake.results["workspace.search"]=[{id:21,kind:"note",title:"Budget",snippet:"some text",updated_at:1}];fake.open();workspaceCache.select({projectId:1,area:"documents",itemId:null});render(<ClientProvider client={fake.client}><WorkspaceShell/></ClientProvider>);await userEvent.type(screen.getByRole("searchbox",{name:"Search this project"}),"Budget");await screen.findByText("Budget");expect(fake.calls("workspace.items.get")).toHaveLength(0);
});
