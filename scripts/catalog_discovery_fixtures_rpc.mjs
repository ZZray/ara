// Explicit RPC corpus. All model payloads are encoded by unchanged fixed schemas.
import * as http2 from 'node:http2';
import * as fs from 'node:fs';
import { gzipSync } from 'node:zlib';
const cursorPath='/agent.v1.AgentService/GetUsableModels';
const cursorHeaders=['content-type','te','authorization','x-ghost-mode','x-cursor-client-version','x-cursor-client-type'];
const b64=value=>Buffer.from(value).toString('base64');
const frame=(value,flags=0)=>{const header=Buffer.alloc(5);header[0]=flags;header.writeUInt32BE(value.length,1);return Buffer.concat([header,Buffer.from(value)]);};
export async function fixtures(api){
 const cursor=await api.load('discovery/cursor-proto'),devin=await api.load('discovery/devin-proto');
 const cases=[];
 const c=(id,models,extra={})=>{const bytes=cursor.GetUsableModelsResponseSchema.encode(cursor.GetUsableModelsResponseSchema.create({models:models.map(model=>cursor.ModelDetailsSchema.create(model))}));cases.push({id:`cursor/${id}`,family:'cursor',module:'discovery/cursor',op:'fetchCursorUsableModels',options:{apiKey:'fixture-token'},responses:[{status:200,bodyBase64:b64(frame(bytes))}],...extra});};
 c('framed-empty',[]);
 c('raw-basic',[{modelId:'plain',displayName:' Plain '}],{raw:true});
 c('names-duplicates',[{modelId:' z ',displayName:' ' ,displayNameShort:' Short '},{modelId:'a',displayModelId:' Alternate '},{modelId:'z',aliases:['',' Last ','alias']},{modelId:' ',displayName:'blank'}]);
 c('native-families',[{modelId:'k3'},{modelId:'kimi-k3'},{modelId:'cursor-grok-4.5'},{modelId:'grok-code-fast-1'},{modelId:'glm-5.2'},{modelId:'glm-5.2-air'},{modelId:'glm-5.2-vision'},{modelId:'glm-5.1'}]);
 c('bundled-reference',[{modelId:'claude-sonnet-4.5',maxMode:true},{modelId:'gpt-5',displayName:' GPT-5 1M '},{modelId:'gemini-3-pro',maxMode:true}]);
 c('signals',[{modelId:'plain',thinkingDetails:{}},{modelId:'plain-1m'},{modelId:'odd',aliases:['Alias 1M']},{modelId:'claude-odd',maxMode:true},{modelId:'gemini-odd',maxMode:true}]);
 c('unicode-sort',[{modelId:'z'},{modelId:'Z'},{modelId:'ä'},{modelId:'e\u0301'},{modelId:'é'},{modelId:'\ud800'},{modelId:'2'},{modelId:'10'}]);
 c('custom-request',[],{options:{apiKey:'fixture-token',clientVersion:'fixture-version',customModelIds:[' a ','a','',7,null,'\ud800','b']},baseSuffix:'/ignored?query=yes#fragment'});
 c('empty-raw',[],{responses:[{status:200,bodyBase64:''}]});
 c('invalid-proto',[],{responses:[{status:200,bodyBase64:b64([10,2,255])}]});
 c('status-rejected',[],{responses:[{status:464,bodyBase64:b64(frame([]))}]});
 c('end-stream-first',[{modelId:'plain'}],{framePrefix:b64(frame(Buffer.from('{}'),2))});
 c('compressed-frame',[],{responses:[{status:200,bodyBase64:b64(frame(Buffer.from([10,0]),1))}]});
 c('first-frame-only',[{modelId:'first'}],{frameSuffix:b64(frame(Buffer.from([10,0])))});
 c('connection-reset',[],{reset:true});
 c('deadline',[],{options:{apiKey:'fixture-token',timeoutMs:40},delayMs:120});
 c('connection-close-success',[{modelId:'plain'}],{expectClosed:true});
 c('connection-close-timeout',[],{options:{apiKey:'fixture-token',timeoutMs:40},delayMs:120,expectClosed:true});
 const d=(id,configs,extra={})=>{const bytes=devin.GetCliModelConfigsResponseSchema.encode(devin.GetCliModelConfigsResponseSchema.create({clientModelConfigs:configs.map(config=>devin.ClientModelConfigSchema.create(config))}));cases.push({id:`devin/${id}`,family:'devin',module:'discovery/devin',op:'fetchDevinModels',options:{apiKey:'fixture-token'},responses:[{status:200,bodyBase64:b64(bytes)}],...extra});};
 d('empty',[]);d('empty-gzip',[],{gzip:true});
 d('basic',[{modelUid:' plain ',label:' Plain ',maxTokens:0},{modelUid:'other',label:'',maxTokens:321}]);
 d('gzip',[{modelUid:'gzip',label:'Gzip'}],{gzip:true});
 d('duplicate-first',[{modelUid:'same',label:'First'},{modelUid:'same',label:'Second'}]);
 d('filtered-empty',[{modelUid:'disabled',disabled:true},{modelUid:'review',modelInfo:{displayOption:4}},{modelUid:'internal',modelInfo:{displayOption:6}},{modelUid:' '}]);
 d('display-open',[3,4,6,7,8,99].map(displayOption=>({modelUid:`display-${displayOption}`,modelInfo:{displayOption}})));
 d('feature-precedence',[{modelUid:'label-only',label:'High reasoning',supportsImages:true},{modelUid:'no-think',label:'No thinking High'},{modelUid:'features-false',label:'High',supportsImages:true,modelInfo:{modelFeatures:{}}},{modelUid:'features-true',modelInfo:{maxOutputTokens:456,modelFeatures:{supportsThinking:true,supportsImages:true,supportsToolCalls:true,supportsParallelToolCalls:true}}}]);
 d('image-blind',[{modelUid:'swe-1-6',supportsImages:true},{modelUid:'swe-1-6-fast',modelInfo:{modelFeatures:{supportsImages:true}}},{modelUid:'swe-1-7',supportsImages:true}]);
 d('presentation',[{modelUid:'sparse',description:' Description ',isNew:true,isBeta:true,isRecommended:true},{modelUid:'blank',description:' '}]);
 d('costs',[{modelUid:'priced',modelDimensions:[{kind:devin.ModelDimensionKind.COST,label:' Input ',value:0.1,denominator:'1M tokens'},{kind:devin.ModelDimensionKind.COST_FUZZY,label:'cached input',value:0.000003,denominator:'1K tokens'},{kind:devin.ModelDimensionKind.COST,label:'output',value:3.2,denominator:'n/a'},{kind:99,label:'input',value:9,denominator:'1M'},{kind:devin.ModelDimensionKind.COST,label:'ignored',value:8}]}]);
 for(const denominator of ['0 tokens','0.5 k tokens','2b tokens','prefix 20 M tokens','1e3 tokens','garbage'])d(`cost-denom-${denominator}`,[{modelUid:'priced',modelDimensions:[{kind:devin.ModelDimensionKind.COST,label:'input',value:-0.0000005,denominator}]}]);
 d('round-large-integer',[{modelUid:'priced',modelDimensions:[{kind:devin.ModelDimensionKind.COST,label:'input',value:1,denominator:'0.00022204460492503126 tokens'}]}]);
 const family=(name,effort,extra={})=>({modelFamilyLabel:name,entries:[{key:'Reasoning Effort',value:{name:effort,order:0}},...(extra.entries??[])],...extra});
 d('dynamic-effort',[{modelUid:'family-low',modelFamilyMetadata:family('Fixture Family','Low')},{modelUid:'family-high',isDefaultModelInFamily:true,modelFamilyMetadata:family('Fixture Family','High')},{modelUid:'family-off',modelFamilyMetadata:family('Fixture Family','None')}]);
 d('dynamic-lanes',[{modelUid:'standard',modelFamilyMetadata:family('Fixture','High')},{modelUid:'fast',modelFamilyMetadata:family('Fixture','Low',{entries:[{key:'effort',value:{name:'low'}},{key:'fast mode',value:{order:1}}]})},{modelUid:'million',modelFamilyMetadata:family('Fixture','Max',{entries:[{key:'effort',value:{name:'max'}},{key:'1M Context',value:{order:1}}]})},{modelUid:'router',modelInfo:{displayOption:3},modelFamilyMetadata:family('Fixture','Medium')}]);
 d('thinking-off-duplicate-route',[{modelUid:'one',modelFamilyMetadata:family('Test','High',{entries:[{key:'effort',value:{name:'High'}},{key:'thinking',value:{order:0}}]})},{modelUid:'two',modelFamilyMetadata:family('Test','High')},{modelUid:'three',modelFamilyMetadata:family('Test','High')}]);
 d('static-family',[{modelUid:'claude-4-5-sonnet'},{modelUid:'claude-4-5-sonnet-thinking'}]);
 d('unicode',[{modelUid:'ä'},{modelUid:'a'},{modelUid:'é'},{modelUid:'z'},{modelUid:'\ud800'}]);
 d('base-override',[{modelUid:'plain'}],{options:{apiKey:'devin-session-token$already',baseUrl:'https://fixture.test/path///'}});
 d('undefined-token',[{modelUid:'plain'}],{options:{}});
 d('invalid-proto',[],{responses:[{status:200,bodyBase64:b64([10,2,255])}]});
 d('http-error',[],{responses:[{status:500,bodyBase64:''}]});
 d('transport-error',[],{responses:[{error:{name:'TypeError',message:'fixture connection reset'}}]});
 d('ignored-aborted-signal',[{modelUid:'plain'}],{signal:{aborted:true}});
 d('signal-after-success',[{modelUid:'plain'}],{signal:{aborted:false},afterAbort:true});
 d('ignored-deadline',[{modelUid:'plain'}],{options:{timeoutMs:1},delayMs:25});
 for(const test of cases){if(test.raw)test.responses[0].bodyBase64=b64(Buffer.from(test.responses[0].bodyBase64,'base64').subarray(5));if(test.gzip)test.responses[0].bodyBase64=b64(gzipSync(Buffer.from(test.responses[0].bodyBase64,'base64')));if(test.framePrefix||test.frameSuffix)test.responses[0].bodyBase64=b64(Buffer.concat([Buffer.from(test.framePrefix??'','base64'),Buffer.from(test.responses[0].bodyBase64,'base64'),Buffer.from(test.frameSuffix??'','base64')]));if(test.delayMs&&test.family==='devin')test.responses[0].delayMs=test.delayMs;}
 return cases;
}
export function createServer(test,onRequest,onClose=()=>{}){
 let peerClosed=false;const closeWaiters=[];
 const server=http2.createServer();const sessions=new Set();server.on('session',session=>{sessions.add(session);session.on('close',()=>{sessions.delete(session);peerClosed=true;onClose();for(const resolve of closeWaiters)resolve(true);});session.on('error',()=>{});});
 server.on('stream',(stream,headers)=>{stream.on('error',()=>{});const chunks=[];stream.on('data',chunk=>chunks.push(chunk));stream.on('end',async()=>{const receipt={method:headers[':method'],path:headers[':path'],headers:cursorHeaders.map(name=>[name,String(headers[name])]),bodyBase64:b64(Buffer.concat(chunks)),http2:true};onRequest(receipt);if(test.reset){stream.close(http2.constants.NGHTTP2_INTERNAL_ERROR);return;}if(test.delayMs)await Bun.sleep(test.delayMs);if(stream.destroyed)return;const reply=test.responses[0];stream.respond({':status':reply.status??200});stream.end(Buffer.from(reply.bodyBase64??'','base64'));});});
 return{server,waitClose:async()=>peerClosed||await Promise.race([new Promise(resolve=>closeWaiters.push(resolve)),Bun.sleep(500).then(()=>false)]),close:async()=>{for(const session of sessions)session.destroy();await new Promise(resolve=>server.close(resolve));}};
}
export async function runCase(test,api){
 const module=await api.load(test.module);const options={...test.options};
 if(test.family==='devin'){let retained,controller;options.fetch=async(url,init)=>{retained=init.signal;return api.fetch(url,init);};if(test.signal){controller=new AbortController();if(test.signal.aborted)controller.abort();options.signal=controller.signal;}const models=await module.fetchDevinModels(options);if(test.afterAbort){controller.abort();return{models,retainedSignal:{aborted:retained.aborted}};}return models;}
 const requests=[];const host=createServer(test,receipt=>requests.push(receipt));await new Promise(resolve=>host.server.listen(0,'127.0.0.1',resolve));const port=host.server.address().port;
 api.replacements.push([String(port),'$PORT']);options.baseUrl=`http://127.0.0.1:${port}${test.baseSuffix??''}`;
 try{const models=await module.fetchCursorUsableModels(options);return{models,protocol:{requests,...(test.expectClosed?{peerClosed:await host.waitClose()}: {})}};}finally{await host.close();}
}
if(process.argv[2]==='--server'){
 const test=JSON.parse(fs.readFileSync(process.argv[3],'utf8'));const ready=process.argv[4],records=process.argv[5];const requests=[];let peerClosed=false;
 const flush=()=>fs.writeFileSync(records,JSON.stringify({requests,peerClosed}));
 const host=createServer(test,receipt=>{requests.push(receipt);flush();},()=>{peerClosed=true;flush();});
 await new Promise(resolve=>host.server.listen(0,'127.0.0.1',resolve));fs.writeFileSync(ready,JSON.stringify({port:host.server.address().port,pid:process.pid}));
}
