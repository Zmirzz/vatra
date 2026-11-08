import type {RpcClient} from "../rpc/client";
import {CHUNK_BYTES,type PreparedFile} from "./prepare";
export type FileClient=Pick<RpcClient,"call">;
export type FileRecord={id:number;session_id:number;name:string;media_type:string;origin:"attached"|"created"|"modified";source_path:string|null;revision:number;created_at:number;size:number;hash:string;operation:string|null};
export type FileVersion={revision:number;size:number;hash:string;operation:string|null;created_at:number};
export type FileSelection={id:number;start?:number;end?:number};
export type InputCapabilities={images:"supported"|"unsupported"|"unknown";document_text:boolean;context_tokens:number;estimated:boolean};
function encode(bytes:Uint8Array):string{let text="";for(let i=0;i<bytes.length;i+=8192)text+=String.fromCharCode(...bytes.subarray(i,i+8192));return btoa(text);}
export async function uploadPrepared(client:FileClient,session:number,file:PreparedFile,onProgress?:(progress:number)=>void):Promise<FileRecord>{
  const {upload_id}=await client.call<{upload_id:string}>("files.upload.begin",{session_id:session,size:file.bytes.length});
  try{
    for(let offset=0;offset<file.bytes.length;offset+=CHUNK_BYTES){
      await client.call("files.upload.chunk",{session_id:session,upload_id,offset,base64:encode(file.bytes.subarray(offset,offset+CHUNK_BYTES))});
      onProgress?.(Math.min(1,(offset+CHUNK_BYTES)/file.bytes.length));
    }
    return await client.call<FileRecord>("files.upload.finish",{session_id:session,upload_id,name:file.name,text:file.text});
  }catch(error){await client.call("files.upload.cancel",{session_id:session,upload_id}).catch(()=>undefined);throw error;}
}
export async function readFileBytes(client:FileClient,session:number,file:Pick<FileRecord,"id"|"size"|"revision">):Promise<Uint8Array<ArrayBuffer>>{
  const bytes=new Uint8Array(file.size);
  for(let offset=0;offset<file.size;offset+=CHUNK_BYTES){
    const part=await client.call<{base64:string}>("files.read",{session_id:session,id:file.id,revision:file.revision,offset,length:CHUNK_BYTES});
    const text=atob(part.base64);const expected=Math.min(CHUNK_BYTES,file.size-offset);
    if(text.length!==expected)throw new Error("The file download is incomplete. Reopen it to retry.");
    for(let i=0;i<text.length;i++)bytes[offset+i]=text.charCodeAt(i);
  }
  return bytes;
}
