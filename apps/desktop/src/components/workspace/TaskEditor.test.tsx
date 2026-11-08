import {expect,it} from "vitest";
import {render,screen,fireEvent} from "@testing-library/react";
import {TaskEditor} from "./TaskEditor";
import {FakeService} from "../../rpc/fake";
import {ClientProvider} from "../../rpc/context";
import {resetWorkspace,workspaceCache,hasUnsavedWorkspaceDrafts} from "../../workspace/store";
it("does not silently choose an occurrence of a repeated local time",()=>{resetWorkspace();const fake=new FakeService();fake.open();const item={id:1,project_id:1,kind:"task",title:"Task",body:"",fields:{status:"pending"},revision:1,created_at:1,updated_at:1,deleted_at:null};workspaceCache.merge([item]);render(<ClientProvider client={fake.client}><TaskEditor item={item} timezone="Europe/Bucharest"/></ClientProvider>);fireEvent.change(screen.getByLabelText("Remind me"),{target:{value:"2026-10-25T03:30"}});expect(screen.getByText(/This time happens twice/)).toBeTruthy();expect(screen.getByText(/UTC\+03:00/)).toBeTruthy();expect(screen.getByText(/UTC\+02:00/)).toBeTruthy();expect(fake.calls("workspace.items.save")).toHaveLength(0);expect(hasUnsavedWorkspaceDrafts()).toBe(true);resetWorkspace();});
