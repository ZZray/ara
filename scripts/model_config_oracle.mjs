// Only package facades are supplied by this harness. All schema/validation
// functions and config-file I/O execute the unchanged exported upstream blobs.
import { mock } from 'bun:test';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { createHash } from 'node:crypto';
import { fileURLToPath, pathToFileURL } from 'node:url';

const manifestPath = path.resolve(process.argv[2]);
const root = path.dirname(manifestPath);
const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
const digest = data => createHash('sha256').update(data).digest('hex');
if (Bun.version !== '1.4.0' || manifest.upstreamCommit !== '596f2da7101178214aa27a753529d15e6b7ad91d') throw new Error('wrong runtime/source');
if (digest(fs.readFileSync(fileURLToPath(import.meta.url))) !== manifest.runnerSha256) throw new Error('runner hash mismatch');
for (const source of [...manifest.sourceManifest, manifest.license]) {
  const bytes = fs.readFileSync(path.join(root, 'upstream', source.path));
  if (digest(bytes) !== source.sha256 || createHash('sha1').update(`blob ${bytes.length}\0`).update(bytes).digest('hex') !== source.gitBlob) throw new Error(`source hash mismatch: ${source.path}`);
}
const sourceRoot = path.join(root, 'upstream', 'packages');
const omt = await import(pathToFileURL(path.join(sourceRoot, 'omptype', 'src', 'index.ts')).href);
mock.module('@oh-my-pi/omptype', () => omt);
mock.module('@oh-my-pi/pi-utils', () => ({
  once: fn => { let ready = false, value; return (...args) => { if (!ready) { value = fn(...args); ready = true; } return value; }; },
  getAgentDir: () => path.join(root, 'unused-host-default'),
  isEnoent: error => error?.code === 'ENOENT',
  logger: { warn() {} },
}));
const configRoot = path.join(sourceRoot, 'coding-agent', 'src', 'config');
const { getModelsConfigSchema } = await import(pathToFileURL(path.join(configRoot, 'models-config-schema-bundle.ts')).href);
const { validateProviderConfiguration, ModelsConfigFile } = await import(pathToFileURL(path.join(configRoot, 'models-config.ts')).href);
const schema = getModelsConfigSchema();
const cases = [];
function addSchema(input, label) {
  const result = schema(structuredClone(input));
  cases.push({kind:'schema', label, input, ok: !(result instanceof omt.OmpErrors), ...(result instanceof omt.OmpErrors ? {} : {value:result})});
}
const wrap = provider => ({providers:{fixture:provider}});
const custom = () => ({baseUrl:'https://example.invalid/v1',auth:'none',api:'openai-completions',models:[{id:'m'}]});
for (const input of [{}, [], null, {providers:[]}, {providers:[{auth:'none'}]}, {providers:{p:[]}}, {providers:{p:{auth:'none',keep:{any:[1,null]}}}}, {providers:null}]) addSchema(input, 'root/object semantics');
// A complete valid specimen makes every fixed field execute in one schema call.
const full = custom();
Object.assign(full,{headers:{X:'!credential'},apiKey:'!command',authHeader:true,transport:'pi-native',disableStrictTools:true,guardrailIdentifier:'arn',guardrailVersion:'DRAFT',guardrailTrace:'enabled_full',requestMetadata:{tag:'v'},remoteCompaction:{enabled:true,api:'openai-responses',endpoint:'https://x',model:'m',v2StreamingEnabled:true,v2Endpoint:'v2',streamingEndpoint:'stream'}});
Object.assign(full.models[0],{name:'n',reasoning:true,thinking:{mode:'effort',efforts:['low','high'],defaultLevel:'low',effortMap:{low:'l'},supportsDisplay:true,requiresEffort:false},input:['text','image'],imageInputDecoder:'stb',tokenizer:'qwen3',supportsTools:true,cost:{input:1,output:2,cacheRead:3,cacheWrite:4},premiumMultiplier:1,contextWindow:10,maxTokens:5,omitMaxOutputTokens:false,preferWebsockets:true,headers:{Y:'env'},contextPromotionTarget:'large',compactionModel:'small',remoteCompaction:{enabled:false}});
const bundle = fs.readFileSync(path.join(configRoot,'models-config-schema-bundle.ts'),'utf8');
const flags = [...bundle.matchAll(/"(\w+)\?": "boolean"/g)].map(match=>match[1]);
const compat = Object.fromEntries(flags.filter(key=>!['enabled','v2StreamingEnabled','reasoning','supportsTools','omitMaxOutputTokens','preferWebsockets','authHeader','disableStrictTools','supportsDisplay','requiresEffort','injectV1'].includes(key)).map(key=>[key,true]));
Object.assign(compat,{reasoningEffortMap:{minimal:'m',max:'x'},maxTokensField:'max_tokens',reasoningContentField:'reasoning',thinkingFormat:'qwen',openRouterRouting:{only:['a'],order:['b']},vercelGatewayRouting:{order:['c']},extraBody:{nested:[null,{}]},cacheControlFormat:'anthropic',toolStrictMode:'none',streamIdleTimeoutMs:0,streamMarkupHealingPattern:'kimi',promptCacheMode:'automatic',promptCacheMinimumTokens:0,promptCacheMaximumCheckpoints:1,whenThinking:{supportsStore:false}});
full.compat = compat;
full.models[0].compat = compat;
full.modelOverrides = {m:{...full.models[0],cost:{input:0},contextWindow:0,maxTokens:-1}};
addSchema(wrap(full),'all fixed fields');
for (const field of Object.keys(full)) for (const bad of [null, [], {}, 0, false, 'bad']) { const input=structuredClone(full); input[field]=bad; addSchema(wrap(input),`provider ${field} ${JSON.stringify(bad)}`); }
for (const field of Object.keys(full.models[0])) for (const bad of [null, [], {}, 0, false, '']) { const input=custom(); input.models[0][field]=bad; addSchema(wrap(input),`model ${field} ${JSON.stringify(bad)}`); }
for (const field of Object.keys(compat)) for (const bad of [null, [], {}, -1, 'bad']) { const input=custom(); input.compat={[field]:bad}; addSchema(wrap(input),`compat ${field} ${JSON.stringify(bad)}`); }
for (const thinking of [
 {mode:'effort',efforts:[]}, {mode:'budget',levels:['max','low','low']},
 {mode:'effort',minLevel:'high',maxLevel:'low'}, {mode:'effort',minLevel:'low',maxLevel:'high'},
 {mode:'effort',efforts:['minimal'],levels:['max'],minLevel:'high',maxLevel:'max',extra:'lost'},
 {mode:'google-level',defaultLevel:'high'}, {mode:'anthropic-adaptive',efforts:['low'],defaultLevel:'max'},
]) { const input=custom(); input.models[0].thinking=thinking; addSchema(wrap(input),'thinking precedence'); }
for (const type of ['ollama','llama.cpp','lm-studio','openai-models-list','proxy','litellm']) for (const timeout of [0,1,-1,0.5,null]) addSchema(wrap({discovery:{type,timeoutMs:timeout,injectV1:false}}),'discovery');
for (const api of ['openai-completions','openai-responses','openai-codex-responses','azure-openai-responses','anthropic-messages','bedrock-converse-stream','google-generative-ai','google-gemini-cli','google-vertex']) { const input=custom(); input.api=api; addSchema(wrap(input),'api'); }
function addProvider(input, mode, label) {
  let error;
  try { validateProviderConfiguration('fixture', structuredClone(input), mode); } catch (e) { error = e.message; }
  cases.push({kind:'provider',label,input,mode,ok:error===undefined,...(error===undefined?{}:{error})});
}
for (const mode of ['models-config','runtime-register']) {
 for (const auth of [undefined,'none','oauth','apiKey']) for (const baseUrl of [undefined,'','url']) for (const apiKey of [undefined,'','!cmd']) for (const oauthConfigured of [undefined,true,false]) {
  const input={models:[{id:'m',api:'openai-completions'}],...(auth===undefined?{}:{auth}),...(baseUrl===undefined?{}:{baseUrl}),...(apiKey===undefined?{}:{apiKey}),...(oauthConfigured===undefined?{}:{oauthConfigured})};
  addProvider(input,mode,'authentication precedence');
 }
 for (const field of ['headers','compat','requestMetadata','modelOverrides','remoteCompaction','discovery','disableStrictTools','guardrailIdentifier','transport','auth']) for (const value of [null,{},[],false,'',true]) addProvider({models:[],[field]:value},mode,'empty provider truthiness');
 for (const model of [{id:''},{id:'m'},{id:'',api:'openai-completions'},{id:'m',api:'openai-completions',contextWindow:0},{id:'m',api:'openai-completions',maxTokens:-1}]) addProvider({baseUrl:'url',apiKey:'key',models:[model]},mode,'model error precedence');
}
for (const [ext,content] of [
 ['yml','providers: {local: {auth: none}}'], ['yaml','extra: 0o17'],
 ['jsonc','{/* comment */"providers": {},"a": [1,2,],}'], ['jsonc','{,}'], ['jsonc','[,]'],
 ['yml',''],['yml','providers: bad'],['yml','providers: {p: {models: [{id: x}]}}'],
 ['yml','base: &b {auth: none}\nproviders: {local: {<<: *b}}'],
 ['yml','a: Infinity\nb: NaN\nc: inf'], ['jsonc','{"providers":{"p":{"apiKey":"0o123"}}}'],
]) {
 const file=path.join(root,`fixture-${cases.length}.${ext}`); fs.writeFileSync(file,content);
 const handle=ModelsConfigFile.relocate(file); const result=handle.tryLoad();
 const stage=result.status==='error' ? (result.error.schemaErrors?'Schema':result.error.other?.stage) : undefined;
 cases.push({kind:'file',label:`native ${ext}`,ext,content,status:result.status,...(stage?{stage}:{}),...(result.status==='ok'?{value:result.value}:{})});
}
fs.writeFileSync(path.join(root,'oracle.json'),JSON.stringify({upstreamCommit:manifest.upstreamCommit,sourceManifest:manifest.sourceManifest,bunVersion:Bun.version,cases},null,2)+'\n');
console.log(JSON.stringify({cases:cases.length}));
