// Graph identity and mutation observations on unchanged fixed TLS helpers.
export async function referenceProbe(kind,source){
 const capture=async init=>{let captured;const fetchImpl=async(_input,out)=>{captured=out;return new Response('ok');};await source.wrapFetchForExtraCa(fetchImpl)('https://fixture.test/models',init);return captured;};
 if(kind==='init'){
  const nested={v:1},items=[nested],callback=()=>{},init={headers:nested,tls:{ca:['root'],unknown:nested},items,callback};
  const out=await capture(init),receipt={initSame:out===init,tlsSame:out.tls===init.tls,caSame:out.tls.ca===init.tls.ca,headersSame:out.headers===nested,unknownSame:out.tls.unknown===nested,itemsSame:out.items===items,callbackSame:out.callback===callback,crossAlias:out.headers===out.tls.unknown};
  out.headers.v=2;out.items.push('new');return{...receipt,originalNested:init.tls.unknown.v,originalItems:init.items.length,originalCa:init.tls.ca.length,outputCa:out.tls.ca.length};
 }
 if(kind==='ca-elements'){
  const element={v:1},callback=()=>{},ca=[element,callback,, 'root'],init={tls:{ca}},out=await capture(init);
  const receipt={initSame:out===init,tlsSame:out.tls===init.tls,caSame:out.tls.ca===ca,elementSame:out.tls.ca[0]===element,callbackSame:out.tls.ca[1]===callback,inputHole:Object.hasOwn(ca,2),outputHole:Object.hasOwn(out.tls.ca,2),holeUndefined:out.tls.ca[2]===undefined};
  out.tls.ca[0].v=7;out.tls.ca.push('new');return{...receipt,originalElement:ca[0].v,originalCa:ca.length,outputCa:out.tls.ca.length};
 }
 if(kind==='passthrough'){
  const nested={v:1},init={headers:nested,tls:{ca:['root'],unknown:nested}},out=await capture(init);
  const receipt={initSame:out===init,tlsSame:out.tls===init.tls,caSame:out.tls.ca===init.tls.ca,headersSame:out.headers===nested};out.headers.v=6;return{...receipt,originalNested:init.headers.v};
 }
 if(kind==='options'){
  const nested={v:1},items=[nested],callback=()=>{},fetchImpl=async()=>new Response('ok'),options={nested,items,callback,fetch:fetchImpl};
  const out=source.withExtraCaFetch(options),error=new source.ExtraCaError('graph',{cause:nested});
  const receipt={optionsSame:out===options,fetchSame:out.fetch===fetchImpl,nestedSame:out.nested===nested,itemsSame:out.items===items,callbackSame:out.callback===callback,causeSame:error.cause===nested};
  out.nested.v=3;out.items.push('new');return{...receipt,originalNested:options.nested.v,originalItems:options.items.length,causeNested:error.cause.v};
 }
 if(kind==='options-wrapped'){
  const nested={v:1},wrapped=source.wrapFetchForExtraCa(async()=>new Response('ok')),options={nested,fetch:wrapped},out=source.withExtraCaFetch(options);
  const receipt={optionsSame:out===options,fetchSame:out.fetch===wrapped,nestedSame:out.nested===nested};out.newField='visible';return{...receipt,originalHasNewField:options.newField==='visible'};
 }
 if(kind==='options-mutation'){
  const calls=[],base=async()=>{calls.push('base');return new Response('ok');},replacement=async(_input,init)=>{calls.push('replacement');return new Response('ok');},options={fetch:base};
  const out=source.withExtraCaFetch(options);out.fetch=replacement;
  const again=source.withExtraCaFetch(out);await again.fetch('https://fixture.test/models',{});
  const value={firstSame:out===options,secondSame:again===out,replacementWrapped:again.fetch!==replacement,sourceStillReplacement:out.fetch===replacement,calls};
  out.fetch=undefined;const unset=source.withExtraCaFetch(out);value.undefinedUsesGlobal=unset.fetch!==replacement&&unset.fetch!==base;
  out.fetch=null;const nulled=source.withExtraCaFetch(out);value.nullUsesGlobal=nulled.fetch!==replacement&&nulled.fetch!==base;
  return value;
 }
 if(kind==='options-nullish-mutation'){
  const calls=[],base=async()=>{calls.push('base');return new Response('ok');},globalFetch=async()=>{calls.push('global');return new Response('ok');},previousGlobal=globalThis.fetch;
  globalThis.fetch=globalFetch;
  try{
   const options={fetch:base},out=source.withExtraCaFetch(options);
   out.fetch=undefined;const unset=source.withExtraCaFetch(out);await unset.fetch('https://fixture.test/models',{});
   out.fetch=null;const nulled=source.withExtraCaFetch(out);await nulled.fetch('https://fixture.test/models',{});
   return{calls,originalFetchSame:options.fetch===base,sourceFetchNull:out.fetch===null};
  }finally{globalThis.fetch=previousGlobal;}
 }
 if(kind==='prototype-cycles'){
  const nested={v:1};nested.self=nested;const tlsProto={ca:['custom'],inherited:nested},existingTls=Object.create(tlsProto);existingTls.own=nested;Object.defineProperty(existingTls,'hidden',{value:nested,enumerable:false});
  const init=Object.create({tls:existingTls,inherited:'dontspread'});init.unknown=nested;Object.defineProperty(init,'hidden',{value:nested,enumerable:false});
  const out=await capture(init),receipt={initSame:out===init,tlsSame:out.tls===existingTls,caSame:out.tls.ca===tlsProto.ca,initInherited:Object.hasOwn(out,'inherited'),tlsInherited:Object.hasOwn(out.tls,'inherited'),initHidden:Object.hasOwn(out,'hidden'),tlsHidden:Object.hasOwn(out.tls,'hidden'),tlsOwn:Object.hasOwn(out,'tls'),caOwn:Object.hasOwn(out.tls,'ca'),unknownSame:out.unknown===nested,cycleRetained:out.unknown.self===nested,caFirst:out.tls.ca[0],caLength:out.tls.ca.length};
  out.tls.own.v=9;return{...receipt,originalNested:init.unknown.v};
 }
 throw Error('unknown graph probe');
}
