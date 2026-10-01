import { createHash } from "node:crypto";
import { pathToFileURL } from "node:url";
const root = process.argv[2];
const pb = await import(pathToFileURL(`${root}/packages/catalog/src/discovery/protobuf.ts`).href);
const modules = { cursor: await import(pathToFileURL(`${root}/packages/catalog/src/discovery/cursor-proto.ts`).href), devin: await import(pathToFileURL(`${root}/packages/catalog/src/discovery/devin-proto.ts`).href) };
const ir = JSON.parse(await Bun.file(process.argv[3]).text());
function encoded(value) {
  if (value === undefined) return { kind: "undefined" };
  if (value === null) return { kind: "null" };
  if (typeof value === "boolean") return { kind: "bool", value };
  if (typeof value === "number") { const b = new ArrayBuffer(8);new DataView(b).setFloat64(0, value, false);return { kind: "number", bits: Buffer.from(b).toString("hex") }; }
  if (typeof value === "bigint") return { kind: "bigint", decimal: value.toString() };
  if (typeof value === "string") return { kind: "string", units: Array.from({length:value.length},(_,i)=>value.charCodeAt(i)) };
  if (value instanceof Uint8Array) return { kind: "bytes", data: Array.from(value) };
  if (Array.isArray(value)) return { kind: "array", items: value.map(encoded) };
  if (typeof value === "function") return { kind: "function" };
  const prototype = Object.getPrototypeOf(value);
  return { kind: "object", ordinary: prototype !== null, entries: Object.keys(value).map(k=>[encoded(k),encoded(value[k])]), ...(prototype && prototype !== Object.prototype ? { prototype: encoded(prototype) } : {}) };
}
const cases=[];
function add(id,module,schema,operation,input,run) { let expected;try { expected={status:"ok",value:encoded(run())}; } catch(e) {expected={status:"error",name:e.name,message:e.message};}cases.push({id,module,schema,operation,input,expected}); }
function scalar(kind) { switch(kind) {case "bool": return true;case "bytes":return new Uint8Array([0,255,128]);case "double":case "float":return 0.1;case "enum":return 987654321;case "int32":return -123;case "uint32":return 4294967295;case "int64":return -(1n<<60n)+13n;case "uint64":return (1n<<63n)+17n;case "string":return "测试\ud800";}throw Error(kind); }
for(const module of ir.modules) {
  const exports=modules[module.module];
  for(const schema of module.schemas) {
    const codec=exports[schema.export];
    const run=input=>{const created=codec.create(input);const binary=codec.encode(created);return {created,binary,json:codec.toJson(created),decoded:codec.decode(binary),reencoded:codec.encode(codec.decode(binary))};};
    add(`${module.module}/${schema.export}/defaults`,module.module,schema.typeName,"roundtrip",encoded({}),()=>run({}));
    for(const field of schema.fields) {
      function value(f){return f.kind==="message"?exports[module.schemas.find(s=>s.typeName===f.T).export].create():scalar(f.kind);}
      let variants=field.kind==="oneof"?field.variants:[field];
      for(const variant of variants) {
        let val;
        if(field.kind==="oneof")val={case:variant.name,value:value(variant)};
        else if(field.kind==="map"){const item=field.messageValue?exports[module.schemas.find(s=>s.typeName===field.V).export].create():scalar(field.V);val=Object.create(null);val[""]=item;val.__proto__=item;val["2"]=item;}
        else val=field.repeat?[value(field),value(field)]:value(field);
        const input={[field.name]:val};add(`${module.module}/${schema.export}/${field.name}/${variant.name}`,module.module,schema.typeName,"roundtrip",encoded(input),()=>run(input));
      }
    }
  }
}
const scalarKinds=["bool","bytes","double","enum","float","int32","int64","string","uint32","uint64"];
for(const kind of scalarKinds) {
  const codec=pb.pb(`probe.${kind}`,[{no:1,name:"v",kind}]);
  const values=[undefined,null,false,true,0,-0,0.1,NaN,Infinity,-Infinity,4294967297,-1,1n<<64n,-1n,"","  +123  ","0xFF","-0xFF","\ud800",new Uint8Array([0,255]),{},[]];
  for(let i=0;i<values.length;i++){const input={v:values[i]};add(`scalar/${kind}/${i}`,null,kind,"scalar",encoded(input),()=>{const created=codec.create(input);return {created,binary:codec.encode(created),json:codec.toJson(created)};});}
}
const rawCases=[[0],[8],[8,128],[8,255,255,255,255,127],[8,255,255,255,255,255,255,255,255,255,127],[11],[12],[15],[10,1,128,1],[10,1,255],[10,4,239,187,191,97],[10,0],[0,0],[16,129,0],[8,7,8,9]];
for(const kind of scalarKinds)for(let i=0;i<rawCases.length;i++){const codec=pb.pb(`probe.${kind}`,[{no:1,name:"v",kind}]);const bytes=new Uint8Array(rawCases[i]);add(`decode/${kind}/${i}`,null,kind,"decode",encoded(bytes),()=>{const decoded=codec.decode(bytes);return {decoded,binary:codec.encode(decoded),json:codec.toJson(decoded)};});}
for(const kind of ["bool","int32","uint32","uint64","float","double"])for(let i=0;i<rawCases.length;i++){const codec=pb.pb(`probe.${kind}`,[{no:1,name:"v",kind,repeat:true}]);const bytes=new Uint8Array(rawCases[i]);add(`packed/${kind}/${i}`,null,kind,"packed",encoded(bytes),()=>{const decoded=codec.decode(bytes);return {decoded,binary:codec.encode(decoded),json:codec.toJson(decoded)};});}
const jsonValues=[null,true,false,0,-0,0.1,NaN,Infinity,"\ud800",{"": ["kept",{nested:true}],count:1},JSON.parse('{"__proto__":{"polluted":true},"constructor":1,"2":false}'),[null,[],{}]];
for(let i=0;i<jsonValues.length;i++){const v=jsonValues[i];add(`json-value/${i}`,null,null,"jsonValue",encoded(v),()=>{const binary=pb.encodeJsonValue(v);return {binary,decoded:pb.decodeJsonValue(binary)};});}
// API identity/state observations cannot be represented by a JSON message projection.
const I=pb.pb("probe.identity",[{no:1,name:"v",kind:"int32"}]);const raw=new Uint8Array([16,129,0]);const message=I.decode(raw);raw[1]=130;add("identity/unknown-view",null,null,"unknownView",null,()=>({binary:I.encode(message),data:message.$unknown[0].data}));
const input={opaque:undefined,keep:1};const proto={inherited:2};Object.setPrototypeOf(input,proto);add("create/inherited",null,null,"inherited",null,()=>I.create(input));
const fields=[];const lazy=pb.pb("probe.lazy",fields);fields.push({no:1,name:"v",kind:"int32"});lazy.create();fields.push({no:2,name:"later",kind:"string"});add("pb/lazy-freeze",null,null,"lazyFreeze",null,()=>({created:lazy.create(),binary:lazy.encode({v:7,later:"ignored"})}));
// Additive observations of the public descriptor/callable/identity API.
add("api/default-identities",null,null,"apiDefaultIdentities",null,()=>{
 const codec=pb.pb("probe.defaults",[{no:1,name:"a",kind:"bytes"},{no:2,name:"b",kind:"bytes"},{no:3,name:"map",kind:"map",K:"string",V:"bytes"},{no:4,name:"r",kind:"int32",repeat:true},{no:5,name:"optional",kind:"int32",optional:true},{name:"choice",kind:"oneof",variants:[{no:6,name:"v",kind:"int32"}]}]);
 const first=codec.create(),second=codec.create();const d1=codec.decode(new Uint8Array([26,0])),d2=codec.decode(new Uint8Array([26,0]));const authored={r:[1],map:{x:new Uint8Array([2])}},created=codec.create(authored);
 return{sameDefault:first.a===second.a,distinctField:first.a!==first.b,mapMissingShared:d1.map[""]===d2.map[""],mapDifferentField:d1.map[""]!==first.a,freshMap:first.map!==second.map,freshRepeated:first.r!==second.r,ownOptional:Object.hasOwn(first,"optional"),oneofOwnCase:Object.hasOwn(first.choice,"case"),shallowArray:created.r===authored.r,shallowMap:created.map===authored.map};
});
add("api/map-missing-message",null,null,"apiMapMissingMessage",null,()=>{const child=pb.pb("probe.child",[{no:1,name:"v",kind:"int32"}]),codec=pb.pb("probe.map",[{no:1,name:"map",kind:"map",K:"string",V:()=>child}]);const decoded=codec.decode(new Uint8Array([10,0]));return{decoded,binary:codec.encode(decoded)};});
add("api/map-missing-message-json",null,null,"apiMapMissingMessageJson",null,()=>{const child=pb.pb("probe.child",[{no:1,name:"v",kind:"int32"}]),codec=pb.pb("probe.map",[{no:1,name:"map",kind:"map",K:"string",V:()=>child}]);return codec.toJson(codec.decode(new Uint8Array([10,0])));});
add("api/recursive-reference",null,null,"apiRecursive",null,()=>{let codec;codec=pb.pb("probe.recursive",[{no:1,name:"v",kind:"int32"},{no:2,name:"next",kind:"message",T:()=>codec}]);const created=codec.create({v:1,next:codec.create({v:2,next:codec.create({v:3})})}),binary=codec.encode(created);return{created,binary,decoded:codec.decode(binary),json:codec.toJson(created)};});
add("api/custom-reference-native-value",null,null,"apiCustomReference",null,()=>{let factories=0;const codec=pb.pb("probe.custom",[{no:1,name:"m",kind:"message",T:()=>{factories++;return{encode:value=>new Uint8Array([8,value]),decode:bytes=>({read:bytes[1]}),toJson:value=>({authored:value})};}}]);const created=codec.create({m:7});const before=factories,binary=codec.encode(created),decoded=codec.decode(binary),json=codec.toJson(created);return{before,factories,binary,decoded,json};});
add("api/duplicate-numbers",null,null,"apiDuplicateNumbers",null,()=>{const codec=pb.pb("probe.duplicates",[{no:1,name:"a",kind:"int32"},{no:1,name:"b",kind:"int32"}]),created=codec.create({a:1,b:2}),binary=codec.encode(created);return{created,binary,decoded:codec.decode(binary),json:codec.toJson(created)};});
add("api/oneof-duplicate-name",null,null,"apiOneofDuplicate",null,()=>{const codec=pb.pb("probe.oneof",[{kind:"oneof",name:"pick",variants:[{no:1,name:"shared",kind:"int32"},{no:2,name:"shared",kind:"string"}]}]);const decoded=codec.decode(new Uint8Array([8,7]));return{decoded,binary:codec.encode(decoded)};});
add("api/proto-reserved-string",null,null,"apiProtoString",null,()=>pb.pb("probe.prototype",[{no:1,name:"__proto__",kind:"string"}]).encode({}));
add("api/proto-map-setter",null,null,"apiProtoMap",null,()=>{const codec=pb.pb("probe.prototypeMap",[{no:1,name:"__proto__",kind:"map",K:"string",V:"string"}]);const created=codec.create();return{created,binary:codec.encode(created),json:codec.toJson(created)};});
add("api/map-inherited",null,null,"apiMapInherited",null,()=>{const map=Object.assign(Object.create({inherited:"yes"}),{own:"value"}),codec=pb.pb("probe.inheritedMap",[{no:1,name:"map",kind:"map",K:"string",V:"string"}]);const created=codec.create({map}),binary=codec.encode(created);return{created,binary,decoded:codec.decode(binary),json:codec.toJson(created)};});
add("api/json-inherited",null,null,"apiJsonInherited",null,()=>{const input=Object.assign(Object.create({inherited:2}),{own:1}),binary=pb.encodeJsonValue(input);return{binary,decoded:pb.decodeJsonValue(binary)};});
add("api/singular-replacement",null,null,"apiSingularReplacement",null,()=>{const child=pb.pb("probe.child",[{no:1,name:"v",kind:"int32"}]),codec=pb.pb("probe.replace",[{no:1,name:"child",kind:"message",T:()=>child}]);const decoded=codec.decode(new Uint8Array([10,2,8,1,10,2,16,2]));return{decoded,binary:codec.encode(decoded)};});
add("api/map-length-crossing",null,null,"apiMapCrossing",null,()=>{const codec=pb.pb("probe.cross",[{no:1,name:"map",kind:"map",K:"string",V:"int32"}]),decoded=codec.decode(new Uint8Array([10,2,16,128,1]));return{decoded,binary:codec.encode(decoded),json:codec.toJson(decoded)};});
add("api/callable-and-functions",null,null,"apiFacade",null,()=>{const codec=pb.pb("probe.facade",[{no:1,name:"v",kind:"int32"}]),created=pb.create(codec,{v:7}),binary=pb.toBinary(codec,created);return{binary,json:pb.toJson(codec,created),decoded:pb.fromBinary(codec,binary),callBinary:codec(created),callDecoded:codec(binary)};});
add("api/json-invalid-utf8",null,null,"apiJsonInvalidUtf8",null,()=>pb.decodeJsonValue(new Uint8Array([26,1,128])));
add("api/reference-cache-scope",null,null,"apiReferenceCache",null,()=>{let factories=0;const factory=()=>{const value=++factories;return{encode:()=>new Uint8Array([value]),decode:()=>({value}),toJson:()=>({value})};};const fields=[{no:1,name:'a',kind:'message',T:factory},{no:2,name:'b',kind:'message',T:factory}],one=pb.pb('one',fields),two=pb.pb('two',fields);return{first:one.encode({a:1,b:2}),second:two.encode({a:1,b:2}),factories};});
const sourceHashes={};for(const file of ["packages/catalog/src/discovery/protobuf.ts","packages/catalog/src/discovery/cursor-proto.ts","packages/catalog/src/discovery/devin-proto.ts","packages/utils/src/type-guards.ts"]){sourceHashes[file]=createHash("sha256").update(new Uint8Array(await Bun.file(`${root}/${file}`).arrayBuffer())).digest("hex");}
console.log(JSON.stringify({upstreamCommit:"596f2da7101178214aa27a753529d15e6b7ad91d",bunVersion:Bun.version,schemaCount:635,enumCount:46,sourceHashes,irSha256:createHash("sha256").update(new Uint8Array(await Bun.file(process.argv[3]).arrayBuffer())).digest("hex"),cases}));
