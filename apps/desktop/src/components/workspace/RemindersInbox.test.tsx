import {beforeEach,expect,it} from "vitest";
import {render,screen,waitFor} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {FakeService} from "../../rpc/fake";
import {ClientProvider} from "../../rpc/context";
import {initialState,useAppStore} from "../../state/store";
import {RemindersInbox} from "./RemindersInbox";
beforeEach(()=>useAppStore.setState({...initialState(),projects:[{id:1,name:"p",path:null}]}));
it("shows a durable blocked reminder and dismisses only its inbox entry",async()=>{const fake=new FakeService();fake.open();fake.results["reminders.status"]={state:"disabled"};fake.results["reminders.list"]=[{id:4,item_id:8,project_id:1,generation:2,due_at:100,state:"blocked",attempted_at:100,result:"blocked"}];fake.results["workspace.items.get"]={id:8,project_id:1,title:"Call Ana",body:"",fields:{},kind:"task",revision:1,deleted_at:null};render(<ClientProvider client={fake.client}><RemindersInbox projectId={1}/></ClientProvider>);await screen.findByText("Call Ana");expect(screen.getByText(/Windows notifications are off/)).toBeTruthy();await userEvent.click(screen.getByRole("button",{name:"Dismiss"}));await waitFor(()=>expect(fake.calls("reminders.ack")).toEqual([{project_id:1,id:4,generation:2}]));expect(fake.calls("workspace.items.save")).toHaveLength(0);expect(fake.calls("workspace.ai.start")).toHaveLength(0);});
