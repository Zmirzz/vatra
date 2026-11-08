import {useEffect,useMemo,useRef,useState} from "react";
import {ChevronLeft,Download,FolderOpen,Maximize2,Minimize2,PanelRightClose,Search,X} from "lucide-react";
import Papa from "papaparse";
import {useClient} from "../rpc/context";
import {useAppStore} from "../state/store";
import {usePreferences} from "../settings/preferences";
import {Markdown} from "../components/Markdown";
import {DiffView} from "../components/DiffView";
import {lineDiff} from "../conversation/diff";
import {fileCopy} from "./copy";
import {readFileBytes,type FileRecord,type FileVersion} from "./client";
import {openFile,refreshFiles,selectFile,setFileEdit,updateFiles,useFiles} from "./store";
import {PdfViewer} from "./PdfViewer";
import {prepareFile} from "./prepare";
import {SyntaxCode,fileLanguage} from "./CodeView";
import {HtmlPreview} from "../components/workspace/HtmlPreview";

function TablePreview({text,tsv=false}:{text:string;tsv?:boolean}){
  const result=useMemo(()=>Papa.parse<string[]>(text,{skipEmptyLines:true,delimiter:tsv?"\t":undefined,preview:501}),[text,tsv]);
  return <>{result.data.length>500?<p className="help">{fileCopy().partial}</p>:null}<div className="file-table"><table><tbody>{result.data.slice(0,500).map((row,i)=><tr key={i}>{row.slice(0,100).map((cell,j)=>i===0?<th key={j} scope="col">{cell}</th>:<td key={j}>{cell}</td>)}</tr>)}</tbody></table></div></>;
}
function ImagePreview({url,name}:{url:string;name:string}){
  const [zoom,setZoom]=useState(1);return <><label className="viewer-toolbar">{fileCopy().zoom}<input aria-label={fileCopy().zoom} type="range" min="0.25" max="3" step="0.25" value={zoom} onChange={e=>setZoom(Number(e.target.value))}/>{Math.round(zoom*100)}%</label><div className="image-scroll"><img src={url} alt={name} style={{width:`${zoom*100}%`,maxWidth:"none"}} draggable={false}/></div></>;
}
function FileSurface({file,onRefresh}:{file:FileRecord;onRefresh:()=>void}){
  const client=useClient();const c=fileCopy();
  const pendingEdit=useFiles(s=>s.edits[`${file.session_id}/${file.id}`]);
  const [text,setText]=useState(""),[bytes,setBytes]=useState<Uint8Array|null>(null),[url,setUrl]=useState("");
  const [previewHtml,setPreviewHtml]=useState<string|null>(null);
  const [loading,setLoading]=useState(true),[error,setError]=useState<string|null>(null);
  const [view,setView]=useState<"preview"|"source"|"history"|"edit">(pendingEdit?"edit":"preview");
  const [draft,setDraft]=useState(""),[versions,setVersions]=useState<FileVersion[]>([]),[revision,setRevision]=useState(file.revision);
  const [comparison,setComparison]=useState<string|null>(null),[renaming,setRenaming]=useState(false),[name,setName]=useState(file.name);
  const [rangeStart,setRangeStart]=useState(1),[rangeEnd,setRangeEnd]=useState(1),[partial,setPartial]=useState(false);
  const visibleText=text.slice(0,200_000);
  const textLength=useMemo(()=>[...text].length,[text]);
  const activeVersion=versions.find(v=>v.revision===revision);const size=activeVersion?.size??file.size;
  const isText=file.media_type.startsWith("text/")||file.media_type==="application/json";
  const image=file.media_type.startsWith("image/"),pdf=file.media_type==="application/pdf";
  const semanticDocument=file.name.toLowerCase().endsWith(".docx")||file.media_type==="text/html";
  useEffect(()=>{void client.call<FileVersion[]>("files.versions",{session_id:file.session_id,id:file.id}).then(setVersions).catch(e=>setError(e.message));},[client,file.id,file.session_id,file.revision]);
  useEffect(()=>{
    let active=true;let objectUrl:string|undefined;
    setLoading(true);setError(null);setComparison(null);setBytes(null);setUrl("");setPreviewHtml(null);
    void(async()=>{
      let {text}=await client.call<{text:string}>("files.text",{session_id:file.session_id,id:file.id,revision});
      if(image||pdf||semanticDocument||(!isText&&!text)){
        const data=await readFileBytes(client,file.session_id,{...file,revision,size});
        if(!active)return;setBytes(data);
        if(image){objectUrl=URL.createObjectURL(new Blob([data],{type:file.media_type}));setUrl(objectUrl);}
        if(semanticDocument||!image&&!text&&/\.(pdf|docx|xlsx)$/i.test(file.name)){
          const prepared=await prepareFile(new File([data],file.name));
          if(active)setPreviewHtml(prepared.previewHtml??null);
          if(!text){text=prepared.text;if(text)await client.call("files.prepare_text",{session_id:file.session_id,id:file.id,revision,text});}
        }
      }
      if(active){setText(text);setDraft(useFiles.getState().edits[`${file.session_id}/${file.id}`]?.text??text);setRangeEnd([...text].length);setLoading(false);}
    })().catch(e=>{if(active){setError(e.message);setLoading(false);}});
    return()=>{active=false;if(objectUrl)URL.revokeObjectURL(objectUrl);};
  },[client,file.id,file.session_id,file.revision,revision,size,image,pdf,isText,semanticDocument]);
  const execute=async(action:()=>Promise<void>)=>{setError(null);try{await action();}catch(e){setError((e as Error).message);}};
  const saveAs=()=>execute(async()=>{
    if("__TAURI_INTERNALS__" in window){const {save}=await import("@tauri-apps/plugin-dialog");const path=await save({defaultPath:file.name});if(path)await client.call("files.export",{session_id:file.session_id,id:file.id,revision,path});}
    else{const data=await readFileBytes(client,file.session_id,{...file,revision,size});const href=URL.createObjectURL(new Blob([data],{type:file.media_type}));const link=document.createElement("a");link.href=href;link.download=file.name;link.click();setTimeout(()=>URL.revokeObjectURL(href),1000);}
  });
  const external=(reveal:boolean)=>execute(async()=>{
    const {path}=await client.call<{path:string}>("files.materialize",{session_id:file.session_id,id:file.id,revision});
    const {invoke}=await import("@tauri-apps/api/core");await invoke("open_managed_file",{path,reveal});
  });
  return <section className="file-surface" aria-label={file.name}>
    <div className="file-heading"><div><h2>{file.name}</h2><p>{c[file.origin]} · {Math.ceil(size/1024)} KiB · {c.version} {revision}</p></div><button type="button" aria-label={c.saveAs} title={c.saveAs} onClick={saveAs}><Download size={17}/></button><button type="button" aria-label={c.location} title={c.location} onClick={()=>void external(true)}><FolderOpen size={17}/></button></div>
    <div className="file-tabs" role="tablist" aria-label={c.preview}>{(["preview","source","history"] as const).map(tab=><button type="button" role="tab" aria-selected={view===tab} key={tab} onClick={()=>setView(tab)}>{c[tab]}</button>)}{isText&&revision===file.revision?<button role="tab" aria-selected={view==="edit"} type="button" onClick={()=>setView("edit")}>{c.edit}</button>:null}</div>
    {error?<div role="alert" className="notice notice-danger">{error}</div>:null}
    {!loading&&text.length>200_000?<p className="help">{c.largePreview}</p>:null}
    {!loading&&pdf&&!text?<p className="help">{c.noPdfText}</p>:null}
    {loading?<p role="status">{c.preparing}…</p>:view==="history"?<div className="version-list">
      <label>{c.version}<select value={revision} onChange={e=>{setRevision(Number(e.target.value));setView("preview");}}>{versions.map(v=><option key={v.revision} value={v.revision}>{v.revision} · {new Date(v.created_at*1000).toLocaleString()} · {v.operation??""}</option>)}</select></label>
      {revision>1&&isText?<button type="button" onClick={()=>void execute(async()=>{const previous=await client.call<{text:string}>("files.text",{session_id:file.session_id,id:file.id,revision:revision-1});setComparison(previous.text);})}>{c.compare}</button>:null}
      {comparison!==null?<DiffView lines={lineDiff(comparison,text)}/>:null}
      <p className="file-location">{file.source_path??"SQLite · managed original"}</p><p className="file-hash">SHA-256 {activeVersion?.hash??file.hash}</p>
      <button type="button" onClick={()=>setRenaming(true)}>{c.rename}</button>
      {renaming?<form onSubmit={e=>{e.preventDefault();void execute(async()=>{await client.call("files.rename",{session_id:file.session_id,id:file.id,name});setRenaming(false);onRefresh();});}}><label>{c.untitled}<input value={name} onChange={e=>setName(e.target.value)}/></label><button>{c.save}</button></form>:null}
      <button type="button" onClick={()=>void execute(async()=>{await client.call("files.remove",{session_id:file.session_id,id:file.id});const state=useFiles.getState().sessions[file.session_id];updateFiles(file.session_id,{active:null,tabs:(state?.tabs??[]).filter(id=>id!==file.id),draft:(state?.draft??[]).filter(f=>f.id!==file.id)});onRefresh();})}>{c.delete}</button>
      <button type="button" onClick={()=>void external(false)}>{c.external}</button>
    </div>:view==="edit"?<div className="file-edit"><textarea aria-label={c.edit} value={draft} onChange={e=>{setDraft(e.target.value);setFileEdit(file.session_id,file.id,{text:e.target.value,revision:pendingEdit?.revision??file.revision});}}/><div><button type="button" onClick={()=>void execute(async()=>{const base=pendingEdit?.revision??file.revision;await client.call("files.save",{session_id:file.session_id,id:file.id,expected_revision:base,text:draft});setFileEdit(file.session_id,file.id,null);setRevision(base+1);setView("preview");onRefresh();})}>{c.save}</button><button type="button" onClick={()=>{setDraft(text);setFileEdit(file.session_id,file.id,null);setView("preview");}}>{c.cancel}</button></div></div>:
      view==="source"?<pre className="file-source"><SyntaxCode text={visibleText||c.noPreview} language={fileLanguage(file.name)}/></pre>:
      pdf&&bytes?<PdfViewer bytes={bytes} text={text}/>:image&&url?<ImagePreview url={url} name={file.name}/>:previewHtml!==null?<HtmlPreview html={previewHtml}/>:file.media_type==="text/csv"?<TablePreview text={text}/>:file.name.toLowerCase().endsWith(".xlsx")?<TablePreview text={text} tsv/>:file.media_type==="text/markdown"?<Markdown text={visibleText}/>:text?<pre className="file-source"><SyntaxCode text={visibleText} language={fileLanguage(file.name)}/></pre>:<p>{c.noPreview}</p>}
    {!loading&&text&&view!=="edit"?<div className="file-context"><label><input type="checkbox" checked={partial} onChange={e=>setPartial(e.target.checked)}/>{c.range}</label>{partial?<div><input aria-label={c.firstCharacter} type="number" min={1} max={textLength} value={rangeStart} onChange={e=>setRangeStart(Number(e.target.value))}/><span>–</span><input aria-label={c.lastCharacter} type="number" min={rangeStart} max={textLength} value={rangeEnd} onChange={e=>setRangeEnd(Number(e.target.value))}/></div>:<small>{textLength.toLocaleString()} {c.characters}</small>}<button type="button" onClick={()=>{selectFile(file.session_id,{id:file.id,...(partial?{start:rangeStart-1,end:rangeEnd}:{})});}}>{c.selection}</button></div>:null}
  </section>;
}

