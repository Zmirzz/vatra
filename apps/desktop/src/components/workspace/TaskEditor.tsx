import {useCallback,useEffect,useMemo,useState} from "react";
import {Sparkles} from "lucide-react";
import {copy} from "../../copy";
import type {WorkspaceItem} from "../../workspace/types";
import type {AutosaveController} from "../../workspace/autosave";
import {formatWallTime,localTimeCandidates,localTimezone,offsetLabel,validDateOnly} from "../../workspace/dates";
import {markWorkspaceBlocked} from "../../workspace/store";
import {WorkspaceItemEditor} from "./DocumentsPage";
import {ItemLinks} from "./ItemLinks";
import {WorkspaceAgentPanel} from "./WorkspaceAgentPanel";

function TaskFields({item,controller,timezone}:{item:WorkspaceItem;controller:AutosaveController;timezone:string}){
 const draft=controller.snapshot().draft;const fields=draft.fields;const status=typeof fields.status==="string"?fields.status:"pending";const w=copy.workspace;
 const displayTime=(value:unknown)=>{try{return typeof value==="number"?formatWallTime(value,timezone):"";}catch{return "";}};const initial=()=>displayTime(fields.remind_at);
 const [reminderText,setReminderText]=useState(initial);const [pending,setPending]=useState(false);const [expanded,setExpanded]=useState(()=>!!fields.due_date||!!fields.remind_at);const [open,setOpen]=useState(false);const close=useCallback(()=>setOpen(false),[]);
 const choices=useMemo(()=>{try{return reminderText?localTimeCandidates(reminderText,timezone):[];}catch{return [];}},[reminderText,timezone]);
 useEffect(()=>()=>markWorkspaceBlocked(item.project_id,item.id,false),[item.project_id,item.id]);
 useEffect(()=>{if(!pending)setReminderText(displayTime(fields.remind_at));},[fields.remind_at,timezone,pending]);
 const block=(value:boolean)=>{setPending(value);markWorkspaceBlocked(item.project_id,item.id,value);};
 const patch=(patch:Record<string,unknown>)=>{const current=controller.snapshot().draft;controller.edit({...current,fields:{...current.fields,...patch}});};
 const choose=(instant:number)=>{patch({remind_at:instant,timezone});block(false);};
 const changeTime=(value:string)=>{setReminderText(value);if(!value){patch({remind_at:null,timezone:null});block(false);return;}let matches:ReturnType<typeof localTimeCandidates>=[];try{matches=localTimeCandidates(value,timezone);}catch{/* An invalid or unsupported zone stays an unresolved form. */}if(matches.length===1)choose(matches[0].instant);else block(true);};
 return <div className="workspace-task-editor"><div className="workspace-task-status"><label>{w.taskStatus}<select aria-label={w.taskStatus} value={status} onChange={e=>{patch({status:e.target.value});if(e.target.value==="completed")block(false);}}>{(["pending","in_progress","completed"] as const).map(value=><option key={value} value={value}>{w[value]}</option>)}</select></label><button type="button" className="quiet" onClick={()=>setOpen(true)}><Sparkles size={15}/>{w.askAgent}</button></div><label className="workspace-task-description">{w.taskDescription}<textarea rows={5} value={draft.body} onChange={e=>controller.edit({...controller.snapshot().draft,body:e.target.value})}/></label>
 <details open={expanded} onToggle={e=>setExpanded(e.currentTarget.open)}><summary>{w.optionalDetails}</summary><label>{w.dueDate}<input type="date" value={typeof fields.due_date==="string"?fields.due_date:""} onChange={e=>{if(!e.target.value||validDateOnly(e.target.value))patch({due_date:e.target.value||null});}}/></label><label>{w.remindMe}<input type="datetime-local" value={reminderText} disabled={status==="completed"} onChange={e=>changeTime(e.target.value)}/></label><small>{w.timezone}: {timezone}</small><p className="workspace-agent-help">{w.reminderHelp}</p>{pending&&choices.length===0?<p role="alert">{w.invalidTime}</p>:null}{choices.length>1?<fieldset className="workspace-time-choice"><legend>{w.ambiguousTime}</legend>{choices.map((choice,index)=><label key={choice.instant}><input type="radio" name={`reminder-${item.id}`} checked={!pending&&fields.remind_at===choice.instant} onChange={()=>choose(choice.instant)} disabled={status==="completed"}/>{index===0?w.firstTime:w.secondTime} · {offsetLabel(choice.offsetMinutes)}</label>)}</fieldset>:null}{status==="completed"&&fields.remind_at!==null&&fields.remind_at!==undefined?<small>{w.cancelled}</small>:null}{reminderText?<button type="button" className="quiet" onClick={()=>changeTime("")}>{w.clearReminder}</button>:null}</details><ItemLinks item={item}/><WorkspaceAgentPanel open={open} item={item} controller={controller} selection={null} selectionUnavailable={false} onClose={close}/></div>;
}
export function TaskEditor({item,timezone=localTimezone()}:{item:WorkspaceItem;timezone?:string}){return <WorkspaceItemEditor item={item}>{controller=><TaskFields item={item} controller={controller} timezone={timezone}/>}</WorkspaceItemEditor>;}
