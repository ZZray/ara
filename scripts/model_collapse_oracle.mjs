// Only unchanged fixed-commit source modules compute expected collapse behavior.
import { mock } from 'bun:test';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { createHash } from 'node:crypto';
import { fileURLToPath, pathToFileURL } from 'node:url';
const manifestPath = path.resolve(process.argv[2]);
const root = path.dirname(manifestPath);
const manifest = JSON.parse(fs.readFileSync(manifestPath,'utf8'));
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
if (Bun.version !== '1.4.0' || manifest.upstreamCommit !== '596f2da7101178214aa27a753529d15e6b7ad91d') throw new Error('wrong source/runtime');
if (digest(fs.readFileSync(process.execPath)) !== manifest.bunSha256 || digest(fs.readFileSync(fileURLToPath(import.meta.url))) !== manifest.runnerSha256) throw new Error('runtime/runner mismatch');
for (const source of [...manifest.sourceManifest,...manifest.licenses]) {
  const bytes=fs.readFileSync(path.join(root,'upstream',source.path));
  if (digest(bytes)!==source.sha256 || createHash('sha1').update(`blob ${bytes.length}\0`).update(bytes).digest('hex')!==source.gitBlob) throw new Error(`source mismatch ${source.path}`);
}
const importRaw = p => import(pathToFileURL(path.join(root,'upstream',p)).href);
const guards=await importRaw('packages/utils/src/type-guards.ts'), tls=await importRaw('packages/utils/src/tls-fetch.ts');
mock.module('@oh-my-pi/pi-utils',()=>({...guards,...tls}));
const load = p => importRaw(`packages/catalog/src/${p}.ts`);
const models=await load('models'), build=await load('build'), taxonomy=await load('compat/taxonomy'), policy=await load('compat/resolve');
const collapsePath=path.join(root,'upstream/packages/catalog/src/compat/collapse.ts');
const collapseBytes=fs.readFileSync(collapsePath);
const collapseSha256=digest(collapseBytes);
const inventory=await import(pathToFileURL(collapsePath).href);
const vocabulary=taxonomy.collapseVocabulary();
const units=s=>Array.from({length:s.length},(_,i)=>s.charCodeAt(i));

// JSON text preserves UTF-16 escapes and own-key order. Sidecars distinguish
// own undefined and non-JSON numbers without changing original call inputs.
function encode(value) {
  const undefinedPaths=[],specialNumbers=[],ownKeys=[];
  function visit(v,p) {
    if(v===undefined) {undefinedPaths.push(p);return;}
    if(typeof v==='number' && (!Number.isFinite(v)||Object.is(v,-0))) {
      specialNumbers.push({path:p,value:Object.is(v,-0)?'-0':String(v)});return;
    }
    if(v && typeof v==='object') {
      const keys=Object.keys(v);
      if(keys.some(key=>v[key]===undefined)) ownKeys.push({path:p,keys});
      for(const key of keys) visit(v[key],[...p,key]);
    }
  }
  visit(value,[]);
  return {rawWireJSON:JSON.stringify(value)??null,undefinedPaths,specialNumbers,ownKeys};
}
function tableValue(table) {
  if(table===undefined) return {kind:'undefined'};
  if(table===Object.prototype) return {kind:'inherited',marker:'ObjectPrototype'};
  if(typeof table==='function') return {kind:'inherited',marker:table===Object?'ObjectConstructor':table.name};
  return {kind:'table',families:table.families, ...(table.templates===undefined?{}:{templates:table.templates.map(t=>({family:t.family,
    ...(t.revision===undefined?{}:{revision:t.revision}),pattern:{sourceUnits:units(t.pattern.source),flags:t.pattern.flags,lastIndex:t.pattern.lastIndex}}))}),
    ...(table.providerAliases===undefined?{}:{providerAliases:table.providerAliases})};
}
function tableInput(table) {
  return {families:table.families.map(encode), ...(table.templates===undefined?{}:{templates:table.templates.map(t=>({family:encode(t.family),
    ...(t.revision===undefined?{}:{revision:t.revision}),pattern:{sourceUnits:units(t.pattern.source),flags:t.pattern.flags,lastIndex:t.pattern.lastIndex}}))}),
    ...(table.providerAliases===undefined?{}:{providerAliases:encode(table.providerAliases)})};
}
const exportsList=['reviewedCollapseTable','deriveThinkingPairFamilies','isCollapsedVariantSpec','collapseVariants','collapseBuiltVariants','resolveVariantSelector','resolveBareVariantSelector','getVariantAliasSources'];
const exportCounts=Object.fromEntries(exportsList.map(k=>[k,0]));
const cases=[];
const namespaces=new Set([inventory]);
const functions=new Set(exportsList.map(name=>inventory[name]));
const reviewedTables=new Set([inventory.reviewedCollapseTable('cursor')]);
const isolation={strategy:'byte-identical-sibling-modules',moduleCount:0,namespaceChecks:0,functionChecks:0,
  tableChecks:0,sourceSha256:collapseSha256};
