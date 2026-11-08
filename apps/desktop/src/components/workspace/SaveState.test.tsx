import {expect,it,vi} from "vitest";
import {render,screen} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {AutosaveController} from "../../workspace/autosave";
import {SaveState} from "./SaveState";
it("offers explicit confirmed recovery when a concurrent deletion makes saving fail",async()=>{
 const item={id:1,project_id:1,kind:"note" as const,title:"Note",body:"saved",fields:{},revision:1,created_at:1,updated_at:1,deleted_at:null};const save=vi.fn().mockRejectedValue(Error("not_found"));const controller=new AutosaveController(item,save);controller.edit({...item,body:"my recoverable draft"});await expect(controller.flush()).rejects.toThrow("not_found");render(<SaveState controller={controller}/>);
 const text=screen.getByRole("textbox",{name:"Your text"}) as HTMLTextAreaElement;expect(text.value).toBe("my recoverable draft");expect(text.readOnly).toBe(true);
 const confirm=vi.spyOn(window,"confirm").mockReturnValue(false);await userEvent.click(screen.getByRole("button",{name:"Discard unsaved changes"}));expect(controller.snapshot().dirty).toBe(true);
 confirm.mockReturnValue(true);await userEvent.click(screen.getByRole("button",{name:"Discard unsaved changes"}));await expect(controller.flush()).resolves.toBeUndefined();expect(controller.snapshot().dirty).toBe(false);expect(save).toHaveBeenCalledTimes(1);confirm.mockRestore();controller.dispose();
});
