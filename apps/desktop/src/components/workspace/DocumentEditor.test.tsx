import {beforeAll,expect,it,vi} from "vitest";
import {fireEvent,render,screen} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {DocumentEditor} from "./DocumentEditor";
beforeAll(()=>{
 // jsdom has no layout engine. Only geometry is supplied; Tiptap commands and document mutations are real.
 Object.defineProperty(Range.prototype,"getClientRects",{configurable:true,value:()=>[]});
 Object.defineProperty(Range.prototype,"getBoundingClientRect",{configurable:true,value:()=>new DOMRect()});
});
it("opens and previews HTML without saving or inserting resource URLs",async()=>{const change=vi.fn();render(<DocumentEditor body='<p>Safe</p><img src="https://outside.invalid">' format="html" onChange={change}/>);expect(change).not.toHaveBeenCalled();const frame=screen.getByTitle("Preview");expect(frame.getAttribute("sandbox")).toBe("");expect(frame.getAttribute("srcdoc")).not.toContain("outside.invalid");await userEvent.click(screen.getByRole("button",{name:"Source"}));expect(change).not.toHaveBeenCalled();expect((screen.getByRole("textbox",{name:"Source"}) as HTMLTextAreaElement).value).toContain("<img");});
it("keeps unsupported Markdown intact until a real source edit",async()=>{const change=vi.fn();const body="![pic](https://outside.invalid/a)";render(<DocumentEditor body={body} format="markdown" onChange={change}/>);expect(change).not.toHaveBeenCalled();await userEvent.click(screen.getByRole("button",{name:"Source"}));const source=screen.getByRole("textbox",{name:"Source"});expect((source as HTMLTextAreaElement).value).toBe(body);fireEvent.change(source,{target:{value:body+"!"}});expect(change).toHaveBeenLastCalledWith(body+"!");});
it("reports a text selection with an emoji using exact source offsets",()=>{const selection=vi.fn();render(<DocumentEditor body="a😀b" format="text" onChange={()=>{}} onSelection={selection}/>);const source=screen.getByRole("textbox",{name:"Text"}) as HTMLTextAreaElement;source.setSelectionRange(1,3);fireEvent.select(source);expect(selection).toHaveBeenLastCalledWith({start_utf16:1,end_utf16:3,selected_text:"😀"},false);});
it("preserves a Markdown table with optional outer pipes on the source route",async()=>{
 const body="Name | Value\n--- | ---\nAlice | 2";const change=vi.fn();render(<DocumentEditor body={body} format="markdown" onChange={change}/>);
 expect(screen.queryByRole("button",{name:"Edit"})).toBeNull();expect(screen.queryByRole("textbox",{name:"Text"})).toBeNull();expect(change).not.toHaveBeenCalled();
 await userEvent.click(screen.getByRole("button",{name:"Source"}));const source=screen.getByRole("textbox",{name:"Source"});expect((source as HTMLTextAreaElement).value).toBe(body);
 fireEvent.change(source,{target:{value:body+"\nBob | 3"}});expect(change).toHaveBeenLastCalledWith(body+"\nBob | 3");
});
it("offers real bold/list and undo commands without saving a programmatic load",async()=>{
 const change=vi.fn();render(<DocumentEditor body="Hello" format="markdown" onChange={change}/>);const editor=await screen.findByRole("textbox",{name:"Text"});expect(change).not.toHaveBeenCalled();
 await userEvent.click(screen.getByRole("button",{name:"Bullet list"}));expect(editor.querySelector("ul")).not.toBeNull();expect(change).toHaveBeenCalled();await userEvent.click(screen.getByRole("button",{name:"Undo"}));expect(editor.querySelector("ul")).toBeNull();
 fireEvent.keyDown(editor,{key:"a",ctrlKey:true});await userEvent.click(screen.getByRole("button",{name:"Bold"}));expect(editor.querySelector("strong")).not.toBeNull();
});