async function sequence(id,family,inputs,steps,tables=[]) {
  // Bun 1.4.0 on Windows strips query strings from file: URLs. Each sequence
  // therefore imports a distinct sibling file containing the exact Git blob;
  // relative imports still resolve to the unchanged dependency modules.
  const sibling=path.join(path.dirname(collapsePath),`collapse.oracle-sequence-${cases.length}.ts`);
  fs.writeFileSync(sibling,collapseBytes,{flag:'wx'});
  if(digest(fs.readFileSync(sibling))!==collapseSha256) throw new Error('sequence source hash mismatch');
  const module=await import(pathToFileURL(sibling).href);
  if(namespaces.has(module)) throw new Error('sequence namespace was reused');
  namespaces.add(module);isolation.namespaceChecks++;
  for(const name of exportsList) {
    if(functions.has(module[name])) throw new Error(`sequence function was reused: ${name}`);
    functions.add(module[name]);isolation.functionChecks++;
  }
  const reviewed=module.reviewedCollapseTable('cursor');
  if(reviewedTables.has(reviewed)) throw new Error('sequence reviewed table was reused');
  reviewedTables.add(reviewed);isolation.tableChecks++;isolation.moduleCount++;
  const result={id,family,inputs:inputs.map(encode),tables:tables.map(tableInput),steps,expected:[]};
  const values=[],held=[];
  for(const step of steps) {
    exportCounts[step.op]++;
    const specs=step.fromStep===undefined?(step.inputIndices??[]).map(i=>inputs[i]):[...values[step.fromStep],...(step.appendInputIndices??[]).map(i=>inputs[i])];
    try {
      let value;
      switch(step.op) {
        case 'reviewedCollapseTable':value=module.reviewedCollapseTable(step.provider);break;
        case 'deriveThinkingPairFamilies':value=module.deriveThinkingPairFamilies(specs,step.tableIndex===undefined?undefined:tables[step.tableIndex],step.provider);break;
        case 'isCollapsedVariantSpec':value=module.isCollapsedVariantSpec(step.fromStep===undefined?inputs[step.specIndex]:values[step.fromStep][step.resultIndex??0]);break;
        case 'collapseVariants':value=module.collapseVariants(specs,step.tableIndex===undefined?undefined:{table:tables[step.tableIndex]});break;
        case 'collapseBuiltVariants':value=module.collapseBuiltVariants(specs);break;
        case 'resolveVariantSelector':value=module.resolveVariantSelector(step.provider,step.modelId);break;
        case 'resolveBareVariantSelector':value=module.resolveBareVariantSelector(step.modelId);break;
        case 'getVariantAliasSources':value=module.getVariantAliasSources(step.provider,step.modelId);held.push({step:values.length,value});break;
        default:throw new Error(`unknown operation ${step.op}`);
      }
      const items=Array.isArray(value)?value:[];
      const inputRefs=items.map(v=>v!==null&&typeof v==='object'?inputs.indexOf(v):-1);
      const priorRefs=items.map(v=>{if(v===null||typeof v!=='object')return null;for(let s=0;s<values.length;s++)if(Array.isArray(values[s])){const index=values[s].indexOf(v);if(index>=0)return {step:s,index};}return null;});
      values.push(value);
      result.expected.push({status:'ok',value:encode(step.op==='reviewedCollapseTable'?tableValue(value):value),inputRefs,priorRefs,
        heldReverse:held.map(h=>({step:h.step,value:encode(h.value),sameAsCurrent:h.value===value})),
        tableStates:tables.map(t=>(t.templates??[]).map(x=>x.pattern.lastIndex))});
    } catch(error) {
      values.push(undefined);
      result.expected.push({status:'error',name:error.name,message:error.message,
        heldReverse:held.map(h=>({step:h.step,value:encode(h.value),sameAsCurrent:false})),
        tableStates:tables.map(t=>(t.templates??[]).map(x=>x.pattern.lastIndex))});
    }
  }
  cases.push(result);
}
const cost={input:1,output:2,cacheRead:0.1,cacheWrite:0};
const spec=(id,provider='fixture',extra={})=>({id,name:id,api:provider==='cursor'?'cursor-agent':provider==='devin'?'devin-agent':provider.startsWith('google-')?'google-gemini-cli':'openai-completions',
  provider,baseUrl:'https://example.invalid/v1',reasoning:false,input:['text'],cost:{...cost},contextWindow:32768,maxTokens:4096,...extra});
