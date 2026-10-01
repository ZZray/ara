// Expected values are computed only by unchanged fixed OMP source modules.
import { mock } from 'bun:test';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { createHash } from 'node:crypto';
import { pathToFileURL, fileURLToPath } from 'node:url';
const manifestPath=path.resolve(process.argv[2]);
const root=path.dirname(manifestPath), manifest=JSON.parse(fs.readFileSync(manifestPath,'utf8'));
const digest=bytes=>createHash('sha256').update(bytes).digest('hex');
if(Bun.version!=='1.4.0'||manifest.upstreamCommit!=='596f2da7101178214aa27a753529d15e6b7ad91d')throw Error('wrong source/runtime');
if(digest(fs.readFileSync(process.execPath))!==manifest.bunSha256||digest(fs.readFileSync(fileURLToPath(import.meta.url)))!==manifest.runnerSha256)throw Error('runtime/runner mismatch');
for(const entry of [...manifest.sourceManifest,...manifest.licenses]){
 const bytes=fs.readFileSync(path.join(root,'upstream',entry.path));
 if(digest(bytes)!==entry.sha256||createHash('sha1').update(`blob ${bytes.length}\0`).update(bytes).digest('hex')!==entry.gitBlob)throw Error(`source mismatch: ${entry.path}`);
}
for(const adapter of manifest.adapters)if(digest(fs.readFileSync(path.join(root,adapter.path)))!==adapter.sha256)throw Error('fixture adapter mismatch');

export function encode(value){
 const undefinedPaths=[],specialNumbers=[],ownKeys=[];
 function visit(v,p){
  if(v===undefined){undefinedPaths.push(p);return;}
  if(typeof v==='number'&&(!Number.isFinite(v)||Object.is(v,-0))){specialNumbers.push({path:p,value:Object.is(v,-0)?'-0':String(v)});return;}
  if(v&&typeof v==='object'){
   const keys=Object.keys(v);ownKeys.push({path:p,keys});
   for(const key of keys)visit(v[key],[...p,key]);
  }
 }
 visit(value,[]);return{rawWireJSON:JSON.stringify(value)??null,undefinedPaths,specialNumbers,ownKeys};
}
export function decode(encoded){
 let value=encoded.rawWireJSON===null?undefined:JSON.parse(encoded.rawWireJSON);
 const assign=(p,v)=>{if(!p.length){value=v;return;}let target=value;for(const key of p.slice(0,-1))target=target[key];Object.defineProperty(target,p.at(-1),{value:v,enumerable:true,writable:true,configurable:true});};
 for(const p of encoded.undefinedPaths??[])assign(p,undefined);
 for(const item of encoded.specialNumbers??[])assign(item.path,Number(item.value));
 for(const item of [...(encoded.ownKeys??[])].reverse()){
  let target=value;for(const key of item.path)target=target[key];if(!target||typeof target!=='object'||Array.isArray(target))continue;
  const entries=item.keys.map(key=>[key,target[key]]);for(const key of Object.keys(target))delete target[key];
  for(const[key,v]of entries)Object.defineProperty(target,key,{value:v,enumerable:true,writable:true,configurable:true});
 }
 return value;
}
const importRaw=p=>import(pathToFileURL(path.join(root,'upstream',p)).href);
const load=p=>importRaw(`packages/catalog/src/${p.replace(/\.ts$/,'')}.ts`);
const logs=[];
const logger=Object.fromEntries(['debug','info','warn','error','trace','fatal'].map(level=>[level,(...args)=>logs.push({level,args:encode(args)})]));
const guards=await importRaw('packages/utils/src/type-guards.ts');
const tls=await importRaw('packages/utils/src/tls-fetch.ts');
const abortable=await importRaw('packages/utils/src/abortable.ts');
const retry=await importRaw('packages/utils/src/fetch-retry.ts');
const sqlite=await importRaw('packages/utils/src/sqlite.ts');
const fsErrors=await importRaw('packages/utils/src/fs-error.ts');
const dirs=await importRaw('packages/utils/src/dirs.ts');
const omptype=await importRaw('packages/omptype/src/index.ts');
mock.module('@oh-my-pi/omptype',()=>omptype);
mock.module('@oh-my-pi/pi-utils',()=>({...guards,...tls,...abortable,...retry,...sqlite,...fsErrors,...dirs,logger}));
mock.module('@oh-my-pi/pi-utils/logger',()=>logger);
const adapters=await Promise.all(manifest.adapters.map(adapter=>import(pathToFileURL(path.join(root,adapter.path)).href)));

