import {useCallback,useState} from "react";
import {Sparkles} from "lucide-react";
import {copy} from "../../copy";
import type {DocumentFormat,Selection,WorkspaceItem} from "../../workspace/types";
import type {AutosaveController} from "../../workspace/autosave";
import {WorkspaceItemEditor} from "./DocumentsPage";
import {DocumentEditor} from "./DocumentEditor";
import {WorkspaceAgentPanel} from "./WorkspaceAgentPanel";
import {ItemLinks} from "./ItemLinks";
import {ExportDocument} from "./DocumentFiles";
export function DocumentBody({item,controller}:{item:WorkspaceItem;controller:AutosaveController}){const [open,setOpen]=useState(false);const [selection,setSelection]=useState<Selection|null>(null);const [unavailable,setUnavailable]=useState(false);const close=useCallback(()=>setOpen(false),[]);const draft=controller.snapshot().draft;const format=(draft.fields.format??"markdown") as DocumentFormat;
 return <><div className="workspace-document-actions"><details><summary>{copy.workspace.fileActions}</summary><ExportDocument item={{...item,fields:draft.fields}} controller={controller}/><label>{copy.workspace.format}<select aria-label={copy.workspace.format} value={format} disabled={draft.body.length>0} onChange={e=>{setSelection(null);setUnavailable(false);controller.edit({...draft,fields:{...draft.fields,format:e.target.value}});}}>{(item.kind==="note"?["text","markdown"]:["text","markdown","html","csv"]).map(f=><option value={f} key={f}>{f==="text"?copy.workspace.plainText:copy.workspace[f as "markdown"|"html"|"csv"]}</option>)}</select></label></details><button type="button" className="quiet" onClick={()=>setOpen(true)}><Sparkles size={15}/>{copy.workspace.askAgent}</button></div><DocumentEditor body={draft.body} format={format} onChange={body=>controller.edit({...controller.snapshot().draft,body})} onSelection={(selection,unavailable)=>{setSelection(selection);setUnavailable(unavailable);}}/><ItemLinks item={item}/><WorkspaceAgentPanel open={open} item={item} controller={controller} selection={selection} selectionUnavailable={unavailable} onClose={close}/></>;
}
export function DocumentSurface({item}:{item:WorkspaceItem}){return <WorkspaceItemEditor item={item}>{controller=><DocumentBody item={item} controller={controller}/>}</WorkspaceItemEditor>;}