const collapse=(inputIndices,extra={})=>({op:'collapseVariants',inputIndices,...extra});
const indices=xs=>xs.map((_,i)=>i);
function followup(first=0,extra={}) {return [{op:'collapseVariants',fromStep:first,...extra},{op:'collapseVariants',fromStep:first,appendInputIndices:[0],...extra}];}
const providers=[...new Set([...vocabulary.variantFamilies.map(f=>f.provider),...Object.keys(vocabulary.providerAliases)])];
for(const provider of [...providers,'UNKNOWN','CURSOR','__proto__','constructor','toString','hasOwnProperty','valueOf']) {
  await sequence(`table/${provider}`,'reviewed-table',[],[{op:'reviewedCollapseTable',provider}]);
}
for(const provider of providers) {
  const table=inventory.reviewedCollapseTable(provider);
  for(const family of table.families) {
    const subsets=Array.from({length:2**family.members.length},(_,mask)=>family.members.filter((_,i)=>mask&(1<<i)));
    const reversed=[...family.members].reverse();
    if(!subsets.some(s=>JSON.stringify(s)===JSON.stringify(reversed))) subsets.push(reversed);
    for(let n=0;n<subsets.length;n++) {
      const input=subsets[n].map((id,i)=>spec(id,provider,{contextWindow:i%2?null:32768+i,maxTokens:4096+i,input:i%2?['image']:['text']}));
      await sequence(`family/${provider}/${family.id}/${n}`,'reviewed-family',input,[collapse(indices(input)),{op:'collapseVariants',fromStep:0}]);
    }
    const stale=spec(family.id,provider,{reasoning:true,requestModelId:family.retiredMembers?.[0]??family.members[0],thinking:{mode:'effort',efforts:['high'],effortRouting:{high:family.retiredMembers?.[0]??family.members[0]}}});
    await sequence(`stale/${provider}/${family.id}`,'stale-family',[stale,...family.members.map(m=>spec(m,provider))],[collapse([0]),collapse(indices([stale,...family.members])),{op:'isCollapsedVariantSpec',specIndex:0}]);
    if(family.defaultMember!==undefined) {
      const oldMember=family.members.find(m=>m!==family.defaultMember)??family.members[0];
      const staleDefault=spec(family.id,provider,{reasoning:true,requestModelId:oldMember,
        thinking:{...family.thinking,effortRouting:{...family.routing}}});
      await sequence(`default-repair/${provider}/${family.id}`,'default-repair',
        [staleDefault,...family.members.map(m=>spec(m,provider))],[collapse([0]),collapse(indices([staleDefault,...family.members]))]);
    }
    for(const alias of family.extraAliases??[]) await sequence(`extra/${provider}/${family.id}/${alias}`,'extra-alias',[
      {...stale,id:alias},stale],[collapse([0]),collapse([0,1]),{op:'resolveVariantSelector',provider,modelId:alias},{op:'resolveBareVariantSelector',modelId:alias},{op:'getVariantAliasSources',provider,modelId:family.id}]);
    await sequence(`alias/${provider}/${family.id}`,'reviewed-alias',[],[
      {op:'getVariantAliasSources',provider,modelId:family.id},
      ...[family.id,...family.members,...family.extraAliases??[]].flatMap(modelId=>[
        {op:'resolveVariantSelector',provider,modelId:` ${modelId.toUpperCase()} `},{op:'resolveBareVariantSelector',modelId}]),
      {op:'getVariantAliasSources',provider,modelId:family.id}]);
  }
  for(let t=0;t<(table.templates??[]).length;t++) {
    const template=table.templates[t];
    for(const revision of ['0','1','2.5','3.1','3.8','5.9','255','256','03.08']) {
      const replace=x=>x.replaceAll('{rev}',revision);
      const input=template.family.members.map(m=>spec(replace(m),provider));
      await sequence(`template/${provider}/${t}/${revision}`,'reviewed-template',input,[
        {op:'getVariantAliasSources',provider,modelId:replace(template.family.id)},collapse(indices(input)),
        ...input.map(s=>({op:'resolveVariantSelector',provider,modelId:s.id})),{op:'resolveBareVariantSelector',modelId:input[0].id},
        {op:'getVariantAliasSources',provider,modelId:replace(template.family.id)}]);
    }
    for(const firstId of [...template.family.members,...template.family.extraAliases??[]]) {
      const replace=x=>x.replaceAll('{rev}','3.9');
      await sequence(`template-first/${provider}/${t}/${firstId}`,'template-first',[],[
        {op:'resolveVariantSelector',provider,modelId:replace(firstId).toUpperCase()},
        {op:'getVariantAliasSources',provider,modelId:replace(template.family.id)},
        {op:'resolveBareVariantSelector',modelId:replace(firstId)},
        {op:'getVariantAliasSources',provider,modelId:replace(template.family.id)}]);
    }
  }
  for(const [modelId,target] of Object.entries(table.providerAliases??{})) await sequence(`provider-alias/${provider}/${modelId}`,'provider-alias',[],[
    {op:'resolveVariantSelector',provider,modelId},{op:'resolveBareVariantSelector',modelId},{op:'getVariantAliasSources',provider,modelId:target}]);
}

