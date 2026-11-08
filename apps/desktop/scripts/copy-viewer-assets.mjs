import {cpSync,mkdirSync} from 'node:fs';
import {dirname,join} from 'node:path';
import {fileURLToPath} from 'node:url';
import './third-party-notices.mjs';
const root=join(dirname(fileURLToPath(import.meta.url)),'..');
for(const name of ['cmaps','standard_fonts','wasm']){
  const dest=join(root,'public','pdfjs',name);mkdirSync(dest,{recursive:true});
  cpSync(join(root,'node_modules','pdfjs-dist',name),dest,{recursive:true});
}
