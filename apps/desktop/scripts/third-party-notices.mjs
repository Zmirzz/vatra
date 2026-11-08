import {readFileSync,writeFileSync,existsSync,readdirSync} from 'node:fs';
import {dirname,join} from 'node:path';
import {fileURLToPath} from 'node:url';
const app=join(dirname(fileURLToPath(import.meta.url)),'..');
const lock=JSON.parse(readFileSync(join(app,'package-lock.json'),'utf8'));
let text='Vatra desktop production dependencies and bundled viewer assets.\nGenerated from package-lock.json; package scripts are not executed.\n\n';
for(const [location,item] of Object.entries(lock.packages).sort(([a],[b])=>a.localeCompare(b))){
 if(!location||item.dev)continue;const folder=join(app,location),metadata=join(folder,'package.json');if(!existsSync(metadata))continue;
 const pkg=JSON.parse(readFileSync(metadata,'utf8'));text+=`\n=== ${pkg.name}@${pkg.version} (${typeof pkg.license==='string'?pkg.license:JSON.stringify(pkg.license??'See package notices')}) ===\n\n`;
 const files=readdirSync(folder).filter(name=>/^(licen[cs]e|copying|notice)(\.|$)/i.test(name));
 for(const dir of ['standard_fonts','wasm'])if(existsSync(join(folder,dir)))for(const name of readdirSync(join(folder,dir)))if(/^licen[cs]e/i.test(name))files.push(join(dir,name));
 for(const name of files){try{text+=readFileSync(join(folder,name),'utf8')+'\n';}catch{}}
}
writeFileSync(join(app,'src-tauri/resources/licenses/core-experience-third-party.txt'),text);
