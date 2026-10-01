// Scenario declarations and Host seams only. Expected values come from fixed source.
export async function fixtures({encode}) {
 const cases=[];
 const add=(family,id,options,responses,extra={})=>cases.push({id:`${family}/${id}`,family,options,responses,...extra});
 const ok=json=>({status:200,json});
 cases.push({id:'openai/exported-constants',family:'openai',op:'constants',options:{},responses:[]});
 cases.push({id:'antigravity/exported-constants',family:'antigravity',op:'constants',options:{},responses:[]});
 const oa=(id,payload,options={},extra={})=>add('openai',id,{api:'openai-completions',provider:'fixture',baseUrl:'https://fixture.test/v1',...options},[ok(payload)],extra);
 oa('basic',{data:[{id:'z'},{id:'a',name:'Alpha'},{id:'z',name:'Replacement'}]});
 for(const key of ['models','result','items'])oa(`envelope-${key}`,{[key]:[{id:'one'}]});
 oa('nested',{data:{models:{result:{items:[{id:'nested'}]}}}});
 oa('empty-first-wins',{data:[],models:[{id:'ignored'}]});
 oa('invalid-first-fallback',{data:4,models:[{id:'fallback'}]});
 oa('invalid-records',[null,4,{},[],{id:''},{id:3},{id:'bad-name',name:4},{id:'null-name',name:null},{id:'empty-name',name:''}]);
 oa('utf16',[{id:'\ud800',name:'\udfff'},{id:'中'},{id:'é'},{id:'e'},{id:'2'}]);
 oa('headers',{data:[]},{apiKey:'key',headers:{Accept:'caller',accept:'second',Authorization:'old',authorization:'lower','X-Test':' value '}});
 oa('base-single-slash',[],{baseUrl:' https://fixture.test/v1/// '});
 oa('blank-base',[],{baseUrl:' \t\u2000 '});
 oa('empty-api-key',[],{apiKey:''});
 oa('mapper',[{id:'first',mappedId:'same'},{id:'skip',skip:true},{id:'last',mappedId:'same'},{id:'invalid',mappedId:''},{id:'context',ctx:true}],{}, {mapper:'decorate'});
 oa('filter',[{id:'a',keep:false},{id:'b'},{id:'c',keep:true}],{}, {filter:'exclude'});
 oa('mapper-error',[{id:'x',throw:true}],{}, {mapper:'decorate',expectedErrorName:'Error'});
 oa('filter-error',[{id:'x'}],{}, {filter:'throw',expectedErrorName:'Error'});
 for(const [id,response] of [['http',{status:503}],['network',{error:{name:'Error',message:'offline'}}],['json',{status:200,jsonError:{message:'bad json'}}]])add('openai',id,{api:'openai-completions',provider:'fixture',baseUrl:'https://fixture.test'},[response]);
 oa('caller-aborted',[],{}, {signal:{aborted:true}});
 add('openai','timeout-overflow',{api:'openai-completions',provider:'fixture',baseUrl:'https://fixture.test',timeoutMs:2147483648},[{status:200,json:[],delayMs:15}],{observeSignals:true});
 add('openai','timeout-cleared',{api:'openai-completions',provider:'fixture',baseUrl:'https://fixture.test',timeoutMs:5},[ok([])],{observeSignals:true,afterDelayMs:15});
 const model=(name,extra={})=>({name,supportedGenerationMethods:['generateContent'],...extra});
 const gm=(id,responses,options={},extra={})=>add('gemini',id,{apiKey:'key',baseUrl:'https://fixture.test/v1beta',...options},responses,extra);
 gm('basic',[ok({models:[model('models/gemini-new'),model('models/other'),model('models/gemini-new',{displayName:'Last'})]})]);
 gm('bundled',[ok({models:[model('models/gemini-2.5-pro'),model('models/gemini-2.5-flash')]})]);
 gm('limits',[ok({models:[model('models/new',{displayName:' Name ',inputTokenLimit:99.8,outputTokenLimit:1.2}),model('x',{inputTokenLimit:-1,outputTokenLimit:0})]})]);
 gm('native-numbers',[{status:200,jsonEncoded:encode({models:[model('models/unknown',{inputTokenLimit:Infinity,outputTokenLimit:NaN})]})}]);
 gm('invalid-items',[ok({models:[null,{},model(''),model('models/'),model(' x ',{supportedGenerationMethods:['embedContent']}),model('bad',{supportedGenerationMethods:[4]}),model('valid',{displayName:7,inputTokenLimit:'x'})]})]);
 gm('utf16',[ok({models:[model('models/\ud800',{displayName:'\udfff'}),model('models/中')]})]);
 gm('pagination',[ok({models:[model('a')],nextPageToken:' next '}),ok({models:[model('a',{displayName:'New'}),model('b')],nextPageToken:'next'})]);
 gm('page-bound',[ok({models:[model('a')],nextPageToken:'next'})],{maxPages:1.9,pageSize:2.9});
 gm('query-set',[ok({models:[],nextPageToken:'n'}),ok({models:[]})],{baseUrl:'https://fixture.test/v1beta?key=old&pageSize=1&pageToken=old&key=duplicate'});
 gm('option-fallbacks',[ok({})],{baseUrl:'  ',pageSize:0,maxPages:-4});
 gm('blank-key',[],{apiKey:' \u3000 '});
 gm('bad-url',[],{baseUrl:'not a URL'},{expectedErrorName:'TypeError'});
 for(const [id,response]of[['http',{status:403}],['network',{error:{name:'Error',message:'offline'}}],['json',{status:200,jsonError:{message:'bad json'}}],['null',ok(null)],['invalid-models',ok({models:7})]])gm(id,[response]);
 gm('caller-aborted',[ok({})],{}, {signal:{aborted:true}});
 const ag=(id,responses,options={},extra={})=>add('antigravity',id,{token:'token',endpoint:'https://fixture.test',userAgent:'fixture-agent',...options},responses,extra);
 ag('basic',[ok({models:{z:{displayName:'Zed'},a:{displayName:'Alpha',supportsImages:true,supportsThinking:true,maxTokens:99,maxOutputTokens:33}}})]);
 ag('filtered',[ok({models:{chat_20706:{},chat_23310:{},'gemini-2.5-pro':{},private:{isInternal:true},public:{isInternal:'yes'}}})]);
 ag('fallback',[{status:503},ok({models:{fallback:{}}})],{endpoint:''});
 ag('all-fail',[{status:503},{error:{name:'Error',message:'offline'}}],{endpoint:''});
 ag('malformed',[ok(null)],{},{});
 ag('array-models',[ok({models:[{displayName:'first'},null,4,{displayName:'last'}]})]);
 ag('proto-key',[ok(JSON.parse('{"models":{"__proto__":{"displayName":"hidden"},"visible":{}}}'))]);
 ag('limits',[{status:200,jsonEncoded:encode({models:{x:{maxTokens:Infinity,maxOutputTokens:NaN},y:{maxTokens:-1,maxOutputTokens:0},z:{maxTokens:1.5,maxOutputTokens:2.5}}})}]);
 ag('utf16',[ok({models:{'\ud800':{displayName:'\udfff'}}})],{endpoint:'https://fixture.test///',project:'ignored'});
 ag('collapse',[ok({models:{'gemini-3.5-flash-low':{},'gemini-3.5-flash-extra-low':{},'claude-opus-4-6-thinking':{supportsThinking:true}}})]);
 ag('discover-version',[{status:200,body:'version: 3.4.5\n'},ok({models:{x:{}}})],{userAgent:undefined});
 ag('env-version',[ok({models:{}})],{userAgent:undefined},{env:{PI_AI_ANTIGRAVITY_VERSION:'9.8.7'}});
 ag('caller-aborted',[ok({models:{}})],{}, {signal:{aborted:true}});
 const cli=(id,responses,options={},extra={})=>add('gemini-cli',id,{token:'token',endpoint:'https://fixture.test',...options},responses,extra);
 cli('basic',[ok({buckets:[{modelId:'gemini-2.5-pro'},{modelId:'gemini-unknown'},{modelId:'claude-x'},{modelId:'gemini-2.5-pro'}]})],{projectId:'p'});
 cli('project-string',[ok({cloudaicompanionProject:'project'}),ok({buckets:[]})]);
 cli('project-object',[ok({cloudaicompanionProject:{id:'project'}}),ok({buckets:[{modelId:'gemini-3.7-pro'}]})]);
 cli('project-load-fail',[{status:403},ok({buckets:[]})]);
 cli('project-empty',[ok({buckets:[]})],{projectId:''});
 cli('invalid-buckets',[ok({buckets:[null,{}, {modelId:2},{modelId:' '},{modelId:' gemini-3.7-flash '},{modelId:'\ud800'}]})],{projectId:'p'});
 cli('quota-http',[{status:500}],{projectId:'p'});
 cli('quota-json',[{status:200,jsonError:{message:'bad json'}}],{projectId:'p'});
 cli('quota-null',[ok(null)],{projectId:'p'});
 cli('endpoint-trim',[ok({buckets:[]})],{projectId:'p',endpoint:' https://fixture.test/// '});
 cli('caller-aborted',[ok({buckets:[]})],{projectId:'p'},{signal:{aborted:true}});
 const gh=(id,op,args=[],extra={})=>cases.push({id:`google-headers/${id}`,family:'google-headers',op,args,responses:[],...extra});
 gh('constants','constants');
 gh('cli-default','getGeminiCliUserAgent');gh('cli-model','getGeminiCliHeaders',['\ud800'],{env:{PI_AI_GEMINI_CLI_VERSION:'test'}});
 gh('cli-empty','getGeminiCliHeaders',[''],{env:{PI_AI_GEMINI_CLI_VERSION:''}});
 gh('antigravity-default','getAntigravityUserAgent');gh('version-default','getAntigravityVersion');
 gh('antigravity-env','getAntigravityUserAgent',[],{env:{PI_AI_ANTIGRAVITY_VERSION:'9',PI_AI_ANTIGRAVITY_OS:'os',PI_AI_ANTIGRAVITY_ARCH:'arch',PI_AI_ANTIGRAVITY_CL:'0'}});
 for(const [id,text]of [['bare','version: 3.4.5'],['quoted','other: 1\r\n version: " 3.4.5 " # comment\r\n'],['single',"version: '3.4.5'"],['invalid-first','version: no\nversion: 3.4.5'],['trailing','version: 3.4.5-rc'],['missing','key: value'],['unicode','\u3000version: 3.4.5\u3000'],['lone','version: "\ud800"']])gh(`parse-${id}`,'parseAntigravityManifestVersion',[text]);
 for(const id of ['gemini-3.5-flash-extra-low','gemini-3.5-flash-low','gemini-3-flash-agent','gemini-3.1-pro-low','gemini-pro-agent','claude-sonnet-4-6','claude-opus-4-6-thinking','unknown','\ud800'])gh(`profile-${id}`,'getAntigravityModelWireProfile',[id]);
 gh('cache','ensure-sequence',[],{responses:[{status:200,body:'version: 3.4.5'}],steps:['snapshot','ensure','snapshot','ensure','snapshot']});
 gh('retry','ensure-sequence',[],{responses:[{status:500},{status:200,body:'version: 4.5.6'}],steps:['ensure','snapshot','ensure','snapshot']});
 gh('malformed-retry','ensure-sequence',[],{responses:[{status:200,body:'version: nope'},{status:200,body:'version: 4.5.6'}],steps:['ensure','snapshot','ensure','snapshot']});
 gh('parallel','ensure-sequence',[],{responses:[{status:200,body:'version: 3.4.5',delayMs:5}],steps:['parallel','snapshot']});
 gh('env-skips','ensure-sequence',[],{env:{PI_AI_ANTIGRAVITY_VERSION:'8.8.8'},steps:['ensure','snapshot']});
 gh('signal-preabort','ensure-sequence',[],{signal:{aborted:true},responses:[{status:200,body:'version: 3.4.5'}],steps:['ensure','snapshot']});
 gh('signal-after-success','ensure-sequence',[],{signal:{aborted:false},responses:[{status:200,body:'version: 3.4.5'}],steps:['ensure','abort','snapshot']});
 return cases;
}

