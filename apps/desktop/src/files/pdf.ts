import { getDocument, GlobalWorkerOptions } from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
GlobalWorkerOptions.workerSrc=workerUrl;
export function loadPdf(bytes:Uint8Array){
  const task=getDocument({data:bytes,disableAutoFetch:true,disableStream:true,useSystemFonts:true,cMapUrl:"/pdfjs/cmaps/",cMapPacked:true,standardFontDataUrl:"/pdfjs/standard_fonts/",wasmUrl:"/pdfjs/wasm/"});
  return task.promise.catch(async error=>{await task.destroy();throw error;});
}
