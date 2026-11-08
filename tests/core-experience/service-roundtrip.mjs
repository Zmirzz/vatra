// Real release service + real engine and file tools; only the model HTTP responses are fixtures.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {spawn} from 'node:child_process';
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
import {join} from 'node:path';
import {once} from 'node:events';
const root=fileURLToPath(new URL('../../',import.meta.url));
const base=join(root,'.superpowers/core-experience');await mkdir(base,{recursive:true});
const profile=join(base,`profile-${Date.now()}`);await mkdir(profile);
const fixtures=join(base,'fixtures');
const requests=[];
const provider=createServer(async(req,res)=>{
 if(req.url==='/props'){res.setHeader('Content-Type','application/json');res.end(JSON.stringify({modalities:{vision:true},n_ctx:32768}));return;}
 let data='';for await(const part of req)data+=part;const body=JSON.parse(data);requests.push(body);
 const messages=body.messages;const user=[...messages].reverse().find(m=>m.role==='user');const prompt=typeof user?.content==='string'?user.content:JSON.stringify(user?.content);
 const last=messages.at(-1);let text='Fixture response: request received locally.',call;
 if(prompt.includes('CASE_CREATE')&&last.role!=='tool')call={id:'fixture-write',name:'Write',arguments:JSON.stringify({file_path:'results/report.md',content:'# Generated result\n\nVATRA-42, first version.\n'})};
 else if(prompt.includes('CASE_EDIT')){
   const segment=messages.slice(messages.lastIndexOf(user));
   if(!segment.some(m=>m.tool_calls?.some(c=>c.function.name==='Read')))call={id:'fixture-read',name:'Read',arguments:JSON.stringify({file_path:'results/report.md'})};
   else if(!segment.some(m=>m.tool_calls?.some(c=>c.function.name==='Edit')))call={id:'fixture-edit',name:'Edit',arguments:JSON.stringify({file_path:'results/report.md',old_string:'first version',new_string:'second version'})};
 }
 else if(prompt.includes('CASE_SHELL')&&last.role!=='tool')call={id:'fixture-shell',name:'Bash',arguments:JSON.stringify({command:'node -e "require(\'fs\').writeFileSync(\'from-shell.txt\',\'real shell artifact\')"'})};
 else if(prompt.includes('CASE_FALSE'))text='I created imaginary.txt (fixture intentionally makes a false claim).';
 if(prompt.includes('CASE_COMPARE'))text='Fixture verified image payload and document text transport; this is not real visual reasoning.';
 res.setHeader('Content-Type','application/json');res.end(JSON.stringify({choices:[{message:call?{role:'assistant',content:null,tool_calls:[{id:call.id,type:'function',function:{name:call.name,arguments:call.arguments}}]}:{role:'assistant',content:text}}],usage:{prompt_tokens:1234,completion_tokens:40}}));
});
await new Promise(resolve=>provider.listen(0,'127.0.0.1',resolve));
const port=provider.address().port;
await writeFile(join(profile,'vatra.toml'),`summarizer = "Protocol fixture"\n[agents."Protocol fixture"]\nkind = "model"\nbase_url = "http://127.0.0.1:${port}/v1"\nmodel = "fixture-vision"\ncontext_window = 32768\n`);
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
let service,client;
async function start(){
 service=spawn(join(root,'target/release/vatra.exe'),['serve','--data-dir',profile],{windowsHide:true,stdio:['ignore','ignore','pipe']});let stderr='';service.stderr.on('data',data=>{stderr=(stderr+data).slice(-4000);});
 for(let i=0;i<200;i++){
  try{const info=JSON.parse(await readFile(join(profile,'service.json'),'utf8'));const response=await fetch(`http://127.0.0.1:${info.port}/health`);if(!response.ok)throw Error('not ready');
   const token=(await readFile(join(profile,'token'),'utf8')).trim();client=await rpcClient(info.port,token);return;
  }catch{}if(service.exitCode!==null)throw Error(stderr);await delay(100);
 }throw Error('Service startup timeout: '+stderr);
}
async function rpcClient(port,token){
 const socket=new WebSocket(`ws://127.0.0.1:${port}/rpc`,[`vatra.bearer.${token}`]);await once(socket,'open');let id=0;const pending=new Map();
 socket.addEventListener('message',event=>{const value=JSON.parse(event.data);if(value.id){const p=pending.get(value.id);if(p){pending.delete(value.id);value.error?p.reject(Error(value.error.message)):p.resolve(value.result);}}});
 return {socket,call(method,params={}){return new Promise((resolve,reject)=>{pending.set(++id,{resolve,reject});socket.send(JSON.stringify({jsonrpc:'2.0',id,method,params}));});}};
}
async function stop(){if(!client)return;const process=service;await client.call('service.shutdown');client.socket.close();client=null;if(process.exitCode===null)await Promise.race([once(process,'exit'),delay(10000).then(()=>{throw Error('Service did not stop');})]);}
async function upload(session,name,text){
 const bytes=await readFile(join(fixtures,name));const {upload_id}=await client.call('files.upload.begin',{session_id:session,size:bytes.length});
 for(let offset=0;offset<bytes.length;offset+=256*1024)await client.call('files.upload.chunk',{session_id:session,upload_id,offset,base64:bytes.subarray(offset,offset+256*1024).toString('base64')});
 return client.call('files.upload.finish',{session_id:session,upload_id,name,text:text??''});
}
async function turn(session,prompt,attachments=[]){
 await client.call('turn.start',{session_id:session,prompt,mode:'full_access',attachments:attachments.map(id=>({id}))});
 for(let i=0;i<300;i++){const sessions=await client.call('sessions.list');if(!sessions.find(s=>s.id===session).running)return client.call('sessions.events',{session_id:session,after_seq:0});await delay(100);}
 throw Error('Fixture turn did not finish.');
}
try{
 await start();await client.call('onboarding.done');
 const {id}=await client.call('sessions.create',{project_id:null,agent:'Protocol fixture'});
 const source=await upload(id,'source.md');const image=await upload(id,'image.png');const pdf=await upload(id,'evidence.pdf','[Page 1]\nVatra local proof - page one\n[Page 2]\nSecond page: document evidence 42');
 await upload(id,'document.docx','Ședință Vatra – document local\nVenit\t42');await upload(id,'buget.xlsx','[Sheet: Buget]\nCategorie\tValoare\nȘedință\t42');await upload(id,'data.csv');await upload(id,'data.json');
 let events=await turn(id,'CASE_COMPARE: Compare the image and document.',[image.id,pdf.id,source.id]);
 assert.equal(events.at(-1).event.type,'turn_completed');assert.ok(requests.at(-1).messages.some(m=>Array.isArray(m.content)&&m.content.some(p=>p.type==='image_url')));assert.match(JSON.stringify(requests.at(-1)),/document evidence 42/);
 events=await turn(id,'CASE_CREATE: Create a file using Write.');assert.equal(events.at(-1).event.type,'turn_completed');
 let files=await client.call('files.list',{session_id:id});let result=files.find(f=>f.name==='report.md');assert.ok(result);assert.equal(result.origin,'created');
 await turn(id,'CASE_EDIT: Update the actual result.');files=await client.call('files.list',{session_id:id});result=files.find(f=>f.id===result.id);assert.equal(result.revision,2);
 const versions=await client.call('files.versions',{session_id:id,id:result.id});assert.equal(versions.length,2);
 assert.match((await client.call('files.text',{session_id:id,id:result.id,revision:1})).text,/first version/);
 assert.match((await client.call('files.text',{session_id:id,id:result.id})).text,/second version/);
 const beforeFalse=files.length;await turn(id,'CASE_FALSE');assert.equal((await client.call('files.list',{session_id:id})).length,beforeFalse);
 await turn(id,'CASE_SHELL: Create a real file through the command tool.');files=await client.call('files.list',{session_id:id});assert.ok(files.some(f=>f.name==='from-shell.txt'));
 const other=(await client.call('sessions.create',{project_id:null,agent:'Protocol fixture'})).id;
 await assert.rejects(client.call('files.read',{session_id:other,id:source.id}));
 const exported=join(profile,'exported-report.md');await client.call('files.export',{session_id:id,id:result.id,path:exported});assert.match(await readFile(exported,'utf8'),/second version/);
 const located=await client.call('files.materialize',{session_id:id,id:result.id,revision:1});assert.match(await readFile(located.path,'utf8'),/first version/);
 const ui={drafts:{[id]:'Draft restored after restart'},sessions:{[id]:{draft:[{id:source.id}],tabs:[pdf.id,result.id],active:pdf.id,open:true}},edits:{},width:480};await client.call('conversations.ui.set',ui);
 await client.call('workspace.backup',{path:join(profile,'complete-backup.db')});
 const expected=files.map(f=>[f.id,f.revision,f.hash]);await stop();await start();
 assert.deepEqual((await client.call('files.list',{session_id:id})).map(f=>[f.id,f.revision,f.hash]),expected);assert.deepEqual(await client.call('conversations.ui.get'),ui);
 const persisted=await client.call('files.read',{session_id:id,id:result.id,revision:1});assert.match(Buffer.from(persisted.base64,'base64').toString(),/first version/);
 const summary={kind:'Release service/engine integration with model fixture',session:id,profile,files:files.length,checks:['image+document payload','real Write/Edit snapshots','shell discovery','false claims excluded','cross-session protection','export and materialize historical version','service restart','draft/tabs/context persistence','SQLite backup includes managed bytes'],nativeGui:'not verified',realModel:'not used in this fixture test'};
 await writeFile(join(base,'service-results.json'),JSON.stringify(summary,null,2));console.log(JSON.stringify(summary));
}finally{if(client)await stop().catch(error=>console.error(error.message));await new Promise(resolve=>provider.close(resolve));}
