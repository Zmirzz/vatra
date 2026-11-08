import {expect,it} from "vitest";
import {render,screen,waitFor} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {FakeService} from "../../rpc/fake";
import {ClientProvider} from "../../rpc/context";
import {ItemLinks} from "./ItemLinks";
it("finds and links an older item beyond the initial hundred candidates",async()=>{
 const fake=new FakeService();fake.open();fake.results["workspace.links.list"]=[];fake.results["workspace.items.list"]=Array.from({length:100},(_,i)=>({id:i+2,title:`Recent ${i}`}));fake.results["workspace.search"]=[{id:1,title:"Oldest matching note",kind:"note",snippet:"older"}];fake.results["workspace.links.add"]={};
 const item={id:500,project_id:7,kind:"note" as const,title:"Current",body:"",fields:{},revision:1,created_at:1,updated_at:1,deleted_at:null};render(<ClientProvider client={fake.client}><ItemLinks item={item}/></ClientProvider>);
 await userEvent.click(screen.getByText("Linked items"));await userEvent.click(screen.getByRole("button",{name:"Link an item"}));await userEvent.type(screen.getByRole("searchbox",{name:"Search this project"}),"Oldest");await screen.findByRole("option",{name:"Oldest matching note"});
 await userEvent.selectOptions(screen.getByRole("combobox",{name:"Link an item"}),"1");await userEvent.click(screen.getByRole("button",{name:"Link an item"}));await waitFor(()=>expect(fake.calls("workspace.links.add")).toEqual([{project_id:7,from_id:500,to_id:1,rel:"related"}]));expect(fake.calls("workspace.search").at(-1)).toMatchObject({project_id:7,query:"Oldest"});
});
