//! Fixed OMP596f2da Github dependencies from tui/status-line.ts and output-block.ts.
//! This path preserves Github's default body padding and applyBg=false without
//! changing the EDIT output block. Copyright (c) 2025 Mario Zechner;
//! Copyright (c) 2025-2026 Can Bölük; Copyright (c) 2026 Stencil Labs, Inc.
//! MIT; see THIRD_PARTY_NOTICES.md.
use super::{graph_renderer::DisplayGraph, text};
use ara_rpc::{WireString, value_heap::{ValueError, ValueHandle, ValueKind, ValueSnapshot}};
pub(crate) type Result<T> = std::result::Result<T, ValueError>;
pub(crate) fn literal(g:&DisplayGraph<'_>,s:&str)->Result<ValueHandle>{g.0.heap().string(s.into())}
pub(crate) fn concat(g:&DisplayGraph<'_>,parts:&[ValueHandle])->Result<ValueHandle>{
    let mut out=Vec::new();
    for value in parts {
        if g.0.heap().kind(value)?==ValueKind::Symbol{return Err(ValueError::TypeError("Cannot convert a Symbol value to a string".into()))}
        out.extend_from_slice(g.0.heap().to_string(value,g.runtime())?.units());
    }
    g.0.heap().string(WireString::from_units(out))
}
pub(crate) fn fg(g:&DisplayGraph<'_>,theme:&ValueHandle,color:&str,value:ValueHandle)->Result<ValueHandle>{
    g.method(theme,"fg",&[literal(g,color)?,value])
}
pub(crate) fn styled(g:&DisplayGraph<'_>,theme:&ValueHandle,key:&str,color:&str)->Result<ValueHandle>{
    g.method(theme,"styledSymbol",&[literal(g,key)?,literal(g,color)?])
}
pub(crate) fn theme_property(g:&DisplayGraph<'_>,theme:&ValueHandle,group:&str,key:&str)->Result<ValueHandle>{
    g.get(&g.get(theme,group)?,key)
}
pub(crate) fn width(g:&DisplayGraph<'_>,value:&ValueHandle)->Result<usize>{
    Ok(text::visible_width(g.string(value)?.units(),3) as usize)
}
pub(crate) fn truncate(g:&DisplayGraph<'_>,value:ValueHandle,columns:usize)->Result<ValueHandle>{
    let value=g.string(&value)?;
    g.0.heap().string(WireString::from_units(text::truncate_to_width(value.units(),columns.min(u32::MAX as usize)as u32,None,None,3)))
}
pub(crate) fn tabs(g:&DisplayGraph<'_>,value:&ValueHandle)->Result<ValueHandle>{
    // pi-tui replaceTabs is a string-only replacement, without stringify.
    let value=g.string(value)?;let mut out=Vec::new();
    for unit in value.units(){if *unit==9{out.extend_from_slice(&[32,32,32])}else{out.push(*unit)}}
    g.0.heap().string(WireString::from_units(out))
}
pub(crate) fn trim(g:&DisplayGraph<'_>,value:&ValueHandle)->Result<ValueHandle>{g.method(value,"trim",&[])}
pub(crate) fn split(g:&DisplayGraph<'_>,value:&ValueHandle)->Result<Vec<ValueHandle>>{
    let array=g.method(value,"split",&[literal(g,"\n")?])?;g.members(&array)
}
pub(crate) fn join(g:&DisplayGraph<'_>,values:&[ValueHandle],separator:ValueHandle)->Result<ValueHandle>{
    g.method(&g.0.heap().array(values.to_vec())?,"join",&[separator])
}
pub(crate) fn status_icon(g:&DisplayGraph<'_>,theme:&ValueHandle,status:&str,spinner:&ValueHandle)->Result<ValueHandle>{
    if status=="running" && !matches!(g.0.heap().snapshot(spinner)?,ValueSnapshot::Undefined){
        let frames=g.get(theme,"spinnerFrames")?;
        let length=g.0.heap().to_number(&g.get(&frames,"length")?,g.runtime())?;
        let frame=g.0.heap().to_number(spinner,g.runtime())?%length;
        let property=g.0.heap().to_string(&g.0.heap().number(frame)?,g.runtime())?;
        return g.0.heap().get(&frames,&property.into(),g.runtime())
    }
    let color=match status {"success"|"done"=>"success","error"|"aborted"=>"error","warning"=>"warning","pending"=>"muted",_=>"accent"};
    styled(g,theme,&format!("status.{status}"),color)
}
fn flatten(g:&DisplayGraph<'_>,value:&ValueHandle)->Result<ValueHandle>{
    let regex=g.0.heap().regexp(r"\r\n?|\n".into(),"g")?;
    g.method(value,"replace",&[regex,literal(g," ")?])
}
/// Github's status-line call shape: icon and title are evaluated before meta.
pub(crate) fn status_line(g:&DisplayGraph<'_>,theme:&ValueHandle,icon:ValueHandle,title:ValueHandle,color:&str,meta:&[ValueHandle])->Result<ValueHandle>{
    let title=fg(g,theme,color,flatten(g,&title)?)?;
    let mut line=if g.truth(&icon)?{concat(g,&[icon,literal(g," ")?,title])?}else{title};
    let mut nonempty=Vec::new();
    for entry in meta {let value=flatten(g,entry)?;let trimmed=trim(g,&value)?;let length=g.get(&trimmed,"length")?;if g.0.heap().to_number(&length,g.runtime())?>0.0{nonempty.push(value)}}
    if !nonempty.is_empty(){
        let dot=theme_property(g,theme,"sep","dot")?;
        let meta=fg(g,theme,"dim",join(g,&nonempty,dot)?)?;
        line=concat(g,&[line,literal(g," ")?,meta])?;
    }
    Ok(line)
}
pub(crate) fn expand_hint(g:&DisplayGraph<'_>,theme:&ValueHandle)->Result<ValueHandle>{
    let left=theme_property(g,theme,"format","bracketLeft")?;
    let right=theme_property(g,theme,"format","bracketRight")?;
    let key=g.github_expand_key_hint()?;
    fg(g,theme,"dim",concat(g,&[left,key,literal(g,": Expand")?,right])?)
}
pub(crate) fn more(g:&DisplayGraph<'_>,remaining:usize,kind:&str)->Result<ValueHandle>{
    literal(g,&format!("… {remaining} more {kind}{}",if remaining==1{""}else{"s"}))
}
#[derive(Clone,Debug,PartialEq,Eq)]
pub(crate) struct Section {pub label:Option<WireString>,pub lines:Vec<WireString>}
#[derive(Clone,Debug,PartialEq,Eq)]
pub(crate) struct BlockKey {pub width:u32,pub header:WireString,pub error:bool,pub sections:Vec<Section>}
#[derive(Clone)]
pub(crate) struct Block {pub width:f64,pub header:ValueHandle,pub error:bool,pub sections:Vec<(Option<ValueHandle>,Vec<ValueHandle>)>}
impl Block {
    pub fn key(&self,g:&DisplayGraph<'_>)->Result<BlockKey>{
        let width=if self.width.is_finite(){self.width.trunc().rem_euclid(4294967296.0)as u32}else{0};
        Ok(BlockKey{width,header:g.string(&self.header)?,error:self.error,sections:self.sections.iter().map(|(label,lines)|Ok(Section{
            label:label.as_ref().map(|v|g.string(v)).transpose()?,lines:lines.iter().map(|v|g.string(v)).collect::<Result<_>>()?,
        })).collect::<Result<_>>()?})
    }
}
pub(crate) fn content_width(width:f64)->usize {if width.is_nan(){0}else{(width-4.0).max(1.0)as usize}}
fn repeat(g:&DisplayGraph<'_>,value:&ValueHandle,count:usize)->Result<ValueHandle>{
    g.method(value,"repeat",&[g.0.heap().number(count as f64)?])
}
fn padding(g:&DisplayGraph<'_>,count:usize)->Result<ValueHandle>{literal(g,&" ".repeat(count))}
/// Github never requests a state background, images or explicit padding.
/// Retain default 1+1 padding, width0 bars, labeled separators and padToWidth.
pub(crate) fn render(g:&DisplayGraph<'_>,theme:&ValueHandle,block:&Block)->Result<ValueHandle>{
    let h=theme_property(g,theme,"boxRound","horizontal")?;
    let v=theme_property(g,theme,"boxRound","vertical")?;
    let cap=repeat(g,&h,3)?;
    let columns=block.width.max(0.0)as usize;
    let inner=columns.saturating_sub(width(g,&v)?.saturating_mul(2)+2);
    let color=if block.error{"error"}else{"borderMuted"};
    let bar=|left:ValueHandle,right:ValueHandle,label:Option<&ValueHandle>|->Result<ValueHandle>{
        let left=concat(g,&[left,cap.clone()])?;
        if columns==0{return concat(g,&[fg(g,theme,color,left)?,fg(g,theme,color,right)?])}
        let label=label.cloned().unwrap_or(literal(g,"")?);
        let has_label=g.truth(&label)?;
        let label=if has_label{truncate(g,concat(g,&[literal(g," ")?,label,literal(g," ")?])?,columns.saturating_sub(width(g,&left)?+width(g,&right)?))?}else{literal(g,"")?};
        let fill=columns.saturating_sub(width(g,&left)?+width(g,&label)?+width(g,&right)?);
        concat(g,&[fg(g,theme,color,left)?,label,fg(g,theme,color,repeat(g,&h,fill)?)?,fg(g,theme,color,right)?])
    };
    let mut rows=Vec::new();
    rows.push(bar(theme_property(g,theme,"boxRound","topLeft")?,theme_property(g,theme,"boxRound","topRight")?,Some(&block.header))?);
    for(label,lines)in &block.sections {
        if let Some(label)=label {if g.truth(label)?{rows.push(bar(theme_property(g,theme,"boxRound","teeRight")?,theme_property(g,theme,"boxRound","teeLeft")?,Some(label))?)}}
        for line in lines {
            for line in split(g,line)? {
                let trimmed=g.method(&line,"trimEnd",&[])?;
                for wrapped in text::wrap_text_with_ansi(g.string(&trimmed)?.units(),inner.min(u32::MAX as usize)as u32,3){
                    let wrapped=g.0.heap().string(WireString::from_units(wrapped))?;
                    let pad=padding(g,inner.saturating_sub(width(g,&wrapped)?))?;
                    rows.push(concat(g,&[fg(g,theme,color,v.clone())?,literal(g," ")?,wrapped,pad,literal(g," ")?,fg(g,theme,color,v.clone())?])?);
                }
            }
        }
    }
    let left=concat(g,&[theme_property(g,theme,"boxRound","bottomLeft")?,cap])?;
    let right=theme_property(g,theme,"boxRound","bottomRight")?;
    let fill=columns.saturating_sub(width(g,&left)?+width(g,&right)?);
    rows.push(concat(g,&[fg(g,theme,color,left)?,fg(g,theme,color,repeat(g,&h,fill)?)?,fg(g,theme,color,right)?])?);
    let mut padded=Vec::new();
    for row in rows{let pad=padding(g,columns.saturating_sub(width(g,&row)?))?;padded.push(concat(g,&[row,pad])?)}
    g.0.heap().array(padded)
}
