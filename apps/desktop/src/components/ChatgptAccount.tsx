import { useEffect, useState } from "react";
import { copy } from "../copy";
import { useClient } from "../rpc/context";
type Status={connected:boolean;sharing:boolean;account:string|null;connecting?:boolean};
type Account={id:string;account:string|null;active:boolean};
export function ChatgptAccount(){
  const client=useClient();const [status,setStatus]=useState<Status|null>(null);const [busy,setBusy]=useState(false);const [message,setMessage]=useState<string|null>(null);
  const [accounts,setAccounts]=useState<Account[]>([]);
  const refreshAccounts=()=>client.call<Account[]>("chatgpt.accounts",{}).then(values=>setAccounts(Array.isArray(values)?values:[]));
  useEffect(()=>{let live=true;const refresh=()=>client.call<Account[]>("chatgpt.accounts",{}).then(values=>{if(live&&Array.isArray(values))setAccounts(values);}).catch(()=>undefined);void refresh();const off=client.on(notice=>{if(notice.method==="chatgpt_login")void refresh();});return()=>{live=false;off();};},[client]);
  useEffect(()=>{let active=true;const refresh=()=>client.call<Status>("chatgpt.status",{}).then(value=>{if(active){setStatus(value);if(value.connecting)setBusy(true);}}).catch(error=>{if(active)setMessage(error.message);});void refresh();const off=client.on(notice=>{if(notice.method==="chatgpt_login"){setBusy(false);setMessage(notice.params.message);void refresh();}});return()=>{active=false;off();};},[client]);
  const action=async(method:string)=>{setMessage(null);try{const result=await client.call<{revoked?:boolean}>(method,{});if(method==="chatgpt.login")setBusy(true);else{setBusy(false);setStatus(await client.call<Status>("chatgpt.status",{}));await refreshAccounts();if(result.revoked===false)setMessage(copy.chatgpt.revocation);}}catch(error){setMessage((error as Error).message);}};
  return <section><h3>{copy.chatgpt.title}</h3><p className="help">{copy.chatgpt.help}</p>
    {status?.connected?<><p>{status.account ?? copy.chatgpt.connected}</p><p>{status.sharing?copy.chatgpt.enabled:copy.chatgpt.needsPermission}</p></>:null}
    {accounts.length?<label className="setting">{copy.accounts.choose}<select aria-label={copy.accounts.choose} disabled={busy} value={accounts.find(account=>account.active)?.id??""} onChange={async event=>{setBusy(true);setMessage(null);try{await client.call("chatgpt.select",{id:event.target.value});setStatus(await client.call<Status>("chatgpt.status",{}));await refreshAccounts();}catch(error){setMessage((error as Error).message);}finally{setBusy(false);}}}><option value="" disabled>—</option>{accounts.map(account=><option key={account.id} value={account.id}>{account.account??account.id}</option>)}</select></label>:null}
    {message?<p role="status">{message}</p>:null}
    {busy?<><p>{copy.chatgpt.waiting}</p><button type="button" onClick={()=>void action("chatgpt.cancel")}>{copy.cancel}</button></>:<>
      <button type="button" data-oai-siwc-button onClick={()=>void action("chatgpt.login")}>{copy.chatgpt.connect}</button>
      {status?.connected?<button type="button" className="quiet" onClick={()=>void action("chatgpt.logout")}>{copy.chatgpt.disconnect}</button>:null}</>}
  </section>;
}
