import {it,expect} from "vitest";
import {uploadPrepared,readFileBytes,type FileClient} from "./client";
import {CHUNK_BYTES} from "./prepare";
it("uploads bounded chunks in order and reads their original bytes",async()=>{
  const bytes=new Uint8Array(CHUNK_BYTES+17).fill(42);let received=new Uint8Array();const offsets:number[]=[];
  const client={call:async(method:string,p:Record<string,unknown>)=>{
    if(method==="files.upload.begin")return {upload_id:"u"};
    if(method==="files.upload.chunk"){
      offsets.push(p.offset as number);const part=Uint8Array.from(atob(p.base64 as string),c=>c.charCodeAt(0));
      expect(part.length).toBeLessThanOrEqual(CHUNK_BYTES);const next=new Uint8Array(received.length+part.length);next.set(received);next.set(part,received.length);received=next;return {};
    }
    if(method==="files.upload.finish")return {id:1,size:bytes.length,revision:1};
    if(method==="files.read"){const part=received.slice(p.offset as number,(p.offset as number)+CHUNK_BYTES);return {base64:btoa(String.fromCharCode(...part.slice(0,10000)))}};
    return {};
  }} as FileClient;
  await uploadPrepared(client,3,{name:"test.txt",mediaType:"text/plain",bytes,text:"source"});
  expect(offsets).toEqual([0,CHUNK_BYTES]);expect(received).toEqual(bytes);
});
it("cancels failed uploads and never finalizes partial data",async()=>{
  const calls:string[]=[];const client={call:async(method:string)=>{calls.push(method);if(method==="files.upload.begin")return {upload_id:"u"};if(method==="files.upload.chunk")throw new Error("disconnected");return {};}} as FileClient;
  await expect(uploadPrepared(client,1,{name:"a.txt",mediaType:"text/plain",bytes:new Uint8Array([1]),text:"x"})).rejects.toThrow("disconnected");
  expect(calls).toEqual(["files.upload.begin","files.upload.chunk","files.upload.cancel"]);
});
it("rejects a truncated download rather than displaying partial content",async()=>{
  const client={call:async()=>({base64:""})} as FileClient;
  await expect(readFileBytes(client,2,{id:1,size:20,revision:1})).rejects.toThrow();
});
