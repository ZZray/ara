//! Consumed fixed OMP b91 Task Git facade. MIT; see LICENSE.
//! Complete original Git modules/types/errors; filesystem Jujutsu discovery only.
pub mod error;
pub mod git;
pub mod types;
mod jj_discovery;
pub use error::{Error,Result};
pub use types::*;
/// Preserve nearest-root pure-jj rejection without loading jj-lib operations.
pub fn is_pure_jj(dir:&std::path::Path)->Result<bool>{
 let Some(jj)=jj_discovery::JjWorkspace::discover(dir)? else{return Ok(false)};
 Ok(match git::GitRepo::discover(dir)?{Some(git)=>jj.root()!=git.root()&&jj.root().starts_with(git.root()),None=>true})
}
#[cfg(test)] mod task_facade_tests;