// Twin pricing gates and metadata precedence; no native assumptions in expected.
for(const [label,baseExtra,twinExtra] of [
  ['equal',{},{}],['unknown-base',{cost:{input:0,output:0,cacheRead:9,cacheWrite:9}},{}],
  ['unknown-twin',{}, {cost:{input:0,output:0,cacheRead:9,cacheWrite:9}}],
  ...['input','output','cacheRead','cacheWrite'].map(k=>[`price-${k}`,{},{cost:{...cost,[k]:9}}]),
  ['api',{}, {api:'anthropic-messages'}],['base-surface',{thinking:{mode:'budget',efforts:['low','high'],effortBudgets:{low:99,high:999}}},{}],
  ['twin-surface',{thinking:{mode:'effort',efforts:['low']}},{thinking:{mode:'budget',efforts:['medium'],requiresEffort:true,suppressWhenOff:true,effortRouting:{medium:'old'}}}],
  ['empty-surface',{}, {thinking:{mode:'effort',efforts:[]}}],['zero-cost',{cost:{input:0,output:0,cacheRead:0,cacheWrite:0}},{cost:{input:0,output:0,cacheRead:0,cacheWrite:0}}]
]) {
  const input=[spec('example', 'fixture',baseExtra),spec('example-thinking','fixture',twinExtra)];
  await sequence(`twins/${label}`,'thinking-pair',input,[{op:'deriveThinkingPairFamilies',inputIndices:[0,1]},collapse([0,1]),...followup(1)]);
}
for(const [base,twin] of [['foo-tools','foo-thinking-tools'],['foo','foo-THINKING'],['foo','foo-non-thinking'],['foo','foo-no-thinking'],['foo','foo-thinking'],['foo','foo-thinking-thinking']]) {
  await sequence(`tokens/${base}/${twin}`,'thinking-token',[spec(base),spec(twin)],[{op:'deriveThinkingPairFamilies',inputIndices:[0,1]},collapse([1,0]),collapse([1])]);
}
const cursorBase=[spec('novel-low','cursor'),spec('novel-high','cursor')];
const cursorCases=[['normal',cursorBase],['fast',cursorBase.flatMap(s=>[s,{...s,id:`${s.id}-fast`}])],
  ['independent-base',[...cursorBase,spec('novel','cursor')]],['independent-fast',[...cursorBase,spec('novel-low-fast','cursor'),spec('novel-high-fast','cursor'),spec('novel-fast','cursor')]],
  ['duplicate',[...cursorBase,{...cursorBase[0]}]],['equivalent-tier',[spec('novel-extra-high','cursor'),spec('novel-xhigh','cursor')]],
  ['product-tier',[spec('novel-max-low','cursor'),spec('novel-max-high','cursor')]],['product-thinking',[spec('novel-thinking-low','cursor'),spec('novel-thinking-high','cursor')]],
  ['thinking-candidate',[...cursorBase,spec('novel-thinking-low','cursor'),spec('novel-thinking-high','cursor')]],
  ...['thinking','thinking-fast','fast-thinking'].map(s=>[`thinking-${s}`,[...cursorBase,spec(`novel-${s}`,'cursor')]]),
  ['nested-tier',[...cursorBase,spec('novel-low-high','cursor'),spec('novel-low-low','cursor')]],
  ...Object.entries({api:'anthropic-messages',baseUrl:'https://other.invalid',contextWindow:999,maxTokens:999,cursorMaxMode:true,cost:{...cost,input:9},compat:{x:true},thinking:{mode:'effort',efforts:['low']},requestModelId:'wire'}).map(([key,value])=>[key,[cursorBase[0],{...cursorBase[1],[key]:value}]]),
  ['off',[spec('novel-none','cursor'),spec('novel-high','cursor')]],['singleton',[cursorBase[0]]],
  ['name',[{...cursorBase[0],name:' Friendly Low '},{...cursorBase[1],name:'Friendly High'}]],
];
for(const [label,input] of cursorCases) await sequence(`cursor/${label}`,'cursor-gate',input,[collapse(indices(input)),{op:'collapseVariants',fromStep:0}]);

