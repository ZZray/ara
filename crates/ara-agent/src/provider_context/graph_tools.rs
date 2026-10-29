//! Fixed OMP596f2da agent-loop.ts:825-920, dialect/examples.ts and rendering.ts.
//! MIT; Copyright OMP contributors; see THIRD_PARTY_NOTICES.md.
//! All callback captures are same-heap values. No resolver/heap lease is stored
//! in a callback, and original tools/schemas are never exported to JSON.
use crate::{BuildOptions, SessionValues};
use ara_rpc::{WireString,value_heap::{CompiledFunctionForm,CompiledFunctionSpec,PropertyKey,
    ValueError,ValueHandle,ValueHeap,ValueKind,ValueRuntime,ValueSnapshot}};
use std::sync::Arc;

struct Ops<'a>{h:&'a Arc<ValueHeap>,r:&'a dyn ValueRuntime}
impl Ops<'_>{
    fn s(&self,text:&str)->Result<ValueHandle,ValueError>{self.h.string(text.into())}
    fn get(&self,v:&ValueHandle,k:&str)->Result<ValueHandle,ValueError>{self.h.get(v,&k.into(),self.r)}
    fn own(&self,v:&ValueHandle,k:&str,x:ValueHandle)->Result<(),ValueError>{self.h.define_own_data(v,k.into(),x,self.r)}
    fn call(&self,v:&ValueHandle,k:&str,a:&[ValueHandle])->Result<ValueHandle,ValueError>{
        let f=self.get(v,k)?;self.h.call(&f,v,a,self.r)
    }
    fn is_text(&self,v:&ValueHandle,s:&str)->Result<bool,ValueError>{
        Ok(matches!(self.h.snapshot(v)?,ValueSnapshot::String(t) if t.units()==s.encode_utf16().collect::<Vec<_>>().as_slice()))
    }
    fn nullish(&self,v:&ValueHandle)->Result<bool,ValueError>{Ok(matches!(self.h.kind(v)?,ValueKind::Undefined|ValueKind::Null))}
    fn cat(&self,values:&[ValueHandle])->Result<ValueHandle,ValueError>{
        let mut units=Vec::new();for value in values{units.extend_from_slice(self.h.to_string(value,self.r)?.units());}
        self.h.string(WireString::from_units(units))
    }
    fn literal(&self,fields:&[(&str,ValueHandle)])->Result<ValueHandle,ValueError>{
        let o=self.h.object()?;for(k,v)in fields{self.own(&o,k,v.clone())?;}Ok(o)
    }
    fn iterable_array(&self,v:&ValueHandle,tail:ValueHandle)->Result<ValueHandle,ValueError>{
        let mut iterator=self.h.get_iterator(v,self.r)?;let mut items=Vec::new();
        while let Some(item)=self.h.iterator_next(&mut iterator,self.r)?{items.push(item);}
        items.push(tail);self.h.array(items)
    }
    fn record(&self,v:&ValueHandle)->Result<bool,ValueError>{Ok(self.h.kind(v)?==ValueKind::Object)}
    fn inject(&self,schema:&ValueHandle,require:&ValueHandle,describe:&ValueHandle)->Result<ValueHandle,ValueError>{
        if !self.h.truthy(schema)?||!self.record(schema)?{return Ok(schema.clone())}
        let props=self.get(schema,"properties")?;let has=self.record(&props)?;
        if !has{
            for key in ["anyOf","oneOf"]{
                let variants=self.get(schema,key)?;
                if self.h.kind(&variants)?!=ValueKind::Array{continue}
                let out=self.h.spread(schema,self.r)?;
                let map=self.get(&variants,"map")?;
                let callback=self.h.fresh_compiled_function(&INJECT_BRANCH,&[require.clone(),describe.clone()])?;
                let mapped=self.h.call(&map,&variants,&[callback],self.r)?;self.own(&out,key,mapped)?;return Ok(out)
            }
        }
        let props=if has{props}else{self.h.object()?};
        let required_value=self.get(schema,"required")?;
        let required=if self.h.kind(&required_value)?==ValueKind::Array{
            let filter=self.get(&required_value,"filter")?;
            let f=self.h.fresh_compiled_function(&STRING_FILTER,&[])?;self.h.call(&filter,&required_value,&[f],self.r)?
        }else{self.h.array(Vec::new())?};
        if self.h.has(&props,&"i".into(),self.r)?{
            let intent=self.get(&props,"i")?;
            let rest=self.h.spread_excluding(&props,&["i".into()],self.r)?;
            let keys=self.h.own_enumerable_string_keys(&props,self.r)?;
            let reorder=keys.first().is_none_or(|key|key.units()!=[105]);
            let needs_required=self.h.truthy(require)?&&!self.h.truthy(&self.call(&required,"includes",&[self.s("i")?])?)?;
            if !reorder&&!needs_required{return Ok(schema.clone())}
            let out=self.h.spread(schema,self.r)?;
            if reorder{let p=self.literal(&[("i",intent)])?;self.h.spread_into(&p,&rest,self.r)?;self.own(&out,"properties",p)?;}
            if needs_required{self.own(&out,"required",self.iterable_array(&required,self.s("i")?)?)?;}
            return Ok(out)
        }
        let out=self.h.spread(schema,self.r)?;
        let hint=if self.h.truthy(describe)?{self.literal(&[("type",self.s("string")?),("description",self.s("concise intent")?)])?}
            else{self.literal(&[("type",self.s("string")?)])?};
        let p=self.literal(&[("i",hint)])?;self.h.spread_into(&p,&props,self.r)?;self.own(&out,"properties",p)?;
        if self.h.truthy(require)?{self.own(&out,"required",self.iterable_array(&required,self.s("i")?)?)?;}
        Ok(out)
    }
    fn py_string(&self,v:&ValueHandle)->Result<ValueHandle,ValueError>{
        let mut x=v.clone();for(a,b)in[("\\","\\\\"),("\"","\\\""),("\n","\\n"),("\r","\\r"),("\t","\\t")]{
            x=self.call(&x,"replaceAll",&[self.s(a)?,self.s(b)?])?;
        }self.cat(&[self.s("\"")?,x,self.s("\"")?])
    }
    fn py_value(&self,v:&ValueHandle,top:bool)->Result<ValueHandle,ValueError>{
        if top&&self.h.kind(v)?==ValueKind::String&&self.h.truthy(&self.call(v,"includes",&[self.s("\n")?])?)?{
            let safe=!self.h.truthy(&self.call(v,"includes",&[self.s("\"\"\"")?])?)?
                &&!self.h.truthy(&self.call(v,"startsWith",&[self.s("\"")?])?)?
                &&!self.h.truthy(&self.call(v,"endsWith",&[self.s("\"")?])?)?
                &&!self.h.truthy(&self.call(v,"endsWith",&[self.s("\\")?])?)?;
            if safe{return self.cat(&[self.s("\"\"\"")?,v.clone(),self.s("\"\"\"")?])}
        }
        match self.h.snapshot(v)?{
            ValueSnapshot::Undefined|ValueSnapshot::Null=>self.s("None"),
            ValueSnapshot::Boolean(b)=>self.s(if b{"True"}else{"False"}),
            ValueSnapshot::Number(n)=>{let s=self.h.string(self.h.to_string(v,self.r)?)?;if n.is_finite(){Ok(s)}else{self.py_string(&s)}},
            ValueSnapshot::String(_)=>self.py_string(v),
            _ if self.h.kind(v)?==ValueKind::Array=>{
                let map=self.get(v,"map")?;let callback=self.h.fresh_compiled_function(&PY_VALUE,&[])?;
                let values=self.h.call(&map,v,&[callback],self.r)?;
                self.cat(&[self.s("[")?,self.call(&values,"join",&[self.s(", ")?])?,self.s("]")?])
            },
            _ if self.record(v)?=>{
                let mut pieces=Vec::new();for key in self.h.for_in_string_keys(v,self.r)?{
                    let escaped_key=self.py_string(&self.h.string(key.clone())?)?;
                    let value=self.h.get(v,&PropertyKey::String(key.clone()),self.r)?;
                    pieces.push(self.cat(&[escaped_key,self.s(": ")?,self.py_value(&value,false)?])?);
                }
                let array=self.h.array(pieces)?;self.cat(&[self.s("{")?,self.call(&array,"join",&[self.s(", ")?])?,self.s("}")?])
            },
            _=>self.py_string(&self.h.string(self.h.to_string(v,self.r)?)?),
        }
    }
    fn render_call(&self,tool:&ValueHandle,args:&ValueHandle,intent:&ValueHandle)->Result<ValueHandle,ValueError>{
        let mut sole=None;let mut count=0;
        for key in self.h.for_in_string_keys(args,self.r)?{count+=1;sole=Some(self.h.get(args,&key.into(),self.r)?);}
        let bare=count==1&&self.h.kind(sole.as_ref().expect("one enumerated argument"))?==ValueKind::String;
        if bare{
            let attr=if self.h.truthy(intent)?{self.s(" i=\"…\"")?}else{self.s("")?};
            return self.cat(&[self.s("<example")?,attr,self.s(">\n")?,sole.unwrap(),self.s("\n</example>")?])
        }
        let final_args=if self.h.truthy(intent)?{
            let value=self.literal(&[("i",self.s("…")?)])?;self.h.spread_into(&value,args,self.r)?;value
        }else{args.clone()};
        let name=self.get(tool,"name")?;
        let mut kwargs=Vec::new();for key in self.h.for_in_string_keys(&final_args,self.r)?{
            let value=self.h.get(&final_args,&PropertyKey::String(key.clone()),self.r)?;
            kwargs.push(self.cat(&[self.h.string(key)?,self.s("=")?,self.py_value(&value,true)?])?);
        }
        let array=self.h.array(kwargs)?;
        self.cat(&[self.s("<example>\n")?,name,self.s("(")?,self.call(&array,"join",&[self.s(", ")?])?,self.s(")\n</example>")?])
    }
    fn render_examples(&self,tool:&ValueHandle,intent:&ValueHandle)->Result<ValueHandle,ValueError>{
        let examples=self.get(tool,"examples")?;
        if self.nullish(&examples)?||!self.h.truthy(&self.get(&examples,"length")?)?{return self.s("")}
        let map=self.get(&examples,"map")?;let callback=self.h.fresh_compiled_function(&EXAMPLE,&[tool.clone(),intent.clone()])?;
        let parts=self.h.call(&map,&examples,&[callback],self.r)?;
        self.cat(&[self.s("<examples>\n")?,self.call(&parts,"join",&[self.s("\n")?])?,self.s("\n</examples>")?])
    }
}

