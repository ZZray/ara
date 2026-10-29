//! Complete fixed OMP596f2da tools/gh-renderer.ts branch port.
//! Original Graph arguments/options/theme and deferred frame inputs are retained.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::{graph_renderer::DisplayGraph,github_output_block::{self as ui,Result,Block,BlockKey}};
use ara_rpc::{WireString,value_heap::{ValueHandle,ValueSnapshot,ValueKind,CompiledFunctionSpec,CompiledFunctionForm}};
use std::sync::Arc;
#[derive(Clone)]
pub(crate) struct Imports {titles:ValueHandle,success:ValueHandle,failure:ValueHandle,running:ValueHandle,pending:ValueHandle}
impl Imports {
    pub(crate) fn prepare(g:&DisplayGraph<'_>)->Result<Arc<Self>>{
        let heap=g.0.heap();let titles=heap.object()?;
        for(op,title)in [("repo_view","GitHub Repo"),("pr_checkout","GitHub PR Checkout"),("pr_push","GitHub PR Push"),("search_issues","GitHub Search Issues"),("search_prs","GitHub Search PRs"),("search_code","GitHub Search Code"),("search_commits","GitHub Search Commits"),("search_repos","GitHub Search Repos"),("run_watch","GitHub Run Watch")]{
            heap.define_own_data(&titles,op.into(),ui::literal(g,title)?,g.runtime())?;
        }
        let set=|entries:&[&str]|->Result<ValueHandle>{
            let ctor=g.global("Set")?;
            heap.construct(&ctor,&[heap.array(entries.iter().map(|s|ui::literal(g,s)).collect::<Result<_>>()?)?],g.runtime())
        };
        Ok(Arc::new(Self{titles,success:set(&["success","neutral","skipped"])?,failure:set(&["failure","timed_out","cancelled","action_required","startup_failure"])?,running:set(&["in_progress"])?,pending:set(&["queued","requested","waiting","pending"])?}))
    }
}
fn number(g:&DisplayGraph<'_>,value:usize)->Result<ValueHandle>{g.0.heap().number(value as f64)}
fn empty(g:&DisplayGraph<'_>)->Result<ValueHandle>{ui::literal(g,"")}
fn undefined(g:&DisplayGraph<'_>)->Result<ValueHandle>{g.0.heap().undefined()}
fn equal(g:&DisplayGraph<'_>,v:&ValueHandle,s:&str)->Result<bool>{Ok(matches!(g.0.heap().snapshot(v)?,ValueSnapshot::String(v)if v.equals_ascii(s)))}
fn bool_true(g:&DisplayGraph<'_>,value:&ValueHandle)->Result<bool>{Ok(matches!(g.0.heap().snapshot(value)?,ValueSnapshot::Boolean(true)))}
fn length(g:&DisplayGraph<'_>,v:&ValueHandle)->Result<usize>{Ok(g.0.heap().to_number(&g.get(v,"length")?,g.runtime())?as usize)}
fn default(g:&DisplayGraph<'_>,value:ValueHandle,fallback:ValueHandle)->Result<ValueHandle>{if g.nullish(&value)?{Ok(fallback)}else{Ok(value)}}
fn op_title(g:&DisplayGraph<'_>,imports:&Imports,op:&ValueHandle)->Result<ValueHandle>{
    if g.truth(op)?{
        let key=g.0.heap().to_string(op,g.runtime())?;
        let title=g.0.heap().get(&imports.titles,&key.clone().into(),g.runtime())?;
        if g.truth(&title)?{return g.0.heap().get(&imports.titles,&key.into(),g.runtime())}
    }
    ui::literal(g,"GitHub")
}
fn issue_id(g:&DisplayGraph<'_>,value:&ValueHandle)->Result<ValueHandle>{
    if !g.truth(value)?{return undefined(g)}
    let trimmed=ui::trim(g,value)?;
    if !g.truth(&trimmed)?{return undefined(g)}
    // Original regexp results retain the exact decimal spelling (no numeric parse).
    let digits=g.0.heap().regexp(r"^\d+$".into(),"")?;
    if g.truth(&g.method(&digits,"test",&[trimmed.clone()])?)?{return ui::concat(g,&[ui::literal(g,"#")?,trimmed])}
    let regex=g.0.heap().regexp(r"/(?:issues|pull)/(\d+)".into(),"")?;
    let original_match=g.get(&trimmed,"match")?;
    let matched=if g.0.heap().kind(&trimmed)?==ValueKind::String&&g.github_match_initially_missing()?&&matches!(g.0.heap().snapshot(&original_match)?,ValueSnapshot::Undefined){
        // Current Native realm lacks String.match/RegExp @@match. This fixed
        // literal is non-global; original RegExp.exec gives its exact capture
        // result. Arbitrary/global regexp consumers remain outside this port.
        g.method(&regex,"exec",&[trimmed.clone()])?
    }else{g.0.heap().call(&original_match,&trimmed,&[regex],g.runtime())?};
    if g.truth(&matched)?{return ui::concat(g,&[ui::literal(g,"#")?,g.get(&matched,"1")?])}
    ui::truncate(g,trimmed,40)
}
static MAP_PR:CompiledFunctionSpec=CompiledFunctionSpec{
    source:"OMP596f2da gh-renderer formatPrIdentifier: p=>extractIssueId(p)",name:"",length:1,form:CompiledFunctionForm::Arrow,capture_count:0,
    body:|heap,_,_,args,rt|{let value=args.first().cloned().unwrap_or(heap.undefined()?);let values=super::graph_renderer::session_values_for_heap(heap)?;issue_id(&DisplayGraph(values.as_ref(),rt),&value)},
};
static KEEP_DEFINED:CompiledFunctionSpec=CompiledFunctionSpec{
    source:"OMP596f2da gh-renderer p!==undefined",name:"",length:1,form:CompiledFunctionForm::Arrow,capture_count:0,
    body:|heap,_,_,args,_|heap.boolean(!matches!(heap.snapshot(args.first().unwrap_or(&heap.undefined()?))?,ValueSnapshot::Undefined)),
};
fn pr_id(g:&DisplayGraph<'_>,pr:&ValueHandle)->Result<ValueHandle>{
    if matches!(g.0.heap().snapshot(pr)?,ValueSnapshot::Undefined){return undefined(g)}
    let is_array=g.method(&g.global("Array")?,"isArray",&[pr.clone()])?;
    if g.truth(&is_array)?{
        let mapped=g.method(pr,"map",&[g.0.heap().fresh_compiled_function(&MAP_PR,&[]) ?])?;
        let parts=g.method(&mapped,"filter",&[g.0.heap().fresh_compiled_function(&KEEP_DEFINED,&[]) ?])?;
        let count=length(g,&parts)?;if count==0{return undefined(g)}
        if count>3{
            let first=g.method(&parts,"slice",&[number(g,0)?,number(g,3)?])?;
            return ui::concat(g,&[g.method(&first,"join",&[ui::literal(g,", ")?])?,ui::literal(g,&format!(", +{} more",count-3))?])
        }
        return g.method(&parts,"join",&[ui::literal(g,", ")?])
    }
    issue_id(g,pr)
}
fn push_truthy(g:&DisplayGraph<'_>,out:&mut Vec<ValueHandle>,args:&ValueHandle,key:&str)->Result<()> {
    if g.truth(&g.get(args,key)?)?{out.push(g.get(args,key)?)}Ok(())
}
fn meta(g:&DisplayGraph<'_>,args:&ValueHandle)->Result<Vec<ValueHandle>>{
    let op=g.get(args,"op")?;let mut out=Vec::new();
    if equal(g,&op,"pr_checkout")?||equal(g,&op,"pr_push")?{
        let id=pr_id(g,&g.get(args,"pr")?)?;
        if g.truth(&id)?{out.push(id)}else{push_truthy(g,&mut out,args,"branch")?}
        push_truthy(g,&mut out,args,"repo")?;
    }else if ["search_issues","search_prs","search_code","search_commits","search_repos"].into_iter().map(|s|equal(g,&op,s)).collect::<Result<Vec<_>>>()?.into_iter().any(|v|v){
        if g.truth(&g.get(args,"query")?)?{out.push(ui::truncate(g,g.get(args,"query")?,80)?)}
        if !equal(g,&op,"search_repos")?{push_truthy(g,&mut out,args,"repo")?}
    }else if equal(g,&op,"repo_view")?{
        push_truthy(g,&mut out,args,"repo")?;push_truthy(g,&mut out,args,"branch")?;
    }else if !equal(g,&op,"run_watch")?{push_truthy(g,&mut out,args,"repo")?}
    Ok(out)
}
fn normalized_field(g:&DisplayGraph<'_>,args:&ValueHandle,key:&str)->Result<ValueHandle>{
    let first=g.get(args,key)?;
    if g.0.heap().kind(&first)?==ValueKind::String && length(g,&ui::trim(g,&g.get(args,key)?)?)?>0 {ui::trim(g,&g.get(args,key)?)}else{undefined(g)}
}
fn spread_op(g:&DisplayGraph<'_>,args:&ValueHandle,op:ValueHandle)->Result<ValueHandle>{
    let target=g.0.heap().object()?;
    g.0.heap().spread_into(&target,args,g.runtime())?;
    g.0.heap().define_own_data(&target,"op".into(),op,g.runtime())?;Ok(target)
}
fn watch_call(g:&DisplayGraph<'_>,args:&ValueHandle,options:&ValueHandle,theme:&ValueHandle)->Result<ValueHandle>{
    let spinner=g.get(options,"spinnerFrame")?;
    let icon=ui::status_icon(g,theme,if matches!(g.0.heap().snapshot(&spinner)?,ValueSnapshot::Undefined){"pending"}else{"running"},&g.get(options,"spinnerFrame")?)?;
    let run=normalized_field(g,args,"run")?;let branch=normalized_field(g,args,"branch")?;
    let title=ui::fg(g,theme,"accent",ui::literal(g,"GitHub Run Watch")?)?;
    let meta=if g.truth(&run)?{ui::fg(g,theme,"muted",ui::concat(g,&[ui::literal(g,"#")?,run])?)?}
        else if g.truth(&branch)?{ui::fg(g,theme,"text",branch)?}else{ui::fg(g,theme,"muted",ui::literal(g,"current HEAD")?)?};
    let header=ui::concat(g,&[icon,ui::literal(g," ")?,title,ui::literal(g,"  ")?,meta])?;
    ui::concat(g,&[header,ui::literal(g,"\n")?,ui::fg(g,theme,"dim",ui::literal(g,"waiting for workflow data...")?)?])
}
fn call(g:&DisplayGraph<'_>,imports:&Imports,args:&ValueHandle,options:&ValueHandle,theme:&ValueHandle)->Result<Component>{
    let op=normalized_field(g,args,"op")?;
    if equal(g,&op,"run_watch")?{return Ok(Component::text(watch_call(g,&spread_op(g,args,op)?,options,theme)?))}
    let spinner=g.get(options,"spinnerFrame")?;
    let status=if matches!(g.0.heap().snapshot(&spinner)?,ValueSnapshot::Undefined){"pending"}else{"running"};
    // Source options object reads spinnerFrame before computing title/meta; the
    // status-line helper evaluates the icon only after those operands exist.
    let selected_spinner=g.get(options,"spinnerFrame")?;
    let title=op_title(g,imports,&op)?;let meta=meta(g,&spread_op(g,args,op)?)?;
    let icon=ui::status_icon(g,theme,status,&selected_spinner)?;
    Ok(Component::text(ui::status_line(g,theme,icon,title,"accent",&meta)?))
}
fn short_sha(g:&DisplayGraph<'_>,value:&ValueHandle)->Result<ValueHandle>{
    if !g.truth(value)?{undefined(g)}else{g.method(value,"slice",&[number(g,0)?,number(g,12)?])}
}
fn watch_header(g:&DisplayGraph<'_>,watch:&ValueHandle)->Result<ValueHandle>{
    if g.is(watch,"mode","run")?&&g.truth(&g.get(watch,"run")?)?{
        let prefix=if g.is(watch,"state","watching")?{"watching run #"}else{"run #"};
        return ui::concat(g,&[ui::literal(g,prefix)?,g.get(&g.get(watch,"run")?,"id")?,ui::literal(g," on ")?,g.get(watch,"repo")?])
    }
    let sha=default(g,short_sha(g,&g.get(watch,"headSha")?)?,ui::literal(g,"this commit")?)?;
    ui::concat(g,&[ui::literal(g,if g.is(watch,"state","watching")?{"watching "}else{"workflow runs for "})?,sha,ui::literal(g," on ")?,g.get(watch,"repo")?])
}
fn run_label(g:&DisplayGraph<'_>,run:&ValueHandle)->Result<ValueHandle>{
    let name=g.get(run,"workflowName")?;let name=if g.nullish(&name)?{let title=g.get(run,"displayTitle")?;default(g,title,ui::literal(g,"GitHub Actions")?)?}else{name};ui::tabs(g,&name)
}
fn run_meta(g:&DisplayGraph<'_>,run:&ValueHandle)->Result<Vec<ValueHandle>>{
    let mut parts=Vec::new();
    if g.truth(&g.get(run,"branch")?)?{parts.push(ui::tabs(g,&g.get(run,"branch")?)?)}
    else if g.truth(&g.get(run,"headSha")?)?{
        let sha=short_sha(g,&g.get(run,"headSha")?)?;parts.push(if g.nullish(&sha)?{g.get(run,"headSha")?}else{sha});
    }
    parts.push(ui::concat(g,&[ui::literal(g,"#")?,g.get(run,"id")?])?);Ok(parts)
}
fn run_line(g:&DisplayGraph<'_>,run:&ValueHandle,theme:&ValueHandle)->Result<ValueHandle>{
    let title=ui::fg(g,theme,"accent",run_label(g,run)?)?;let meta=run_meta(g,run)?;
    let mut styled=vec![title];let count=meta.len();
    for(index,part)in meta.into_iter().enumerate(){styled.push(ui::fg(g,theme,if index+1==count{"muted"}else{"text"},part)?)}
    ui::join(g,&styled,ui::literal(g,"  ")?)
}
fn has(g:&DisplayGraph<'_>,set:&ValueHandle,value:ValueHandle)->Result<bool>{g.truth(&g.method(set,"has",&[value])?)}
fn job_state(g:&DisplayGraph<'_>,imports:&Imports,job:&ValueHandle,theme:&ValueHandle)->Result<(ValueHandle,&'static str,&'static str)>{
    for(key,set,symbol,icon,text)in [("conclusion",&imports.success,"success","accent","success"),("conclusion",&imports.failure,"error","error","error"),("status",&imports.running,"enabled","warning","warning"),("status",&imports.pending,"shadowed","muted","muted")]{
        if g.truth(&g.get(job,key)?)?&&has(g,set,g.get(job,key)?)?{return Ok((ui::theme_property(g,theme,"status",symbol)?,icon,text))}
    }
    Ok((ui::theme_property(g,theme,"status","shadowed")?,"muted","muted"))
}
fn job_line(g:&DisplayGraph<'_>,imports:&Imports,job:&ValueHandle,columns:usize,theme:&ValueHandle)->Result<ValueHandle>{
    let(icon,icon_color,text_color)=job_state(g,imports,job,theme)?;
    let prefix=ui::fg(g,theme,icon_color,ui::concat(g,&[icon,ui::literal(g," ")?])?)?;
    let duration=g.get(job,"durationSeconds")?;
    let duration=if matches!(g.0.heap().snapshot(&duration)?,ValueSnapshot::Undefined){undefined(g)?}else{ui::concat(g,&[g.get(job,"durationSeconds")?,ui::literal(g,"s")?])?};
    let duration=if g.truth(&duration)?{ui::fg(g,theme,text_color,duration)?}else{undefined(g)?};
    let reserved=ui::width(g,&prefix)?+if g.truth(&duration)?{1+ui::width(g,&duration)?}else{0};
    let name_width=columns.saturating_sub(reserved).max(8);
    let name=ui::fg(g,theme,text_color,ui::truncate(g,ui::tabs(g,&g.get(job,"name")?)?,name_width)?)?;
    let mut line=ui::concat(g,&[prefix,name])?;
    if g.truth(&duration)?{let spaces=columns.saturating_sub(ui::width(g,&line)?+ui::width(g,&duration)?).max(1);line=ui::concat(g,&[line,ui::literal(g,&" ".repeat(spaces))?,duration])?}
    Ok(line)
}
fn run_block(g:&DisplayGraph<'_>,imports:&Imports,run:&ValueHandle,columns:usize,theme:&ValueHandle)->Result<Vec<ValueHandle>>{
    let mut lines=vec![run_line(g,run,theme)?];
    if length(g,&g.get(run,"jobs")?)?==0{lines.push(ui::fg(g,theme,"dim",ui::literal(g,"waiting for workflow jobs...")?)?);return Ok(lines)}
    for job in g.members(&g.get(run,"jobs")?)?{lines.push(job_line(g,imports,&job,columns,theme)?)}Ok(lines)
}
fn failed_logs(g:&DisplayGraph<'_>,failed:&ValueHandle,columns:usize,theme:&ValueHandle,expanded:bool)->Result<Vec<ValueHandle>>{
    if length(g,failed)?==0{return Ok(Vec::new())}
    let mut lines=Vec::new();
    for entry in g.members(failed)?{
        let context=if g.truth(&g.get(&entry,"workflowName")?)?{ui::concat(g,&[g.get(&entry,"workflowName")?,ui::literal(g,"  #")?,g.get(&entry,"runId")?])?}
            else{ui::concat(g,&[ui::literal(g,"run #")?,g.get(&entry,"runId")?])?};
        let error=ui::theme_property(g,theme,"status","error")?;
        let name=ui::tabs(g,&g.get(&entry,"jobName")?)?;
        let context=ui::fg(g,theme,"muted",context)?;
        lines.push(ui::fg(g,theme,"error",ui::concat(g,&[error,ui::literal(g," ")?,name,ui::literal(g,"  ")?,context])?)?);
        if !g.truth(&g.get(&entry,"available")?)?||!g.truth(&g.get(&entry,"tail")?)?{lines.push(ui::fg(g,theme,"dim",ui::literal(g,"  log tail unavailable")?)?);continue}
        let tail=ui::tabs(g,&g.get(&entry,"tail")?)?;
        let mut tail_lines=Vec::new();for line in ui::split(g,&tail)?{if length(g,&line)?>0{tail_lines.push(line)}}
        let limit=if expanded{tail_lines.len()}else{tail_lines.len().min(3)};
        for line in &tail_lines[tail_lines.len().saturating_sub(limit)..]{
            lines.push(ui::fg(g,theme,"dim",ui::concat(g,&[ui::literal(g,"  ")?,ui::truncate(g,line.clone(),columns.saturating_sub(2).max(8))?])?)?);
        }
        if !expanded&&tail_lines.len()>limit{lines.push(ui::fg(g,theme,"dim",ui::concat(g,&[ui::literal(g,&format!("  … {} more log lines ",tail_lines.len()-limit))?,ui::expand_hint(g,theme)?])?)?)}
    }
    Ok(lines)
}
fn watch_sections(g:&DisplayGraph<'_>,imports:&Imports,watch:&ValueHandle,theme:&ValueHandle,options:&ValueHandle,columns:usize)->Result<Vec<(Option<ValueHandle>,Vec<ValueHandle>)>>{
    let mut main=Vec::new();
    if g.truth(&g.get(watch,"note")?)?{main.push(ui::fg(g,theme,"dim",ui::tabs(g,&g.get(watch,"note")?)?)?)}
    if g.is(watch,"mode","run")?&&g.truth(&g.get(watch,"run")?)?{main.extend(run_block(g,imports,&g.get(watch,"run")?,columns,theme)?)}
    else if g.is(watch,"mode","commit")?{
        let runs=default(g,g.get(watch,"runs")?,g.0.heap().array(Vec::new())?)?;
        if length(g,&runs)?==0{main.push(ui::fg(g,theme,"dim",ui::literal(g,"waiting for workflow runs...")?)?)}
        else{for(index,run)in g.members(&runs)?.into_iter().enumerate(){if index>0{main.push(empty(g)?)}main.extend(run_block(g,imports,&run,columns,theme)?);}}
    }
    let mut sections=Vec::new();if !main.is_empty(){sections.push((None,main))}
    let failed=default(g,g.get(watch,"failedLogs")?,g.0.heap().array(Vec::new())?)?;
    let expanded=g.truth(&g.get(options,"expanded")?)?;
    let failed=failed_logs(g,&failed,columns,theme,expanded)?;
    if !failed.is_empty(){sections.push((Some(ui::literal(g,"failed logs")?),failed))}Ok(sections)
}
static IS_TEXT:CompiledFunctionSpec=CompiledFunctionSpec{
    source:"OMP596f2da gh-renderer extractText: part=>part.type===text",name:"",length:1,form:CompiledFunctionForm::Arrow,capture_count:0,
    body:|heap,_,_,args,rt|{let value=heap.get(&args[0],&"type".into(),rt)?;heap.boolean(matches!(heap.snapshot(&value)?,ValueSnapshot::String(s)if s.equals_ascii("text")))},
};
static TEXT_VALUE:CompiledFunctionSpec=CompiledFunctionSpec{
    source:"OMP596f2da gh-renderer extractText: part=>part.text",name:"",length:1,form:CompiledFunctionForm::Arrow,capture_count:0,
    body:|heap,_,_,args,rt|heap.get(&args[0],&"text".into(),rt),
};
static NONEMPTY_STRING:CompiledFunctionSpec=CompiledFunctionSpec{
    source:"OMP596f2da gh-renderer extractText typeof string&&value.length>0",name:"",length:1,form:CompiledFunctionForm::Arrow,capture_count:0,
    body:|heap,_,_,args,rt|{let value=&args[0];if heap.kind(value)?!=ValueKind::String{return heap.boolean(false)}let length=heap.get(value,&"length".into(),rt)?;heap.boolean(heap.to_number(&length,rt)?>0.0)},
};
fn extract_text(g:&DisplayGraph<'_>,content:ValueHandle)->Result<ValueHandle>{
    let filtered=g.method(&content,"filter",&[g.0.heap().fresh_compiled_function(&IS_TEXT,&[]) ?])?;
    let mapped=g.method(&filtered,"map",&[g.0.heap().fresh_compiled_function(&TEXT_VALUE,&[]) ?])?;
    let selected=g.method(&mapped,"filter",&[g.0.heap().fresh_compiled_function(&NONEMPTY_STRING,&[]) ?])?;
    g.method(&selected,"join",&[ui::literal(g,"\n")?])
}
#[derive(Clone)]
enum Content {Text(ValueHandle),Watch(ValueHandle),Fallback(Vec<ValueHandle>)}
#[derive(Clone)]
pub(crate) struct Component {
    content:Content,header:Option<ValueHandle>,theme:Option<ValueHandle>,options:Option<ValueHandle>,error:bool,imports:Option<Arc<Imports>>,
    cache:Option<(BlockKey,ValueHandle)>,text_cache:Option<(u64,ValueHandle)>,
}
impl Component {
    pub(crate) fn text(value:ValueHandle)->Self{Self{content:Content::Text(value),header:None,theme:None,options:None,error:false,imports:None,cache:None,text_cache:None}}
    fn framed(content:Content,header:ValueHandle,theme:ValueHandle,options:ValueHandle,error:bool,imports:Arc<Imports>)->Self{
        Self{content,header:Some(header),theme:Some(theme),options:Some(options),error,imports:Some(imports),cache:None,text_cache:None}
    }
    pub(crate) fn is_framed(&self)->bool{!matches!(self.content,Content::Text(_))}
    pub(crate) fn invalidate(&mut self){self.cache=None;self.text_cache=None;}
    pub(crate) fn render(&mut self,g:&DisplayGraph<'_>,width:ValueHandle)->Result<ValueHandle>{
        let width=g.0.heap().to_number(&width,g.runtime())?;
        if let Content::Text(value)=&self.content {
            let key=width.to_bits();let columns=width.max(0.0)as u32;
            if let Some((old,rows))=&self.text_cache{if *old==key{return Ok(rows.clone())}}
            let rows=if !g.truth(value)?||equal(g,&ui::trim(g,value)?,"")?{g.0.heap().array(Vec::new())?}else{
                let value=ui::tabs(g,value)?;let mut rows=Vec::new();
                for line in super::text::wrap_text_with_ansi(g.string(&value)?.units(),columns.max(1),3){
                    let value=g.0.heap().string(WireString::from_units(line))?;
                    let padding=ui::literal(g,&" ".repeat((columns as usize).saturating_sub(ui::width(g,&value)?)))?;
                    rows.push(ui::concat(g,&[value,padding])?);
                }
                g.0.heap().array(rows)?
            };
            self.text_cache=Some((key,rows.clone()));return Ok(rows)
        }
        let theme=self.theme.as_ref().expect("framed theme");let options=self.options.as_ref().expect("framed options");
        let inner=ui::content_width(if width==0.0||width.is_nan(){80.0}else{width});
        let sections=match &self.content {
            Content::Watch(watch)=>watch_sections(g,self.imports.as_ref().unwrap(),watch,theme,options,inner)?,
            Content::Fallback(all)=>{
                let expanded=g.truth(&g.get(options,"expanded")?)?;let limit=if expanded{all.len()}else{all.len().min(10)};
                let mut out=Vec::new();for line in &all[..limit]{let colored=ui::fg(g,theme,if self.error{"error"}else{"toolOutput"},line.clone())?;out.push(ui::truncate(g,colored,inner)?)}
                if !expanded&&all.len()>limit{
                    let hint=ui::expand_hint(g,theme)?;let more=ui::more(g,all.len()-limit,"line")?;
                    let more=if g.truth(&hint)?{ui::concat(g,&[more,ui::literal(g," ")?,hint])?}else{more};out.push(ui::fg(g,theme,"dim",more)?);
                }
                if out.is_empty(){Vec::new()}else{vec![(None,out)]}
            },Content::Text(_)=>unreachable!(),
        };
        let block=Block{width,header:self.header.clone().unwrap(),error:self.error,sections};
        // fixed framedBlock always evaluates build(width), then the cache key.
        let key=block.key(g)?;if let Some((old,rows))=&self.cache{if *old==key{return Ok(rows.clone())}}
        let rows=ui::render(g,theme,&block)?;self.cache=Some((key,rows.clone()));Ok(rows)
    }
}
fn result(g:&DisplayGraph<'_>,imports:Arc<Imports>,result:&ValueHandle,options:&ValueHandle,theme:&ValueHandle,args:&ValueHandle)->Result<Component>{
    let details=g.get(result,"details")?;let watch=if g.nullish(&details)?{undefined(g)?}else{g.get(&details,"watch")?};
    if g.truth(&watch)?{
        let error=bool_true(g,&g.get(result,"isError")?)?;
        let (icon,meta)=if error{
            let meta=watch_header(g,&watch)?;(ui::status_icon(g,theme,"error",&undefined(g)?)?,meta)
        }else{let icon=ui::styled(g,theme,"tool.gh","accent")?;(icon,watch_header(g,&watch)?)};
        let header=ui::status_line(g,theme,icon,ui::literal(g,"GitHub Run Watch")?,if error{"error"}else{"accent"},&[meta])?;
        return Ok(Component::framed(Content::Watch(watch),header,theme.clone(),options.clone(),error,imports))
    }
    let args=default(g,args.clone(),g.0.heap().object()?)?;
    let text=extract_text(g,g.get(result,"content")?)?;let title=op_title(g,&imports,&g.get(&args,"op")?)?;let meta=meta(g,&args)?;
    let error=bool_true(g,&g.get(result,"isError")?)?;let success=!error&&g.truth(&text)?;
    let icon=if success{ui::styled(g,theme,"tool.gh","accent")?}else{ui::status_icon(g,theme,if error{"error"}else{"warning"},&undefined(g)?)?};
    let header=ui::status_line(g,theme,icon,title,if error{"error"}else{"accent"},&meta)?;
    if !g.truth(&text)?{return Ok(Component::text(ui::concat(g,&[header,ui::literal(g,"\n")?,ui::fg(g,theme,"dim",ui::literal(g,if error{"request failed"}else{"no output"})?)?])?))}
    let text=ui::tabs(g,&text)?;let mut all=ui::split(g,&text)?;
    while !all.is_empty()&&equal(g,&ui::trim(g,&all[0])?,"")?{all.remove(0);}
    while !all.is_empty()&&equal(g,&ui::trim(g,all.last().unwrap())?,"")?{all.pop();}
    if all.len()<=1&&!error{
        let body=all.first().cloned().unwrap_or(undefined(g)?);
        if !g.truth(&body)?{return Ok(Component::text(header))}
        return Ok(Component::text(ui::concat(g,&[header,ui::literal(g,"\n")?,ui::fg(g,theme,"toolOutput",body)?])?))
    }
    Ok(Component::framed(Content::Fallback(all),header,theme.clone(),options.clone(),error,imports))
}
pub(crate) fn construct(g:&DisplayGraph<'_>,imports:Arc<Imports>,is_result:bool,args:&[ValueHandle])->Result<Component>{
    let selected=|index|args.get(index).cloned().unwrap_or(g.0.heap().undefined().expect("heap live"));
    if is_result{result(g,imports,&selected(0),&selected(1),&selected(2),&selected(3))}else{call(g,&imports,&selected(0),&selected(1),&selected(2))}
}