// A retained reverse result is observed after subsequent registration. Later
// lookup is not a substitute for preserving the earlier live-array identity.
await sequence('dynamic/alias-state','alias-state',[
  spec('base'),spec('base-thinking'),spec('other','second'),spec('other-thinking','second'),
  spec('base','fixture',{thinking:{mode:'effort',efforts:['low'],effortRouting:{low:'new-wire'}}})],[
  {op:'resolveVariantSelector',provider:'fixture',modelId:'base-thinking'},collapse([0,1]),
  {op:'getVariantAliasSources',provider:'fixture',modelId:'base'},collapse([4]),
  {op:'getVariantAliasSources',provider:'fixture',modelId:'base'},
  {op:'resolveVariantSelector',provider:'FIXTURE',modelId:' BASE-THINKING '},
  {op:'resolveBareVariantSelector',modelId:'new-wire'},collapse([2,3]),
  {op:'resolveBareVariantSelector',modelId:'other-thinking'},collapse([0]),
  {op:'resolveVariantSelector',provider:'fixture',modelId:'base-thinking'}]);
await sequence('retarget/references','retarget',[
  ...cursorBase,spec('client','cursor',{contextPromotionTarget:'novel-high',compactionModel:'CURSOR/novel-low'}),
  spec('client2','fixture',{contextPromotionTarget:'cursor/novel-high',compactionModel:'missing/novel-low'}),
  spec('novel','cursor')],[collapse([0,1,2,3]),collapse([0,1,2,3,4])]);
await sequence('caller/mixed-provider-order','caller-contract',[
  spec('a','first'),spec('b','second'),spec('a-thinking','first'),spec('b-thinking','second'),spec('untouched','third')],
  [collapse([0,1,2,3,4]),{op:'collapseVariants',fromStep:0},{op:'isCollapsedVariantSpec',specIndex:4},{op:'isCollapsedVariantSpec',fromStep:0}]);
await sequence('caller/first-duplicate-wins','caller-contract',[
  spec('a','fixture',{name:'first'}),spec('a','fixture',{name:'second',contextWindow:999}),spec('a-thinking')],
  [collapse([0,1,2]),collapse([1,0,2])]);
await sequence('caller/detection-boundaries','caller-contract',[
  spec('unrelated','fixture',{requestModelId:'wire'}),spec('x','fixture',{thinking:{effortRouting:{}}}),
  spec('x','fixture',{thinking:{effortRouting:undefined}}),spec('x','fixture',{thinking:{effortRouting:null}})],
  [0,1,2,3].map(specIndex=>({op:'isCollapsedVariantSpec',specIndex})));
await sequence('cursor/matching-collapsed','caller-contract',[
  ...cursorBase,spec('novel','cursor',{requestModelId:'novel-low',thinking:{mode:'effort',efforts:['low','high'],effortRouting:{high:'novel-high'}}})],
  [collapse([2,0,1]),collapse([0,1,2])]);