static STRING_FILTER:CompiledFunctionSpec=CompiledFunctionSpec{source:"OMP596f2da/agent-loop860:string-required",name:"",length:1,form:CompiledFunctionForm::Arrow,capture_count:0,
    body:|h,_,_,args,_|h.boolean(h.kind(&args.first().cloned().unwrap_or(h.undefined()?))?==ValueKind::String)};
static INJECT_BRANCH:CompiledFunctionSpec=CompiledFunctionSpec{source:"OMP596f2da/agent-loop852:inject-branch",name:"",length:1,form:CompiledFunctionForm::Arrow,capture_count:2,
    body:|h,c,_,a,r|{let o=Ops{h,r};o.inject(a.first().unwrap_or(&h.undefined()?),&c[0],&c[1])}};
static PY_VALUE:CompiledFunctionSpec=CompiledFunctionSpec{source:"OMP596f2da/rendering60:pyValue",name:"pyValue",length:1,form:CompiledFunctionForm::Ordinary,capture_count:0,
    body:|h,_,_,a,r|Ops{h,r}.py_value(a.first().unwrap_or(&h.undefined()?),false)};
static EXAMPLE:CompiledFunctionSpec=CompiledFunctionSpec{source:"OMP596f2da/examples34:example",name:"",length:1,form:CompiledFunctionForm::Arrow,capture_count:2,
    body:|h,c,_,a,r|{
        let o=Ops{h,r};let ex=a.first().cloned().unwrap_or(h.undefined()?);
        let head=if h.truthy(&o.get(&ex,"caption")?)?{o.cat(&[o.s("# ")?,o.get(&ex,"caption")?,o.s("\n")?])?}else{o.s("")?};
        if h.has(&ex,&"call".into(),r)?{return o.cat(&[head,o.render_call(&c[0],&o.get(&ex,"call")?,&c[1])?])}
        if h.has(&ex,&"good".into(),r)?{return o.cat(&[head,o.s("WRONG:\n")?,o.render_call(&c[0],&o.get(&ex,"bad")?,&c[1])?,o.s("\nRIGHT:\n")?,o.render_call(&c[0],&o.get(&ex,"good")?,&c[1])?])}
        let head=o.call(&head,"trimEnd",&[])?;
        if h.truthy(&o.get(&ex,"note")?)?{o.cat(&[head,o.s("\n")?,o.get(&ex,"note")?])}else{Ok(head)}
    }};
