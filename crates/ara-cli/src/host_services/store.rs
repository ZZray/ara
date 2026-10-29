//! Private project service files and process-held leases. OMP launch paths,
//! broker metadata/presence contracts at b91db66e; MIT attribution retained.
use super::protocol::*;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{fs::{self, File, OpenOptions}, io::Write, path::{Path,PathBuf}};

#[derive(Clone, Debug)]
pub struct ProjectScope { pub project: PathBuf, pub runtime: PathBuf, pub endpoint: String }
impl ProjectScope {
    pub fn new(project: &Path, runtime_root: &Path) -> Result<Self> {
        if !runtime_root.is_absolute(){bail!("Service runtime root must be absolute");}
        let project=fs::canonicalize(project).context("canonical service project")?;
        if !project.is_dir() { bail!("Service project is not a directory"); }
        let raw=project.to_string_lossy();
        let normalized=if cfg!(windows) { raw.to_lowercase() } else { raw.into_owned() };
        let hash=ring::digest::digest(&ring::digest::SHA256,normalized.as_bytes());
        let key=hash.as_ref()[..16].iter().map(|b|format!("{b:02x}")).collect::<String>();
        let runtime=runtime_root.join(&key);
        private_dir(&runtime)?;
        #[cfg(windows)] let endpoint=format!(r"\\.\pipe\ara-services-{key}");
        #[cfg(unix)] let endpoint=runtime.join("broker.sock").to_string_lossy().into_owned();
        Ok(Self{project,runtime,endpoint})
    }
    pub fn token(&self) -> Result<String> {
        let path=self.runtime.join("token");
        reject_link(&path)?;
        if !path.exists(){
            // Publish a completely written token without a create-then-read
            // race and without overwriting another concurrent client's token.
            let temp=self.runtime.join(format!(".token-{}",uuid::Uuid::new_v4()));
            let result=(||->Result<()>{
                let mut file=private_new(&temp)?;let token=format!("{}{}",uuid::Uuid::new_v4(),uuid::Uuid::new_v4());
                file.write_all(token.as_bytes())?;file.sync_all()?;drop(file);
                match fs::hard_link(&temp,&path){Ok(())=>Ok(()),Err(e)if e.kind()==std::io::ErrorKind::AlreadyExists=>Ok(()),Err(e)=>Err(e.into())}
            })();let _=fs::remove_file(&temp);result?;
        }
        reject_link(&path)?;let token=fs::read_to_string(&path)?;
        if token.len()!=72||!token.chars().all(|c|c.is_ascii_hexdigit()||c=='-'){bail!("Invalid service authentication token");}
        Ok(token)
    }
}
pub(crate) fn reject_link(path:&Path)->Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta)=>{
            if meta.file_type().is_symlink() { bail!("Service runtime link refused: {}",path.display()); }
            #[cfg(windows)] {
                use std::os::windows::fs::MetadataExt;
                if meta.file_attributes() & 0x400 != 0 { bail!("Service runtime reparse point refused"); }
            }
        },
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{},
        Err(error)=>return Err(error.into()),
    }
    Ok(())
}
pub(crate) fn private_dir(path:&Path)->Result<()> {
    if let Some(parent)=path.parent() { fs::create_dir_all(parent)?; }
    reject_link(path)?;fs::create_dir_all(path)?;
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path,fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)] protect_windows(path)?;
    Ok(())
}
fn private_new(path:&Path)->std::io::Result<File> {
    let mut options=OpenOptions::new();options.write(true).create_new(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt;options.mode(0o600); }
    let file=options.open(path)?;
    #[cfg(windows)] protect_windows(path).map_err(|e|std::io::Error::other(e.to_string()))?;
    Ok(file)
}
#[cfg(windows)]
fn protect_windows(path:&Path)->Result<()> {
    use std::{os::windows::ffi::OsStrExt,ptr};
    use windows_sys::Win32::{Foundation::LocalFree,Security::{Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW,SetNamedSecurityInfoW,SE_FILE_OBJECT},GetSecurityDescriptorDacl,DACL_SECURITY_INFORMATION,PROTECTED_DACL_SECURITY_INFORMATION}};
    let sddl:Vec<u16>="D:P(A;;FA;;;OW)(A;;FA;;;SY)\0".encode_utf16().collect();
    let mut descriptor=ptr::null_mut();
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(),1,&mut descriptor,ptr::null_mut())==0 {return Err(std::io::Error::last_os_error().into());}
        let mut present=0;let mut defaulted=0;let mut acl=ptr::null_mut();
        let parsed=GetSecurityDescriptorDacl(descriptor,&mut present,&mut acl,&mut defaulted);
        let wide:Vec<u16>=path.as_os_str().encode_wide().chain(Some(0)).collect();
        let result=if parsed==0 || present==0 {std::io::Error::last_os_error().raw_os_error().unwrap_or(5) as u32}
            else {SetNamedSecurityInfoW(wide.as_ptr(),SE_FILE_OBJECT,DACL_SECURITY_INFORMATION|PROTECTED_DACL_SECURITY_INFORMATION,ptr::null_mut(),ptr::null_mut(),acl,ptr::null_mut())};
        LocalFree(descriptor);
        if result!=0 {return Err(std::io::Error::from_raw_os_error(result as i32).into());}
    }
    Ok(())
}
pub(crate) fn atomic_json<T:Serialize>(path:&Path,value:&T)->Result<()> {
    reject_link(path)?;
    let parent=path.parent().context("service file parent")?;private_dir(parent)?;
    let temp=parent.join(format!(".{}.tmp",uuid::Uuid::new_v4()));
    let result=(||{
        let mut file=private_new(&temp)?;file.write_all(&serde_json::to_vec(value)?)?;file.sync_all()?;drop(file);
        #[cfg(unix)] fs::rename(&temp,path)?;
        #[cfg(windows)] {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Storage::FileSystem::{MoveFileExW,MOVEFILE_REPLACE_EXISTING,MOVEFILE_WRITE_THROUGH};
            let a:Vec<u16>=temp.as_os_str().encode_wide().chain(Some(0)).collect();let b:Vec<u16>=path.as_os_str().encode_wide().chain(Some(0)).collect();
            if unsafe{MoveFileExW(a.as_ptr(),b.as_ptr(),MOVEFILE_REPLACE_EXISTING|MOVEFILE_WRITE_THROUGH)}==0{return Err(std::io::Error::last_os_error().into());}
        }
        Ok(())
    })();
    if result.is_err(){let _=fs::remove_file(&temp);}result
}
pub(crate) struct BrokerLease { _file:File }
impl BrokerLease {
    pub fn acquire(scope:&ProjectScope)->Result<Self>{
        let path=scope.runtime.join("broker.lock");reject_link(&path)?;
        let mut opts=OpenOptions::new();opts.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]{use std::os::windows::fs::OpenOptionsExt;opts.share_mode(0);}
        #[cfg(unix)]{use std::os::unix::fs::OpenOptionsExt;opts.mode(0o600);}
        let file=opts.open(path)?;
        #[cfg(unix)]{
            use std::os::fd::AsRawFd;
            if unsafe{libc::flock(file.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)}!=0{return Err(std::io::Error::last_os_error().into());}
        }
        Ok(Self{_file:file})
    }
}
#[derive(Clone,Debug,Serialize,Deserialize)]
pub(crate) struct StoredRecord {
    pub spec:ServiceSpec,pub snapshot:ServiceSnapshot,pub process:Option<ProcessIdentity>,
    pub completion_capable:bool,pub subscription:Option<String>,pub pending:Vec<Completion>,
}
#[derive(Clone)] pub(crate) struct Store {pub scope:ProjectScope}
impl Store {
    pub fn new(scope:ProjectScope)->Result<Self>{private_dir(&scope.runtime.join("services"))?;Ok(Self{scope})}
    pub fn dir(&self,name:&str)->Result<PathBuf>{validate_name(name).map_err(anyhow::Error::msg)?;let p=self.scope.runtime.join("services").join(name);private_dir(&p)?;Ok(p)}
    pub fn save(&self,record:&StoredRecord)->Result<()>{
        let dir=self.dir(&record.spec.name)?;
        atomic_json(&dir.join("spec.json"),&record.spec)?;
        atomic_json(&dir.join("state.json"),record)
    }
    pub fn load(&self)->Result<Vec<StoredRecord>>{
        let mut records=Vec::new();
        for entry in fs::read_dir(self.scope.runtime.join("services"))?{
            let entry=entry?;if !entry.file_type()?.is_dir(){continue;}
            let name=entry.file_name().to_string_lossy().into_owned();validate_name(&name).map_err(anyhow::Error::msg)?;
            let path=entry.path().join("state.json");reject_link(&entry.path())?;reject_link(&path)?;
            if !path.exists(){continue;}
            let record:StoredRecord=serde_json::from_slice(&fs::read(&path)?)?;
            record.spec.validate_structure().map_err(anyhow::Error::msg)?;
            if record.spec.name!=name||record.snapshot.name!=name{bail!("Service metadata identity mismatch");}
            records.push(record);
        }Ok(records)
    }
}
#[derive(Serialize,Deserialize)] struct PresenceRecord {identity:ProcessIdentity,project:PathBuf}
pub(crate) struct Presence { path:PathBuf }
impl Presence {
    pub fn new(scope:&ProjectScope)->Result<Self>{
        let dir=scope.runtime.join("clients");private_dir(&dir)?;
        let identity=super::process::inspect_identity(std::process::id())?.context("current service client identity unavailable")?;
        let path=dir.join(format!("{}-{}.json",identity.pid,uuid::Uuid::new_v4()));
        atomic_json(&path,&PresenceRecord{identity,project:scope.project.clone()})?;Ok(Self{path})
    }
}
impl Drop for Presence {fn drop(&mut self){let _=fs::remove_file(&self.path);}}
pub(crate) fn live_presence(scope:&ProjectScope)->Result<bool>{
    let dir=scope.runtime.join("clients");private_dir(&dir)?;let mut live=false;
    for entry in fs::read_dir(dir)?{
        let path=entry?.path();reject_link(&path)?;
        let record:PresenceRecord=serde_json::from_slice(&fs::read(&path)?)?;
        if record.project!=scope.project{bail!("Service presence project mismatch");}
        if super::process::inspect_identity(record.identity.pid)?.as_ref()==Some(&record.identity){live=true;}else{fs::remove_file(path)?;}
    }Ok(live)
}