const ok=json=>({status:200,json});
const avail=(ref='claude_opus_4_8')=>({data:{aiChatAvailableModels:{defaultModel:{ref,name:'Chosen'}}}});
function fixtures(){
 const cases=[],add=(id,family,module,op,options,responses=[],extra={})=>cases.push({id,family,module,op,options,responses,...extra});
 const codex=(id,payload,options={},extra={})=>add(`codex/${id}`,'codex','discovery/codex','fetchCodexModels',{accessToken:'fixture-token',...options},[ok(payload)],extra);
 codex('empty-object',{});codex('empty-models',{models:[]});codex('data-alternative',{data:[{id:'plain'}]});
 codex('models-precedence',{models:[],data:[{id:'ignored'}]});
 codex('normalization',{models:[null,[],5,{}, {id:' second ',display_name:' Name ',context_window:0.9,priority:-2,input_modalities:['IMAGE','bad','text','image']},
  {slug:' hidden ',visibility:' HIDDEN '},{slug:'none',default_reasoning_level:'none',supported_reasoning_levels:[null,'high',{effort:' HIGH '}]},
  {slug:'lite',prefer_websockets:true,use_responses_lite:true,tool_mode:'code_mode_only',context_window:900_000}]});
 codex('worker-safe-and-unknown',{models:[{slug:'gpt-5.6-luna-wm',context_window:272000},{slug:'private-wm',context_window:14},{slug:'-wm'}]});
 codex('worker-plain-wins',{models:[{slug:'gpt-5.6-sol-wm',display_name:'Worker'},{slug:'gpt-5.6-sol',display_name:'Explicit'}]});
 codex('worker-hidden-plain',{models:[{slug:'gpt-5.6-terra-wm'},{slug:'gpt-5.6-terra',visibility:'hidden'}]});
 codex('duplicates-retained',{models:[{slug:'x',priority:2},{slug:'x',priority:1},{slug:'a',priority:2}]});
 codex('taxonomy-limits',{models:['gpt-5.6','gpt-5.6-unknown','gpt-5.6-luna','gpt-5.6-sol','gpt-5.6-terra','gpt-6-astra','unknown'].map(slug=>({slug}))});
 codex('daybreak-aliases',{models:[{slug:'gpt-daybreak-blue-latest'},{slug:'gpt-daybreak-red-latest',context_window:400000},{slug:'gpt-5.6-luna',context_window:2000000}]});
 codex('lone-utf16',{models:[{slug:'a\ud800',display_name:'\udc00'},{slug:'é'},{slug:'Z'},{slug:'a'}]});
 codex('empty-modalities',{models:[{id:'x',input_modalities:[],prefer_websockets:'true',use_responses_lite:1,tool_mode:true}]});
 codex('headers-and-query',{models:[]},{baseUrl:' https://fixture.test/root/?client_version=old&x=2/// ',paths:[' /models?client_version=bad&a=b#anchor '],clientVersion:' 001.2.03 ',accountId:' acct ',headers:{Authorization:'old',Accept:'old',version:'bad','X-Custom':' value ','CHATGPT-ACCOUNT-ID':'caller'}});
 codex('blank-options',{models:[]},{baseUrl:'   ',paths:[' ',''],clientVersion:'bad'});
 for(const status of [401,403])add(`codex/rejected-${status}`,'codex','discovery/codex','fetchCodexModels',{accessToken:'fake'},[{status},ok({models:[{id:'must-not-fetch'}]})]);
 add('codex/all-fail','codex','discovery/codex','fetchCodexModels',{accessToken:'fake'},[{error:{name:'Error',message:'network'}},{status:503}]);
 add('codex/fallback-body','codex','discovery/codex','fetchCodexModels',{accessToken:'fake'},[{status:200,body:'not json'},ok({models:[{id:'fallback'}]})]);
 add('codex/fallback-shape','codex','discovery/codex','fetchCodexModels',{accessToken:'fake'},[ok({models:null}),ok({models:[],data:[]})]);
 add('codex/etag','codex','discovery/codex','fetchCodexModels',{accessToken:'fake'},[{status:200,headers:{etag:'  "catalog-v1"  '},json:{models:[]}}]);
 for(const [id,options]of [['bad-url',{baseUrl:'not a URL'}],['bad-header-name',{headers:{'bad name':'x'}}],['bad-header-value',{headers:{a:'x\ny'}}],['header-nonbyte',{accessToken:'\ud800'}]])
  add(`codex/${id}`,'codex','discovery/codex','fetchCodexModels',{accessToken:'fake',...options},[]);
 const special={models:[{slug:'nan',priority:NaN,context_window:Infinity},{slug:'negative-zero',priority:-0},{slug:'own-undefined',display_name:undefined,context_window:undefined}]};
 add('codex/native-values','codex','discovery/codex','fetchCodexModels',{accessToken:'fake'},[{status:200,jsonEncoded:encode(special)}]);
 for(const [id,payload]of [['null',null],['array',[]],['number',1],['string','x'],['null-data',{data:null}],['wrong-models',{models:{}}],['invalid-secondary',{models:[],data:'bad'}]])
  add(`codex/envelope-${id}`,'codex','discovery/codex','fetchCodexModels',{accessToken:'fake'},[ok(payload),{status:404}]);
 add('codex/own-undefined-envelope','codex','discovery/codex','fetchCodexModels',{accessToken:'fake'},[{status:200,jsonEncoded:encode({models:undefined,data:[{id:'from-data'}]})}]);
 codex('coercion-boundaries',{models:[false,'string',[],{slug:4,id:'fallback-id'}, {slug:' ',id:'fallback'}, {slug:'numeric',context_window:'3000',priority:'1',input_modalities:'text',default_reasoning_level:5,supported_reasoning_levels:['high',{effort:5}]},
  {slug:'infinite',context_window:-3,input_modalities:['audio']},{slug:'text-only',input_modalities:[' TEXT ']},{slug:'image-only',input_modalities:['image']},{slug:'none-effort',default_reasoning_level:' NONE ',supported_reasoning_levels:[{effort:' none '}]},
  {slug:'priority-max',priority:Number.MAX_SAFE_INTEGER},{slug:'priority-beyond',priority:Number.MAX_SAFE_INTEGER+1}]});
 add('codex/caller-abort-caught','codex','discovery/codex','fetchCodexModels',{accessToken:'fake'},[{error:{name:'AbortError',message:'abort'}},{error:{name:'AbortError',message:'abort'}}],{signal:{aborted:true}});
 codex('account-empty-preserves-caller',{models:[]},{accountId:' ',headers:{'chatgpt-account-id':' original '}});
 codex('path-order-and-etag-blank',{models:[]},{paths:['models',' /models ','']});
 const gl=(id,op,options,responses,extra={})=>add(`gitlab/${id}`,'gitlab','discovery/gitlab-duo-workflow',op,{apiKey:'fixture-token',baseUrl:'https://fixture.test',...options},responses,{env:{},...extra});
 gl('override-models','fetchGitLabDuoWorkflowModels',{namespaceId:' 42 '},[ok(avail()),ok(avail())]);
 gl('second-fetch-fails','fetchGitLabDuoWorkflowModels',{namespaceId:'42'},[ok(avail()),{status:503}]);
 gl('second-fetch-empty','fetchGitLabDuoWorkflowModels',{namespaceId:'42'},[ok(avail()),ok({data:{aiChatAvailableModels:{}}})]);
 gl('pinned-precedence','fetchGitLabDuoWorkflowModels',{namespaceId:'gid://gitlab/Namespace/42'},[ok(avail()),ok({data:{aiChatAvailableModels:{pinnedModel:{ref:' pinned ',name:' Pin '},selectableModels:[{ref:'ignored'}],defaultModel:{ref:'ignored-too'}}}})]);
 gl('selectable-order','fetchGitLabDuoWorkflowModels',{namespaceId:'42'},[ok(avail()),ok({data:{aiChatAvailableModels:{selectableModels:[{ref:'gemini-x'},{ref:'gpt-5-x',name:'Gpt'},{ref:'gemini-x',name:'Duplicate'},null,{ref:4},{ref:' \ud800 ',name:5}],defaultModel:{ref:'ignored'}}}})]);
 gl('runtime-override-enrich','discoverGitLabDuoWorkflowRuntimeNamespace',{namespaceId:'gid://gitlab/Group/9'},[ok({root_namespace:{id:1},full_path:' group '})]);
 gl('runtime-override-denied','discoverGitLabDuoWorkflowRuntimeNamespace',{namespaceId:'9'},[{status:403}]);
 gl('runtime-string-override','discoverGitLabDuoWorkflowRuntimeNamespace',{namespaceId:'group/path'},[]);
 gl('runtime-project','discoverGitLabDuoWorkflowRuntimeNamespace',{projectId:'group/project'},[ok({root_namespace_id:22})]);
 gl('model-namespace-drops-project','discoverGitLabDuoWorkflowNamespace',{projectId:'group/project'},[ok({root_namespace_id:22}),ok(avail())]);
 gl('numeric-project-graphql','discoverGitLabDuoWorkflowRuntimeNamespace',{projectId:'77'},[ok({namespace:{id:2},path_with_namespace:'group/sub/project'}),ok({data:{project:{namespace:{rootAncestor:{id:'gid://gitlab/Group/1'}}}}})]);
 gl('project-rest-fail-path-fallback','discoverGitLabDuoWorkflowRuntimeNamespace',{projectPath:'group/repo'},[{status:404},ok({data:{project:{namespace:{rootAncestor:{id:8}}}}})]);
 gl('groups-pagination-preference','discoverGitLabDuoWorkflowNamespace',{},[{status:200,headers:{'x-next-page':' 2 '},json:[{id:1,full_path:'a'}]},ok([{id:2,full_path:'b',duo_features_enabled:true},{id:3,duo_core_features_enabled:true}]),ok(avail())]);
 gl('groups-failure-keeps-earlier','discoverGitLabDuoWorkflowRuntimeNamespace',{},[{status:200,headers:{'x-next-page':'2'},json:[{id:1}]},{error:{name:'Error',message:'offline'}}]);
 gl('groups-skip-unavailable','discoverGitLabDuoWorkflowNamespace',{},[ok([{id:1},{id:2},{id:3}]),ok({data:{aiChatAvailableModels:null}}),ok({data:{aiChatAvailableModels:{selectableModels:[]}}}),ok(avail())]);
 gl('project-env-id','discoverGitLabDuoWorkflowRuntimeNamespace',{},[ok({root_namespace_id:42})],{env:{GITLAB_DUO_PROJECT_ID:'7',GITLAB_DUO_PROJECT_PATH:'must-not-use'}});
 gl('project-env-path','discoverGitLabDuoWorkflowRuntimeNamespace',{},[ok({root_namespace_id:42})],{env:{GITLAB_DUO_PROJECT_PATH:'group/project'}});
 gl('no-namespace','discoverGitLabDuoWorkflowNamespace',{},[ok([])]);
 gl('runtime-no-namespace','discoverGitLabDuoWorkflowRuntimeNamespace',{},[ok([])]);
 gl('pagination-bound','discoverGitLabDuoWorkflowRuntimeNamespace',{},Array.from({length:50},()=>({status:200,headers:{'x-next-page':'1'},json:[]})));
 gl('numeric-pathless-project','discoverGitLabDuoWorkflowRuntimeNamespace',{projectId:'77'},[ok({namespace:{id:2}}),ok([])]);
 gl('invalid-availability-falls-back','discoverGitLabDuoWorkflowNamespace',{namespaceId:'1'},[ok({data:{aiChatAvailableModels:5}}),ok([{id:2}]),ok(avail())]);
 gl('resilient-selectables','fetchGitLabDuoWorkflowModels',{namespaceId:'1'},[ok(avail()),ok({data:{aiChatAvailableModels:{selectableModels:'invalid',defaultModel:{ref:'fallback',name:3},pinnedModel:{ref:12}}}})]);
 gl('malformed-model-json','fetchGitLabDuoWorkflowModels',{namespaceId:'1'},[ok(avail()),{status:200,body:'not-json'}]);
 gl('bad-group-url','discoverGitLabDuoWorkflowRuntimeNamespace',{baseUrl:'not a URL'},[]);
 gl('override-gid-namespace','discoverGitLabDuoWorkflowRuntimeNamespace',{namespaceId:'gid://gitlab/Namespace/123'},[ok({rootAncestor:{fullPath:'root/path'},path:'sub'})]);
 gl('group-native-numbers','discoverGitLabDuoWorkflowRuntimeNamespace',{},[{status:200,jsonEncoded:encode([{id:NaN},{id:Infinity},{id:-0,duo_features_enabled:true}])}]);
 gl('project-lone-encoding','discoverGitLabDuoWorkflowRuntimeNamespace',{projectId:'group/\ud800'},[ok({data:{project:{root_namespace_id:3}}})]);
 gl('override-http-invalid-key','discoverGitLabDuoWorkflowRuntimeNamespace',{namespaceId:'12',apiKey:'\ud800'},[]);
 gl('namespace-invalid-then-project','discoverGitLabDuoWorkflowNamespace',{namespaceId:'1',projectId:'2'},[ok({data:{aiChatAvailableModels:{}}}),ok({rootNamespaceId:3}),ok(avail())]);
 gl('env-precedence','discoverGitLabDuoWorkflowRuntimeNamespace',{namespaceId:' '},[{status:404}],{env:{GITLAB_DUO_NAMESPACE_ID:' 8 ',GITLAB_DUO_PROJECT_ID:'must-not-fetch'}});
 const remoteCases=[['https-port','https://fixture.test:8443/gitlab/group/repo.git','https://fixture.test:8443/gitlab'],['ssh-port','ssh://git@fixture.test:2222/gitlab/group/repo.git','https://fixture.test:8443/gitlab'],['scp','git@fixture.test:group/repo.git','https://fixture.test:8443'],['nonmatching','https://other.test/group/repo.git','https://fixture.test']];
 for(const[id,url,baseUrl]of remoteCases)gl(`remote-${id}`,'discoverGitLabDuoWorkflowRuntimeNamespace',{baseUrl,cwd:'$CASE/work/sub'},id==='nonmatching'?[ok([])]:[ok({root_namespace_id:1})],{files:{'work/.git/config':`[remote "origin"]\n url = ${url}\n`}});
 gl('remote-cross-port','discoverGitLabDuoWorkflowRuntimeNamespace',{baseUrl:'https://fixture.test:8443',cwd:'$CASE/work'},[ok([{id:'group-root'}])],{files:{'work/.git/config':'[remote "origin"]\nurl = https://fixture.test:9443/group/project.git\n'}});
 gl('linked-worktree','discoverGitLabDuoWorkflowRuntimeNamespace',{cwd:'$CASE/work/sub'},[ok({root_namespace_id:42})],{files:{'work/.git':'gitdir: ../common/worktrees/w\n','common/worktrees/w/commondir':'../..\n','common/config':'[remote "origin"]\nurl = git@fixture.test:group/repo.git\n'}});
 gl('normal-gitdir','discoverGitLabDuoWorkflowRuntimeNamespace',{cwd:'$CASE/work'},[ok({root_namespace_id:42})],{files:{'work/.git':'gitdir: ../gitdir\n','gitdir/config':'[core]\nurl = bad\n[remote "upstream"]\nurl = https://fixture.test/group/repo.GIT\n'}});
 gl('cwd-parent-above-root','discoverGitLabDuoWorkflowRuntimeNamespace',{},[ok({root_namespace_id:42})],{cwdAboveRoot:true,files:{'work/.git/config':'[remote "origin"]\nurl = git@fixture.test:group/repo.git\n'}});
 for(const ref of ['claude_opus_4_8','claude-sonnet-4','claudehaiku','gemini','gpt5','unknown','\ud800'])add(`gitlab/build-${ref}`,'gitlab','discovery/gitlab-duo-workflow','buildGitLabDuoWorkflowModelSpec',{},[],{args:[{ref,name:'Name'},'///','root']});
 add('gitlab/fallback-default','gitlab','discovery/gitlab-duo-workflow','buildGitLabDuoWorkflowFallbackModel',{},[],{args:[]});
 const intentionalErrors=new Map([
  ['codex/bad-url','TypeError'],['codex/bad-header-name','TypeError'],['codex/bad-header-value','TypeError'],['codex/header-nonbyte','TypeError'],
  ['gitlab/no-namespace','Error'],['gitlab/runtime-no-namespace','Error'],['gitlab/pagination-bound','Error'],['gitlab/numeric-pathless-project','Error'],['gitlab/bad-group-url','TypeError'],['gitlab/remote-nonmatching','Error'],
 ]);
 for(const test of cases)if(intentionalErrors.has(test.id))test.expectedErrorName=intentionalErrors.get(test.id);
 return cases;
}

