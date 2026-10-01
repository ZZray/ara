// Actual unchanged Cursor source through node:http2, without the fetch CA shim.
import { mock } from 'bun:test';
import * as fs from 'node:fs';
import * as path from 'node:path';
import * as http2 from 'node:http2';
import { createHash } from 'node:crypto';
import { pathToFileURL } from 'node:url';
const [mode,sourceRun,configFile]=process.argv.slice(2);
const config=JSON.parse(fs.readFileSync(configFile,'utf8'));
const manifest=JSON.parse(fs.readFileSync(path.join(sourceRun,'source-manifest.json'),'utf8'));
const digest=bytes=>createHash('sha256').update(bytes).digest('hex');
if(manifest.upstreamCommit!=='596f2da7101178214aa27a753529d15e6b7ad91d'||Bun.version!=='1.4.0'||digest(fs.readFileSync(process.execPath))!==manifest.bunSha256)throw Error('source/runtime mismatch');
for(const entry of [...manifest.sourceManifest,...manifest.licenses]){
 const bytes=fs.readFileSync(path.join(sourceRun,'upstream',entry.path));
 if(digest(bytes)!==entry.sha256||createHash('sha1').update(`blob ${bytes.length}\0`).update(bytes).digest('hex')!==entry.gitBlob)throw Error(`source drift ${entry.path}`);
}
if(mode==='--server'){
 const requests=[],sessions=new Set();const flush=()=>fs.writeFileSync(config.receipts,JSON.stringify({requests}));
 const bind=async(server,scheme)=>{
  server.on('session',session=>{sessions.add(session);session.on('error',()=>{});session.on('close',()=>sessions.delete(session));});
  server.on('stream',(stream,headers)=>{stream.on('error',()=>{});const chunks=[];stream.on('data',chunk=>chunks.push(chunk));stream.on('end',()=>{
   requests.push({scheme,method:headers[':method'],path:headers[':path'],authorization:headers.authorization,bodyBase64:Buffer.concat(chunks).toString('base64'),http2:true});flush();
   stream.respond({':status':200});stream.end(Buffer.from([0,0,0,0,0]));
  });});
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));return server.address().port;
 };
 const clearPort=await bind(http2.createServer(),'http');
 const tlsPort=await bind(http2.createSecureServer({cert:fs.readFileSync(config.cert),key:fs.readFileSync(config.key),allowHTTP1:false}),'https');
 fs.writeFileSync(config.ready,JSON.stringify({clearPort,tlsPort,pid:process.pid}));
}else if(mode==='--client'){
 const load=p=>import(pathToFileURL(path.join(sourceRun,'upstream',p)).href);
 const guards=await load('packages/utils/src/type-guards.ts'),tls=await load('packages/utils/src/tls-fetch.ts');
 const abortable=await load('packages/utils/src/abortable.ts'),retry=await load('packages/utils/src/fetch-retry.ts');
 const sqlite=await load('packages/utils/src/sqlite.ts'),fsErrors=await load('packages/utils/src/fs-error.ts'),dirs=await load('packages/utils/src/dirs.ts');
 const logger=Object.fromEntries(['debug','info','warn','error','trace','fatal'].map(level=>[level,()=>{}]));
 mock.module('@oh-my-pi/omptype',()=>load('packages/omptype/src/index.ts'));
 mock.module('@oh-my-pi/pi-utils',()=>({...guards,...tls,...abortable,...retry,...sqlite,...fsErrors,...dirs,logger}));
 mock.module('@oh-my-pi/pi-utils/logger',()=>logger);
 const cursor=await load('packages/catalog/src/discovery/cursor.ts');
 const startupEnv=process.env.NODE_EXTRA_CA_CERTS??null,steps=[];
 for(const step of config.steps){
  if(Object.hasOwn(step,'env')){if(step.env===null)delete process.env.NODE_EXTRA_CA_CERTS;else process.env.NODE_EXTRA_CA_CERTS=step.env;}
  const models=await cursor.fetchCursorUsableModels({apiKey:'fixture-token',baseUrl:step.url,timeoutMs:500});
  steps.push({id:step.id,env:process.env.NODE_EXTRA_CA_CERTS??null,models});
 }
 console.log(JSON.stringify({startupEnv,steps}));
}else throw Error('unknown mode');