for(const reverse of [false,true]) {
  const input=['first','second'].flatMap((provider,i)=>[spec(`logical-${i}`,provider,{thinking:{mode:'effort',efforts:['high'],effortRouting:{high:'shared-alias'}}})]);
  await sequence(`dynamic/ambiguous-${reverse}`,'alias-state',input,[collapse(reverse?[1,0]:[0,1]),
    {op:'resolveBareVariantSelector',modelId:'shared-alias'},
    {op:'resolveVariantSelector',provider:'first',modelId:'shared-alias'},
    {op:'resolveVariantSelector',provider:'second',modelId:'shared-alias'}]);
}
await sequence('dynamic/first-alias-wins','alias-state',[
  spec('one','fixture',{thinking:{mode:'effort',efforts:['high'],effortRouting:{high:'wire'}}}),
  spec('two','fixture',{thinking:{mode:'effort',efforts:['high'],effortRouting:{high:'wire'}}})],
  [collapse([0]),{op:'getVariantAliasSources',provider:'fixture',modelId:'one'},collapse([1]),
    {op:'resolveVariantSelector',provider:'fixture',modelId:'wire'},
    {op:'getVariantAliasSources',provider:'fixture',modelId:'one'},
    {op:'getVariantAliasSources',provider:'fixture',modelId:'two'},
    {op:'getVariantAliasSources',provider:'fixture',modelId:'two'}]);
await sequence('dynamic/static-plus-dynamic-reverse','alias-state',[
  spec('gpt-5.6','cursor',{thinking:{mode:'effort',efforts:['high'],effortRouting:{high:'custom-wire'}}})],
  [{op:'getVariantAliasSources',provider:'cursor',modelId:'gpt-5.6'},collapse([0]),
    {op:'getVariantAliasSources',provider:'cursor',modelId:'gpt-5.6'},
    {op:'getVariantAliasSources',provider:'cursor',modelId:'gpt-5.6'},
    {op:'getVariantAliasSources',provider:'cursor',modelId:'GPT-5.6'}]);

const customFamily={id:'logical',name:'Logical',members:['wire-low','wire-high'],routing:{off:'wire-low',low:'wire-low',high:'wire-high'},thinking:{mode:'budget',efforts:['low','high'],effortBudgets:{low:128,high:1024}},defaultMember:'wire-high',extraAliases:['old']};
for(const [label,delta] of [['basic',{}],['retired',{retiredMembers:['wire-low']}],['all-retired',{retiredMembers:['wire-low','wire-high']}],['absent',{preserveAbsentEffortRoutes:true}],['surface-less',{thinking:undefined}],['suppress',{suppressWhenOff:true}]]) {
  const table={families:[{...customFamily,...delta}]};
  const input=[spec('wire-low'),spec('wire-high'),spec('old','fixture',{requestModelId:'wire-low',thinking:{mode:'effort',efforts:['high'],effortRouting:{high:'wire-low'}}})];
  await sequence(`custom/${label}`,'custom-table',input,[collapse([0,1],{tableIndex:0}),collapse([0],{tableIndex:0}),collapse([2],{tableIndex:0}),{op:'deriveThinkingPairFamilies',inputIndices:[0,1],tableIndex:0}], [table]);
}
for(const flags of ['i','gi','y']) {
  const table={families:[],templates:[{family:{id:'x-{rev}',name:'X {rev}',members:['x-{rev}-low','x-{rev}-high'],routing:{low:'x-{rev}-low',high:'x-{rev}-high'},thinking:{mode:'effort',efforts:['low','high']}},pattern:new RegExp('^x-(\\d+)-(?:low|high)$',flags)}]};
  await sequence(`custom/template-state-${flags}`,'custom-template',[spec('x-8-low'),spec('x-8-high')],[collapse([0,1],{tableIndex:0}),collapse([0,1],{tableIndex:0})],[table]);
}
await sequence('custom/reviewed-claims-pair','custom-table',[spec('a'),spec('a-thinking')],
  [{op:'deriveThinkingPairFamilies',inputIndices:[0,1],tableIndex:0},collapse([0,1],{tableIndex:0})],
  [{families:[{id:'logical',name:'Claimed',members:['a','a-thinking'],routing:{high:'a-thinking'},thinking:{mode:'effort',efforts:['high']}}]}]);
await sequence('custom/all-retired-canonical-stale','custom-table',[
  spec('logical','fixture',{reasoning:true,requestModelId:'wire-low',thinking:{mode:'effort',efforts:['low','high'],effortRouting:{low:'wire-low',high:'wire-high'}}})],
  [collapse([0],{tableIndex:0}),{op:'collapseVariants',fromStep:0,tableIndex:0}],
  [{families:[{...customFamily,retiredMembers:['wire-low','wire-high']}]}]);