export async function runCase(test,api) {
 const {load,fetch,decode,substitute}=api;
 for(const key of ['PI_AI_GEMINI_CLI_VERSION','PI_AI_ANTIGRAVITY_VERSION','PI_AI_ANTIGRAVITY_CL','PI_AI_ANTIGRAVITY_OS','PI_AI_ANTIGRAVITY_ARCH'])delete process.env[key];
 for(const [key,value]of Object.entries(test.env??{})){if(value!==null)process.env[key]=value;}
 const retained=[],controller=test.signal?new AbortController():undefined;
 if(test.signal?.aborted)controller.abort();
 const fetcher=(url,init)=>{if(init?.signal)retained.push(init.signal);return fetch(url,init);};
 if(test.family==='google-headers') {
  const module=await load('wire/gemini-headers');
  if(test.op==='constants')return {defaultVersion:module.DEFAULT_ANTIGRAVITY_VERSION,profiles:module.ANTIGRAVITY_MODEL_WIRE_PROFILES};
  if(test.op==='ensure-sequence'){
   const snapshots=[];
   for(const step of test.steps){
    if(step==='ensure')await module.ensureAntigravityVersion(fetcher,controller?.signal);
    else if(step==='parallel')await Promise.all([module.ensureAntigravityVersion(fetcher,controller?.signal),module.ensureAntigravityVersion(fetcher,controller?.signal)]);
    else if(step==='abort')controller.abort();
    else if(step==='snapshot')snapshots.push({version:module.getAntigravityVersion(),userAgent:module.getAntigravityUserAgent(),retainedSignals:retained.map(signal=>signal.aborted)});
    else throw Error(`unknown header step ${step}`);
   }
   return snapshots;
  }
  return module[test.op](...(test.args??[]));
 }
 const options=substitute(test.optionsEncoded?decode(test.optionsEncoded):test.options??{});
 options.fetch=fetcher;options.fetchFn=fetcher;options.fetcher=fetcher;
 if(controller)options.signal=controller.signal;
 if(test.mapper==='decorate')options.mapModel=(entry,defaults,context)=>{if(entry.throw)throw Error('mapper failed');if(entry.skip)return null;return {...defaults,...(Object.hasOwn(entry,'mappedId')?{id:entry.mappedId}:{}),...(entry.ctx?{extraContext:{...context}}:{})};};
 if(test.filter==='exclude')options.filterModel=entry=>entry.keep!==false;
 if(test.filter==='throw')options.filterModel=()=>{throw Error('filter failed');};
 const route={openai:['discovery/openai-compatible','fetchOpenAICompatibleModels'],gemini:['discovery/gemini','fetchGeminiModels'],antigravity:['discovery/antigravity','fetchAntigravityDiscoveryModels'],'gemini-cli':['discovery/gemini-cli','fetchGeminiCliQuotaModels']}[test.family];
 const module=await load(route[0]);
 if(test.op==='constants')return test.family==='openai'?{defaultTimeout:module.DEFAULT_OPENAI_COMPATIBLE_DISCOVERY_TIMEOUT_MS}:{primaryEndpoint:module.ANTIGRAVITY_PRIMARY_ENDPOINT,sandboxEndpoint:module.ANTIGRAVITY_SANDBOX_ENDPOINT};
 const models=await module[route[1]](options);
 if(test.afterDelayMs)await new Promise(resolve=>setTimeout(resolve,test.afterDelayMs));
 return test.observeSignals?{models,retainedSignals:retained.map(signal=>signal.aborted)}:models;
}
