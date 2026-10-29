//! Authenticated project service client. OMP launch/client.ts (b91db66e, MIT).
//! Rust uses bounded polling for notifications, retaining source sink/ACK order.
use super::{protocol::*,store::{Presence,ProjectScope},ServicePolicy};
use std::{path::{Path,PathBuf},sync::{Arc,Mutex},time::Duration};
use tokio::io::{AsyncBufReadExt,AsyncRead,AsyncReadExt,AsyncWrite,AsyncWriteExt,BufReader};
use tokio_util::sync::CancellationToken;
use tokio::time::Instant;

#[derive(Clone,Debug)]
pub enum ServiceError {
    NotDispatched{request_id:String,message:String},
    OutcomeUnknown{request_id:String,message:String},
    Rejected{request_id:String,code:String,message:String},
}
impl std::fmt::Display for ServiceError{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{match self{
    Self::NotDispatched{request_id,message}=>write!(f,"service request {request_id} not dispatched: {message}"),
    Self::OutcomeUnknown{request_id,message}=>write!(f,"service request {request_id} outcome unknown: {message}; do not replay"),
    Self::Rejected{request_id,code,message}=>write!(f,"service request {request_id} {code}: {message}"),
}}}
impl std::error::Error for ServiceError{}
impl From<ServiceError> for crate::actual_tools::async_effect::NativeToolFault {
    fn from(error:ServiceError)->Self{let text=error.to_string();match error{
        ServiceError::NotDispatched{..}=>Self::NotDispatched(text),
        ServiceError::OutcomeUnknown{..}=>Self::OutcomeUnknown(text),
        ServiceError::Rejected{..}=>Self::Kernel(text),
    }}
}

