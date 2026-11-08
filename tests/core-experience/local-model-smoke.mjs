// Uses one model already present on disk. No downloads, cloud APIs or subscription agents.
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {mkdir,readFile,writeFile} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
import {join} from 'node:path';
import {once} from 'node:events';
const modelPath=process.argv[2];assert.ok(modelPath,'Pass the path of an existing GGUF.');
const root=fileURLToPath(new URL('../../',import.meta.url));const base=join(root,'.superpowers/core-experience');const profile=join(base,`local-${Date.now()}`);await mkdir(profile,{recursive:true});
// An absent config intentionally uses Config::empty(); model registration is persisted through RPC.
const service=spawn(join(root,'target/release/vatra.exe'),['serve','--data-dir',profile],{windowsHide:true,stdio:['ignore','ignore','pipe']});let stderr='';service.stderr.on('data',b=>stderr=(stderr+b).slice(-6000));
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));let socket,call;const pending=new Map();let nextId=0;
async function ready(){for(let i=0;i<200;i++){try{const {port}=JSON.parse(await readFile(join(profile,'service.json'),'utf8'));const health=await fetch(`http://127.0.0.1:${port}/health`);if(!health.ok)throw Error('not ready');const token=(await readFile(join(profile,'token'),'utf8')).trim();socket=new WebSocket(`ws://127.0.0.1:${port}/rpc`,[`vatra.bearer.${token}`]);await once(socket,'open');socket.addEventListener('message',event=>{const value=JSON.parse(event.data);const p=pending.get(value.id);if(p){pending.delete(value.id);value.error?p.reject(Error(value.error.message)):p.resolve(value.result);}});call=(method,params={})=>new Promise((resolve,reject)=>{pending.set(++nextId,{resolve,reject});socket.send(JSON.stringify({jsonrpc:'2.0',id:nextId,method,params}));});return;}catch{}if(service.exitCode!==null)throw Error(stderr);await delay(100);}throw Error('Service startup timeout: '+stderr);}
async function upload(session,name,text){const bytes=await readFile(join(base,'fixtures',name));const {upload_id}=await call('files.upload.begin',{session_id:session,size:bytes.length});for(let offset=0;offset<bytes.length;offset+=256*1024)await call('files.upload.chunk',{session_id:session,upload_id,offset,base64:bytes.subarray(offset,offset+256*1024).toString('base64')});return call('files.upload.finish',{session_id:session,upload_id,name,text:text??''});}
async function turn(session,prompt,attachments=[]){const start=Date.now();await call('turn.start',{session_id:session,prompt,mode:'full_access',attachments:attachments.map(id=>({id}))});for(let i=0;i<1800;i++){const sessions=await call('sessions.list');if(!sessions.find(s=>s.id===session).running){const events=await call('sessions.events',{session_id:session,after_seq:0});console.log(JSON.stringify({turnFinishedSeconds:Math.round((Date.now()-start)/1000),last:events.at(-1).event}));return events;}await delay(200);}await call('turn.stop',{session_id:session});throw Error('Real local turn exceeded six minutes.');}
try{
 await ready();await call('onboarding.done');
 const model=await call('models.add_path',{path:modelPath});const agent=`local:${model.slug}`;console.log(JSON.stringify({model:model.slug,profile,stage:'model registered without copying or downloading'}));
 const {id}=await call('sessions.create',{project_id:null,agent});
 const source=await upload(id,'source.md');const pdf=await upload(id,'evidence.pdf','[Page 1]\nVatra local proof - page one\n[Page 2]\nSecond page: document evidence 42');
 const docx=await upload(id,'document.docx','Ședință Vatra – document local\nVenit\t42');await upload(id,'buget.xlsx','[Sheet: Buget]\nCategorie\tValoare\nȘedință\t42');await upload(id,'data.csv');const image=await upload(id,'image.png');
 const caps=await call('files.capabilities',{session_id:id});assert.equal(caps.images,'unsupported');
 await assert.rejects(call('turn.start',{session_id:id,prompt:'image',mode:'supervised',attachments:[{id:image.id}]}));
 let events=await turn(id,'Read the attached source. Reply with only the exact verification code stated in it. Do not call tools.',[source.id]);
 assert.equal(events.at(-1).event.type,'turn_completed');assert.ok(events.some(e=>e.event.type==='assistant_text'&&e.event.text.includes('VATRA-42')));
 const afterFirst=events.at(-1).seq;
 events=await turn(id,'Create a Markdown file named local-result.md in the current working folder, containing the heading "Local verification" and the code VATRA-42. Use the Write tool to create the actual file.');
 assert.equal(events.at(-1).event.type,'turn_completed');const files=await call('files.list',{session_id:id});const result=files.find(f=>f.name==='local-result.md'&&f.origin==='created');assert.ok(result,'The real model did not produce a tracked output file.');
 const text=await call('files.text',{session_id:id,id:result.id});assert.match(text.text,/VATRA-42/);
 await call('conversations.ui.set',{drafts:{[id]:'Analyze these documents together.'},sessions:{[id]:{draft:[{id:pdf.id},{id:docx.id}],tabs:[pdf.id,result.id],active:pdf.id,open:true}},edits:{},width:480});
 const summary={kind:'Real local llama.cpp smoke test',model:model.slug,session:id,profile,documentCode:'VATRA-42',artifact:result.name,artifactHash:result.hash,capabilities:caps,checks:['actual local document input','actual local Write tool','artifact capture','text-only model rejects images before execution'],vision:'No compatible vision projector installed; no real vision inference claimed',firstTurnEnd:afterFirst};
 await writeFile(join(base,'local-model-results.json'),JSON.stringify(summary,null,2));console.log(JSON.stringify(summary));
}finally{
 if(call){await call('models.stop').catch(()=>{});await call('service.shutdown').catch(()=>{});socket.close();}
 if(service.exitCode===null)await Promise.race([once(service,'exit'),delay(10000)]);
}
