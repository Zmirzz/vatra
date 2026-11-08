/** Validate declared archive bounds before handing untrusted Office input to a decompressor. */
export function validateZipDirectory(bytes:Uint8Array):void {
  const view=new DataView(bytes.buffer,bytes.byteOffset,bytes.byteLength);let end=-1;
  for(let i=bytes.length-22;i>=Math.max(0,bytes.length-65557);i--){if(view.getUint32(i,true)===0x06054b50){end=i;break;}}
  if(end<0)throw new Error("The Office archive is incomplete or corrupt.");
  const count=view.getUint16(end+10,true),size=view.getUint32(end+12,true),offset=view.getUint32(end+16,true);
  if(count>2000||count===65535||offset+size>end||view.getUint16(end+4,true)!==0)throw new Error("The Office archive exceeds supported limits.");
  let cursor=offset,total=0;
  for(let i=0;i<count;i++){
    if(cursor+46>end||view.getUint32(cursor,true)!==0x02014b50)throw new Error("The Office archive directory is invalid.");
    const flags=view.getUint16(cursor+8,true),expanded=view.getUint32(cursor+24,true);total+=expanded;
    if(flags&1||expanded===0xffffffff||total>64*1024*1024)throw new Error("Encrypted or oversized Office archives are not supported.");
    cursor+=46+view.getUint16(cursor+28,true)+view.getUint16(cursor+30,true)+view.getUint16(cursor+32,true);
    if(cursor>offset+size)throw new Error("The Office archive directory is invalid.");
  }
}