export function FileInspector({sessionId}:{sessionId:number}){
  const client=useClient();usePreferences(s=>s.preferences.language);const c=fileCopy();
  const state=useFiles(s=>s.sessions[sessionId]);const records=useFiles(s=>s.records[sessionId]);const width=useFiles(s=>s.width);
  const eventCount=useAppStore(s=>s.eventsBySession[sessionId]?.length??0);
  const [filter,setFilter]=useState("all"),[query,setQuery]=useState(""),[hits,setHits]=useState<FileRecord[]|null>(null),[expanded,setExpanded]=useState(false),[error,setError]=useState<string|null>(null);
  const panel=useRef<HTMLElement>(null);
  useEffect(()=>{if(!state?.open)return;panel.current?.querySelector<HTMLButtonElement>(".inspector-header button")?.focus();const close=(event:KeyboardEvent)=>{if(event.key==="Escape"&&!event.defaultPrevented){updateFiles(sessionId,{open:false});document.querySelector<HTMLButtonElement>(".files-toggle")?.focus();}};window.addEventListener("keydown",close);return()=>window.removeEventListener("keydown",close);},[sessionId,state?.open]);
  useEffect(()=>{if(state?.open)void refreshFiles(client,sessionId).catch(e=>setError(e.message));},[client,sessionId,eventCount,state?.open]);
  useEffect(()=>{let current=true;if(!query.trim()){setHits(null);return;}const timer=setTimeout(()=>{client.call<FileRecord[]>("files.search",{session_id:sessionId,query}).then(found=>{if(current)setHits(found);}).catch(e=>{if(current)setError(e.message);});},150);return()=>{current=false;clearTimeout(timer);};},[client,sessionId,query]);
  if(!state?.open)return null;
  const selected=(records??[]).find(f=>f.id===state.active);
  const shown=(hits??records??[]).filter(f=>filter==="all"||f.origin===filter);
  return <aside ref={panel} className="file-inspector" data-expanded={expanded} style={{width:expanded?"min(80vw,1100px)":width}} aria-label={c.files}>
    <div className="inspector-resize" role="separator" tabIndex={0} aria-orientation="vertical" aria-label={c.resize} aria-valuemin={320} aria-valuemax={900} aria-valuenow={width}
      onKeyDown={e=>{if(e.key==="ArrowLeft"||e.key==="ArrowRight"){e.preventDefault();useFiles.setState({width:Math.max(320,Math.min(900,width+(e.key==="ArrowLeft"?24:-24)))});}}}
      onPointerDown={e=>{e.currentTarget.setPointerCapture(e.pointerId);}}
      onPointerMove={e=>{if(e.currentTarget.hasPointerCapture(e.pointerId)){useFiles.setState({width:Math.max(320,Math.min(window.innerWidth*.6,window.innerWidth-e.clientX))});}}}/>
    <header className="inspector-header"><button type="button" className="inspector-back" aria-label={c.back} title={c.back} onClick={()=>updateFiles(sessionId,{open:false})}><ChevronLeft size={16}/><span>{c.back}</span></button><strong>{c.files}</strong><span className="spacer"/><button type="button" aria-label={expanded?c.restore:c.expand} onClick={()=>setExpanded(v=>!v)}>{expanded?<Minimize2 size={16}/>:<Maximize2 size={16}/>}</button><button type="button" aria-label={c.close} onClick={()=>updateFiles(sessionId,{open:false})}><PanelRightClose size={18}/></button></header>
    {state.tabs.length?<div className="inspector-tabs">{state.tabs.map(id=>{const file=(records??[]).find(f=>f.id===id);return file?<div key={id} data-active={id===state.active}><button type="button" onClick={()=>updateFiles(sessionId,{active:id})}>{file.name}</button><button type="button" aria-label={`${c.close}: ${file.name}`} onClick={()=>updateFiles(sessionId,{tabs:state.tabs.filter(t=>t!==id),active:state.active===id?null:state.active})}><X size={12}/></button></div>:null;})}</div>:null}
    {selected?<><button className="file-back" type="button" onClick={()=>updateFiles(sessionId,{active:null})}><ChevronLeft size={14}/>{c.all}</button><FileSurface key={selected.id} file={selected} onRefresh={()=>void refreshFiles(client,sessionId)}/></>:<div className="file-catalog">
      <label className="file-search"><Search size={16}/><input type="search" aria-label={c.search} placeholder={c.search} value={query} onChange={e=>setQuery(e.target.value)}/></label>
      <div className="file-filters">{(["all","attached","created","modified"] as const).map(f=><button type="button" aria-pressed={filter===f} key={f} onClick={()=>setFilter(f)}>{c[f]}</button>)}</div>
      {error?<p role="alert">{error}</p>:null}
      {shown.length?<ul>{shown.map(file=><li key={file.id}><button type="button" onClick={()=>openFile(sessionId,file.id)}><span>{file.name}</span><small>{c[file.origin]} · {Math.ceil(file.size/1024)} KiB · v{file.revision}</small></button></li>)}</ul>:<p className="file-empty">{c.empty}</p>}
    </div>}
  </aside>;
}
