import {useEffect,useRef,useState} from "react";
import type {PDFDocumentProxy} from "pdfjs-dist";
import {ChevronLeft,ChevronRight,ZoomIn,ZoomOut} from "lucide-react";
import {fileCopy} from "./copy";

export function PdfViewer({bytes,text}:{bytes:Uint8Array;text:string}){
  const c=fileCopy();const [pdf,setPdf]=useState<PDFDocumentProxy|null>(null);
  const [page,setPage]=useState(1),[zoom,setZoom]=useState(1),[query,setQuery]=useState("");
  const [error,setError]=useState<string|null>(null);const canvas=useRef<HTMLCanvasElement>(null);const layer=useRef<HTMLDivElement>(null);
  useEffect(()=>{
    let active=true;let loaded:PDFDocumentProxy|undefined;
    void import("./pdf").then(({loadPdf})=>loadPdf(bytes.slice())).then(doc=>{loaded=doc;if(active){setPdf(doc);setPage(1);}else void doc.loadingTask.destroy();}).catch(e=>{if(active)setError((e as Error).message);});
    return()=>{active=false;setPdf(null);void loaded?.loadingTask.destroy();};
  },[bytes]);
  useEffect(()=>{
    if(!pdf||!canvas.current||!layer.current)return;
    let cancelled=false;let cancelRender:(()=>void)|undefined;let cancelText:(()=>void)|undefined;
    const element=canvas.current,textElement=layer.current;
    void Promise.all([pdf.getPage(page),import("pdfjs-dist")]).then(async([p,{TextLayer}])=>{
      if(cancelled)return;
      const viewport=p.getViewport({scale:zoom});const ratio=Math.min(window.devicePixelRatio||1,2);
      element.width=Math.ceil(viewport.width*ratio);element.height=Math.ceil(viewport.height*ratio);
      element.style.width=`${viewport.width}px`;element.style.height=`${viewport.height}px`;
      const task=p.render({canvas:element,viewport,transform:ratio!==1?[ratio,0,0,ratio,0,0]:undefined});cancelRender=()=>task.cancel();
      textElement.replaceChildren();textElement.style.setProperty("--scale-factor",String(zoom));
      textElement.style.setProperty("--total-scale-factor",String(zoom*p.userUnit));
      textElement.style.width=`${viewport.width}px`;textElement.style.height=`${viewport.height}px`;
      const textTask=new TextLayer({textContentSource:await p.getTextContent(),container:textElement,viewport});cancelText=()=>textTask.cancel();
      if(cancelled){task.cancel();textTask.cancel();return;}
      await Promise.all([task.promise,textTask.render()]);
    }).catch(e=>{if(!cancelled&&e?.name!=="RenderingCancelledException")setError((e as Error).message);});
    return()=>{cancelled=true;cancelRender?.();cancelText?.();};
  },[pdf,page,zoom]);
  const matches=query.trim()?text.split(/\[Page (\d+)\]/).flatMap((part,i,all)=>i%2===0&&i>0&&part.toLocaleLowerCase().includes(query.toLocaleLowerCase())?[Number(all[i-1])]:[]):[];
  return <div className="pdf-viewer">
    <div className="viewer-toolbar">
      <button type="button" disabled={page<=1} aria-label={c.previous} onClick={()=>setPage(p=>p-1)}><ChevronLeft size={16}/></button>
      <label><input aria-label={c.page} type="number" min={1} max={pdf?.numPages??1} value={page} onChange={e=>setPage(Math.max(1,Math.min(pdf?.numPages??1,Number(e.target.value)||1)))}/> / {pdf?.numPages??"…"}</label>
      <button type="button" disabled={!pdf||page>=pdf.numPages} aria-label={c.next} onClick={()=>setPage(p=>p+1)}><ChevronRight size={16}/></button>
      <button type="button" aria-label={`${c.zoom} −`} disabled={zoom<=.25} onClick={()=>setZoom(z=>Math.max(.25,z-.25))}><ZoomOut size={16}/></button><span>{Math.round(zoom*100)}%</span>
      <button type="button" aria-label={`${c.zoom} +`} disabled={zoom>=3} onClick={()=>setZoom(z=>Math.min(3,z+.25))}><ZoomIn size={16}/></button>
      <input type="search" aria-label={c.find} placeholder={c.find} value={query} onChange={e=>setQuery(e.target.value)}/>
      {query?<span role="status">{matches.length}</span>:null}
      {matches.map(n=><button type="button" key={n} onClick={()=>setPage(n)}>{n}</button>)}
    </div>
    {error?<p role="alert">{error}</p>:null}
    {!pdf&&!error?<p role="status">{c.preparing}…</p>:null}
    <div className="pdf-scroll"><div className="pdf-page"><canvas ref={canvas}/><div ref={layer} className="textLayer"/></div></div>
  </div>;
}
