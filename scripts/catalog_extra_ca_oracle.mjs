// Invoke unchanged fixed tls-fetch.ts; expected values are never implemented here.
import {mock} from 'bun:test';
import * as fs from 'node:fs';
import * as tls from 'node:tls';
import * as path from 'node:path';
import {pathToFileURL,fileURLToPath} from 'node:url';
import {createHash} from 'node:crypto';
const manifestPath=path.resolve(process.argv[2]),root=path.dirname(manifestPath),manifest=JSON.parse(fs.readFileSync(manifestPath,'utf8'));
const digest=bytes=>createHash('sha256').update(bytes).digest('hex');
if(Bun.version!=='1.4.0'||digest(fs.readFileSync(process.execPath))!==manifest.bunSha256||digest(fs.readFileSync(fileURLToPath(import.meta.url)))!==manifest.runnerSha256)throw Error('runtime/runner drift');
for(const source of manifest.sourceManifest){const bytes=fs.readFileSync(path.join(root,'upstream',source.path));if(digest(bytes)!==source.sha256||createHash('sha1').update(`blob ${bytes.length}\0`).update(bytes).digest('hex')!==source.gitBlob)throw Error(`source drift: ${source.path}`);}
let state={files:{},stats:[],reads:[]};
const originalFs={...fs};
const errorFor=e=>Object.assign(new Error(e.message),{name:e.name??'Error',code:e.code});
mock.module('node:fs',()=>({...originalFs,
 statSync:file=>{if(!String(file).startsWith('/fixture/'))return originalFs.statSync(file);state.stats.push(file);const entry=state.files[file];if(!entry)throw errorFor({code:'ENOENT',message:'missing stat'});if(entry.statError)throw errorFor(entry.statError);return{mtimeMs:entry.mtimeMs};},
 readFileSync:(file,encoding)=>{if(!String(file).startsWith('/fixture/'))return originalFs.readFileSync(file,encoding);state.reads.push(file);const entry=state.files[file];if(!entry)throw errorFor({code:'ENOENT',message:'missing read'});if(entry.readError)throw errorFor(entry.readError);return entry.contents;},
}));
const source=await import(pathToFileURL(path.join(root,'upstream/packages/utils/src/tls-fetch.ts')).href);
if(digest(originalFs.readFileSync(path.join(root,'reference-probes.mjs')))!==manifest.referenceProbesSha256)throw Error('reference probe drift');
const {referenceProbe}=await import(pathToFileURL(path.join(root,'reference-probes.mjs')).href);
function encode(value){const undefinedPaths=[],ownKeys=[];function visit(v,p){if(v===undefined){undefinedPaths.push(p);return;}if(v&&typeof v==='object'){ownKeys.push({path:p,keys:Object.keys(v)});for(const key of Object.keys(v))visit(v[key],[...p,key]);}}visit(value,[]);return{rawWireJSON:JSON.stringify(value)??null,undefinedPaths,ownKeys};}
function decode(encoded){if(!encoded)return undefined;let value=encoded.rawWireJSON===null?undefined:JSON.parse(encoded.rawWireJSON);for(const p of encoded.undefinedPaths??[]){if(!p.length){value=undefined;continue;}let record=value;for(const key of p.slice(0,-1))record=record[key];Object.defineProperty(record,p.at(-1),{value:undefined,enumerable:true,writable:true,configurable:true});}return value;}
const PEM='-----BEGIN CERTIFICATE-----\nfixture-a\n-----END CERTIFICATE-----\n',SECOND='-----BEGIN CERTIFICATE-----\nfixture-b\n-----END CERTIFICATE-----\n';
const cases=[],add=(id,steps)=>cases.push({id,steps});
const env=value=>({op:'env',...(value!==undefined?{value}: {})}),wrap=(from='base',to='wrapped')=>({op:'wrap',from,to}),fetch=(from='wrapped',init)=>({op:'fetch',from,...(init!==undefined?{init:encode(init)}:{})}),file=(contents=PEM,mtimeMs=100)=>({op:'file',path:'/fixture/ca',entry:{contents,mtimeMs}});
add('unset-gate',[env(),wrap(),fetch()]);add('whitespace-gate',[env(' \t\r\n\u00a0\ufeff'),wrap(),fetch()]);
add('gate-does-not-enable-later',[env(),wrap(),env(PEM),fetch()]);
add('inline-escaped',[env(` \ufeff${PEM.replaceAll('\n','\\n')}\u00a0 `),wrap(),fetch()]);
add('inline-header-is-enough',[env('abc-----BEGIN PRIVATE KEY-----\\nxyz'),wrap(),fetch()]);
add('inline-env-rotation',[env(PEM),wrap(),fetch(),env(SECOND),fetch()]);
add('installed-wrapper-env-unset',[env(PEM),wrap(),fetch(),env(),fetch()]);
add('file-extensionless',[file(),env(' /fixture/ca '),wrap(),fetch()]);
add('file-cache-hit',[file(),env('/fixture/ca'),wrap(),fetch(),fetch()]);
add('file-rotation',[file(),env('/fixture/ca'),wrap(),fetch(),file(SECOND,101),fetch()]);
add('file-same-mtime-is-cached',[file(),env('/fixture/ca'),wrap(),fetch(),file(SECOND),fetch()]);
add('reset-reloads-file',[file(),env('/fixture/ca'),wrap(),fetch(),file(SECOND),{op:'reset'},fetch()]);
add('file-empty',[file(''),env('/fixture/ca'),wrap(),fetch('wrapped',{tls:{ca:'curated'},tag:'same'})]);
add('missing-path-error',[env('/fixture/missing'),wrap(),fetch(),fetch()]);
add('stat-failure-read-success',[{op:'file',path:'/fixture/ca',entry:{contents:PEM,statError:{code:'EACCES',message:'stat denied'}}},env('/fixture/ca'),wrap(),fetch(),fetch()]);
add('read-failure-bubbles',[{op:'file',path:'/fixture/ca',entry:{mtimeMs:100,readError:{code:'EACCES',name:'PermissionError',message:'read denied'}}},env('/fixture/ca'),wrap(),fetch()]);
add('read-missing-extra-error',[{op:'file',path:'/fixture/ca',entry:{mtimeMs:100,readError:{code:'ENOENT',message:'read disappeared'}}},env('/fixture/ca'),wrap(),fetch()]);
add('deleted-after-cache-errors',[file(),env('/fixture/ca'),wrap(),fetch(),{op:'deleteFile',path:'/fixture/ca'},fetch()]);
add('failed-read-keeps-old-cache',[file(),env('/fixture/ca'),wrap(),fetch(),{op:'file',path:'/fixture/ca',entry:{mtimeMs:101,readError:{code:'EIO',message:'read broken'}}},fetch(),file(SECOND),fetch()]);
for(const [id,init] of [
 ['no-init',undefined],['empty-init',{}],['tls-absent',{method:'POST',headers:{x:'y'},tag:undefined}],
 ['tls-fields',{tls:{cert:'cert',key:'key',rejectUnauthorized:false,serverName:'proxy',ciphers:'cipher'},tag:'kept'}],
 ['ca-string',{tls:{ca:'curated'},tag:'kept'}],['ca-array',{tls:{ca:['curated-a','curated-b'],key:'key'}}],
 ['ca-empty-array',{tls:{ca:[]}}],['ca-undefined',{tls:{ca:undefined,key:undefined}}],
 ['ca-null',{tls:{ca:null}}],['tls-null',{tls:null}],['native-array-undefined',{tls:{ca:['one',undefined,'two'],tag:undefined}}],
 ['init-string','AB'],['init-array',['A','B']],['tls-string',{tls:'AB'}],['tls-array',{tls:['A','B']}],
 ]) add(`merge/${id}`,[env(PEM),wrap(),fetch('wrapped',init),{op:'originalInit',init:encode(init)}]);