static NORMALIZE:CompiledFunctionSpec=CompiledFunctionSpec{source:"OMP596f2da/agent-loop894:normalize-tool",name:"",length:1,form:CompiledFunctionForm::Arrow,capture_count:1,
    body:|h,c,_,a,r|{
        let o=Ops{h,r};let tool=a.first().cloned().unwrap_or(h.undefined()?);
        let intent=o.get(&tool,"intent")?;
        let omit=h.kind(&intent)?==ValueKind::Function||o.is_text(&intent,"omit")?;
        let optional=o.is_text(&intent,"optional")?;
        let inject=h.truthy(&o.get(&c[0],"inject")?)?&&!omit;
        let inject=if inject {
            let authored=ara_ai::graph_tool_wire_schema_in(h,r,&tool)?;
            !ara_ai::schema_validator::graph_schema_defines_property_in(h,r,&authored,"i")?
        } else { false };
        let prune=h.truthy(&o.get(&c[0],"prune")?)?;
        let mut parameters=ara_ai::graph_tool_wire_schema_in(h,r,&tool)?;
        if prune{parameters=ara_ai::strip_graph_schema_descriptions_in(h,r,&parameters)?;}
        if inject{
            let name=match (optional,prune) {
                (false,true)=>"intent:require", (false,false)=>"intent:require:described",
                (true,true)=>"intent:optional", (true,false)=>"intent:optional:described",
            };
            let key=h.module_symbol("ara-agent.intent-schema",name)?;
            parameters=ara_ai::tool_schema::graph_schema_stamp_in(h,r,&parameters,&key,|schema|
                o.inject(schema,&h.boolean(!optional)?,&h.boolean(!prune)?))?;
        }
        let description=if prune{o.s("")?}else{
            let d=o.get(&tool,"description")?;let d=if o.nullish(&d)?{o.s("")?}else{d};
            let examples_tool=h.spread(&tool,r)?;o.own(&examples_tool,"parameters",parameters.clone())?;
            let block=o.render_examples(&examples_tool,&h.boolean(inject)?)?;
            if h.truthy(&block)?{o.cat(&[d,o.s("\n\n")?,block])?}else{d}
        };
        let out=h.spread(&tool,r)?;o.own(&out,"parameters",parameters)?;o.own(&out,"description",description)?;Ok(out)
    }};

