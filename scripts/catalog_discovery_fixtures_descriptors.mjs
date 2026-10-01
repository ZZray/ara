// Registry observations invoke only the unchanged fixed source callbacks.
import * as http2 from 'node:http2';
export async function fixtures(api) {
 const module=await api.load('provider-models/descriptors');
 const cases=[{id:'descriptor/registry',family:'descriptor',op:'registry',options:{},responses:[]}];
 for(const entry of module.CATALOG_PROVIDERS)if(entry.createModelManagerOptions){
  for(const key of [false,true])cases.push({id:`descriptor/create-${entry.id}-${key?'key':'no-key'}`,family:'descriptor',op:'create',provider:entry.id,runDynamic:false,options:{...(key?{apiKey:entry.id==='alibaba-token-plan'?'sk-fixture-key':'fixture-key'}:{}),baseUrl:'https://fixture.test/v1',authenticated:key},responses:[],env:{COREWEAVE_PROJECT:'fixture-project'}});
  let responses=Array.from({length:8},()=>({json:{data:[],models:[]}}));
  if(entry.id==='umans')responses=[{json:{}}];
  if(entry.id==='cline-pass')responses=[{json:{clinePass:[{id:'cline-pass/glm-5.2'}],free:[]}},{json:{data:[]}}];
  if(entry.id==='gitlab-duo-agent')responses=Array.from({length:2},()=>({json:{data:{aiChatAvailableModels:{defaultModel:{ref:'claude_opus_4_8',name:'Chosen'}}}}}));
  if(entry.id==='devin')responses=[{bodyBase64:''}];
  if(entry.id==='cursor')responses=[{bodyBase64:'AAAAAAA='}];
  if(entry.id.startsWith('xiaomi'))responses=[{json:{data:[{id:'mimo-v2.5-pro',context_length:262144}]}}];
  cases.push({id:`descriptor/dynamic-${entry.id}`,family:'descriptor',op:'create',provider:entry.id,options:{apiKey:entry.id==='alibaba-token-plan'?'sk-fixture-key':'fixture-key',baseUrl:'https://fixture.test/v1',authenticated:true},responses,env:{COREWEAVE_PROJECT:'fixture-project',GITLAB_DUO_NAMESPACE_ID:'42'},cursor:entry.id==='cursor'});
 }
 return cases;
}
function discovery(value){return value?{label:value.label,envVars:value.envVars??null,oauthProvider:value.oauthProvider??null,allowUnauthenticated:value.allowUnauthenticated??null}:null;}
export async function runCase(test,api) {
 const module=await api.load('provider-models/descriptors'),types=await api.load('provider-models/descriptor-types');
 if(test.op==='registry')return{
  catalog:module.CATALOG_PROVIDERS.map(entry=>({id:entry.id,defaultModel:entry.defaultModel,envVars:entry.envVars??null,hasFactory:typeof entry.createModelManagerOptions==='function',allowUnauthenticated:entry.allowUnauthenticated??null,dynamicModelsAuthoritative:entry.dynamicModelsAuthoritative??null,catalogDiscovery:discovery(entry.catalogDiscovery),specialModelManager:entry.specialModelManager??null,lookupSame:module.getCatalogProviderEntry(entry.id)===entry})),
  runtime:module.PROVIDER_DESCRIPTORS.map(entry=>({providerId:entry.providerId,defaultModel:entry.defaultModel,allowUnauthenticated:entry.allowUnauthenticated??null,dynamicModelsAuthoritative:entry.dynamicModelsAuthoritative??null,catalogDiscovery:discovery(entry.catalogDiscovery),factorySame:entry.createModelManagerOptions===module.getCatalogProviderEntry(entry.providerId).createModelManagerOptions,isCatalog:types.isCatalogDescriptor(entry),allowsUnauthenticated:types.isCatalogDescriptor(entry)?types.allowsUnauthenticatedCatalogDiscovery(entry):null})),
  defaults:module.DEFAULT_MODEL_PER_PROVIDER,unknownLookup:module.getCatalogProviderEntry('fixture-unknown')??null,
  precedence:[undefined,false,true].flatMap(outer=>[undefined,false,true].map(inner=>types.allowsUnauthenticatedCatalogDiscovery({allowUnauthenticated:outer,catalogDiscovery:{label:'Fixture',allowUnauthenticated:inner}})))
 };
 let server,options={...test.options,fetch:api.fetch};const requests=[];
 if(test.cursor){
  server=http2.createServer();server.on('stream',(stream,headers)=>{const chunks=[];stream.on('data',c=>chunks.push(c));stream.on('end',()=>{requests.push({method:headers[':method'],path:headers[':path'],headers:['content-type','te','authorization','x-ghost-mode','x-cursor-client-version','x-cursor-client-type'].map(key=>[key,String(headers[key])]),bodyBase64:Buffer.concat(chunks).toString('base64'),http2:true});stream.respond({':status':200});stream.end(Buffer.from(test.responses[0].bodyBase64,'base64'));});});
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));const port=server.address().port;api.replacements.push([String(port),'$PORT']);options.baseUrl=`http://127.0.0.1:${port}`;
 }
 try{
  const row=module.PROVIDER_DESCRIPTORS.find(row=>row.providerId===test.provider);
  const built=(row??module.getCatalogProviderEntry(test.provider)).createModelManagerOptions(options),hasDynamic=typeof built.fetchDynamicModels==='function';
  const result={providerId:built.providerId,authoritative:!!built.dynamicModelsAuthoritative,cacheProviderId:built.cacheProviderId??null,dropIds:built.dropCachedModelIdsOnStaticMismatch??null,staticModels:built.staticModels??null,hasDynamic,models:test.runDynamic!==false&&hasDynamic?await built.fetchDynamicModels():undefined};
  if(test.cursor)result.protocol={requests};return result;
 }finally{if(server)await new Promise(resolve=>server.close(resolve));}
}
