import {useEffect,useState} from "react";
import {copy} from "../../copy";
import {localTimezone,todayInZone,type TaskView} from "../../workspace/dates";
import {DocumentsPage} from "./DocumentsPage";
import {TaskEditor} from "./TaskEditor";
import {RemindersInbox} from "./RemindersInbox";
export function TasksPage({projectId}:{projectId:number}){const [view,setView]=useState<TaskView>("today");const [searching,setSearching]=useState(false);const [today,setToday]=useState(()=>todayInZone(Math.floor(Date.now()/1000),localTimezone()));useEffect(()=>{const tick=setInterval(()=>setToday(todayInZone(Math.floor(Date.now()/1000),localTimezone())),60000);return()=>clearInterval(tick);},[]);
 const controls=<><RemindersInbox projectId={projectId}/>{!searching?<div className="workspace-task-views">{(["today","upcoming","completed"] as const).map(value=><button type="button" key={value} aria-pressed={view===value} onClick={()=>setView(value)}>{value==="completed"?copy.workspace.done:copy.workspace[value]}</button>)}</div>:null}</>;
 return <DocumentsPage projectId={projectId} taskMode taskView={view} today={today} filters={controls} onSearch={setSearching} onCreated={()=>setView("upcoming")} renderEditor={item=><TaskEditor key={item.id} item={item}/>}/>;
}