pub(crate) fn normalize_original_tools(values:&SessionValues,tools:&ValueHandle,options:&BuildOptions)->Result<ValueHandle,ValueError>{
    let o=Ops{h:values.heap(),r:values.runtime()};
    if o.nullish(tools)?{return o.h.undefined()}
    let opts=o.literal(&[("inject",o.h.boolean(options.intent_tracing&&!options.suppress_intent_schema)?),
        ("prune",o.h.boolean(options.prune_tool_descriptions==Some(true))?)])?;
    // Member Get precedes function literal creation, preserving altered map.
    let map=o.get(tools,"map")?;let callback=o.h.fresh_compiled_function(&NORMALIZE,&[opts])?;
    o.h.call(&map,tools,&[callback],o.r)
}

static FINGERPRINT_TOOL:CompiledFunctionSpec=CompiledFunctionSpec{source:"OMP596f2da/append-only-context374:fingerprint-tool",name:"",length:1,form:CompiledFunctionForm::Arrow,capture_count:0,
    body:|h,_,_,a,r|{
        let o=Ops{h,r};let tool=a.first().cloned().unwrap_or(h.undefined()?);let result=h.object()?;
        for(target,source)in[("n","name"),("d","description"),("p","parameters"),("s","strict"),("cf","customFormat"),("cw","customWireName")]{
            o.own(&result,target,o.get(&tool,source)?)?;
        }Ok(result)
    }};
pub(crate) fn fingerprint_original_tools(values:&SessionValues,prompt:&ValueHandle,tools:&ValueHandle,options:&BuildOptions)->Result<String,ValueError>{
    let o=Ops{h:values.heap(),r:values.runtime()};
    let payload=o.h.object()?;o.own(&payload,"s",prompt.clone())?;
    let map=o.get(tools,"map")?;let callback=o.h.fresh_compiled_function(&FINGERPRINT_TOOL,&[])?;
    o.own(&payload,"t",o.h.call(&map,tools,&[callback],o.r)?)?;
    o.own(&payload,"i",o.h.boolean(options.intent_tracing)?)?;
    o.own(&payload,"pd",match options.prune_tool_descriptions{Some(b)=>o.h.boolean(b)?,None=>o.h.undefined()?})?;
    let encoded=o.h.stringify(&payload,ara_rpc::value_heap::StringifyMode::Plain,None,o.r)?
        .ok_or_else(||ValueError::TypeError("Cannot read properties of undefined (reading 'length')".into()))?;
    let hash=encoded.units().iter().fold(0u32,|hash,unit|hash.wrapping_mul(31).wrapping_add(u32::from(*unit)));
    let mut value=hash;let mut digits=Vec::new();
    loop{let d=(value%36)as u8;digits.push(if d<10{b'0'+d}else{b'a'+d-10});value/=36;if value==0{break}}
    digits.reverse();Ok(String::from_utf8(digits).expect("base36 ASCII"))
}


