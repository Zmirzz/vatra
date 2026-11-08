import {beforeEach,it,expect} from "vitest";
import {render,screen,waitFor,fireEvent} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {FakeService} from "../rpc/fake";
import {ClientProvider} from "../rpc/context";
import {initialState,useAppStore} from "../state/store";
import {Composer} from "../components/Composer";
import {PromptBox} from "../components/PromptBox";
import {FileInspector} from "./FileInspector";
import {AttachmentTray} from "./AttachmentTray";
import {Conversation} from "../components/Conversation";
import {useFiles,emptyFiles} from "./store";
import type {FileRecord} from "./client";
const file:FileRecord={id:7,session_id:3,name:"report.md",media_type:"text/markdown",origin:"attached",source_path:null,revision:1,created_at:1,size:5,hash:"hash",operation:null};
let service:FakeService;
beforeEach(()=>{
  service=new FakeService();service.open();
  service.results["files.list"]=[file];service.results["files.links"]=[];
  service.results["files.capabilities"]={images:"unsupported",document_text:true,context_tokens:32768,estimated:false};
  service.results["files.text"]={text:"# Local evidence"};service.results["files.versions"]=[{revision:1,hash:"hash",size:5,operation:null,created_at:1}];
  service.results["turn.start"]={};
  useAppStore.setState({...initialState(),activeSessionId:3,sessions:[{id:3,project_id:null,agent:"local",title:"test",updated_seq:0,last_turn:"none",running:false}]});
  useFiles.setState({sessions:{3:{...emptyFiles(),draft:[{id:7}]}},records:{3:[file]},links:{},edits:{},capabilities:{},busy:{},errors:{}});
});
it("sends attachments without a text prompt and preserves them when sending fails",async()=>{
  service.failures["turn.start"]={code:1001,message:"busy"};
  render(<ClientProvider client={service.client}><Composer sessionId={3}/></ClientProvider>);
  await userEvent.click(screen.getByRole("button",{name:"Send"}));
  await waitFor(()=>expect(screen.getByRole("alert").textContent).toContain("busy"));
  expect(useFiles.getState().sessions[3].draft).toEqual([{id:7}]);
  await userEvent.click(screen.getByRole("button",{name:"Send"}));
  await waitFor(()=>expect(useFiles.getState().sessions[3].draft).toEqual([]));
  expect(service.calls("turn.start")[1]).toEqual({session_id:3,prompt:"",mode:"supervised",attachments:[{id:7}]});
});
it("opens the selected attachment beside the same conversation and can close it",async()=>{
  useFiles.setState({sessions:{3:{...emptyFiles(),open:true,active:7,tabs:[7]}}});
  render(<ClientProvider client={service.client}><FileInspector sessionId={3}/></ClientProvider>);
  expect(screen.getByRole("complementary",{name:"Files"})).toBeTruthy();
  await waitFor(()=>expect(screen.getByRole("heading",{name:"Local evidence"})).toBeTruthy());
  expect(useAppStore.getState().activeSessionId).toBe(3);
  await userEvent.click(screen.getByRole("button",{name:/^Close files$/}));
  expect(screen.queryByRole("complementary")).toBeNull();
});
it("retains an unsaved file edit when the server rejects its revision",async()=>{
  useFiles.setState({sessions:{3:{...emptyFiles(),open:true,active:7,tabs:[7]}}});
  service.failures["files.save"]={code:-32602,message:"The file changed. Reload before saving."};
  render(<ClientProvider client={service.client}><FileInspector sessionId={3}/></ClientProvider>);
  await waitFor(()=>expect(screen.getByRole("heading",{name:"Local evidence"})).toBeTruthy());
  await userEvent.click(screen.getByRole("tab",{name:"Edit text"}));
  await userEvent.type(screen.getByRole("textbox",{name:"Edit text"})," unsaved");
  await userEvent.click(screen.getByRole("button",{name:/^Save$/}));
  await waitFor(()=>expect(screen.getByRole("alert").textContent).toContain("changed"));
  expect(useFiles.getState().edits["3/7"]).toEqual({text:"# Local evidence unsaved",revision:1});
});
it("shows verified artifacts from the catalog in the turn that produced them",async()=>{
  const write:FileRecord={...file,id:11,name:"report.md",origin:"created",operation:"tool-1",size:20};
  const shell:FileRecord={...file,id:12,name:"from-shell.txt",origin:"created",operation:"turn:1",size:17};
  service.results["files.list"]=[write,shell];service.results["files.links"]=[];
  useAppStore.setState({...initialState(),activeSessionId:3,sessions:[{id:3,project_id:null,agent:"local",title:"test",updated_seq:3,last_turn:"none",running:false}],
    eventsBySession:{3:[
      {seq:1,session_id:3,event:{type:"user_message",text:"make it"}},
      {seq:2,session_id:3,event:{type:"tool_call",id:"tool-1",name:"Write",arguments:{file_path:"report.md"}}},
      {seq:3,session_id:3,event:{type:"tool_result",id:"tool-1",output:"ok",is_error:false}},
    ]}});
  useFiles.setState({sessions:{3:emptyFiles()},records:{3:[write,shell]},links:{3:[]},edits:{},capabilities:{},busy:{},errors:{}});
  render(<ClientProvider client={service.client}><Conversation/></ClientProvider>);
  expect(screen.getByRole("button",{name:"report.md"})).toBeTruthy();
  expect(screen.getByRole("button",{name:"from-shell.txt"})).toBeTruthy();
});
it("warns that earlier images are kept but not sent to a route without vision",async()=>{
  const image:FileRecord={...file,id:9,name:"shot.png",media_type:"image/png",origin:"attached"};
  service.results["files.list"]=[image];service.results["files.links"]=[[1,9,1]];
  useFiles.setState({sessions:{3:emptyFiles()},records:{3:[image]},links:{3:[[1,9,1]]},edits:{},capabilities:{},busy:{},errors:{}});
  render(<ClientProvider client={service.client}><AttachmentTray sessionId={3}/></ClientProvider>);
  await waitFor(()=>expect(screen.getByRole("status").textContent).toContain("no confirmed image input"));
  expect(useFiles.getState().records[3].length).toBe(1);
});
it("accepts dropped files and clipboard images while leaving ordinary paste alone",()=>{
  const calls:File[][]=[];
  render(<ClientProvider client={service.client}><PromptBox value="" onChange={()=>{}} onSubmit={()=>{}} placeholder="Test composer" onFiles={files=>calls.push(files)}/></ClientProvider>);
  const input=screen.getByRole("textbox");const image=new File(["image"],"image.png",{type:"image/png"});const document=new File(["text"],"note.txt");
  fireEvent.drop(input.closest(".prompt")!,{dataTransfer:{files:[image,document]}});
  fireEvent.paste(input,{clipboardData:{items:[{kind:"file",getAsFile:()=>image}]}});
  fireEvent.paste(input,{clipboardData:{items:[{kind:"string"}]}});
  expect(calls).toEqual([[image,document],[image]]);
});
