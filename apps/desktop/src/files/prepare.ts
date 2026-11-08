import Papa from "papaparse";
import {sanitizePreview} from "../workspace/sanitize";

export const MAX_FILE_BYTES=25*1024*1024;
export const MAX_TEXT_CHARS=2_000_000;
export const CHUNK_BYTES=256*1024;
const encoder=new TextEncoder();
const decoder=new TextDecoder("utf-8",{fatal:true});
const starts=(bytes:Uint8Array,signature:number[])=>signature.every((b,i)=>bytes[i]===b);

export function detectFileType(name:string,bytes:Uint8Array):string {
  if(bytes.length>MAX_FILE_BYTES)throw new Error("The file is larger than 25 MiB.");
  const ext=name.split(".").pop()?.toLowerCase();
  const ascii=(start:number,end:number)=>String.fromCharCode(...bytes.slice(start,end));
  switch(ext){
    case "pdf":if(ascii(0,5)==="%PDF-")return "application/pdf";break;
    case "png":if(starts(bytes,[137,80,78,71,13,10,26,10]))return "image/png";break;
    case "jpg":case "jpeg":if(starts(bytes,[255,216,255]))return "image/jpeg";break;
    case "webp":if(ascii(0,4)==="RIFF"&&ascii(8,12)==="WEBP")return "image/webp";break;
    case "docx":case "xlsx":if(starts(bytes,[80,75,3,4]))return ext==="docx"?"application/vnd.openxmlformats-officedocument.wordprocessingml.document":"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";break;
    case "md":case "markdown":return "text/markdown";
    case "csv":return "text/csv";
    case "json":return "application/json";
    case "html":case "htm":return "text/html";
    default:decoder.decode(bytes);return "text/plain";
  }
  throw new Error("File contents do not match its extension.");
}

export function prepareText(name:string,bytes:Uint8Array):string {
  const text=decoder.decode(bytes);
  if(text.includes("\0"))throw new Error("This file contains binary data.");
  if(text.length>MAX_TEXT_CHARS)throw new Error("This document exceeds 2 million characters.");
  if(name.toLowerCase().endsWith(".json"))return JSON.stringify(JSON.parse(text),null,2);
  if(name.toLowerCase().endsWith(".csv")){
    const parsed=Papa.parse<string[]>(text,{skipEmptyLines:true});
    if(parsed.errors.some(e=>e.type==="Quotes"))throw new Error("The CSV contains malformed quotes.");
  }
  return text;
}

export {validateZipDirectory} from "./zip";

export function selectDocumentContext(text:string,budget:number,selection?:{start:number;end:number}):string {
  if(selection&&(!Number.isInteger(selection.start)||!Number.isInteger(selection.end)||selection.start<0||selection.end<=selection.start||selection.end>text.length))throw new Error("Select a valid document range.");
  const content=selection?text.slice(selection.start,selection.end):text;
  // Conservative UTF-8 budget, deliberately marked as an estimate in the UI.
  if(Math.ceil(encoder.encode(content).length/3)>budget)throw new Error("This document exceeds the context budget. Select a smaller section before sending.");
  return content;
}

export type PreparedFile={name:string;mediaType:string;bytes:Uint8Array;text:string;warning?:string;previewHtml?:string};
export async function prepareFile(file:File):Promise<PreparedFile>{
  if(file.size>MAX_FILE_BYTES)throw new Error("The file is larger than 25 MiB.");
  const bytes=new Uint8Array(await file.arrayBuffer());
  const mediaType=detectFileType(file.name,bytes);
  let text="",warning:string|undefined,previewHtml:string|undefined;
  if(mediaType.startsWith("image/")){
    const url=URL.createObjectURL(new Blob([bytes],{type:mediaType}));
    try{await new Promise<void>((resolve,reject)=>{const image=new Image();image.onload=()=>image.naturalWidth*image.naturalHeight>40_000_000?reject(new Error("Images are limited to 40 million pixels.")):resolve();image.onerror=()=>reject(new Error("The image cannot be decoded."));image.src=url;});}finally{URL.revokeObjectURL(url);}
  }else if(mediaType==="application/pdf"){
    const {loadPdf}=await import("./pdf");const pdf=await loadPdf(bytes.slice());
    try{
      if(pdf.numPages>500)throw new Error("PDF preparation is limited to 500 pages.");
      for(let page=1;page<=pdf.numPages;page++){
        const content=await(await pdf.getPage(page)).getTextContent();
        text+=`\n\n[Page ${page}]\n`+content.items.map(item=>"str" in item?item.str+(item.hasEOL?"\n":" "):"").join("");
        if(text.length>MAX_TEXT_CHARS)throw new Error("The extracted PDF exceeds 2 million characters.");
      }
      if(!text.replace(/\[Page \d+\]/g,"").trim()){text="";warning="This PDF has no extractable text. It can be viewed, but OCR is not available.";}
    }finally{await pdf.loadingTask.destroy();}
  }else if(file.name.toLowerCase().endsWith(".docx")){
    const {extractOffice}=await import('./office');
    const result=await extractOffice(bytes,'docx');
    previewHtml=sanitizePreview(result.html??'');
    const parsed=new DOMParser().parseFromString(previewHtml,'text/html');
    parsed.querySelectorAll('script,style,img,iframe,object').forEach(node=>node.remove());
    parsed.querySelectorAll('p,tr,h1,h2,h3,h4,li').forEach(node=>node.append('\n'));
    parsed.querySelectorAll('td,th').forEach(node=>node.append('\t'));
    text=parsed.body.textContent??'';warning=result.warning;
  }else if(file.name.toLowerCase().endsWith('.xlsx')){
    const {extractOffice}=await import('./office');text=(await extractOffice(bytes,'xlsx')).text??'';
  }else if(mediaType==="text/html"){
    previewHtml=sanitizePreview(prepareText(file.name,bytes));
    const parsed=new DOMParser().parseFromString(previewHtml,"text/html");
    parsed.querySelectorAll("script,style,iframe,object,img").forEach(n=>n.remove());
    parsed.querySelectorAll("p,div,li,tr,h1,h2,h3").forEach(n=>n.append("\n"));text=parsed.body.textContent??"";
  }else{text=prepareText(file.name,bytes);}
  if(text.length>MAX_TEXT_CHARS)throw new Error("The extracted document exceeds 2 million characters.");
  return {name:file.name,mediaType,bytes,text,warning,previewHtml};
}
