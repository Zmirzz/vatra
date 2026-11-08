import {MessageCircle,Files,ListChecks,CalendarClock,CalendarDays,Mail,Search,Scale} from "lucide-react";
import {useHubCopy} from "../../hub/copy";
import {copy} from "../../copy";
import {useAppStore} from "../../state/store";
import {navigateWorkspace,useWorkspace} from "../../workspace/store";
import type {WorkspaceArea} from "../../workspace/types";
export function WorkspaceNav(){const hub=useHubCopy();const {cache}=useWorkspace();const projects=useAppStore(s=>s.projects);const session=useAppStore(s=>s.sessions.find(c=>c.id===s.activeSessionId));
 const go=(area:WorkspaceArea)=>void navigateWorkspace({area,projectId:cache.selection.projectId??session?.project_id??projects[0]?.id??null,itemId:area===cache.selection.area?cache.selection.itemId:null});
 return <div className="workspace-nav" role="navigation" aria-label="Vatra">{([["conversations",MessageCircle],["documents",Files],["tasks",ListChecks],["schedule",CalendarClock],["calendar",CalendarDays],["email",Mail],["research",Search],["compare",Scale]] as const).map(([area,Icon])=><button type="button" key={area} aria-current={cache.selection.area===area?"page":undefined} onClick={()=>go(area)}><Icon size={16} aria-hidden="true"/>{area==="conversations"||area==="documents"||area==="tasks"?copy.workspace[area]:hub[area]}</button>)}</div>;
}