await sequence('custom/explicit-does-not-register','caller-contract',[spec('wire-low'),spec('wire-high')],
  [collapse([0,1],{tableIndex:0}),{op:'resolveVariantSelector',provider:'fixture',modelId:'wire-low'},
    {op:'getVariantAliasSources',provider:'fixture',modelId:'logical'}],[{families:[customFamily]}]);
await sequence('custom/explicit-does-not-retarget','caller-contract',[
  spec('logical','fixture',{thinking:{mode:'effort',efforts:['high'],effortRouting:{high:'wire-high'}}}),
  spec('wire-low'),spec('wire-high'),spec('observer','fixture',{contextPromotionTarget:'wire-high',compactionModel:'fixture/wire-high'})],
  [collapse([0]),collapse([1,2,3],{tableIndex:0}),{op:'resolveVariantSelector',provider:'fixture',modelId:'wire-high'}],
  [{families:[customFamily]}]);
await sequence('custom/template-revision-override','custom-template',[spec('x-1-low'),spec('x-2-low'),spec('x-256-low')],
  [collapse([0,1,2],{tableIndex:0}),collapse([0,1,2],{tableIndex:0})],
  [{families:[{id:'x-2',name:'Concrete',members:['x-2-low'],routing:{}}],templates:[{
    family:{id:'x-{rev}',name:'Template {rev}',members:['x-{rev}-low'],routing:{}},
    revision:[{op:'>=',revision:[2,0,0]}],pattern:/^x-(\d+)-low$/i}]}]);

// Mandatory lossless values remain original successful/error expected outputs;
// limitations of a native JSON-backed policy never remove these cases.
for(const [label,extra] of [['undefined',{opaque:undefined,thinking:undefined}],['nan-limit',{contextWindow:NaN}],['nan-cost',{cost:{...cost,input:NaN}}],
  ['infinity',{maxTokens:Infinity}],['negative-infinity',{contextWindow:-Infinity}],['negative-zero',{cost:{...cost,cacheRead:-0}}],
  ['lone-name',{name:'name\ud800'}],['lone-thinking',{thinking:{mode:'budget',efforts:['high'],opaque:'\ud800'}}],
  ['lone-metadata',{opaque:'\udc00',nested:{missing:undefined,n:NaN}}],['undefined-array',{opaque:[undefined,NaN,-0,'\ud800']}],
  ['key-order',{opaque:{first:undefined,middle:1,last:undefined}}]]) {
  const input=[spec('x', 'fixture',extra),spec('x-thinking','fixture',extra)];
  await sequence(`lossless/${label}`,'lossless-mandatory',input,[{op:'deriveThinkingPairFamilies',inputIndices:[0,1]},collapse([0,1]),{op:'collapseBuiltVariants',inputIndices:[0,1]}]);
}
await sequence('lossless/lone-id','lossless-mandatory',[spec('x\ud800'),spec('x\ud800-thinking')],
  [collapse([0,1]),{op:'deriveThinkingPairFamilies',inputIndices:[0,1]},{op:'collapseBuiltVariants',inputIndices:[0,1]}]);
await sequence('lossless/lowercase-expansion-id','lossless-mandatory',[
  spec('İ-\ude00','fixture',{thinking:{mode:'budget',efforts:['high']}}),
  spec('İ-thinking😀','fixture',{thinking:{mode:'budget',efforts:['high']}})],
  [{op:'deriveThinkingPairFamilies',inputIndices:[0,1]},collapse([0,1]),{op:'collapseBuiltVariants',inputIndices:[0,1]}]);
for(const id of ['novel','gpt-5.4']) {
  const extra={cost:{input:0,output:0,cacheRead:NaN,cacheWrite:Infinity},contextWindow:NaN,maxTokens:Infinity};
  await sequence(`lossless/new-build-cost/${id}`,'lossless-mandatory',[spec(id,'fixture',extra),spec(`${id}-thinking`,'fixture',extra)],
    [{op:'deriveThinkingPairFamilies',inputIndices:[0,1]},collapse([0,1]),{op:'collapseBuiltVariants',inputIndices:[0,1]}]);
}
for(const provider of ['__proto__','constructor','toString']) await sequence(`inherited/${provider}`,'inherited-marker',[spec('x',provider)],
  [{op:'reviewedCollapseTable',provider},collapse([0]),{op:'resolveVariantSelector',provider,modelId:'x'}]);

