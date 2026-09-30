// Runtime imports below are unchanged Git blobs. The package facade only
// reexports original pi-utils leaves; no policy or utility function is mocked.
import { mock } from 'bun:test';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { createHash } from 'node:crypto';
import { fileURLToPath, pathToFileURL } from 'node:url';

const manifestPath = path.resolve(process.argv[2]);
const root = path.dirname(manifestPath);
const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
if (Bun.version !== '1.4.0' || manifest.upstreamCommit !== '596f2da7101178214aa27a753529d15e6b7ad91d') throw new Error('wrong runtime/source');
if (digest(fs.readFileSync(fileURLToPath(import.meta.url))) !== manifest.runnerSha256) throw new Error('runner hash mismatch');
for (const source of [...manifest.sourceManifest, ...manifest.licenses]) {
  const bytes = fs.readFileSync(path.join(root, 'upstream', source.path));
  if (digest(bytes) !== source.sha256 || createHash('sha1').update(`blob ${bytes.length}\0`).update(bytes).digest('hex') !== source.gitBlob) throw new Error(`source hash mismatch: ${source.path}`);
}
const importRaw = p => import(pathToFileURL(path.join(root, 'upstream', p)).href);
const guards = await importRaw('packages/utils/src/type-guards.ts');
const tls = await importRaw('packages/utils/src/tls-fetch.ts');
mock.module('@oh-my-pi/pi-utils', () => ({...guards, ...tls}));
const load = p => importRaw(`packages/catalog/src/${p}.ts`);
const rev = await load('compat/revision'), taxonomy = await load('compat/taxonomy');
const cascade = await load('compat/cascade'), policy = await load('compat/resolve');
const axes = await load('compat/axes'), apply = await load('compat/apply');
const anthropic = await load('compat/anthropic'), openai = await load('compat/openai');
const build = await load('build'), tokenizer = await load('model-tokenizer'), utils = await load('utils');
const ids = await load('identity/id'), dialect = await load('identity/dialect');
const references = await load('identity/reference'), bundled = await load('identity/bundled');
const metrics = await load('identity/metrics'), priority = await load('identity/priority');
const hosts = await load('hosts'), models = await load('models');
const behavior = await load('compat/behavior'), auth = await load('compat/auth');
const bundledReferences = await load('provider-models/bundled-references');
const rawRows = JSON.parse(fs.readFileSync(path.join(root, 'upstream/packages/catalog/src/models.json'), 'utf8'));
const rules = JSON.parse(fs.readFileSync(path.join(root, 'upstream/packages/catalog/src/compat/rules.json'), 'utf8'));
const rows = models.getBundledProviders().flatMap(p => models.getBundledModels(p));
const catalogKeys = Object.entries(rawRows).flatMap(([p, values]) => Object.keys(values).map(id => `${p}/${id}`));
if (rows.length !== 4776 || new Set(catalogKeys).size !== 4776) throw new Error('incomplete fixed catalog');
const cases = [];
const clean = value => JSON.parse(JSON.stringify(value));
function capture(fn) {
  try { const value = fn(); return value === undefined ? {status:'undefined'} : {status:'ok', value:clean(value)}; }
  catch (error) { return {status:'error', error:error.message}; }
}
function add(kind, input, fn, label = kind) { cases.push({kind, label, input:clean(input), expected:capture(fn)}); }
function identityOf(id) {
  return {bare:ids.bareModelId(id), segments:ids.getModelLikeIdSegments(id),
    longest:ids.getLongestModelLikeIdSegment(id), brackets:ids.getBracketStrippedModelIdCandidates(id),
    stripped:ids.stripBracketedModelIdAffixes(id), candidates:references.getReferenceCandidateIds(id), dialect:dialect.preferredDialect(id)};
}
function taxonomyOf(provider, id, options) {
  return {identity:taxonomy.classifyModel(provider,id,options), collapse:taxonomy.collapseVariantId(provider,id),
    thinking:taxonomy.stripThinkingVariantSuffix(id), billing:taxonomy.billingVariantPlain(id),
    routing:taxonomy.routingVariantPlain(provider,id), lane:taxonomy.stripEffortLane(provider,id)};
}
function hostOf(model) {
  const names = Object.keys(hosts.KNOWN_HOSTS);
  return {url:Object.fromEntries(names.map(h=>[h,hosts.hostMatchesUrl(model.baseUrl,h)])),
    model:Object.fromEntries(names.map(h=>[h,hosts.modelMatchesHost(model,h)])),
    vertexExpress:hosts.isVertexExpressOpenAIUrl(model.baseUrl ?? ''), vertexRaw:hosts.isVertexRawPredictUrl(model.baseUrl ?? ''),
    azure:hosts.isAzureDeploymentsUrl(model.baseUrl ?? ''), dashscope:hosts.isDashscopeCompatibleModeUrl(model.baseUrl ?? '')};
}
function behaviorOf(provider, id) {
  return {responses:behavior.isLikelyOpenAIResponsesId(id), operations:behavior.modelOperationOverrides(provider,id),
    effort:behavior.cursorEffortSuffix(id), parameters:behavior.cursorModelParameters(id), quota:behavior.quotaTierFor(provider,id),
    route:behavior.apiRouteFor(provider,id), limits:behavior.modelLimitsFor(provider,id), excluded:behavior.isExcludedModel(provider,id),
    plan:behavior.planRequirementFor(provider,id), pricingPeer:behavior.pricingPeerFor(provider,id)};
}
function providerOf(provider) {
  return {routing:taxonomy.hasRoutingVariants(provider), recovery:taxonomy.recoversCanonicalParams(provider),
    hint:taxonomy.responsesHintGroup(provider), routes:taxonomy.responsesRouteModels(provider), siblings:taxonomy.supportsDynamicEffortSiblings(provider),
    families:taxonomy.effortFamiliesFor(provider), quota:behavior.hasQuotaTierPolicy(provider), hosted:behavior.hostedDefaultModel(provider),
    exactRoutes:behavior.apiRouteExactModelIds(provider), retired:behavior.isRetiredProvider(provider), auth:auth.authPolicyFor(provider)};
}
const cost = {input:1,output:2,cacheRead:0.5,cacheWrite:1.25};
const specOf = (id, provider='fixture', api='openai-completions', extra={}) => ({id,name:id,provider,api,baseUrl:'https://example.invalid/v1',reasoning:false,input:['text'],cost:{...cost},contextWindow:32768,maxTokens:4096,...extra});
for (const row of rows) {
  const key = `${row.provider}/${row.id}`;
  add('catalog-taxonomy',{provider:row.provider,id:row.id},()=>taxonomyOf(row.provider,row.id),key);
  const identity = taxonomy.classifyModel(row.provider,row.id);
  const target = {provider:row.provider,model:row.id,class:identity.class,...(identity.family===undefined?{}:{family:identity.family}),...(identity.revision===undefined?{}:{revision:identity.revision}),reasoning:row.reasoning};
  add('catalog-cascade',target,()=>cascade.resolveCascade(target),key);
  add('catalog-policy',row,()=>policy.resolveModelPolicy(structuredClone(row)),key);
  add('catalog-build',row,()=>build.buildModel(structuredClone(row)),key);
  add('catalog-identity',{id:row.id},()=>identityOf(row.id),key);
  add('catalog-tokenizer',{id:row.id},()=>tokenizer.resolveModelTokenizer(row.id),key);
  add('catalog-host',row,()=>hostOf(row),key);
}
const fixtureIds = new Set(['','unknown',' GPT-5.4 ','gpt-5.4-high','gpt-5.4:cloud','grok-4.6','claude-opus-4.7','[Kiro] claude-opus-4-8','【gcli转】gemini-3.1-pro-preview [假流]','@modal/GLM-5-2-FP8','non-thinking','no-thinking','model-thinking-tools','vendor/GPT-5.4','gpt-5.4 gpt:5-4 gpt.5:4']);
for (const fixturePath of manifest.fixturePaths) {
  const text = fs.readFileSync(path.join(root,'upstream',fixturePath),'utf8');
  for (const match of text.matchAll(/"([^"\\]*(?:\\.[^"\\]*)*)"|'([^'\\]*(?:\\.[^'\\]*)*)'/g)) {
    const value = match[1] ?? match[2];
    if (value.length < 180 && !value.includes('\\') && /(?:claude|gemini|gpt|grok|glm|qwen|deepseek|kimi|minimax|llama|o[1345])/i.test(value)) fixtureIds.add(value);
  }
}
for (const id of fixtureIds) {
  add('fixture-identity',{id},()=>identityOf(id));
  add('fixture-tokenizer',{id},()=>tokenizer.resolveModelTokenizer(id));
  for (const provider of ['fixture','openai','cursor','amazon-bedrock']) add('fixture-taxonomy',{provider,id},()=>taxonomyOf(provider,id));
}
for (const cls of rules.taxonomy.classes) for (const override of cls.overrides) {
  for (const observedAtMs of [undefined, ...(override.expiresAtMs===undefined?[]:[override.expiresAtMs-1,override.expiresAtMs,override.expiresAtMs+1])]) {
    const input={provider:override.provider??'fixture',id:override.model,...(observedAtMs===undefined?{}:{options:{observedAtMs}})};
    add('override-taxonomy',input,()=>taxonomyOf(input.provider,input.id,input.options));
  }
}
for (const value of ['', '0','00','255','256','1.2','1-2-3','1.2.3.4','1.256','1..2',' 1','1 ','1e2','1.2-3','1.2.3-more','3-32b','3.3-70b','4-6-turbo','4.6x','２']) {
  add('revision',{value},()=>({parse:rev.parseRevision(value),prefix:rev.parseRevisionPrefix(value)}));
}
for (const expression of ['', ' ', '>=2.5 <4','>=2-5','>1 <=2 =2','>=256','=>2','2','>= 2','\t>=2.5\n<3.8']) {
  add('constraint',{expression},()=>rev.parseRevisionConstraint(expression));
}
for (const left of [[0,0,0],[255,255,255],[4,6,0],[4,6,1]]) for (const right of [[0,0,0],[255,255,255],[4,6,0],[4,6,1]]) {
  const terms=['>=','>','<=','<','='].map(op=>({op,revision:right}));
  add('revision-compare',{left,right,terms},()=>({compare:rev.compareRevision(left,right),format:rev.formatRevision(left),satisfies:terms.map(term=>rev.revisionSatisfies(left,[term]))}));
}
for (const pattern of ['','*','**','a','a*','*a','a*b*c','a**c','🔥*x']) for (const value of ['','a','A','abc','abxbxc','a🔥c','🔥xx']) add('glob',{pattern,value},()=>cascade.globMatch(pattern,value));
const target={provider:'fixture',model:'GPT-5.4',class:'openai',family:'gpt',revision:'5.4.0',reasoning:false};
const rule=(source,extra={})=>({source,wire:{supportsStore:true},...extra});
for (const compiled of [
  {rules:[]}, {rules:[rule('global')]},
  {rules:[rule('global'),rule('provider',{providers:['fixture'],wire:{supportsStore:false}})]},
  {rules:[rule('exact',{models:[{kind:'exact',value:'GPT-5.4'}],thinking:{efforts:['low','high']}})]},
  {rules:[rule('glob',{models:[{kind:'glob',value:'gpt-*'}]}),rule('token',{models:[{kind:'token',value:'gpt'}],priority:1,wire:{supportsStore:false}})]},
  {rules:[rule('first'),rule('second')]},
  {rules:[rule('invalid',{revision:[{op:'>=',revision:'256'}]})]},
  {rules:[rule('missing-family',{family:'other'})]},
]) add('cascade-rules',{cascade:compiled,target},()=>cascade.resolveCascadeRules(compiled,target));
const apis=['openai-completions','openai-responses','openai-codex-responses','azure-openai-responses','anthropic-messages','bedrock-converse-stream','google-generative-ai','google-gemini-cli','google-vertex','devin','devin-agent','openrouter','unknown'];
const urls=['','https://api.openai.com/v1','https://api.anthropic.com','https://api.anthropic.com.evil.test','http://localhost:1234/v1','http://[::1]:8080','http://172.16.2.1','http://172.32.1.1','https://portal.qwen.ai','https://api.kimi.com/v1','https://gateway.ai.cloudflare.com/v1/a/b/anthropic','https://x.services.ai.azure.com','https://api.zenmux.ai/anthropic','HTTPS://API.ANTHROPIC.COM/v1'];
for (const api of apis) for (const id of ['gpt-5.4','claude-opus-5','qwen3.5','grok-4.6','kimi-k3','glm-5','unknown']) for (const reasoning of [false,true]) {
  const spec=specOf(id,'fixture',api,{reasoning});
  add('fixture-policy',spec,()=>policy.resolveModelPolicy(spec));
  add('fixture-build',spec,()=>build.buildModel(spec));
}
for (const provider of ['openai','azure','anthropic','github-copilot','deepseek','zai','zhipu-coding-plan','moonshot','kimi-code','cursor','litellm','llama.cpp','xai','openrouter','amazon-bedrock','google','nvidia','venice','alibaba-coding-plan','fixture']) for (const baseUrl of urls) {
  const spec=specOf('gpt-5.4',provider,'openai-completions',{baseUrl,reasoning:true});
  add('endpoint-policy',spec,()=>policy.resolveModelPolicy(spec));
  add('endpoint-host',spec,()=>hostOf(spec));
}
for(const api of apis) for(const baseUrl of urls) for(const id of ['gpt-5.4','claude-opus-5','qwen3.5','grok-4.6','kimi-k3','glm-5','unknown']) {
  const spec=specOf(id,'fixture',api,{baseUrl,reasoning:true});
  add('api-endpoint-policy',spec,()=>policy.resolveModelPolicy(spec));
}
for (const api of ['openai-completions','openai-responses','anthropic-messages','bedrock-converse-stream','google-generative-ai','devin']) {
  for (const patch of [
    {compat:{}}, {compat:{supportsStore:false,unknown:{keep:true}}},
    {compat:{reasoningEffortMap:{low:'custom',max:'maximum'}}},
    {compat:{whenThinking:null}}, {compat:{whenThinking:false}},
    {compat:{whenThinking:{supportsStore:false,supportsToolChoice:false}}},
    {thinking:null}, {thinking:{}}, {thinking:{mode:'effort',efforts:[]}},
    {thinking:{mode:'budget',efforts:['high','low','low'],defaultLevel:'high',effortMap:{low:'L'}}},
    {thinking:{mode:'google-level',levels:['low','high'],supportsDisplay:false,requiresEffort:true}},
    {thinking:{mode:'anthropic-adaptive',efforts:['low','max'],defaultLevel:'max'}},
    {requestModelId:'gpt-5.4'}, {tokenizer:null}, {tokenizer:'custom'},
  ]) {
    const spec=specOf('claude-opus-5','fixture',api,{reasoning:true,...patch});
    add('sparse-policy',spec,()=>policy.resolveModelPolicy(spec));
    add('sparse-build',spec,()=>build.buildModel(spec));
  }
}
for (const provider of ['openai','azure','azure-openai','fixture']) for(const api of ['openai-responses','azure-openai-responses']) for(const baseUrl of ['', 'https://api.openai.com/v1','http://api.openai.com/v1','https://x.openai.azure.com/openai/deployments/x','https://models.inference.ai.azure.com','https://api.openai.com.evil.test']) for(const support of [undefined,false,true,null]) {
  const spec=specOf('gpt-5.4',provider,api,{baseUrl,supportsComputerUse:true,...(support===undefined?{}:{supportsComputerUseConfig:support})});
  add('computer-use-build',spec,()=>build.buildModel(spec));
}
for (const baseUrl of urls) add('anthropic-url',{baseUrl},()=>({official:anthropic.isOfficialAnthropicApiUrl(baseUrl),azure:anthropic.isAzureAnthropicRoute(baseUrl),proxy:anthropic.isAnthropicSigningProxyUrl(baseUrl)}));
for(const baseUrl of ['https://ſ.services.ai.azure.com','https://a.ſervices.ai.azure.com','https://bedrocK-runtime.us-east-1.amazonaws.com']) add('anthropic-url',{baseUrl},()=>({official:anthropic.isOfficialAnthropicApiUrl(baseUrl),azure:anthropic.isAzureAnthropicRoute(baseUrl),proxy:anthropic.isAnthropicSigningProxyUrl(baseUrl)}));
add('anthropic-url',{},()=>({official:anthropic.isOfficialAnthropicApiUrl(),azure:anthropic.isAzureAnthropicRoute(),proxy:anthropic.isAnthropicSigningProxyUrl()}));
for (const id of ['grok-3-mini','grok-4.3','grok-4.5','grok-4.6','grok-5','unknown']) add('xai-map',{id},()=>openai.xaiResponsesReasoningEffortMap(id));
for (const compat of [{a:true,b:{x:1}}, {a:null,b:[]},{}]) for (const overrides of [undefined,{a:false,b:{y:2},unknown:1},{a:null,b:[]},{}]) add('compat-apply',{compat,...(overrides===undefined?{}:{overrides})},()=>{const value=structuredClone(compat);apply.applyCompatOverrides(value,overrides);return value;});
for(const raw of ['{"toString":"x","constructor":1,"hasOwnProperty":false,"__proto__":null,"extraJunk":true}','{"__proto__":null,"toString":"x","constructor":1}','{"toString":"x","constructor":1}']) {
  const overrides=JSON.parse(raw);add('compat-apply',{compat:{},overrides},()=>{const value={};apply.applyCompatOverrides(value,overrides);return value;});
}
const prototypeSpec=specOf('gpt-5.4','fixture','openai-completions',{compat:JSON.parse('{"toString":"x","constructor":1}')});
add('prototype-policy',prototypeSpec,()=>policy.resolveModelPolicy(prototypeSpec));
add('prototype-build',prototypeSpec,()=>build.buildModel(prototypeSpec));
for (const catalog of [{},{longContext:{inputThreshold:10,multiplier:2,inputThresholdInclusive:true}}, {longContext:{inputThreshold:10,input:2,output:3,cacheRead:4,cacheWrite:5}}, {costPatch:{input:0,output:-2},limitsPatch:{contextWindow:0,maxTokens:5},contextWindowFloor:12,inputModalities:['text','image']}, {longContext:[],costPatch:[],inputModalities:['audio']}]) {
  for (const zero of [false,true]) {const model=specOf('unknown');if(zero)model.cost={input:0,output:0,cacheRead:0,cacheWrite:0};add('catalog-corrections',{model,catalog},()=>{const value=structuredClone(model);build.applyCatalogCorrections(value,catalog);return value;});}
}
for (const value of [null,false,true,0,-1,1,0.5,'',' ','1','0x10','0o17','1e3','Infinity','NaN',[],{},' 2.5 ','\uFEFF2\uFEFF','\u00852\u0085','0x'+ 'f'.repeat(40),'0b'+ '1'.repeat(100),'-0x10','0x20000000000001','0x20000000000003']) add('utility-number',{value},()=>({number:utils.toNumber(value),positive:utils.toPositiveNumber(value,7),nullable:utils.toPositiveNumberOrNull(value),boolean:utils.toBoolean(value),record:utils.isRecord(value)}));
for (const name of ['OpenAI: GPT-5.4 (latest)','Z.ai: GLM-5 (20% off)','Arcee AI: Trinity ($$$$)','Claude (Thinking) (Fast)',' (latest) ','plain','GPT (retires Jun 5)','\uFEFF OpenAI: GPT (latest)\uFEFF','\u0085GPT\u0085','GPT (２０% off)']) add('utility-name',{name},()=>utils.cleanModelName(name));
for (const key of ['','sk-ant-oat','prefix-sk-ant-oat-secret','sk-ant-api','SK-ANT-OAT']) add('utility-oauth',{key},()=>utils.isAnthropicOAuthToken(key));
add('axes',{},()=>({axes:axes.AXES,apiRecords:axes.API_COMPAT_RECORDS,efforts:axes.EFFORT_TIERS}));
for(const value of ['minimal','low','medium','high','xhigh','max','off','budget','effort','google-level','anthropic-adaptive','anthropic-budget-effort','unknown']) add('axis-predicates',{value},()=>({effort:axes.isEffortTier(value),mode:axes.isThinkingMode(value)}));
for (const location of ['global','eu','us','us-central1','','EU']) add('vertex-location',{location},()=>hosts.resolveVertexEndpointHost(location));
for (const configured of [undefined,[],[' OpenAI ','custom','openai','','ANTHROPIC']]) add('provider-priority',{...(configured===undefined?{}:{configured})},()=>Object.fromEntries(priority.buildModelProviderPriorityRank(configured)));
const referenceIndex=bundled.getBundledModelReferenceIndex();
for (const id of [...new Set(rows.map(row=>row.id)),...fixtureIds]) add('bundled-reference',{id},()=>references.resolveModelReference(id,referenceIndex));
for (const provider of models.getBundledProviders()) add('provider-reference-map',{provider},()=>Object.fromEntries(bundledReferences.createBundledReferenceMap(provider)));
const providerRefs={sentinel:{id:'sentinel',compat:{authored:true}},'gpt-5.4':{id:'custom-gpt',compat:null}};
const referenceQueries=[...new Set(rows.map(row=>row.id)),'sentinel','not-a-model'];
add('reference-resolver',{references:providerRefs,ids:referenceQueries},()=>{
  let sourceCalls=0;const resolve=bundledReferences.createReferenceResolver(()=>{sourceCalls++;return new Map(Object.entries(providerRefs));});
  const results=referenceQueries.map(id=>capture(()=>resolve(id)));return {sourceCalls,results};
});
const sampleModels=[specOf('gpt-5.4','relay','openai-responses',{contextWindow:10,maxTokens:5}),specOf('GPT-5.4','openai','openai-responses',{contextWindow:20,maxTokens:10}),specOf('vendor/claude-opus-4-8','anthropic','anthropic-messages'),specOf('grok-4.6','xai-oauth','openai-responses',{cost:{input:0,output:0,cacheRead:0,cacheWrite:0}})];
add('reference-index',{models:sampleModels},()=>{const index=references.buildModelReferenceIndex(sampleModels);return {exact:Object.fromEntries(index.exact),suffixAlias:Object.fromEntries(index.suffixAlias)};});
for(const modelThinking of [undefined,null,{mode:'effort',efforts:['low']}]) for(const reference of [undefined,{provider:'fixture',thinking:{mode:'effort',efforts:['high']}},{provider:'other',thinking:{mode:'budget'}}]) add('inherit-thinking',{provider:'fixture',...(modelThinking===undefined?{}:{modelThinking}),...(reference===undefined?{}:{reference})},()=>references.inheritReferenceThinking(modelThinking,reference,'fixture'));
const metricsIndex=new metrics.CatalogMetricsIndex(rows);
for(const row of rows) add('catalog-metrics',row,()=>({own:metrics.catalogMetricsOf(row),resolved:metricsIndex.resolve(row)}),`${row.provider}/${row.id}`);
for(const id of fixtureIds) {const model=specOf(id);model.identity=taxonomy.classifyModel('fixture',id,{lenient:true});add('fixture-metrics',model,()=>({own:metrics.catalogMetricsOf(model),resolved:metricsIndex.resolve(model)}));}
const scored=[{...specOf('gpt-5.4'),identity:taxonomy.classifyModel('','gpt-5.4'),int:70},{...specOf('gpt-5.4'),identity:taxonomy.classifyModel('','gpt-5.4'),tps:100},{...specOf('claude-opus-4-8'),identity:taxonomy.classifyModel('','claude-opus-4-8'),int:0,tps:0}];
const queries=scored.map(({int,tps,...row})=>row);
add('metrics-apply',{scored,queries},()=>{const index=new metrics.CatalogMetricsIndex(scored);return {empty:index.isEmpty,models:metrics.applyCatalogMetrics(queries,index),resolved:queries.map(q=>capture(()=>index.resolve(q)))};});
add('auth-global',{},()=>({providers:auth.authProviders(),hooks:auth.authHookNames()}));
add('taxonomy-vocab',{},()=>({collapse:taxonomy.collapseVocabulary(),discovery:taxonomy.discoveryVocabulary()}));
for(const id of ['İ-thinking😀','İ-thinking','İ-thinking-x','😀-thinking']) add('taxonomy-unicode',{id},()=>{
  const value=taxonomy.stripThinkingVariantSuffix(id);
  return value===undefined ? {} : {rawJson:JSON.stringify(value),units:Array.from({length:value.length},(_,i)=>value.charCodeAt(i)),representable:!/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/u.test(value)};
});
for(const [provider,id] of [['fixture','😀xxxx'],['openai-codex','😀xx'],['cursor','😀xxxx']]) add('fixture-taxonomy',{provider,id},()=>taxonomyOf(provider,id));
const providerIds=[...new Set([...models.getBundledProviders(),...auth.authProviders().map(p=>p.id),...rules.behavior.retiredProviders,'fixture','OPENAI'])];
const behaviorIds=[...new Set([...rows.map(row=>row.id),...fixtureIds,...rules.behavior.cursorParameters.map(p=>p.model),...rules.behavior.modelLimits.flatMap(p=>p.limits.map(l=>l.model))])];
const branchStrings=new Set();
function collectStrings(value) {if(typeof value==='string')branchStrings.add(value);else if(Array.isArray(value))value.forEach(collectStrings);else if(value&&typeof value==='object')Object.values(value).forEach(collectStrings);}
collectStrings(rules.behavior);
const branchIds=[...new Set([...branchStrings].filter(v=>v.length<180).flatMap(v=>[v,v.toUpperCase(),`prefix-${v}-suffix`,v.replaceAll('*','probe')]))];
for(const provider of providerIds) add('behavior-branches',{provider,ids:branchIds},()=>branchIds.map(id=>capture(()=>behaviorOf(provider,id))));
for(const provider of providerIds) {
  add('provider-vocab',{provider},()=>providerOf(provider));
  add('behavior-provider',{provider,ids:behaviorIds},()=>behaviorIds.map(id=>capture(()=>behaviorOf(provider,id))));
}
const counts=Object.fromEntries([...new Set(cases.map(c=>c.kind))].map(kind=>[kind,cases.filter(c=>c.kind===kind).length]));
const oracle={upstreamCommit:manifest.upstreamCommit,bunVersion:Bun.version,runnerSha256:manifest.runnerSha256,sourceManifest:manifest.sourceManifest,catalogRowCount:rows.length,catalogKeys,behaviorProviders:providerIds,behaviorIds,behaviorPairs:providerIds.length*behaviorIds.length,fixtureIdCount:fixtureIds.size,counts,cases};
fs.writeFileSync(path.join(root,'oracle.json'),JSON.stringify(oracle)+'\n');
console.log(JSON.stringify({cases:cases.length,counts,behaviorPairs:oracle.behaviorPairs,fixtureIds:fixtureIds.size}));