add('idempotent-wrapper',[env(PEM),wrap(),wrap('wrapped','twice'),fetch('twice')]);
add('idempotent-after-unset',[env(PEM),wrap(),env(),wrap('wrapped','twice'),fetch('twice')]);
add('preconnect-identity',[env(PEM),{op:'preconnect',fetch:'base',identity:77},wrap(),{op:'observeFetch',fetch:'wrapped'}]);
add('preconnect-snapshotted',[env(PEM),{op:'preconnect',fetch:'base',identity:77},wrap(),{op:'preconnect',fetch:'base',identity:88},{op:'observeFetch',fetch:'wrapped'}]);
add('preconnect-absent-stays-absent',[env(PEM),wrap(),{op:'preconnect',fetch:'base',identity:88},{op:'observeFetch',fetch:'wrapped'}]);
add('options-unset-undefined',[env(),{op:'options',to:'options'}, {op:'withOptions',from:'options',to:'merged'}]);
add('options-unset-identity',[env(),{op:'options',to:'options',fields:encode({tag:'a',fetch:undefined})},{op:'withOptions',from:'options',to:'merged'}]);
add('options-supplied-fetch',[env(PEM),{op:'options',to:'options',fields:encode({tag:'a'}),fetch:'base'},{op:'withOptions',from:'options',to:'merged'},{op:'optionsFetch',from:'merged',to:'selected'},fetch('selected')]);
add('options-default-global',[env(PEM),{op:'options',to:'options',fields:encode({tag:'a'})},{op:'withOptions',from:'options',to:'merged'},{op:'optionsFetch',from:'merged',to:'selected'},fetch('selected')]);
add('options-undefined-global',[env(PEM),{op:'options',to:'options'},{op:'withOptions',from:'options',to:'merged'},{op:'optionsFetch',from:'merged',to:'selected'},fetch('selected')]);
add('options-wrapped-identity',[env(PEM),wrap(),{op:'options',to:'options',fields:encode({tag:'a'}),fetch:'wrapped'},{op:'withOptions',from:'options',to:'merged'}]);
add('options-null-already-wrapped-global',[env(PEM),wrap(),{op:'globalFetch',from:'wrapped'},{op:'options',to:'options',fields:encode({fetch:null,tag:'a'})},{op:'withOptions',from:'options',to:'merged'}]);
add('options-undefined-already-wrapped-global',[env(PEM),wrap(),{op:'globalFetch',from:'wrapped'},{op:'options',to:'options',fields:encode({fetch:undefined,tag:'a'})},{op:'withOptions',from:'options',to:'merged'}]);
add('options-absent-already-wrapped-global',[env(PEM),wrap(),{op:'globalFetch',from:'wrapped'},{op:'options',to:'options'},{op:'withOptions',from:'options',to:'merged'}]);
add('error-no-cause',[{op:'error',message:'configured badly'}]);
add('error-with-cause',[{op:'error',message:'configured badly',cause:encode({code:'SOURCE',detail:undefined})}]);
for(const kind of ['init','ca-elements','passthrough','options','options-wrapped','prototype-cycles','options-mutation','options-nullish-mutation'])add(`reference/${kind}`,[env(kind==='passthrough'?undefined:PEM),{op:'referenceProbe',kind}]);
const originalEnv=Bun.env.NODE_EXTRA_CA_CERTS,originalGlobal=globalThis.fetch;
for(const test of cases){
 source.__resetExtraCaCache();state={files:{},stats:[],reads:[]};delete Bun.env.NODE_EXTRA_CA_CERTS;
 const calls=[],events=[],registry=new Map(),options=new Map();let lastInit;
 const makeFetch=name=>Object.assign(async(input,init)=>{const record={fetch:name,input,init:encode(init),initSame:init===lastInit,tlsSame:init?.tls===lastInit?.tls,caSame:init?.tls?.ca===lastInit?.tls?.ca};calls.push(record);return new Response('ok');},{tag:name});
 registry.set('base',makeFetch('base'));registry.set('global',makeFetch('global'));globalThis.fetch=registry.get('global');
 for(const step of test.steps){try{
  if(step.op==='env'){if(step.value===undefined)delete Bun.env.NODE_EXTRA_CA_CERTS;else Bun.env.NODE_EXTRA_CA_CERTS=step.value;}
  else if(step.op==='file')state.files[step.path]={...step.entry};
  else if(step.op==='deleteFile')delete state.files[step.path];
  else if(step.op==='reset')source.__resetExtraCaCache();
  else if(step.op==='wrap'){const input=registry.get(step.from),out=source.wrapFetchForExtraCa(input);registry.set(step.to,out);events.push({op:step.op,same:out===input});}
  else if(step.op==='fetch'){lastInit=decode(step.init);await registry.get(step.from)('https://fixture.test/models',lastInit);events.push({op:step.op,ok:true});}
  else if(step.op==='originalInit')events.push({op:step.op,value:encode(lastInit)});
  else if(step.op==='preconnect')registry.get(step.fetch).preconnect=Object.assign(()=>{}, {identity:step.identity});
  else if(step.op==='observeFetch')events.push({op:step.op,preconnect:registry.get(step.fetch).preconnect?.identity??null});
  else if(step.op==='globalFetch')globalThis.fetch=registry.get(step.from);
  else if(step.op==='options'){const value=decode(step.fields);if(value&&step.fetch)value.fetch=registry.get(step.fetch);options.set(step.to,value);}
  else if(step.op==='withOptions'){const value=options.get(step.from),out=source.withExtraCaFetch(value);options.set(step.to,out);events.push({op:step.op,same:out===value,fetchSame:out?.fetch===value?.fetch,keys:out?Object.keys(out):null});}
  else if(step.op==='optionsFetch')registry.set(step.to,options.get(step.from).fetch);
  else if(step.op==='error'){const cause=decode(step.cause),error=new source.ExtraCaError(step.message,step.cause?{cause}:undefined);events.push({op:step.op,name:error.name,message:error.message,hasOwnCause:Object.hasOwn(error,'cause'),cause:encode(error.cause),causeSame:error.cause===cause});}
  else if(step.op==='referenceProbe')events.push({op:step.op,result:await referenceProbe(step.kind,source)});
 }catch(error){events.push({op:step.op,error:{name:error.name,message:error.message}});}}
 test.expected={calls,events,stats:state.stats,reads:state.reads};
}
if(originalEnv===undefined)delete Bun.env.NODE_EXTRA_CA_CERTS;else Bun.env.NODE_EXTRA_CA_CERTS=originalEnv;globalThis.fetch=originalGlobal;
const output={...manifest,rootCertificates:[...tls.rootCertificates],cases};
fs.writeFileSync(path.join(root,'oracle.json'),JSON.stringify(output));console.log(JSON.stringify({cases:cases.length}));