const rows=models.getBundledProviders().flatMap(provider=>models.getBundledModels(provider));
const catalogKeys=rows.map(row=>`${row.provider}/${row.id}`);
if(rows.length!==4776 || new Set(catalogKeys).size!==4776) throw new Error('bundled row coverage mismatch');
await sequence('catalog/all-built','catalog-built',rows,[{op:'collapseBuiltVariants',inputIndices:indices(rows)},
  {op:'collapseBuiltVariants',fromStep:0},{op:'collapseVariants',inputIndices:indices(rows)}]);
for(const provider of models.getBundledProviders()) {
  const subset=rows.filter(row=>row.provider===provider);
  await sequence(`catalog/${provider}`,'catalog-provider',subset,[{op:'collapseBuiltVariants',inputIndices:indices(subset)}]);
}
// Direct policy seams confirmed by independent native/source review. These
// calls are separate from the frozen collapse sequences and retain every own
// undefined property and raw UTF-16 leaf through the same lossless codec.
const policySpec=(provider,id,api,compat)=>({provider,id,api,compat,name:id,baseUrl:'https://example.invalid/v1',
  reasoning:true,input:['text'],cost:{input:1,output:2,cacheRead:0,cacheWrite:0},contextWindow:32768,maxTokens:4096});
const policyInputs=[
  ['proto-native-own',()=>policySpec('fixture','novel','openai-completions',JSON.parse('{"__proto__":{"junk":true},"junk":"\\ud800"}'))],
  ['xai-map-own-undefined',()=>policySpec('xai','grok-4','openai-responses',{reasoningEffortMap:{extra:undefined,low:'old',xhigh:undefined}})],
  ['deepseek-extra-own-undefined',()=>policySpec('deepseek','deepseek-reasoner','openai-completions',{extraBody:{thinking:{type:'enabled'},opaque:undefined}})],
  ['thinking-map-own-undefined',()=>policySpec('fixture','novel','openai-completions',{reasoningEffortMap:{low:undefined,extra:'unused'}})],
  ...['mistral-mixtral','claude-haiku-fable','minimax-m1-minimax-m2'].map(id=>[
    `ambiguous-${id}`,()=>policySpec('fixture',id,'openai-completions',undefined)]),
  ['lone-url-deepseek',()=>({...policySpec('fixture','deepseek-reasoner','openai-completions',{}),baseUrl:'https://api.deepseek.com/v1/\ud800'})],
  ['lone-url-loopback',()=>({...policySpec('fixture','deepseek-reasoner','openai-completions',{}),baseUrl:'http://127.0.0.1:8000/v1/\ud800'})],
  ['deepseek-extra-authored-thinking',()=>policySpec('deepseek','deepseek-reasoner','openai-completions',{extraBody:{thinking:{type:'enabled'},opaque:undefined},whenThinking:{thinkingFormat:'openai'}})],
  ...['mistral-mixtral','claude-haiku-fable','minimax-m1-minimax-m2'].map(id=>[
    `ambiguous-lone-${id}`,()=>policySpec('fixture',`${id}\ud800`,'openai-completions',undefined)]),
];
const losslessPolicyCases=[];
for(const [label,makeInput] of policyInputs) for(const op of ['resolveModelPolicy','buildModel']) {
  const input=makeInput();
  const item={id:`${label}/${op}`,op,input:encode(input)};
  try {
    const value=op==='resolveModelPolicy'?policy.resolveModelPolicy(input):build.buildModel(input);
    item.expected={status:'ok',value:encode(value)};
  } catch(error) {item.expected={status:'error',name:error.name,message:error.message};}
  losslessPolicyCases.push(item);
}
const counts=Object.fromEntries([...new Set(cases.map(c=>c.family))].map(f=>[f,cases.filter(c=>c.family===f).length]));
const output={...manifest,catalogRowCount:rows.length,catalogKeys,reviewedProviders:providers,
  reviewedFamilyCount:providers.reduce((n,p)=>n+inventory.reviewedCollapseTable(p).families.length,0),
  reviewedTemplateCount:providers.reduce((n,p)=>n+(inventory.reviewedCollapseTable(p).templates??[]).length,0),
  originalTestAssertionsExecuted:false,fixtureUse:'The unchanged test source is retained as coverage evidence; its assertions, discovery and model-manager integration suites are not executed.',
  isolation,counts,exportCounts,cases,losslessPolicyCaseCount:losslessPolicyCases.length,losslessPolicyCases};
fs.writeFileSync(path.join(root,'oracle.json'),JSON.stringify(output)+'\n');
console.log(JSON.stringify({cases:cases.length,counts,exportCounts}));
