//! READ-only SQLite connection, selectors and output; fixed OMP 596f2da
//! tools/read-sqlite.ts and sqlite-reader.ts (MIT; THIRD_PARTY_NOTICES.md).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc.
//! Host paths/metrics remain injected. Accepted WRITE SQLite is unchanged.

use crate::{ToolContext, DEFAULT_MAX_BYTES, output};
use ara_agent::{ToolError, ToolOutput};
use ara_prompt::js::{trim, parse_int, to_number};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use rusqlite::types::Value;
use serde_json::json;
use std::{collections::HashMap, path::{Path, PathBuf}, io::Read, sync::LazyLock, time::{Duration, Instant}};
use tokio_util::sync::CancellationToken;

type Result<T> = std::result::Result<T, ToolError>;
fn error(e: impl std::fmt::Display) -> ToolError { ToolError(e.to_string()) }
fn aborted(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() { Err(error("Operation aborted")) } else { Ok(()) }
}

/// The existing Host owns Unicode/ANSI metrics; shared Tools do not import CLI.
pub trait ReadTextMetrics: Send + Sync {
    fn width(&self, text: &str) -> Result<usize>;
    fn truncate(&self, text: &str, columns: usize) -> Result<String>;
}

/// A missing Host port permits only exactly known ASCII and renderer glyphs.
/// This is explicitly not the complete Unicode path.
pub struct AsciiReadTextMetrics;
impl ReadTextMetrics for AsciiReadTextMetrics {
    fn width(&self, text: &str) -> Result<usize> {
        text.chars().try_fold(0, |count, c| {
            if c.is_ascii() && !c.is_ascii_control() || matches!(c, '…' | '─') { Ok(count + 1) }
            else { Err(error("SQLite READ Unicode/ANSI text metrics require the Host ReadTextMetrics service")) }
        })
    }
    fn truncate(&self, text: &str, columns: usize) -> Result<String> {
        if self.width(text)? <= columns { return Ok(text.to_owned()); }
        if columns == 0 { return Ok(String::new()); }
        Ok(text.chars().take(columns - 1).chain(['…']).collect())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathCandidate { pub database_path: String, pub sub_path: String, pub query_string: String }
pub fn parse_path_candidates(path: &str) -> Vec<PathCandidate> {
    static PATTERN: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)\.(?:sqlite3?|db3?)").unwrap());
    let normalized = path.replace('\\', "/");
    let mut candidates: Vec<_> = PATTERN.find_iter(&normalized)
        .filter(|m| normalized.as_bytes().get(m.end()).is_none_or(|b| matches!(b, b':' | b'?')))
        .map(|m| {
            let remainder = &normalized[m.end()..];
            let (sub, query) = remainder.split_once('?').unwrap_or((remainder, ""));
            PathCandidate { database_path: path[..m.end()].into(), sub_path: sub.trim_start_matches(':').into(), query_string: query.into() }
        }).collect();
    candidates.sort_by_key(|c| std::cmp::Reverse(c.database_path.len()));
    candidates
}

#[derive(Debug, Clone, PartialEq)]
pub enum Selector {
    List,
    Schema { table: String, sample_limit: usize },
    Row { table: String, key: String },
    Query { table: String, limit: usize, offset: f64, order: Option<String>, where_sql: Option<String> },
    Raw { sql: String },
}

/// URLSearchParams form decode: '+' is a space and malformed bytes use UTF8 replacement.
fn form_decode(s: &str) -> String {
    let bytes = s.as_bytes(); let mut out = Vec::with_capacity(bytes.len()); let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' { out.push(b' '); }
        else if bytes[i] == b'%' && i + 2 < bytes.len()
            && let (Some(a), Some(b)) = ((bytes[i+1] as char).to_digit(16), (bytes[i+2] as char).to_digit(16)) {
                out.push((a * 16 + b) as u8); i += 2;
            } else { out.push(bytes[i]); }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
const COMMENT_ERROR: &str = "SQLite 'where' clause must not contain comments or statement terminators; use '?q=SELECT ...' for raw SQL";
const KEYWORD_ERROR: &str = "SQLite 'where' clause must not contain LIMIT/OFFSET/UNION/INTERSECT/EXCEPT/ATTACH/DETACH/PRAGMA; use '?q=SELECT ...' for raw SQL";
pub fn validate_where(value: Option<&str>) -> Result<Option<String>> {
    let Some(sql) = value.map(trim).filter(|s| !s.is_empty()) else { return Ok(None); };
    let bytes = sql.as_bytes(); let mut quote = None; let mut token = None; let mut keyword = false; let mut i = 0;
    while i <= bytes.len() {
        let c = bytes.get(i).copied(); let next = bytes.get(i+1).copied();
        if let Some(q) = quote {
            if c == Some(q) && next == Some(q) { i += 2; continue; }
            if c == Some(q) { quote = None; } i += 1; continue;
        }
        if c.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_') { token.get_or_insert(i); i += 1; continue; }
        if let Some(start) = token.take() {
            keyword |= matches!(sql[start..i].to_ascii_lowercase().as_str(), "limit" | "offset" | "union" | "intersect" | "except" | "attach" | "detach" | "pragma");
        }
        if c.is_none() { break; }
        if matches!(c, Some(b'\'' | b'"')) { quote = c; }
        else if c == Some(b';') || matches!((c,next), (Some(b'-'),Some(b'-')) | (Some(b'/'),Some(b'*')) | (Some(b'*'),Some(b'/'))) { return Err(error(COMMENT_ERROR)); }
        i += 1;
    }
    if keyword { return Err(error(KEYWORD_ERROR)); }
    Ok(Some(sql.to_owned()))
}
pub fn parse_selector(sub: &str, query: &str) -> Result<Selector> {
    let sub = trim(sub.trim_start_matches(':'));
    let pairs: Vec<(String,String)> = query.strip_prefix('?').unwrap_or(query).split('&').filter(|s| !s.is_empty())
        .map(|s| { let (key,value)=s.split_once('=').unwrap_or((s,"")); (form_decode(key),form_decode(value)) }).collect();
    let get = |key: &str| pairs.iter().find(|(k,_)| k==key).map(|(_,v)| v.as_str());
    if let Some(sql) = get("q") {
        if !sub.is_empty() || pairs.iter().any(|(k,_)| k!="q") { return Err(error("SQLite raw queries cannot be combined with table selectors or pagination")); }
        if trim(sql).is_empty() { return Err(error("SQLite query parameter 'q' cannot be empty")); }
        return Ok(Selector::Raw { sql: sql.into() });
    }
    if sub.is_empty() {
        if !pairs.is_empty() { return Err(error("SQLite query parameters require a table selector or q=SELECT...")); }
        return Ok(Selector::List);
    }
    let (table,key)=sub.split_once(':').map_or((sub,None), |(t,k)|(t,Some(k)));
    if table.is_empty() { return Err(error("SQLite selectors must include a table name")); }
    if let Some(key)=key.filter(|k| !k.is_empty()) {
        if !pairs.is_empty() { return Err(error("SQLite row lookups cannot be combined with query parameters")); }
        return Ok(Selector::Row{table:table.into(),key:key.into()});
    }
    let where_sql=validate_where(get("where"))?;
    let order=get("order").map(trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let query_mode = get("limit").is_some() || get("offset").is_some() || order.is_some() || where_sql.is_some();
    for (key,_) in &pairs {
        if !query_mode || !matches!(key.as_str(),"limit"|"offset"|"order"|"where") { return Err(error(format!("Unsupported SQLite query parameter '{key}'"))); }
    }
    if !query_mode { return Ok(Selector::Schema{table:table.into(),sample_limit:5}); }
    let positive = |value:Option<&str>,fallback:f64,offset:bool| -> Result<f64> {
        let Some(value)=value.filter(|s| !trim(s).is_empty()) else { return Ok(fallback); };
        let parsed=parse_int(&json!(value));
        if !parsed.is_finite() || parsed < if offset {0.0}else{1.0} {
            return Err(error(format!("SQLite {} must be a {} integer; got '{value}'", if offset {"offset"}else{"limit"},if offset {"non-negative"}else{"positive"})));
        }
        Ok(if offset {parsed}else{parsed.min(500.0)})
    };
    Ok(Selector::Query {table:table.into(),limit:positive(get("limit"),20.0,false)? as usize,offset:positive(get("offset"),0.0,true)?,order,where_sql})
}

pub struct ResolvedPath { pub absolute: PathBuf, pub sub_path: String, pub query_string: String, pub suffix_resolution: Option<(String,String)> }
fn has_magic(path:&Path) -> bool {
    let mut bytes=[0u8;16]; std::fs::File::open(path).and_then(|mut f|f.read_exact(&mut bytes)).is_ok() && &bytes==b"SQLite format 3\0"
}
fn missing(e:&std::io::Error)->bool { matches!(e.kind(), std::io::ErrorKind::NotFound|std::io::ErrorKind::NotADirectory) }
fn suffix_match(ctx:&ToolContext, raw:&str, cancel:&CancellationToken) -> Result<Option<(PathBuf,String)>> {
    let normalized=raw.replace('\\',"/"); let normalized=normalized.strip_prefix("./").unwrap_or(&normalized).trim_end_matches('/');
    if normalized.is_empty(){return Ok(None);}
    let escaped: String=normalized.chars().map(|c|if matches!(c,'*'|'?'|'['|'{'){format!("[{c}]")}else{c.to_string()}).collect();
    let matcher=crate::engine::compile_glob(&format!("**/{escaped}")).map_err(|e|error(format!("{e:?}")))?;
    let budget=ara_walk::Budget { cancel:cancel.clone(),deadline:Instant::now()+Duration::from_secs(5) };
    let mut found=Vec::new();
    let walked=ara_walk::walk(&ctx.cwd,ara_walk::WalkOptions {include_hidden:true,use_gitignore:true,skip_node_modules:!normalized.contains("node_modules"),max_depth:usize::MAX},&budget,|entry| {
        if matcher.is_match(&entry.relative){ found.push((entry.path,entry.relative)); }
        // Don't stop on ambiguity: original glob must observe final cancellation.
        ara_walk::Visit::Continue
    });
    aborted(cancel)?;
    if walked.is_err() || found.len()!=1 {return Ok(None);}
    Ok(found.pop())
}
/// One READ execute owns the context, cancellation and hit/miss memo.
/// Stages use these bound owners; no caller can supply a second context/token.
pub(crate) struct ScopedSuffixMemo<'a> {
    ctx: &'a ToolContext,
    cancel: &'a CancellationToken,
    cache: HashMap<String, Option<(PathBuf, String)>>,
}
impl<'a> ScopedSuffixMemo<'a> {
    pub(crate) fn new(ctx: &'a ToolContext, cancel: &'a CancellationToken) -> Self {
        Self { ctx, cancel, cache: HashMap::new() }
    }
    pub(crate) fn context(&self) -> &'a ToolContext { self.ctx }
    pub(crate) fn find(&mut self, raw: &str) -> Result<Option<(PathBuf, String)>> {
        if let Some(hit) = self.cache.get(raw) {
            return Ok(hit.clone());
        }
        let found = suffix_match(self.ctx, raw, self.cancel)?;
        self.cache.insert(raw.to_owned(), found.clone());
        Ok(found)
    }
    pub(crate) fn resolve(&mut self, input: &str) -> Result<Option<ResolvedPath>> {
        let ctx = self.ctx;
    for candidate in parse_path_candidates(input) {
        let absolute=ctx.resolve(&candidate.database_path);
        match std::fs::metadata(&absolute) {
            Ok(meta) if !meta.is_dir() && has_magic(&absolute) => return Ok(Some(ResolvedPath{absolute,sub_path:candidate.sub_path,query_string:candidate.query_string,suffix_resolution:None})),
            Ok(_)=>continue,
            Err(e) if !missing(&e)=>continue,
            Err(_)=>{}
        }
        if ctx.read_remote_mount_root().is_some_and(|root| absolute.starts_with(root) && absolute!=root) {continue;}
        let found = self.find(&candidate.database_path)?;
        let Some((absolute,display))=found else {continue;};
        match std::fs::metadata(&absolute) {
            Ok(meta) if !meta.is_dir() && has_magic(&absolute)=> return Ok(Some(ResolvedPath{absolute,sub_path:candidate.sub_path,query_string:candidate.query_string,suffix_resolution:Some((candidate.database_path,display))})),
            Ok(_)=>{}, Err(e) if missing(&e)=>{}, Err(e)=>return Err(error(e)),
        }
    }
    Ok(None)
    }
}

pub fn resolve(ctx:&ToolContext,input:&str,cancel:&CancellationToken)->Result<Option<ResolvedPath>> {
    ScopedSuffixMemo::new(ctx, cancel).resolve(input)
}

fn configure(db:Connection)->rusqlite::Result<Connection> {
    db.execute_batch("PRAGMA query_only = ON; PRAGMA busy_timeout = 3000;")?;
    db.query_row("SELECT name FROM sqlite_master LIMIT 1",[],|row|row.get::<_,String>(0)).optional()?;
    Ok(db)
}
pub fn open_read_connection(path:&Path)->Result<Connection> {
    let mut header=[0u8;20];
    let wal=std::fs::File::open(path).and_then(|mut f|f.read_exact(&mut header)).is_ok() && (header[18]==2 || header[19]==2);
    let sidecar=|suffix:&str| { let mut p=path.as_os_str().to_os_string();p.push(suffix);PathBuf::from(p).exists() };
    let rw=||configure(Connection::open_with_flags(path,OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(error)?).map_err(error);
    if wal && (!sidecar("-wal") || !sidecar("-shm")){return rw();}
    match Connection::open_with_flags(path,OpenFlags::SQLITE_OPEN_READ_ONLY).and_then(configure) {
        Ok(db)=>Ok(db),
        Err(rusqlite::Error::SqliteFailure(code,_)) if code.code==rusqlite::ErrorCode::CannotOpen=>rw(),
        Err(e)=>Err(error(e)),
    }
}
fn quoted(name:&str)->String {format!("\"{}\"",name.replace('"',"\"\""))}
#[derive(Debug,Clone)] struct Column {name:String,kind:String,pk:i64}
fn schema(db:&Connection,table:&str)->Result<(Option<String>,Vec<Column>)> {
    let sql:Option<Option<String>>=db.query_row("SELECT sql FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name = ?",[table],|r|r.get(0)).optional().map_err(error)?;
    let Some(sql)=sql else {return Err(error(format!("SQLite table '{table}' not found")));};
    let mut statement=db.prepare(&format!("PRAGMA table_info({})",quoted(table))).map_err(error)?;
    let columns=statement.query_map([],|r|Ok(Column{name:r.get(1)?,kind:r.get(2)?,pk:r.get(5)?})).map_err(error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(error)?;
    Ok((sql,columns))
}
type Row=Vec<(String,Value)>;
struct Page {columns:Vec<String>,rows:Vec<Row>,total:usize}
fn collect(statement:&mut rusqlite::Statement<'_>, values:impl rusqlite::Params, cap:Option<usize>)->Result<(Vec<String>,Vec<Row>,bool)> {
    let columns=statement.column_names().into_iter().map(str::to_owned).collect::<Vec<_>>();
    let mut rows=Vec::new();let mut cursor=statement.query(values).map_err(error)?;let mut capped=false;
    while let Some(row)=cursor.next().map_err(error)? {
        if cap.is_some_and(|cap|rows.len()>=cap){capped=true;break;}
        let mut fields:Row=Vec::new();
        for (i,name) in columns.iter().enumerate(){let value:Value=row.get(i).map_err(error)?;
            if let Some((_,old))=fields.iter_mut().find(|(key,_)|key==name){*old=value;}else{fields.push((name.clone(),value));}}
        // JS Object.entries enumerates canonical array-index names first;
        // SELECT columnNames still retain declaration order for tables.
        let array_index=|name:&str|name.parse::<u32>().ok().filter(|n|*n!=u32::MAX && n.to_string()==name);
        fields.sort_by(|(left,_),(right,_)|match(array_index(left),array_index(right)){
            (Some(l),Some(r))=>l.cmp(&r),(Some(_),None)=>std::cmp::Ordering::Less,(None,Some(_))=>std::cmp::Ordering::Greater,(None,None)=>std::cmp::Ordering::Equal});
        rows.push(fields);
    }
    Ok((columns,rows,capped))
}
fn query_rows(db:&Connection,table:&str,limit:usize,offset:f64,order:Option<&str>,where_sql:Option<&str>)->Result<Page> {
    let (_,columns)=schema(db,table)?;let where_sql=validate_where(where_sql)?;let where_clause=where_sql.map_or(String::new(),|w|format!(" WHERE {w}"));
    let order_clause=if let Some(order)=order.map(trim).filter(|s|!s.is_empty()) {
        let (column,direction)=order.rsplit_once(':').unwrap_or((order,"asc"));let direction=trim(direction).to_lowercase();
        if !columns.iter().any(|c|c.name==column){return Err(error(format!("SQLite order column '{column}' not found in table schema")));}
        if !matches!(direction.as_str(),"asc"|"desc"){return Err(error(format!("SQLite order direction must be 'asc' or 'desc'; got '{direction}'")));}
        format!(" ORDER BY {} {}",quoted(column),direction.to_uppercase())
    }else{String::new()};
    let total:usize=db.query_row(&format!("SELECT COUNT(*) AS count FROM {}{where_clause}",quoted(table)),[],|r|r.get(0)).map_err(error)?;
    let mut statement=db.prepare(&format!("SELECT * FROM {}{where_clause}{order_clause} LIMIT ? OFFSET ?",quoted(table))).map_err(error)?;
    if statement.parameter_count()!=2{return Err(error("SQLite where clause changed the expected pagination parameters; use q=SELECT ... for raw SQL"));}
    let offset_value=if offset<=9_007_199_254_740_991.0{Value::Integer(offset as i64)}else{Value::Real(offset)};
    let (_,rows,_)=collect(&mut statement,params![limit as i64,offset_value],None)?;
    Ok(Page{columns:columns.into_iter().map(|column|column.name).collect(),rows,total})
}
fn integer_key(key:&str,label:&str)->Result<Value> {
    let value=trim(key);let digits=value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|b|b.is_ascii_digit()){return Err(error(format!("{label} must be an integer; got '{key}'")));}
    let n=digits.bytes().fold(0u64,|n,b|n.wrapping_mul(10).wrapping_add(u64::from(b-b'0')));
    Ok(Value::Integer(if value.starts_with('-'){n.wrapping_neg() as i64}else{n as i64}))
}
/// JS Number rounds an unsigned power-of-two radix integer once, rather than
/// rounding after every digit. Keep this READ-local; shared/WRITE are unchanged.
fn lookup_number(key:&str)->f64 {
    let text=trim(key);
    let bits_per_digit=match text.get(..2) {
        Some("0x"|"0X")=>4usize,Some("0o"|"0O")=>3,Some("0b"|"0B")=>1,
        _=>return to_number(&json!(key)),
    };
    let radix=1u32<<bits_per_digit;
    let digits=&text[2..];
    if digits.is_empty(){return f64::NAN;}
    let mut bits=Vec::with_capacity(digits.len().saturating_mul(bits_per_digit));
    for digit in digits.chars() {
        let Some(value)=digit.to_digit(radix) else{return f64::NAN;};
        for shift in (0..bits_per_digit).rev(){bits.push(((value>>shift)&1) as u8);}
    }
    let Some(first)=bits.iter().position(|bit|*bit!=0) else{return 0.0;};
    let bits=&bits[first..];
    if bits.len()>1024{return f64::INFINITY;}
    let mut mantissa=bits.iter().take(53).fold(0u64,|value,bit|(value<<1)|u64::from(*bit));
    if bits.len()<=53{return mantissa as f64;}
    let guard=bits[53]!=0;
    let sticky=bits[54..].iter().any(|bit|*bit!=0);
    if guard && (sticky || mantissa&1!=0){mantissa+=1;}
    (mantissa as f64)*2.0f64.powi((bits.len()-53) as i32)
}
fn lookup_row(db:&Connection,table:&str,key:&str)->Result<Option<Row>> {
    let (sql,columns)=schema(db,table)?;let mut primary=columns.iter().filter(|c|c.pk>0).collect::<Vec<_>>();primary.sort_by_key(|c|c.pk);
    let (column,value)=match primary.as_slice(){
        [c]=>{let kind=trim(&c.kind).to_uppercase();let value=if kind.contains("INT"){integer_key(key,&format!("Primary key '{key}'"))?}
            else if kind.contains("REAL")||kind.contains("FLOA")||kind.contains("DOUB"){let n=lookup_number(key);if n.is_finite(){Value::Real(n)}else{Value::Text(key.into())}}else{Value::Text(key.into())};(quoted(&c.name),value)},
        []=>{static WITHOUT:LazyLock<regex::Regex>=LazyLock::new(||regex::Regex::new(r"(?i)\bWITHOUT\s+ROWID\b").unwrap());
            if sql.as_deref().is_some_and(|s|WITHOUT.is_match(s)){return Err(error(format!("SQLite table '{table}' does not expose ROWID; use '?where=' instead")));}
            ("rowid".into(),integer_key(key,"SQLite ROWID")?)},
        _=>return Err(error(format!("SQLite table '{table}' has a composite primary key; use '?where=' instead"))),
    };
    let mut statement=db.prepare(&format!("SELECT * FROM {} WHERE {column} = ? LIMIT 1",quoted(table))).map_err(error)?;
    let (_,mut rows,_)=collect(&mut statement,params![value],None)?;Ok(rows.pop())
}

#[derive(Debug,Clone,PartialEq,Eq)] pub enum TableCount {Exact(usize),Estimate(usize),AtLeast(usize)}
pub fn list_tables(db:&Connection,cap:usize)->Result<Vec<(String,TableCount)>> {
    let mut names=db.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name COLLATE NOCASE").map_err(error)?;
    let names=names.query_map([],|r|r.get::<_,String>(0)).map_err(error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(error)?;
    let has_stat:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'sqlite_stat1')",[],|r|r.get(0)).map_err(error)?;
    let mut estimates=HashMap::<String,usize>::new();
    if has_stat {let mut stmt=db.prepare("SELECT tbl, stat FROM sqlite_stat1").map_err(error)?;let mut cursor=stmt.query([]).map_err(error)?;
        while let Some(row)=cursor.next().map_err(error)?{let table:String=row.get(0).map_err(error)?;let stat:Option<String>=row.get(1).map_err(error)?;
            if let Some(stat)=stat {let n=parse_int(&json!(stat));if n.is_finite()&&n>=0.0 {let n=n as usize;estimates.entry(table).and_modify(|old|*old=(*old).max(n)).or_insert(n);}}}}
    names.into_iter().map(|name|{
        let count=if let Some(estimate)=estimates.get(&name).filter(|n|**n>cap){TableCount::Estimate(*estimate)}else{
            let n:usize=db.query_row(&format!("SELECT COUNT(*) AS count FROM (SELECT 1 FROM {} LIMIT {})",quoted(&name),cap+1),[],|r|r.get(0)).map_err(error)?;
            if n>cap {TableCount::AtLeast(cap)}else{TableCount::Exact(n)}};Ok((name,count))}).collect()
}
fn sanitize(s:&str)->String{s.replace('\t',"   ").replace("\r\n","\\n").replace('\n',"\\n")}
// READ-local exact JS byte formatter is supplied by Root's separately pinned
// composition packet; accepted shared formatting/WRITE remain untouched.
fn stringify(value:&Value)->String{match value{Value::Null=>"NULL".into(),Value::Integer(n)=>ara_prompt::js::to_string(&ara_prompt::js::number(*n as f64)),Value::Real(n)=>if n.is_nan(){"NaN".into()}else if n.is_infinite(){if n.is_sign_positive(){"Infinity".into()}else{"-Infinity".into()}}else{ara_prompt::js::to_string(&json!(n))},Value::Text(s)=>s.clone(),Value::Blob(b)=>format!("<BLOB {}>",crate::read::format_bytes(b.len() as u64))}}
fn pad(metrics:&dyn ReadTextMetrics,text:&str,width:usize)->Result<String>{let value=metrics.truncate(&sanitize(text),width.max(3))?;let cells=metrics.width(&value)?;Ok(format!("{value}{}"," ".repeat(width.saturating_sub(cells))))}
fn cell<'a>(row:&'a Row,column:&str)->Option<&'a Value>{row.iter().find(|(name,_)|name==column).map(|(_,value)|value)}
pub fn render_table(metrics:&dyn ReadTextMetrics,columns:&[String],rows:&[Row])->Result<String>{
    if columns.is_empty(){return Ok(if rows.is_empty(){"(no rows)"}else{"(rows returned without named columns)"}.into());}
    if columns.len()*6+1>120{
        if rows.is_empty(){return Ok("(no rows)".into());}
        let name_width=columns.iter().try_fold(3usize,|w,name|Ok::<_,ToolError>(w.max(metrics.width(&sanitize(name))?)))?.min(40);
        let mut blocks=Vec::new();for (index,row) in rows.iter().enumerate(){let mut lines=vec![format!("── Row {} ──",index+1)];
            for column in columns{let name=pad(metrics,column,name_width)?;let value=cell(row,column).map(stringify).unwrap_or_default();lines.push(metrics.truncate(&format!("{name}: {}",sanitize(&value)),120)?);}blocks.push(lines.join("\n"));}return Ok(blocks.join("\n\n"));
    }
    let mut widths=columns.iter().map(|name|Ok(metrics.width(&sanitize(name))?.clamp(3,40))).collect::<Result<Vec<_>>>()?;
    for row in rows {for (i,column) in columns.iter().enumerate(){let value=cell(row,column).map(stringify).unwrap_or_default();widths[i]=widths[i].max(metrics.width(&sanitize(&value))?.min(40));}}
    let overhead=columns.len()*3+1;while widths.iter().sum::<usize>()+overhead>120 {
        let widest=widths.iter().enumerate().filter(|(_,w)|**w>3).max_by(|(li,l),(ri,r)|l.cmp(r).then_with(||ri.cmp(li))).map(|(i,_)|i);
        let Some(i)=widest else{break;};widths[i]-=1;
    }
    let header=columns.iter().zip(&widths).map(|(name,w)|pad(metrics,name,*w)).collect::<Result<Vec<_>>>()?;
    let mut lines=vec![format!("| {} |",header.join(" | ")),format!("| {} |",widths.iter().map(|w|"-".repeat(*w)).collect::<Vec<_>>().join(" | "))];
    if rows.is_empty(){lines.push("(no rows)".into());}
    for row in rows{let cells=columns.iter().zip(&widths).map(|(column,w)|pad(metrics,&cell(row,column).map(stringify).unwrap_or_default(),*w)).collect::<Result<Vec<_>>>()?;lines.push(format!("| {} |",cells.join(" | ")));}
    Ok(lines.iter().map(|line|metrics.truncate(&line.replace('\t',"   "),120)).collect::<Result<Vec<_>>>()?.join("\n"))
}
pub fn render_table_list(metrics:&dyn ReadTextMetrics,tables:&[(String,TableCount)])->Result<String>{
    if tables.is_empty(){return Ok("(no tables)".into());}
    tables.iter().map(|(name,count)|{let count=match count{TableCount::Exact(n)=>format!("{n} rows"),TableCount::Estimate(n)=>format!("~{n} rows"),TableCount::AtLeast(n)=>format!("{n}+ rows")};metrics.truncate(&format!("{name} ({count})").replace('\t',"   "),120)}).collect::<Result<Vec<_>>>().map(|lines|lines.join("\n"))
}
fn render_page(metrics:&dyn ReadTextMetrics,page:&Page,offset:f64,limit:usize,table:&str)->Result<String>{
    let mut text=render_table(metrics,&page.columns,&page.rows)?;let next=offset+page.rows.len() as f64;let shown=(page.total as f64).min(next);
    if shown<(page.total as f64){text.push('\n');text.push_str(&metrics.truncate(&format!("[{} more rows; append :{table}?limit={limit}&offset={} to the database path to continue]",ara_prompt::js::to_string(&ara_prompt::js::number(page.total as f64-shown)),ara_prompt::js::to_string(&ara_prompt::js::number(next))).replace('\t',"   "),120)?);}Ok(text)
}
pub fn read(ctx:&ToolContext,path:&ResolvedPath,cancel:&CancellationToken)->Result<ToolOutput>{
    aborted(cancel)?;let selector=parse_selector(&path.sub_path,&path.query_string)?;let db=open_read_connection(&path.absolute)?;aborted(cancel)?;
    let supplied=ctx.read_text_metrics();let metrics:&dyn ReadTextMetrics=supplied.as_deref().unwrap_or(&AsciiReadTextMetrics);
    let mut details=json!({"resolvedPath":path.absolute.to_string_lossy(),"meta":{"source":{"type":"path","value":path.absolute.to_string_lossy()}}});
    if let Some((from,to))=&path.suffix_resolution{details["suffixResolution"]=json!({"from":from,"to":to});}
    let prefix=|text:String|path.suffix_resolution.as_ref().map_or(text.clone(),|(from,to)|format!("[Path '{from}' not found; resolved to '{to}' via suffix match]{}{}",if text.is_empty(){""}else{"\n"},text));
    let text=match selector {
        Selector::List=>{let tables=list_tables(&db,50_000)?;let reached=tables.len()>500;let text=prefix(render_table_list(metrics,&tables[..tables.len().min(500)])?);let truncation=output::truncate_head(&text,usize::MAX,DEFAULT_MAX_BYTES);
            if reached{details["meta"]["limits"]["resultLimit"]=json!({"reached":500,"suggestion":1000});}
            if truncation.truncated{details["truncation"]=truncation.details();details["meta"]["truncation"]=json!({"direction":"head","truncatedBy":if truncation.by_bytes{"bytes"}else{"lines"},"totalLines":truncation.total_lines,"totalBytes":truncation.total_bytes,"outputLines":truncation.output_lines,"outputBytes":truncation.output_bytes,"shownRange":{"start":1,"end":truncation.output_lines},"nextOffset":truncation.output_lines+1});}return Ok(ToolOutput::text(truncation.content).with_details(details));},
        Selector::Schema{table,sample_limit}=>{let page=query_rows(&db,&table,sample_limit,0.0,None,None)?;let sql=schema(&db,&table)?.0.ok_or_else(||error(format!("SQLite schema for table '{table}' is unavailable")))?;
            let mut text=sql.replace('\t',"   ").split('\n').map(|line|metrics.truncate(line,120)).collect::<Result<Vec<_>>>()?.join("\n");text.push_str("\n\nSample rows:\n");text.push_str(&render_table(metrics,&page.columns,&page.rows)?);
            if page.rows.len()<page.total{text.push_str(&format!("\n[{} more rows; append :{table}?limit=20&offset={} to the database path to continue]",page.total-page.rows.len(),page.rows.len()));}text},
        Selector::Row{table,key}=>match lookup_row(&db,&table,&key)?{
            None=>format!("No row found in table '{table}' for key '{key}'."),Some(row)=>if row.is_empty(){"(no columns)".into()}else{row.iter().map(|(column,value)|metrics.truncate(&format!("{column}: {}",stringify(value)).replace('\t',"   "),120)).collect::<Result<Vec<_>>>()?.join("\n")}},
        Selector::Query{table,limit,offset,order,where_sql}=>{let page=query_rows(&db,&table,limit,offset,order.as_deref(),where_sql.as_deref())?;render_page(metrics,&page,offset,limit,&table)?},
        Selector::Raw{sql}=>{let mut stmt=db.prepare(&sql).map_err(error)?;if stmt.parameter_count()>0{return Err(error("SQLite raw queries do not support bound parameters"));}let (columns,rows,capped)=collect(&mut stmt,[],Some(1000))?;
            let mut text=render_table(metrics,&columns,&rows)?;if capped{text.push_str("\n[Output capped at 1000 rows; add a LIMIT/OFFSET clause to the query to page through more]");}text},
    };
    Ok(ToolOutput::text(prefix(text)).with_details(details))
}
