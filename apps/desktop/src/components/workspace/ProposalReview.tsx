import {useEffect,useMemo,useState} from "react";
import {copy} from "../../copy";
import {lineDiff} from "../../conversation/diff";
import {DiffView} from "../DiffView";
import {WorkspaceConflict,type AutosaveController} from "../../workspace/autosave";
import type {WorkspaceClient} from "../../workspace/client";
import type {WorkspaceProposal} from "../../workspace/types";
import {workspaceCache} from "../../workspace/store";
export function ProposalReview({proposal,controller,client,onChanged}:{proposal:WorkspaceProposal;controller:AutosaveController;client:WorkspaceClient;onChanged:()=>void}){
 const [busy,setBusy]=useState(false);const [error,setError]=useState<"stale"|"failed"|null>(null);const [state,setState]=useState(proposal.state);const w=copy.workspace;
 useEffect(()=>setState(proposal.state),[proposal.state]);
 const payload=proposal.payload;const diff=useMemo(()=>payload.type==="edit"?lineDiff(payload.base_body.slice(0,80000),payload.new_body.slice(0,80000)):[],[payload]);
 const truncated=payload.type==="edit"&&(payload.base_body.length>80000||payload.new_body.length>80000||diff.length>400||diff.some(line=>line.text.length>10000));
 const stale=error==="stale"||(proposal.base_revision!==null&&proposal.base_revision!==controller.snapshot().revision);
 const apply=async()=>{setBusy(true);setError(null);try{await controller.flush();const result=await client.accept(proposal.project_id,proposal.id);workspaceCache.merge(result.items);const item=result.items.find(i=>i.id===proposal.target_id);if(item)controller.receive(item);setState("applied");onChanged();}catch(error){setError(error instanceof WorkspaceConflict?"stale":"failed");}finally{setBusy(false);}};
 const discard=async()=>{setBusy(true);setError(null);try{await client.reject(proposal.project_id,proposal.id);setState("rejected");onChanged();}catch{setError("failed");}finally{setBusy(false);}};
 return <section className="workspace-proposal" aria-label={w.suggestions}><h3>{w.suggestions}</h3>{payload.type==="edit"?<><DiffView lines={diff}/>{truncated?<p className="workspace-format-help">{w.truncatedDiff}</p>:null}</>:<ul>{payload.items.map((item,index)=><li key={index}><b>{item.title}</b>{item.body?<p>{item.body.slice(0,500)}</p>:null}</li>)}</ul>}{state==="pending"?<>{stale?<p role="alert">{w.staleProposal}</p>:error?<p role="alert">{w.actionError}</p>:null}<div className="workspace-actions"><button type="button" className="primary" disabled={busy||stale} onClick={()=>void apply()}>{w.apply}</button><button type="button" disabled={busy} onClick={()=>void discard()}>{w.discard}</button></div></>:<p role="status">{state==="applied"?w.applied:w.discarded}</p>}</section>;
}
