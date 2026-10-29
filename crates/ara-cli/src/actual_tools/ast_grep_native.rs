//! G09 lang getters execute on the existing admitted original Graph driver.
//! The native worker receives optional UTF8 strings only; no Graph capture.
use super::*;

pub(super) struct NativeAstGrepLanguage(pub(super) async_effect::NativeEffectPublisher);
#[async_trait]
impl ara_tools::ast_grep::AstGrepLanguagePort for NativeAstGrepLanguage {
    async fn language(&self,explicit:bool)->Result<Option<String>,ToolError> {
        self.0.ast_grep_language(explicit).await
    }
}
impl InvocationRuntime {
    pub(super) fn ast_grep_language_original(&self,argument:&ValueHandle,effective:&dyn ValueRuntime)->Result<ValueHandle,ValueError> {
        let run=self.run.as_ref().and_then(Weak::upgrade).ok_or(ValueError::ExpiredCapability(self.completion))?;
        let heap=self.request.values.heap();
        {
            let state=self.state.lock().unwrap_or_else(|e|e.into_inner());
            let native=state.native.as_ref().ok_or(ValueError::ExpiredCapability(self.completion))?;
            if state.phase!=Phase::Running || native.kernel_completed || !matches!(&self.owner.members[self.index].kernel,Kernel::AstGrep(_)) {
                return Err(ValueError::ExpiredCapability(self.completion));
            }
        }
        let ValueSnapshot::Boolean(explicit)=heap.snapshot(argument)? else {return Err(ValueError::WrongRuntimeReply("AST language read phase"));};
        let result: Result<ValueHandle, ActualProviderError>=(|| {
            self.check_not_aborted()?;
            if explicit {
                let condition=self.get("lang",effective)?;
                if !heap.truthy(&condition)? {return Ok(heap.undefined()?);}
            }
            // Explicit install performs a second Get only when truthy. Each
            // search closure performs a new single Get, including its rerun.
            let language=self.get("lang",effective)?;
            match self.path_string(&language,effective)? {
                Some(language)=>Ok(heap.string(language.into())?),
                None=>Ok(heap.undefined()?),
            }
        })();
        match result {
            Ok(value)=>Ok(value),
            Err(error)=>{
                run.fail(error.clone());
                match error {ActualProviderError::Value(error)=>Err(error),_=>Err(ValueError::UnsupportedOperation("AST language original failure retained"))}
            }
        }
    }
}

#[cfg(test)]
#[path = "ast_grep_native_tests.rs"]
mod tests;