pub struct ProjectServiceClient {
    pub scope:ProjectScope,
    token:String,executable:PathBuf,policy:ServicePolicy,
    ensure:tokio::sync::Mutex<()>,presence:Mutex<Option<Presence>>,
}
impl ProjectServiceClient {
    pub fn new(project:&Path,runtime_root:&Path,executable:PathBuf,policy:ServicePolicy)->anyhow::Result<Arc<Self>>{
        let scope=ProjectScope::new(project,runtime_root)?;let token=scope.token()?;let presence=Presence::new(&scope)?;
        Ok(Arc::new(Self{scope,token,executable,policy,ensure:tokio::sync::Mutex::new(()),presence:Mutex::new(Some(presence))}))
    }
    async fn connect(&self, deadline:Instant)->std::io::Result<(Box<dyn AsyncRead+Unpin+Send>,Box<dyn AsyncWrite+Unpin+Send>)>{
        #[cfg(windows)]{
            // Like the existing LSP mux dialer: an occupied instance is a
            // pre-dispatch open race, not a failed or replayable request.
            loop {
                match tokio::net::windows::named_pipe::ClientOptions::new().open(&self.scope.endpoint) {
                    Ok(pipe)=>{let(reader,writer)=tokio::io::split(pipe);return Ok((Box::new(reader),Box::new(writer)));},
                    Err(error) if error.raw_os_error()==Some(231)&&Instant::now()<deadline=>{
                        tokio::time::sleep(Duration::from_millis(30)).await;
                    },
                    Err(error)=>return Err(error),
                }
            }
        }
        #[cfg(unix)]{
            let _=deadline; // The request's outer timeout also owns Unix connect.
            let stream=tokio::net::UnixStream::connect(&self.scope.endpoint).await?;let(reader,writer)=stream.into_split();Ok((Box::new(reader),Box::new(writer)))
        }
    }
    async fn ensure_broker(&self,deadline:Instant)->anyhow::Result<(Box<dyn AsyncRead+Unpin+Send>,Box<dyn AsyncWrite+Unpin+Send>)>{
        let _guard=self.ensure.lock().await;
        match self.connect(deadline).await {
            Ok(pair)=>return Ok(pair),
            Err(error) if matches!(error.kind(),std::io::ErrorKind::NotFound|std::io::ErrorKind::ConnectionRefused)=>{},
            Err(error)=>return Err(error.into()),
        }
        let mut command=tokio::process::Command::new(&self.executable);
        command.env_clear().envs(super::process::worker_environment(&Default::default()));
        command.arg("__ara_worker_services").env("ARA_SERVICE_PROJECT",&self.scope.project)
            .env("ARA_SERVICE_RUNTIME_ROOT",self.scope.runtime.parent().ok_or_else(||anyhow::anyhow!("Service runtime parent missing"))?)
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).kill_on_drop(false);
        #[cfg(windows)]command.creation_flags(0x00000008|0x00000200);
        #[cfg(unix)]unsafe{command.pre_exec(||{if libc::setsid()<0{Err(std::io::Error::last_os_error())}else{Ok(())}});}
        let mut child=command.spawn()?;
        tokio::spawn(async move{let _=child.wait().await;});
        let startup_deadline=deadline.min(Instant::now()+Duration::from_secs(5));
        loop {
            match self.connect(startup_deadline).await {
                // Keep this ready connection for the actual request. Opening
                // and dropping a probe consumed the sole pipe instance and
                // left the immediately following open racing server accept.
                Ok(pair)=>return Ok(pair),
                Err(error) if matches!(error.kind(),std::io::ErrorKind::NotFound|std::io::ErrorKind::ConnectionRefused)
                    &&Instant::now()<startup_deadline=>tokio::time::sleep(Duration::from_millis(50)).await,
                Err(error)=>return Err(error.into()),
            }
        }
    }
    pub async fn request(&self,operation:Operation,cancel:&CancellationToken)->Result<Reply,ServiceError>{
        let id=uuid::Uuid::new_v4().to_string();
        let not_sent=|message:String|ServiceError::NotDispatched{request_id:id.clone(),message};
        if cancel.is_cancelled(){return Err(not_sent("caller cancelled before submission".into()));}
        let mutation=operation.mutation();
        let timeout=if mutation{Duration::from_secs(30)}else{Duration::from_secs(10)};
        let deadline=Instant::now()+timeout;
        let request=Request{version:VERSION,id:id.clone(),token:self.token.clone(),operation};
        let mut bytes=serde_json::to_vec(&request).map_err(|e|not_sent(e.to_string()))?;
        if bytes.len()>=MAX_FRAME{return Err(not_sent("service frame exceeds limit".into()));}bytes.push(b'\n');
        let establish=self.ensure_broker(deadline);
        let(reader,mut writer)=tokio::select!{
            biased;
            _=cancel.cancelled()=>return Err(not_sent("caller cancelled before submission".into())),
            result=tokio::time::timeout_at(deadline,establish)=>match result{
                Ok(result)=>result.map_err(|e|not_sent(format!("{e:#}")))?,
                Err(_)=>return Err(not_sent("service connection timed out before submission".into())),
            },
        };
        // From the first transport write onward, cancellation, partial write,
        // timeout and disconnect cannot establish mutation absence.
        let uncertain=|message:String|if mutation{ServiceError::OutcomeUnknown{request_id:id.clone(),message}}else{not_sent(message)};
        let exchange=async{
            writer.write_all(&bytes).await?;writer.flush().await?;
            let mut body=Vec::new();let mut reader=BufReader::new(reader.take((MAX_FRAME+1)as u64));
            let n=reader.read_until(b'\n',&mut body).await?;
            if n==0||body.len()>MAX_FRAME{return Err(std::io::Error::other("service response missing or oversized"));}
            let response:Response=serde_json::from_slice(&body).map_err(std::io::Error::other)?;
            Ok::<_,std::io::Error>(response)
        };
        let response=tokio::select!{
            biased;
            _=cancel.cancelled()=>return Err(uncertain("caller cancelled after transport submission".into())),
            result=tokio::time::timeout_at(deadline,exchange)=>match result{
                Ok(Ok(v))=>v,Ok(Err(e))=>return Err(uncertain(e.to_string())),Err(_)=>return Err(uncertain("broker response timed out".into())),
            },
        };
        if response.version!=VERSION||response.id!=id{return Err(uncertain("service response correlation mismatch".into()));}
        match(response.result,response.error){
            (Some(reply),None) if mutation&&!matches!(&reply,Reply::Service{..})=>Err(uncertain("Unexpected typed mutation reply; effect cannot be inferred".into())),
            (Some(reply),None)=>Ok(reply),
            (None,Some(error)) if error.outcome_unknown=>Err(ServiceError::OutcomeUnknown{request_id:id,message:error.message}),
            (None,Some(error))=>Err(ServiceError::Rejected{request_id:id,code:error.code,message:error.message}),
            _=>Err(uncertain("invalid service result/error envelope".into())),
        }
    }
    pub async fn list(&self,cancel:&CancellationToken)->Result<Vec<ServiceSnapshot>,ServiceError>{
        match self.request(Operation::List,cancel).await?{Reply::List{services}=>Ok(services),_=>Err(self.unexpected())}
    }
    pub async fn find(&self,name:&str,cancel:&CancellationToken)->Result<Option<ServiceSnapshot>,ServiceError>{
        match self.request(Operation::Find{name:name.into()},cancel).await{
            Ok(Reply::Service{daemon})=>Ok(Some(daemon)),
            Err(ServiceError::Rejected{code,..})if code=="not_found"=>Ok(None),
            Err(error)=>Err(error),_=>Err(self.unexpected()),
        }
    }
    pub async fn list_tolerant(&self,cancel:&CancellationToken)->Result<(Vec<ServiceSnapshot>,Option<String>),ServiceError>{
        match self.list(cancel).await{Ok(v)=>Ok((v,None)),Err(error) if cancel.is_cancelled()=>Err(error),Err(error)=>Ok((Vec::new(),Some(error.to_string())))}
    }
    pub async fn start(&self,spec:ServiceSpec,owner:Option<String>,replace:bool,cancel:&CancellationToken)->Result<ServiceSnapshot,ServiceError>{
        if !self.policy.launch_enabled{return Err(ServiceError::Rejected{request_id:String::new(),code:"disabled".into(),message:"Service launch is disabled in this Host".into()});}
        spec.validate().map_err(|message|ServiceError::NotDispatched{request_id:String::new(),message})?;
        self.service(Operation::Start{spec,owner,replace},cancel).await
    }
    pub async fn send(&self,name:&str,content:&str,cancel:&CancellationToken)->Result<ServiceSnapshot,ServiceError>{
        let data=if content.ends_with('\r')||content.ends_with('\n'){content.to_owned()}else{format!("{content}\r")};
        self.service(Operation::Send{name:name.into(),data},cancel).await
    }
    pub async fn mode(&self,name:&str,mode:ServiceMode,cancel:&CancellationToken)->Result<ServiceSnapshot,ServiceError>{
        if mode==ServiceMode::Detached&&super::shared_browser_name(name){return Err(ServiceError::Rejected{request_id:String::new(),code:"shared_browser".into(),message:"Project-shared browser cannot be detached".into()});}
        self.service(Operation::Mode{name:name.into(),mode},cancel).await
    }
    pub async fn stop(&self,name:&str,cancel:&CancellationToken)->Result<ServiceSnapshot,ServiceError>{self.service(Operation::Stop{name:name.into(),timeout_ms:5000},cancel).await}
    async fn service(&self,operation:Operation,cancel:&CancellationToken)->Result<ServiceSnapshot,ServiceError>{match self.request(operation,cancel).await?{Reply::Service{daemon}=>Ok(daemon),_=>Err(ServiceError::OutcomeUnknown{request_id:String::new(),message:"Unexpected typed mutation result".into()})}}
    fn unexpected(&self)->ServiceError{ServiceError::NotDispatched{request_id:String::new(),message:"Unexpected typed service result".into()}}
    /// Releases this exact project's process presence. Detached/persistent
    /// lifetime remains the broker's decision, never kill-on-drop.
    pub fn close_presence(&self){self.presence.lock().unwrap().take();}
}
