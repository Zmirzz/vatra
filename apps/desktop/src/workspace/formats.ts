import Papa from "papaparse";
import type {DocumentFormat,Selection} from "./types";
export interface DocumentAdapter {load(body:string):void;serialize():string;selection():Selection|null;}
export const MAX_DOCUMENT_BYTES=2*1024*1024;
export function parseCsv(source:string):string[][] {
 if(new TextEncoder().encode(source).length>MAX_DOCUMENT_BYTES)throw Error("CSV exceeds 2 MB");
 const body=source.replace(/^\uFEFF/,"");if(body==="")return [];
 let state:"start"|"plain"|"quoted"|"closed"="start";
 for(let i=0;i<body.length;i++){const ch=body[i];if(state==="quoted"){if(ch==='"'){if(body[i+1]==='"')i++;else state="closed";}continue;}if(ch===","||ch==="\n"||ch==="\r"){state="start";continue;}if(ch==='"'){if(state!=="start")throw Error("Invalid CSV quote");state="quoted";continue;}if(state==="closed")throw Error("Text after CSV quote");state="plain";}
 if(state==="quoted")throw Error("Unclosed CSV quote");
 const rows:string[][]=[];let cursor=0;let problem=false;
 Papa.parse<string[]>(body,{delimiter:",",dynamicTyping:false,skipEmptyLines:false,step:(result,parser)=>{const raw=body.slice(cursor,result.meta.cursor);cursor=result.meta.cursor;if(raw===""||/^(?:\r\n|\r|\n)$/.test(raw))return;if(result.errors.length){problem=true;parser.abort();return;}rows.push(result.data);if(rows.length>10000||result.data.length>100||rows.length*(rows[0]?.length??0)>50000){problem=true;parser.abort();}}});
 if(problem||rows.some(row=>row.length!==rows[0].length))throw Error("CSV rows or dimensions are invalid");return rows;
}
export function serializeCsv(rows:string[][]){if(rows.length===0)return "";if(rows.length>10000||rows[0].length>100||rows.length*rows[0].length>50000||rows.some(r=>r.length!==rows[0].length))throw Error("CSV dimensions are invalid");const body=Papa.unparse(rows,{quotes:true,newline:"\n"});if(new TextEncoder().encode(body).length>MAX_DOCUMENT_BYTES)throw Error("CSV exceeds 2 MB");return body;}
export function sourceSelection(body:string,start:number,end:number):Selection|null {
 const split=(index:number)=>index>0&&index<body.length&&body.charCodeAt(index)>=0xdc00&&body.charCodeAt(index)<=0xdfff&&body.charCodeAt(index-1)>=0xd800&&body.charCodeAt(index-1)<=0xdbff;
 if(start<0||end>body.length||start>=end||split(start)||split(end))return null;return {start_utf16:start,end_utf16:end,selected_text:body.slice(start,end)};
}
export function richSelection(body:string,selected:string):Selection|null {if(!selected)return null;const at=body.indexOf(selected);return at>=0&&body.lastIndexOf(selected)===at?sourceSelection(body,at,at+selected.length):null;}
export function supportsRichEditing(body:string,format:DocumentFormat):boolean {
 if(body.length>200000||body.includes("\0"))return false;
 if(format==="markdown"){
  // GFM allows tables without outer pipes. StarterKit cannot preserve table nodes.
  const tableDelimiter=/^[ \t]*\|?[ \t]*:?-+:?[ \t]*(?:\|[ \t]*:?-+:?[ \t]*)+\|?[ \t]*$/m;
  return !tableDelimiter.test(body)&&!(/<|>|!?\[[^\]]*\]\(|^\s*\|.*\||\[\^[^\]]+\]|^\s*\[[^\]]+\]:|^\s*[-*+]\s+\[[ xX]\]/m.test(body));
 }
 if(format==="html")return !(/<[!?]|<\/?(?!p\b|div\b|h[1-6]\b|ul\b|ol\b|li\b|strong\b|b\b|em\b|i\b|code\b|pre\b|br\b|hr\b|blockquote\b)[a-z]|<[a-z][^>]*\s[a-z_:][\w:.-]*\s*=/i.test(body));
 return false;
}
