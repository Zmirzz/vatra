import {render,screen} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {beforeEach,expect,test} from "vitest";
import {ClientProvider} from "../rpc/context";
import {FakeService} from "../rpc/fake";
import {defaults,updatePreferences} from "../settings/preferences";
import {SkillsPage} from "./SkillsPage";
beforeEach(()=>updatePreferences(defaults()));

test("skills stay separate from MCP tools and installation uses the reviewed hash",async()=>{
  const service=new FakeService();service.open();
  service.results["skills.sources"]=[];
  service.results["skills.scan"]=[{path:"C:/skills/notes",projectId:null,name:"notes",description:"Take notes",resources:["scripts/run.sh"],errors:[],hash:"reviewed-hash"}];
  service.results["skills.install"]={problems:[],unapproved:["notes"]};
  render(<ClientProvider client={service.client}><SkillsPage/></ClientProvider>);
  expect(await screen.findByText("Take notes")).toBeTruthy();
  await userEvent.click(screen.getByRole("button",{name:"Install notes"}));
  expect(service.calls("skills.install")[0]).toMatchObject({path:"C:/skills/notes",expectedHash:"reviewed-hash",projectId:null});
  expect(service.calls("market.install")).toHaveLength(0);
});

test("invalid skills cannot be installed and their errors are visible",async()=>{
  const service=new FakeService();service.open();service.results["skills.sources"]=[];
  service.results["skills.scan"]=[{path:"C:/skills/bad",projectId:null,name:"bad",description:"Invalid",resources:[],errors:["Duplicate skill name"],hash:null}];
  render(<ClientProvider client={service.client}><SkillsPage/></ClientProvider>);
  expect(await screen.findByText("Duplicate skill name")).toBeTruthy();
  expect(screen.getByRole("button",{name:"Install bad"}).hasAttribute("disabled")).toBe(true);
});

test("a missing skill source remains removable after a scan failure",async()=>{
  const service=new FakeService();service.open();
  service.results["skills.sources"]=[{path:"C:/missing",projectId:null}];
  service.failures["skills.scan"]={code:1000,message:"Cannot read skill source"};
  render(<ClientProvider client={service.client}><SkillsPage/></ClientProvider>);
  expect(await screen.findByText("C:/missing")).toBeTruthy();
  expect(screen.getByRole("button",{name:"Remove source"})).toBeTruthy();
});

test("a project-scoped installed skill is offered as an update",async()=>{
  const service=new FakeService();service.open();service.results["skills.sources"]=[];
  service.results["skills.scan"]=[{path:"C:/skills/notes",projectId:7,name:"notes",description:"Notes",resources:[],errors:[],hash:"new"}];
  service.results["catalog.list"]=[{id:1,kind:"skill",name:"notes",origin:"manual",plugin:null,scope:{project:7},enabled:true,content_hash:"old"}];
  render(<ClientProvider client={service.client}><SkillsPage/></ClientProvider>);
  expect(await screen.findByRole("button",{name:"Update notes"})).toBeTruthy();
});