#[derive(Default)]
struct PrefixObjectIds {
    heaps: std::collections::HashMap<u64, PrefixHeapIds>,
    next: f64,
}
struct PrefixHeapIds {
    owner: std::sync::Weak<ValueHeap>,
    ids: std::collections::HashMap<ara_rpc::value_heap::DeferredValue, f64>,
}
fn prefix_object_id(heap:&Arc<ValueHeap>,value:&ValueHandle)->Result<ValueHandle,ValueError>{
    if !matches!(heap.kind(value)?,ValueKind::Object|ValueKind::Array|ValueKind::Function){return heap.number(0.0)}
    static IDS:std::sync::OnceLock<std::sync::Mutex<PrefixObjectIds>>=std::sync::OnceLock::new();
    let edge=heap.capture_deferred(value)?;
    let id={
        let mut state=IDS.get_or_init(||std::sync::Mutex::new(PrefixObjectIds{next:1.0,..Default::default()}))
            .lock().unwrap_or_else(|poison|poison.into_inner());
        state.heaps.retain(|_,entries|entries.owner.strong_count()>0);
        if let Some(id)=state.heaps.get(&heap.heap_token()).and_then(|entries|entries.ids.get(&edge)){*id}
        else {
            let id=state.next;state.next+=1.0;
            state.heaps.entry(heap.heap_token()).or_insert_with(||PrefixHeapIds{
                owner:Arc::downgrade(heap),ids:std::collections::HashMap::new()}).ids.insert(edge,id);id
        }
    };
    heap.number(id)
}
static PREFIX_TOOL_KEY:CompiledFunctionSpec=CompiledFunctionSpec{
    source:"OMPb91db66e/append-only-context:toolKeyForPrefix",name:"",length:1,
    form:CompiledFunctionForm::Arrow,capture_count:0,
    body:|h,_,_,a,r|{
        let o=Ops{h,r};let tool=a.first().cloned().unwrap_or(h.undefined()?);
        let parameters=ara_ai::graph_tool_wire_schema_in(h,r,&tool)?;
        let custom_format=o.get(&tool,"customFormat")?;
        let examples=o.get(&tool,"examples")?;
        let fallback=|value:ValueHandle|->Result<ValueHandle,ValueError>{
            if o.nullish(&value)?{o.s("")}else{Ok(value)}
        };
        let name=fallback(o.get(&tool,"name")?)?;
        let description=fallback(o.get(&tool,"description")?)?;
        let strict=fallback(o.get(&tool,"strict")?)?;
        let strict=h.string(h.to_string(&strict,r)?)?;
        let wire_name=fallback(o.get(&tool,"customWireName")?)?;
        let intent=o.get(&tool,"intent")?;
        let intent=if h.kind(&intent)?==ValueKind::Function {
            o.cat(&[o.s("fn:")?,prefix_object_id(h,&o.get(&tool,"intent")?)?])?
        }else{
            let live=o.get(&tool,"intent")?;
            let live=if o.nullish(&live)?{o.s("require")?}else{live};
            let primitive=h.to_primitive(&live,true,r)?;
            if h.kind(&primitive)?==ValueKind::Symbol {
                return Err(ValueError::TypeError("Cannot implicitly convert Symbol to string".into()));
            }
            o.cat(&[o.s("mode:")?,primitive])?
        };
        let parts=h.array(vec![name,description,strict,wire_name,intent,
            prefix_object_id(h,&parameters)?,prefix_object_id(h,&custom_format)?,prefix_object_id(h,&examples)?])?;
        o.call(&parts,"join",&[o.s("\0")?])
    }
};
/// Preserve the caller collection's original map, holes, callback order and throws.
pub(crate) fn prefix_tool_keys(values:&SessionValues,tools:&ValueHandle)->Result<ValueHandle,ValueError>{
    let o=Ops{h:values.heap(),r:values.runtime()};
    let tools=if o.nullish(tools)?{o.h.array(Vec::new())?}else{tools.clone()};
    let map=o.get(&tools,"map")?;
    let callback=o.h.fresh_compiled_function(&PREFIX_TOOL_KEY,&[])?;
    o.h.call(&map,&tools,&[callback],o.r)
}
