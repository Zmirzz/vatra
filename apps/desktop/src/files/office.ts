export function extractOffice(bytes:Uint8Array,format:"docx"|"xlsx"):Promise<{text?:string;html?:string;warning?:string}>{
  return new Promise((resolve,reject)=>{
    const worker=new Worker(new URL("./office.worker.ts",import.meta.url),{type:"module"});
    const timeout=setTimeout(()=>{worker.terminate();reject(new Error("Document preparation exceeded 30 seconds."));},30_000);
    worker.onmessage=e=>{clearTimeout(timeout);worker.terminate();if(e.data.error)reject(new Error(e.data.error));else resolve(e.data);};
    worker.onerror=e=>{clearTimeout(timeout);worker.terminate();reject(new Error(e.message||"Document preparation failed."));};
    const data=bytes.slice();worker.postMessage({bytes:data,format},[data.buffer]);
  });
}
