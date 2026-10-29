//! Fixed OMP596f2da xdevDocsAll/read-doc metadata consumer.
//! Copyright (c) 2025 Mario Zechner, 2025-2026 Can Bölük,
//! 2026 Stencil Labs, Inc. MIT; THIRD_PARTY_NOTICES.md.
//! Containers carry original source identities only; this metadata view grants
//! no executable capability and never registers derived objects in a catalog.
use super::xdev_native;
use ara_agent::ActualProviderError;
use ara_rpc::{WireString,WireValue,value_heap::{ValueError,ValueHandle,ValueKind,ValueRuntime}};
type Result<T>=std::result::Result<T,ActualProviderError>;

/// Original Map/Set presentation state. Hosts copy containers when publishing a
/// mutable docs-only view; the contained original source objects stay shared.
pub struct NativeXdevDocsMetadata {
    pub tools:ValueHandle,
    pub mounted_names:ValueHandle,
    pub active_names:ValueHandle,
    pub builtin_names:ValueHandle,
    pub glob_constructor:Option<ValueHandle>,
}
/// Same original compileInlineGlobs raw boundary used by Host settings and
/// docsAll: scalar/non-string entries are ignored; strings retain their order.
pub fn normalize_inline_patterns(raw:&WireValue)->Vec<WireString>{
    match raw{WireValue::Array(values)=>values.iter().filter_map(|value|match value{WireValue::String(value) if !value.is_empty()=>Some(value.clone()),_=>None}).collect(),_=>Vec::new()}
}
pub fn normalize_docs_mode(raw:&WireValue)->WireString{
    match raw{WireValue::String(mode)=>mode.clone(),_=>"".into()}
}
pub fn render_device_docs(source:&ValueHandle,runtime:&dyn ValueRuntime)->Result<WireString>{
    xdev_native::docs(source,"#",None,runtime).map_err(Into::into)
}
impl NativeXdevDocsMetadata {
    /// Original xdevDocsAll default arguments are inline and an empty allowlist.
    pub fn default_prompt_docs(&self,runtime:&dyn ValueRuntime)->Result<WireString>{
        self.prompt_docs(&WireValue::String("inline".into()),&WireValue::Array(Vec::new()),runtime)
    }
    /// Fixed listXdevTools: original mounted Set iteration then live Map.get.
    pub fn list_tools(&self,runtime:&dyn ValueRuntime)->Result<Vec<ValueHandle>>{
        let heap=self.tools.heap();let mut iterator=heap.get_iterator(&self.mounted_names,runtime)?;
        let mut names=Vec::new();
        while let Some(name)=heap.iterator_next(&mut iterator,runtime)?{names.push(name);}
        // Fixed spread exhausts the Set before flatMap starts any live Map.get.
        let mut tools=Vec::new();
        for name in names{
            let get=heap.get(&self.tools,&"get".into(),runtime)?;
            let selected=heap.call(&get,&self.tools,&[name],runtime)?;
            if heap.truthy(&selected)?{tools.push(selected);}
        }
        Ok(tools)
    }
    pub fn device_docs(&self,name:&WireString,runtime:&dyn ValueRuntime)->Result<WireString>{
        let selected=xdev_native::resolve_device_source(&self.tools,&self.mounted_names,&self.active_names,false,name,runtime)?;
        if self.tools.heap().kind(&selected)?==ValueKind::Undefined{return Err(ValueError::TypeError("No such metadata tool".into()).into())}
        render_device_docs(&selected,runtime)
    }
    fn is_builtin(&self,tool:&ValueHandle,runtime:&dyn ValueRuntime)->Result<bool>{
        let heap=self.tools.heap();
        // Evaluate the member callee before its argument, as in has(tool.name).
        let has=heap.get(&self.builtin_names,&"has".into(),runtime)?;
        let name=heap.get(tool,&"name".into(),runtime)?;
        Ok(heap.truthy(&heap.call(&has,&self.builtin_names,&[name],runtime)?)?)
    }
    /// Shared production docsAll algorithm: UTF-16 budgets, original source
    /// schemas, actual Native Bun.Glob consumers and source-backed summaries.
    pub fn prompt_docs(&self,raw_mode:&WireValue,raw_patterns:&WireValue,runtime:&dyn ValueRuntime)->Result<WireString>{
        let heap=self.tools.heap();let mode=normalize_docs_mode(raw_mode);
        let patterns=normalize_inline_patterns(raw_patterns);let mut globs=Vec::new();
        for pattern in patterns{
            let constructor=self.glob_constructor.as_ref().ok_or(ActualProviderError::Unsupported("Required Native Bun.Glob consumer is not installed"))?;
            globs.push(heap.construct(constructor,&[heap.string(pattern)?],runtime)?);
        }
        let mut sections=Vec::new();let mut overflow=Vec::new();let mut used=0;
        for tool in self.list_tools(runtime)?{
            // Fixed shouldInlineXdevTool short-circuits before any has/name Get.
            if mode.equals_ascii("catalog"){overflow.push(tool);continue}
            let mut inline=mode.equals_ascii("inline");
            if !inline{inline=self.is_builtin(&tool,runtime)?;}
            if !inline{
                for glob in &globs{
                    let matcher=heap.get(glob,&"match".into(),runtime)?;
                    let name=heap.get(&tool,&"name".into(),runtime)?;
                    if heap.truthy(&heap.call(&matcher,glob,&[name],runtime)?)?{inline=true;break}
                }
            }
            if inline{
                // descriptionCap performs its own original has/name lookup.
                let builtin=self.is_builtin(&tool,runtime)?;
                let docs=xdev_native::docs(&tool,"##",(!builtin).then_some(xdev_native::EXTERNAL_DESCRIPTION_CAP),runtime)?;
                if docs.len()<=xdev_native::DOCS_PER_DEVICE_CAP&&used+docs.len()<=xdev_native::DOCS_TOTAL_BUDGET{used+=docs.len();sections.push(docs);continue}
            }
            overflow.push(tool);
        }
        if !overflow.is_empty(){
            let mut rows=vec!["## Additional devices (docs on demand)".into()];
            for tool in overflow{
                let (name,summary,_)=xdev_native::device_summary_entry(&tool,&self.builtin_names,runtime)?;
                rows.push(xdev_native::concat(&["- xd://".into(),name," — ".into(),summary]));
            }
            rows.push("".into());rows.push("Read xd://<tool> for full docs + JSON schema before first use.".into());
            sections.push(xdev_native::join(&rows,"\n"));
        }
        Ok(xdev_native::join(&sections,"\n\n"))
    }
}
