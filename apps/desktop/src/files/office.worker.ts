import {validateZipDirectory} from "./zip";
const MAX_TEXT=2_000_000;
self.onmessage=async(event:MessageEvent<{bytes:Uint8Array;format:"docx"|"xlsx"}>)=>{
  try{
    const {bytes,format}=event.data;validateZipDirectory(bytes);
    if(format==="docx"){
      const mammoth=await import("mammoth/mammoth.browser");
      const result=await mammoth.convertToHtml({arrayBuffer:bytes.buffer},{externalFileAccess:false});
      if(result.value.length>8_000_000)throw new Error("The document exceeds the extracted content limit.");
      self.postMessage({html:result.value,warning:result.messages.map(m=>m.message).join(" ")});
    }else{
      const {default:ExcelJS}=await import("exceljs");const book=new ExcelJS.Workbook();await book.xlsx.load(new Uint8Array(bytes).buffer);
      let text="";for(const sheet of book.worksheets){
        text+=`\n\n[Sheet: ${sheet.name}]\n`;
        sheet.eachRow(row=>{
          if(row.values instanceof Array)text+=row.values.slice(1).map(value=>{
            if(value&&typeof value==="object"){
              if("formula" in value)return `=${value.formula} [cached: ${String(value.result??"")}]`;
              if("richText" in value)return value.richText.map(part=>part.text).join("");
              if("text" in value)return value.text;
            }return String(value??"");
          }).join("\t")+"\n";
          if(text.length>MAX_TEXT)throw new Error("The workbook exceeds 2 million extracted characters.");
        });
      }self.postMessage({text});
    }
  }catch(error){self.postMessage({error:(error as Error).message});}
};