if(process.argv[3]!=='--worker'){
 let cases=fixtures();const api={load,encode,decode,root};
 for(let index=0;index<adapters.length;index++)if(adapters[index].fixtures)for(const test of await adapters[index].fixtures(api))cases.push({...test,adapter:index});
 if(manifest.families.length)cases=cases.filter(test=>manifest.families.includes(test.family));
 if(new Set(cases.map(test=>test.id)).size!==cases.length)throw Error('duplicate fixture IDs');
 const counts={};let processCount=0;const processIds=[];
 for(let index=0;index<cases.length;index++){
  const test=cases[index];const caseDir=path.join(root,'cases',String(index));fs.mkdirSync(caseDir,{recursive:true});
  const input=path.join(caseDir,'input.json');fs.writeFileSync(input,JSON.stringify(test));
  const child=Bun.spawn([process.execPath,fileURLToPath(import.meta.url),manifestPath,'--worker',input],{cwd:caseDir,stdout:'pipe',stderr:'pipe',env:{...process.env,PI_CODING_AGENT_DIR:caseDir}});
  const[exit,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
  fs.writeFileSync(path.join(caseDir,'stdout.txt'),stdout);fs.writeFileSync(path.join(caseDir,'stderr.txt'),stderr);
  if(exit!==0)throw Error(`case ${test.id} failed: ${stderr}`);
  const receipt=JSON.parse(fs.readFileSync(path.join(caseDir,'result.json'),'utf8'));
  if(receipt.id!==test.id||receipt.sourceCommit!==manifest.upstreamCommit)throw Error('worker receipt mismatch');
  if(test.expectedErrorName?receipt.expected.status!=='error'||receipt.expected.name!==test.expectedErrorName:receipt.expected.status!=='ok')throw Error(`unexpected original outcome: ${test.id}: ${JSON.stringify(receipt.expected)}`);
  test.expected=receipt.expected;processCount++;processIds.push(receipt.pid);counts[test.family]=(counts[test.family]??0)+1;
 }
 fs.writeFileSync(path.join(root,'oracle.json'),JSON.stringify({...manifest,cases,counts,isolation:{strategy:'independent-process-per-case',processCount,processIds},originalTestAssertionsExecuted:false}));
 console.log(JSON.stringify({cases:cases.length,counts}));
}else{
 const input=path.resolve(process.argv[4]),caseDir=path.dirname(input),test=JSON.parse(fs.readFileSync(input,'utf8'));
 const replacements=[[caseDir,'$CASE']],fetchCalls=[];let responseIndex=0;
 const normalize=value=>{
  if(typeof value==='string'){for(const[from,to]of replacements)value=value.split(from).join(to);return value;}
  if(Array.isArray(value))return value.map(normalize);
  if(value&&typeof value==='object'){const result={};for(const key of Object.keys(value))Object.defineProperty(result,key,{value:normalize(value[key]),enumerable:true,writable:true,configurable:true});return result;}
  return value;
 };
 const substitute=value=>{
  if(typeof value==='string')return value.replaceAll('$CASE',caseDir);
  if(Array.isArray(value))return value.map(substitute);
  if(value&&typeof value==='object')return Object.fromEntries(Object.entries(value).map(([key,v])=>[key,substitute(v)]));return value;
 };
 for(const key of ['GITLAB_DUO_NAMESPACE_ID','GITLAB_DUO_PROJECT_ID','GITLAB_DUO_PROJECT_PATH','PI_AI_ANTIGRAVITY_VERSION','PI_AI_ANTIGRAVITY_CL','PI_AI_ANTIGRAVITY_OS','PI_AI_ANTIGRAVITY_ARCH'])delete process.env[key];
 for(const[key,value]of Object.entries(test.env??{})){if(value===null)delete process.env[key];else process.env[key]=value;}
 for(const[name,content]of Object.entries(test.files??{})){const target=path.resolve(caseDir,name);if(!target.startsWith(caseDir+path.sep))throw Error('fixture file escaped case directory');fs.mkdirSync(path.dirname(target),{recursive:true});fs.writeFileSync(target,substitute(content));}
 fs.mkdirSync(path.join(caseDir,'work','sub'),{recursive:true});
 const captureRequest=async(url,init={})=>{
  let body=init.body;if(typeof body==='string')body=Buffer.from(body);else if(body instanceof ArrayBuffer)body=Buffer.from(body);else if(ArrayBuffer.isView(body))body=Buffer.from(body.buffer,body.byteOffset,body.byteLength);
  const headers=init.headers instanceof Headers?[...init.headers.entries()]:Array.isArray(init.headers)?init.headers:Object.entries(init.headers??{});
  const receipt={url:String(url),method:init.method??'GET',headers,bodyBase64:body?Buffer.from(body).toString('base64'):null,signal:{present:!!init.signal,aborted:!!init.signal?.aborted}};
  fetchCalls.push(receipt);return receipt;
 };
 const fetch=async(url,init={})=>{
  await captureRequest(url,init);
  const step=test.responses?.[responseIndex++];if(!step)throw Error('unexpected fixture fetch');
  if(step.delayMs)await Bun.sleep(step.delayMs);
  if(step.error){const error=Error(step.error.message);error.name=step.error.name??'Error';throw error;}
  const bytes=step.bodyBase64!==undefined?Buffer.from(step.bodyBase64,'base64'):step.body??(step.json!==undefined?JSON.stringify(step.json):'');
  const response=new Response(bytes,{status:step.status??200,headers:step.headers});
  if(step.jsonEncoded)response.json=async()=>decode(step.jsonEncoded);
  else if(Object.hasOwn(step,'json'))response.json=async()=>structuredClone(step.json);
  if(step.jsonError)response.json=async()=>{const error=Error(step.jsonError.message);error.name=step.jsonError.name??'Error';throw error;};
  return response;
 };
 globalThis.fetch=fetch;
 const api={load,encode,decode,fetch,fetchCalls,captureRequest,logs,caseDir,root,normalize,replacements,substitute};
 let expected;
 try{
  let result;
  if(test.adapter!==undefined&&adapters[test.adapter].runCase)result=await adapters[test.adapter].runCase(test,api);
  else{
   const module=await load(test.module);
   const options=substitute(test.optionsEncoded?decode(test.optionsEncoded):test.options??{});
   if(test.cwdAboveRoot){const volume=path.parse(caseDir).root;options.cwd=volume+'../'.repeat(50)+path.relative(volume,path.join(caseDir,'work'));}
   options.fetchFn=fetch;options.fetch=fetch;
   if(test.signal){const controller=new AbortController();if(test.signal.aborted)controller.abort();options.signal=controller.signal;}
   const args=test.argsEncoded?decode(test.argsEncoded):test.args;
   result=await module[test.op](...(args?substitute(args):[options]));
  }
  expected={status:'ok',value:encode(normalize(result))};
 }catch(error){expected={status:'error',name:error.name,message:normalize(error.message)};}
 expected.fetchCalls=normalize(fetchCalls);expected.logs=normalize(logs);
 fs.writeFileSync(path.join(caseDir,'result.json'),JSON.stringify({id:test.id,sourceCommit:manifest.upstreamCommit,pid:process.pid,expected}));
}
