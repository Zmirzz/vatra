import {render,screen} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {beforeEach,expect,test} from "vitest";
import {ClientProvider} from "../rpc/context";
import {FakeService} from "../rpc/fake";
import {defaults,updatePreferences} from "../settings/preferences";
import {InstalledMcpPage} from "./InstalledMcpPage";
beforeEach(()=>updatePreferences(defaults()));
test("opening MCP management starts no servers and uninstall requires confirmation",async()=>{
 const service=new FakeService();service.open();
 service.results["catalog.list"]=[{id:7,kind:"mcp_server",name:"demo",origin:"manual",scope:"global",enabled:false,runs_code:[],content_hash:"hash",approved_hash:null,plugin:null,details:{url:"https://example.org/mcp",disabled_tools:[]}}];
 render(<ClientProvider client={service.client}><InstalledMcpPage/></ClientProvider>);
 expect(await screen.findByText("demo")).toBeTruthy();
 expect(service.calls("mcp.test")).toHaveLength(0);expect(service.calls("mcp.connect")).toHaveLength(0);
 await userEvent.click(screen.getByRole("button",{name:"Uninstall demo"}));
 expect(service.calls("mcp.remove")).toHaveLength(0);
 await userEvent.click(screen.getByRole("button",{name:"Confirm remove demo"}));
 expect(service.calls("mcp.remove")[0]).toMatchObject({id:7,expectedHash:"hash"});
});
