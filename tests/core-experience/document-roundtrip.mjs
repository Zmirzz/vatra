// Offline integration: production Office worker, real PDF.js extraction/rasterization and native fixtures.
import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
import {mkdir,writeFile,readdir,readFile} from 'node:fs/promises';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {dirname,join} from 'node:path';
import {Worker} from 'node:worker_threads';
const root=fileURLToPath(new URL('../../',import.meta.url));
const require=createRequire(join(root,'apps/desktop/package.json'));
const ExcelJS=require('exceljs');
const zipRequire=createRequire(require.resolve('exceljs'));
const JSZip=zipRequire('jszip');
const {createCanvas}=require('@napi-rs/canvas');
const out=join(root,'.superpowers/core-experience/fixtures');await mkdir(out,{recursive:true});
const book=new ExcelJS.Workbook();const sheet=book.addWorksheet('Buget');
sheet.addRows([['Categorie','Valoare'],['Ședință',42],['Formulă',{formula:'SUM(B2,8)',result:50}]]);
await book.xlsx.writeFile(join(out,'buget.xlsx'));
const zip=new JSZip();
zip.file('[Content_Types].xml','<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>');
zip.file('_rels/.rels','<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>');
zip.file('word/document.xml','<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Ședință Vatra – document local</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>Venit</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>42</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>');
await writeFile(join(out,'document.docx'),await zip.generateAsync({type:'nodebuffer'}));
function pdfDocument(){
 const objects=['<< /Type /Catalog /Pages 2 0 R >>','<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>','<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 7 0 R >> >> /Contents 4 0 R >>',null,'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 7 0 R >> >> /Contents 6 0 R >>',null,'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>'];
 for(const [index,line] of [[3,'Vatra local proof - page one'],[5,'Second page: document evidence 42']]){const content=`BT /F1 20 Tf 50 720 Td (${line}) Tj ET`;objects[index]=`<< /Length ${Buffer.byteLength(content)} >>\nstream\n${content}\nendstream`;}
 let result='%PDF-1.4\n';const offsets=[0];for(let i=0;i<objects.length;i++){offsets.push(Buffer.byteLength(result));result+=`${i+1} 0 obj\n${objects[i]}\nendobj\n`;}
 const xref=Buffer.byteLength(result);result+=`xref\n0 ${objects.length+1}\n0000000000 65535 f \n`+offsets.slice(1).map(n=>`${String(n).padStart(10,'0')} 00000 n \n`).join('')+`trailer\n<< /Size ${objects.length+1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;return Buffer.from(result);
}
await writeFile(join(out,'evidence.pdf'),pdfDocument());
const image=createCanvas(640,360),ctx=image.getContext('2d');ctx.fillStyle='#edf2fa';ctx.fillRect(0,0,640,360);ctx.fillStyle='#245ea8';ctx.fillRect(60,70,150,150);ctx.fillStyle='#17171a';ctx.font='30px sans-serif';ctx.fillText('Vatra local image',250,145);ctx.font='20px sans-serif';ctx.fillText('Blue square · 42',250,185);await writeFile(join(out,'image.png'),image.toBuffer('image/png'));
await writeFile(join(out,'source.md'),'# Sursă locală\n\nCodul de verificare este VATRA-42.\n\n| Element | Valoare |\n| --- | --- |\n| Ședință | 42 |\n');
await writeFile(join(out,'data.csv'),'name,value\n"Ședință, local",42\nFormula,=1+1\n');
await writeFile(join(out,'data.json'),JSON.stringify({proof:'VATRA-42',local:true},null,2));
const assets=join(root,'apps/desktop/dist/assets');const entry=(await readdir(assets)).find(n=>n.startsWith('office.worker-')&&n.endsWith('.js'));assert.ok(entry,'Build the frontend first.');
async function extract(format,path){
 const worker=new Worker(new URL('./worker-runner.mjs',import.meta.url),{workerData:{module:pathToFileURL(join(assets,entry)).href}});
 try{return await new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(Error('Worker timeout')),45000);worker.on('error',e=>{clearTimeout(timer);reject(e);});worker.on('message',async result=>{if(result.ready){worker.postMessage({format,bytes:new Uint8Array(await readFile(path))});return;}clearTimeout(timer);if(result.error)reject(Error(result.error));else resolve(result);});});}finally{await worker.terminate();}
}
const docx=await extract('docx',join(out,'document.docx'));assert.match(docx.html,/Ședință Vatra/);assert.match(docx.html,/<table>/);assert.match(docx.html,/42/);
const xlsx=await extract('xlsx',join(out,'buget.xlsx'));assert.match(xlsx.text,/Ședință/);assert.match(xlsx.text,/SUM\(B2,8\)/);assert.match(xlsx.text,/cached: 50/);
const pdfjs=await import(pathToFileURL(require.resolve('pdfjs-dist/legacy/build/pdf.mjs')).href);
const task=pdfjs.getDocument({data:new Uint8Array(await readFile(join(out,'evidence.pdf'))),standardFontDataUrl:join(root,'apps/desktop/node_modules/pdfjs-dist/standard_fonts/')+'/'});
const pdf=await task.promise;assert.equal(pdf.numPages,2);
const page=await pdf.getPage(2);const content=await page.getTextContent();assert.match(content.items.map(i=>i.str??'').join(' '),/evidence 42/);
const viewport=page.getViewport({scale:1});const rendered=createCanvas(viewport.width,viewport.height);await page.render({canvas:rendered,canvasContext:rendered.getContext('2d'),viewport}).promise;await writeFile(join(out,'pdf-page-2.png'),rendered.toBuffer('image/png'));await task.destroy();
const result={officeWorker:'production bundle',docx:'Unicode paragraphs and table preserved',xlsx:'Sheet, Unicode cells and cached formula preserved without execution',pdf:'Two pages, text extraction and page 2 rasterization passed',fixtures:out};
await writeFile(join(root,'.superpowers/core-experience/document-results.json'),JSON.stringify(result,null,2));console.log(JSON.stringify(result));
