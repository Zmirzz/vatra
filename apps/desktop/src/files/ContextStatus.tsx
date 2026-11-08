import {useAppStore} from "../state/store";
import {usePreferences} from "../settings/preferences";
import {useFiles} from "./store";
export function ContextStatus({sessionId}:{sessionId:number}){
  const ro=usePreferences(s=>s.preferences.language)==="ro";
  const session=useAppStore(s=>s.sessions.find(s=>s.id===sessionId));
  const events=useAppStore(s=>s.eventsBySession[sessionId]);const caps=useFiles(s=>s.capabilities[sessionId]);
  const usage=[...(events??[])].reverse().find(e=>e.event.type==="usage")?.event;
  const tokens=usage?.type==="usage"?usage.input_tokens:0;
  if(caps&&!Number.isFinite(caps.context_tokens))return null;
  return <div className="context-status" aria-live="polite"><span><i data-running={session?.running}/>{session?.running?(ro?"Inferență în curs":"Running"):(ro?"Pregătit":"Ready")}</span>{caps?<span title={ro?"Utilizarea raportată pentru ultima cerere; nu include ciorna curentă.":"Reported input for the last request; excludes the current draft."}>{caps.estimated?(tokens>0?<>{tokens.toLocaleString()} tokens</>:null):<>{tokens>0?<>{tokens.toLocaleString()} / </>:null}{caps.context_tokens.toLocaleString()} tokens{tokens>0?<meter aria-label={ro?"Contextul ultimei cereri":"Last request context"} min={0} max={caps.context_tokens} value={Math.min(tokens,caps.context_tokens)}/>:null}</>}</span>:null}</div>;
}
