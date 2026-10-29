//! Fixed OMP 596f2da path-utils.ts async scope locals, owned by the existing
//! InvocationRuntime. No realm, Run, Session or graph driver is created here.
//! Worker futures carry native scalars and a native receipt slot only.

use super::*;
use crate::native_realm::ast::NativeAstModule;
use ara_rpc::value_heap::IteratorRecord;
use ara_tools::paths::{self, SearchScope, SearchTarget};
use ara_tools::{
    ast_edit_engine::{self, AstResult},
    resolve::{NativeAstSelection, PendingResolutionQueue},
};

enum ScopePhase {
    Expand(IteratorRecord),
    Resolve(IteratorRecord),
    Finished,
    PreparingNative,
    PreparingApply,
    Native(NativePlan),
}
enum PendingEntry {
    Expand(ValueHandle),
    Resolve,
}
pub(super) enum ScopeReceipt {
    Expanded(Option<Vec<String>>),
    Resolved(String),
    Prepared(SearchScope),
    Computed(AstResult),
    Grammars(bool),
    UnpublishedPreview { scope: SearchScope, max_files: usize, result: AstResult, scope_inputs:Option<Vec<String>> },
    Preview { model: Vec<u16>, display: Option<Vec<u16>> },
}
pub(super) type ScopeReceiptSlot = Arc<Mutex<Option<ScopeReceipt>>>;
pub(super) enum AstOsStep {
    Expand(String),
    Resolve(String),
    Prepare(Vec<String>),
    PrepareApply(ara_tools::ast_edit::NativeAstStage),
    AcquireGrammars(Vec<String>),
    Compute { target: SearchTarget, rewrites: Vec<(String, String)>, dry_run: bool, max_files: usize },
}
pub(super) enum AstFinishStep {
    Preview { scope: SearchScope, max_files: usize, result: AstResult },
    PreviewWithScopeInputs { scope: SearchScope, max_files: usize, result: AstResult, scope_inputs:Vec<String> },
    Apply { selection: NativeAstSelection, queue: PendingResolutionQueue, reason: String, result: AstResult },
    Discard { selection: NativeAstSelection, queue: PendingResolutionQueue, reason: String },
}
struct AstApply {
    selection: NativeAstSelection,
    queue: PendingResolutionQueue,
    reason: String,
}
struct NativePlan {
    scope: SearchScope,
    targets: Vec<SearchTarget>,
    next: usize,
    result: AstResult,
    grammar_rerun_checked: bool,
}
impl NativePlan {
    fn new(scope: SearchScope, dry_run: bool) -> Self {
        Self {
            targets: ast_edit_engine::native_targets(&scope),
            scope,
            next: 0,
            result: AstResult { applied: !dry_run, ..Default::default() },
            grammar_rerun_checked: false,
        }
    }
}

struct AstScopeProgram {
    phase: ScopePhase,
    expanded: ValueHandle,
    resolved: ValueHandle,
    pending: Option<PendingEntry>,
    rewrites: ValueHandle,
    module: Arc<NativeAstModule>,
    apply: Option<AstApply>,
    max_files: usize,
    scope_inputs:Vec<String>,
}
impl AstScopeProgram {
    fn begin(
        values: &SessionValues,
        args: &ValueHandle,
        module: Arc<NativeAstModule>,
        effective: &dyn ValueRuntime,
    ) -> Result<Self, ActualProviderError> {
        let input = ast_edit_input::prepare_arguments(values, args, &module, effective)?;
        let heap = values.heap();
        Ok(Self {
            phase: ScopePhase::Expand(heap.get_iterator(&input.normalized, effective)?),
            expanded: heap.array(Vec::new())?,
            resolved: heap.array(Vec::new())?,
            pending: None,
            rewrites: input.rewrites,
            module,
            apply: None,
            max_files: input.max_files,
            scope_inputs:Vec::new(),
        })
    }
    fn push(
        heap: &Arc<ara_rpc::value_heap::ValueHeap>,
        array: &ValueHandle,
        args: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<(), ActualProviderError> {
        let push = heap.get(array, &"push".into(), effective)?;
        heap.call(&push, array, args, effective)?;
        Ok(())
    }
    fn accept(
        &mut self,
        values: &SessionValues,
        receipt: ScopeReceipt,
        effective: &dyn ValueRuntime,
    ) -> Result<(), ActualProviderError> {
        let heap = values.heap();
        match (self.pending.take(), receipt) {
            (Some(PendingEntry::Expand(original)), ScopeReceipt::Expanded(None)) => {
                Self::push(heap, &self.expanded, &[original], effective)
            }
            (Some(PendingEntry::Expand(_)), ScopeReceipt::Expanded(Some(parts))) => {
                // `expanded.push(...split)` gets the original push method before
                // spreading the fresh native string array in this same realm.
                let push = heap.get(&self.expanded, &"push".into(), effective)?;
                let parts =
                    heap.array(parts.into_iter().map(|part| heap.string(part.into())).collect::<Result<_, _>>()?)?;
                let mut iterator = heap.get_iterator(&parts, effective)?;
                let mut arguments = Vec::new();
                while let Some(part) = heap.iterator_next(&mut iterator, effective)? {
                    arguments.push(part);
                }
                heap.call(&push, &self.expanded, &arguments, effective)?;
                Ok(())
            }
            (Some(PendingEntry::Resolve), ScopeReceipt::Resolved(path)) => {
                Self::push(heap, &self.resolved, &[heap.string(path.into())?], effective)
            }
            (None, ScopeReceipt::Prepared(scope)) if matches!(self.phase, ScopePhase::PreparingNative) => {
                self.phase = ScopePhase::Native(NativePlan::new(scope, true));
                Ok(())
            }
            (None,ScopeReceipt::Prepared(scope)) if matches!(self.phase,ScopePhase::PreparingApply)=>{
                self.phase=ScopePhase::Native(NativePlan::new(scope,false));Ok(())
            }
            (None, ScopeReceipt::Grammars(installed)) => {
                let ScopePhase::Native(plan) = &mut self.phase else {
                    return Err(ActualProviderError::Protocol("AST grammar receipt has no preview caller"));
                };
                if self.apply.is_some() { return Err(ActualProviderError::Protocol("AST apply cannot rerun for grammars")); }
                if installed {
                    plan.next = 0;
                    plan.result = AstResult { applied: false, ..Default::default() };
                }
                Ok(())
            }
            (None, ScopeReceipt::Computed(part)) => {
                let ScopePhase::Native(plan) = &mut self.phase else {
                    return Err(ActualProviderError::Protocol("AST native target receipt has no caller"));
                };
                ast_edit_engine::merge_native_target(&mut plan.result, part);
                Ok(())
            }
            _ => Err(ActualProviderError::Protocol("AST scope receipt has no matching original await")),
        }
    }
    fn next(
        &mut self,
        values: &SessionValues,
        tool: Arc<AstEditTool>,
        receipt: ScopeReceiptSlot,
        effective: &dyn ValueRuntime,
    ) -> Result<Action, ActualProviderError> {
        let heap = values.heap();
        if self.pending.is_some() {
            return Err(ActualProviderError::Protocol("AST scope iterator advanced before its Host await"));
        }
        loop {
            match &mut self.phase {
                ScopePhase::Expand(iterator) => {
                    if let Some(entry) = heap.iterator_next(iterator, effective)? {
                        let normalized = ast_edit_input::normalize(heap, &entry, effective)?;
                        // The nested split helper normalizes again before its
                        // first native path probe. Keep those Calls observable.
                        let split_input = ast_edit_input::normalize(heap, &normalized, effective)?;
                        let input = ast_edit_input::scalar(heap, &split_input)?;
                        self.pending = Some(PendingEntry::Expand(normalized));
                        return Ok(Action::AstScope(tool, AstOsStep::Expand(input), receipt));
                    }
                    ast_edit_input::check_nonempty(heap, &self.expanded, &self.module, effective)?;
                    self.phase = ScopePhase::Resolve(heap.get_iterator(&self.expanded, effective)?);
                }
                ScopePhase::Resolve(iterator) => {
                    if let Some(entry) = heap.iterator_next(iterator, effective)? {
                        let input = ast_edit_input::scalar(heap, &entry)?;
                        self.scope_inputs.push(input.clone());
                        if ara_tools::ast_edit::scope_entry_needs_await(&tool.ctx, &input) {
                            self.pending = Some(PendingEntry::Resolve);
                            return Ok(Action::AstScope(tool, AstOsStep::Resolve(input), receipt));
                        }
                        // A plain local path takes no upstream await. Do not
                        // introduce an extra scheduler boundary in this branch.
                        Self::push(heap, &self.resolved, &[entry], effective)?;
                    } else {
                        self.phase = ScopePhase::Finished;
                    }
                }
                ScopePhase::Finished => {
                    let mut iterator = heap.get_iterator(&self.resolved, effective)?;
                    let mut paths = Vec::new();
                    while let Some(path) = heap.iterator_next(&mut iterator, effective)? {
                        match ast_edit_input::scalar(heap, &path) {
                            Ok(path) => paths.push(path),
                            Err(error) => {
                                let _ = heap.iterator_close(&mut iterator, effective);
                                return Err(error);
                            }
                        }
                    }
                    self.phase = ScopePhase::PreparingNative;
                    return Ok(Action::AstScope(tool, AstOsStep::Prepare(paths), receipt));
                }
                ScopePhase::PreparingNative | ScopePhase::PreparingApply => {
                    return Err(ActualProviderError::Protocol("AST native scope preparation advanced before receipt"));
                }
                ScopePhase::Native(plan) => {
                    if let Some(target) = plan.targets.get(plan.next).cloned() {
                        // Original rewrites is read at each individual native
                        // call, after the preceding target's await. No frozen
                        // map, worker Graph Get, second realm or extra pump.
                        let rewrites = native_engine_input(heap, &self.rewrites, &self.module, effective)?;
                        plan.next += 1;
                        return Ok(Action::AstScope(
                            tool,
                            AstOsStep::Compute {
                                target,
                                rewrites,
                                dry_run: self.apply.is_none(),
                                max_files: self.max_files,
                            },
                            receipt,
                        ));
                    }
                    if self.apply.is_none() && !plan.grammar_rerun_checked && !plan.result.missing_grammars.is_empty() {
                        plan.grammar_rerun_checked = true;
                        return Ok(Action::AstScope(tool, AstOsStep::AcquireGrammars(plan.result.missing_grammars.clone()), receipt));
                    }
                    let result = std::mem::take(&mut plan.result);
                    let finish = if let Some(apply) = &self.apply {
                        AstFinishStep::Apply {
                            selection: apply.selection.clone(),
                            queue: apply.queue.clone(),
                            reason: apply.reason.clone(),
                            result,
                        }
                    } else {
                        AstFinishStep::PreviewWithScopeInputs { scope: plan.scope.clone(), max_files: self.max_files, result, scope_inputs:self.scope_inputs.clone() }
                    };
                    return Ok(Action::AstEdit(tool, finish, receipt));
                }
            }
        }
    }
    fn apply_error(&self, error: ActualProviderError, effective: &dyn ValueRuntime) -> ActualProviderError {
        let Some(apply) = &self.apply else {
            return error;
        };
        // Original onApplyError re-pushes the SAME id before instanceof/Get/Call
        // in its catch. A concurrent newer proposal must not steal its head.
        apply.queue.restore_native_ast(&apply.selection);
        match error {
            ActualProviderError::Value(error) => self.module.wrap_apply_failure(error, effective).into(),
            other => other,
        }
    }
    fn close(&mut self, values: &SessionValues, effective: &dyn ValueRuntime) {
        // An existing throw wins over IteratorClose's Native throw. The original
        // iterator is retained while a Host operation is pending, so cancellation
        // and URI errors close it after the actual receipt, never in the worker.
        match &mut self.phase {
            ScopePhase::Expand(iterator) | ScopePhase::Resolve(iterator) => {
                let _ = values.heap().iterator_close(iterator, effective);
            }
            ScopePhase::Finished | ScopePhase::PreparingNative | ScopePhase::PreparingApply | ScopePhase::Native(_) => {}
        }
        self.pending = None;
        self.phase = ScopePhase::Finished;
    }
}

pub(super) struct AstPreparation {
    tool: Arc<AstEditTool>,
    program: Option<AstScopeProgram>,
    receipt: ScopeReceiptSlot,
    awaiting: bool,
    continuation: Option<AstContinuation>,
    publication: Option<AstPreviewPublication>,
}
/// Only the original driver owns this guard and the deferred Graph binding.
/// It remains armed through returned-result adoption and authoritative ticket
/// acceptance. Workers return an unpublished scalar preview, never this guard.
struct AstPreviewPublication {
    id: String,
    queue: PendingResolutionQueue,
    module: Arc<NativeAstModule>,
    accepted: bool,
}
impl Drop for AstPreviewPublication {
    fn drop(&mut self) {
        if !self.accepted {
            self.queue.remove(&self.id);
            self.module.remove(&self.id);
        }
    }
}
struct AstContinuation {
    job: async_effect::HostEffectId,
    kernel: NativePendingPromise,
    result: NativePendingPromise,
    completion: CapabilityId,
    port: ValueHandle,
    kernel_completed: bool,
    result_completed: bool,
}

impl InvocationRuntime {
    pub(super) fn prepare_ast_scope(
        &self,
        tool: &Arc<AstEditTool>,
        effective: &dyn ValueRuntime,
    ) -> Result<Action, ActualProviderError> {
        let mut program = AstScopeProgram::begin(
            &self.request.values,
            &self.request.args,
            self.owner.ast_module()?.clone(),
            effective,
        )?;
        let receipt = Arc::new(Mutex::new(None));
        let action = match program.next(&self.request.values, tool.clone(), receipt.clone(), effective) {
            Ok(action) => action,
            Err(error) => {
                program.close(&self.request.values, effective);
                return Err(error);
            }
        };
        let awaiting = matches!(&action, Action::AstScope(..));
        self.state.lock().unwrap_or_else(|e| e.into_inner()).ast = Some(AstPreparation {
            tool: tool.clone(),
            program: Some(program),
            receipt,
            awaiting,
            continuation: None,
            publication: None,
        });
        Ok(action)
    }
    pub(super) fn prepare_ast_resolution(
        &self,
        tool: &Arc<WriteTool>,
        path: &str,
        content: &str,
        effective: &dyn ValueRuntime,
    ) -> Result<Option<Action>, ActualProviderError> {
        let Some(device) = ara_tools::resolve::resolution_device(path) else {
            return Ok(None);
        };
        self.check_not_aborted()?;
        let Some(selection) = tool.ctx.pending_resolution.native_ast_head() else {
            return Ok(None);
        };
        let module = self.owner.ast_module()?.clone();
        let rewrites = module
            .staged(&selection.id)?
            .ok_or(ActualProviderError::Protocol("Original AST pending binding absent from this Session"))?;
        let apply = AstApply {
            selection: selection.clone(),
            queue: tool.ctx.pending_resolution.clone(),
            reason: content.trim().into(),
        };
        let heap = self.request.values.heap();
        let mut program = AstScopeProgram {
            phase: ScopePhase::Native(NativePlan::new(selection.stage.scope.clone(), false)),
            expanded: heap.array(Vec::new())?,
            resolved: heap.array(Vec::new())?,
            pending: None,
            rewrites,
            module,
            max_files: selection.stage.max_files,
            scope_inputs:Vec::new(),
            apply: Some(apply),
        };
        let native_tool = Arc::new(AstEditTool { ctx: tool.ctx.clone() });
        let receipt = Arc::new(Mutex::new(None));
        let action = if device == "reject" {
            Action::AstEdit(
                native_tool.clone(),
                AstFinishStep::Discard {
                    selection,
                    queue: tool.ctx.pending_resolution.clone(),
                    reason: content.trim().into(),
                },
                receipt.clone(),
            )
        } else if selection.stage.needs_fresh_apply_scope() {
            program.phase=ScopePhase::PreparingApply;
            Action::AstScope(native_tool.clone(),AstOsStep::PrepareApply(selection.stage.clone()),receipt.clone())
        } else {
            program
                .next(&self.request.values, native_tool.clone(), receipt.clone(), effective)
                .map_err(|error| program.apply_error(error, effective))?
        };
        let awaiting = matches!(&action, Action::AstScope(..));
        self.state.lock().unwrap_or_else(|e| e.into_inner()).ast = Some(AstPreparation {
            tool: native_tool,
            program: Some(program),
            receipt,
            awaiting,
            continuation: None,
            publication: None,
        });
        Ok(Some(action))
    }
    fn next_ast_action(&self, effective: &dyn ValueRuntime) -> Result<Action, ActualProviderError> {
        let (mut program, tool, receipt) = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let ast = state.ast.as_mut().ok_or(ActualProviderError::Protocol("AST scope preparation absent"))?;
            (
                ast.program.take().ok_or(ActualProviderError::Protocol("AST scope original driver already entered"))?,
                ast.tool.clone(),
                ast.receipt.clone(),
            )
        };
        // Every Graph Get/Call/iterator observation runs with state unlocked;
        // programmable getters may reenter their same InvocationRuntime.
        let result = (|| {
            let received = receipt
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .ok_or(ActualProviderError::Protocol("AST native backing-path await has not settled"))?;
            program.accept(&self.request.values, received, effective)?;
            program.next(&self.request.values, tool, receipt, effective)
        })();
        let result = result.map_err(|error| program.apply_error(error, effective));
        if result.is_err() {
            program.close(&self.request.values, effective);
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let ast = state.ast.as_mut().ok_or(ActualProviderError::Protocol("AST scope preparation disappeared"))?;
        ast.awaiting = matches!(&result, Ok(Action::AstScope(..)));
        ast.program = Some(program);
        result
    }
    pub(super) fn has_ast_scope(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).ast.as_ref().is_some_and(|ast| ast.awaiting)
    }
    pub(super) fn finish_ast_output(
        &self,
        value: &ValueHandle,
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle, ValueError> {
        let (slot, tool, module, rewrites, applied_id) = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(ast) = state.ast.as_ref() else {
                return Ok(value.clone());
            };
            let program = ast
                .program
                .as_ref()
                .ok_or(ValueError::WrongRuntimeReply("AST original program absent at settlement"))?;
            (
                ast.receipt.clone(),
                ast.tool.clone(),
                program.module.clone(),
                program.rewrites.clone(),
                program.apply.as_ref().map(|apply| apply.selection.id.clone()),
            )
        };
        let received = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        let (value, model, display) = match received {
            Some(ScopeReceipt::UnpublishedPreview { scope, max_files, result, scope_inputs }) => {
                if self.cancel.is_cancelled() {
                    return Err(module.throw("Aborted: Cancelled".into(), effective));
                }
                // Capture the original edge before taking the admission fence.
                // The worker never publishes, and a dropped waiter permanently
                // closes this invocation's scalar-only registration/binding gate.
                let binding = module.prepare_stage(&rewrites)?;
                let prepared = {
                    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    if !state.ast_publication_open {
                        None
                    } else {
                        let ast =
                            state.ast.as_mut().ok_or(ValueError::WrongRuntimeReply("AST publication owner absent"))?;
                        // This formatter/register path consumes native scalar
                        // data only. No Graph Get/Call/capture occurs under fence.
                        let result = match scope_inputs {
                            Some(inputs)=>tool.finish_native_preview_with_scope_inputs(scope,max_files,result,inputs),
                            None=>tool.finish_native_preview(scope,max_files,result),
                        };
                        if let Ok(result) = &result {
                            if let Some(id) = &result.pending_id {
                                let guard = AstPreviewPublication {
                                    id: id.clone(),
                                    queue: tool.ctx.pending_resolution.clone(),
                                    module: module.clone(),
                                    accepted: false,
                                };
                                ast.publication = Some(guard);
                                module.stage_prepared(id.clone(), binding)?;
                            }
                        }
                        Some(result)
                    }
                };
                let Some(result) = prepared else {
                    return Err(module.throw("Aborted: AST preview waiter exited".into(), effective));
                };
                let result = result.map_err(|error| module.throw(error.0.into(), effective))?;
                let output = output_graph(&self.request.values, result.output).map_err(|error| match error {
                    ActualProviderError::Value(error) => error,
                    other => {
                        if let Some(run) = self.run.as_ref().and_then(Weak::upgrade) {
                            run.fail(other);
                        }
                        ValueError::UnsupportedOperation("AST final output infrastructure failure retained")
                    }
                })?;
                (output, result.model_units, result.display_units)
            }
            Some(ScopeReceipt::Preview { model, display }) => (value.clone(), model, display),
            _ => return Err(ValueError::WrongRuntimeReply("AST final native UTF16 receipt absent")),
        };
        // Apply/discard already completed native effects. Their removal is never
        // part of preview rollback, even if output adoption subsequently fails.
        if let Some(id) = applied_id {
            module.remove(&id);
        }
        let heap = self.request.values.heap();
        let content = heap.get(&value, &"content".into(), effective)?;
        let first = heap.get(&content, &"0".into(), effective)?;
        heap.define_own_data(&first, "text".into(), heap.string(ara_rpc::WireString::from_units(model))?, effective)?;
        if let Some(display) = display {
            let details = heap.get(&value, &"details".into(), effective)?;
            heap.define_own_data(
                &details,
                "displayContent".into(),
                heap.string(ara_rpc::WireString::from_units(display))?,
                effective,
            )?;
        }
        Ok(value)
    }
    pub(super) fn close_ast_publication_admission(&self) {
        let guard = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.ast_publication_open = false;
            state.ast.as_mut().and_then(|ast| ast.publication.take())
        };
        // Exact ID queue/edge deletion has no Graph reads/calls and runs outside
        // invocation state lock; it never cancels Apply/discard or another Run.
        drop(guard);
    }
    pub(super) fn accept_ast_publication(&self, accepted: bool) -> Result<(), ActualProviderError> {
        let guard = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.ast_publication_open = false;
            state.ast.as_mut().and_then(|ast| ast.publication.take())
        };
        if let Some(mut guard) = guard {
            if accepted {
                // Publication acceptance is the last scalar-only grant gate.
                // A thenable getter may cancel/revoke after local stage returned.
                let grants = self.owner.grants.lock().unwrap_or_else(|e| e.into_inner());
                if !grants[self.index].enabled {
                    return Err(ActualProviderError::Callback(
                        "Tool execution was blocked by the current Host grant".into(),
                    ));
                }
                self.check_not_aborted()?;
                guard.accepted = true;
            }
        }
        Ok(())
    }
    pub(super) fn ast_apply_failure(&self, reason: &ValueHandle, effective: &dyn ValueRuntime) -> ValueError {
        let apply = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.ast.as_ref().and_then(|ast| ast.program.as_ref()).and_then(|program| {
                program
                    .apply
                    .as_ref()
                    .map(|apply| (program.module.clone(), apply.queue.clone(), apply.selection.clone()))
            })
        };
        let Some((module, queue, selection)) = apply else {
            return ValueError::Thrown(reason.clone());
        };
        queue.restore_native_ast(&selection);
        module.wrap_apply_failure(ValueError::Thrown(reason.clone()), effective)
    }
    pub(super) fn close_ast_scope(&self, effective: &dyn ValueRuntime) {
        let _ = self.accept_ast_publication(false);
        let program = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.ast.as_mut().and_then(|ast| {
                ast.awaiting = false;
                ast.program.take()
            })
        };
        if let Some(mut program) = program {
            program.close(&self.request.values, effective);
            if let Some(ast) = self.state.lock().unwrap_or_else(|e| e.into_inner()).ast.as_mut() {
                ast.program = Some(program);
            }
        }
    }
    pub(super) fn resume_ast_native(&self, effective: &dyn ValueRuntime) -> Result<ValueHandle, ActualProviderError> {
        let prepared = (|| {
            let run =
                self.run.as_ref().and_then(Weak::upgrade).ok_or(ActualProviderError::Protocol("AST Run expired"))?;
            run.check_live()?;
            let action = self.next_ast_action(effective).map_err(|error| match error {
                ActualProviderError::Value(ValueError::TypeError(message)) => {
                    match run.native_type_error(&self.request.values, &message, effective) {
                        Ok(reason) => ValueError::Thrown(reason).into(),
                        Err(error) => error.into(),
                    }
                }
                other => other,
            })?;
            let scope = run.admission.capture_scope(effective)?;
            let parent = run.admission.capture_parent(effective)?;
            if self.parent.get() != Some(&parent) {
                return Err(ActualProviderError::Protocol("AST scope continuation parent changed"));
            }
            let kernel = run.admission.prepare_pending(scope, parent)?;
            let result = run.admission.prepare_pending(scope, parent)?;
            let completion = capability()?;
            let heap = self.request.values.heap();
            let port = heap.function(completion)?;
            let fulfilled = heap.fresh_compiled_function(&NATIVE_AWAIT_OK, &[port.clone()])?;
            let rejected = heap.fresh_compiled_function(&NATIVE_AWAIT_ERR, &[port.clone()])?;
            run.admission.attach_native_await(&kernel.ticket(), &fulfilled, &rejected, scope, parent)?;
            let reservation = run.reserve(&self.request.values, scope, parent, &port)?;
            let job = reservation.id();
            let permit = match run.final_enter(
                &reservation,
                &self.owner,
                self.index,
                self.update,
                &self.request,
                self.cancel.clone(),
            ) {
                Ok(permit) => permit,
                Err(error) => {
                    run.release(&reservation);
                    return Err(error);
                }
            };
            let public = result.promise.clone();
            self.state.lock().unwrap_or_else(|e| e.into_inner()).ast.as_mut().unwrap().continuation =
                Some(AstContinuation {
                    job,
                    kernel,
                    result,
                    completion,
                    port,
                    kernel_completed: false,
                    result_completed: false,
                });
            let update: BashUpdateFn = Arc::new(|_| {}); // fixed ast_edit has no updates
            if let Err(error) =
                reservation.enter(self.cancel.clone(), action_future(action, permit, self.cancel.clone(), update))
            {
                run.finish_graph(job);
                return Err(ActualProviderError::Callback(error.message().to_owned()));
            }
            Ok(public)
        })();
        // After original Get/Call resumes, setup can fail synchronously before
        // a next Host effect exists. Close here on the same graph driver.
        if prepared.is_err() {
            self.close_ast_scope(effective);
        }
        prepared
    }
    pub(super) fn resume_ast_legacy(&self) -> Result<LegacyEffect, ActualProviderError> {
        let prepared = (|| {
            let action = self.next_ast_action(self)?;
            let permit = self.owner.enter(self.index, self.update, &self.request, self.cancel.clone())?;
            let partials = Arc::new(Mutex::new(std::collections::VecDeque::new()));
            let wake = Arc::new(tokio::sync::Notify::new());
            let update: BashUpdateFn = Arc::new(|_| {});
            Ok(LegacyEffect { future: action_future(action, permit, self.cancel.clone(), update), partials, wake })
        })();
        if prepared.is_err() {
            self.close_ast_scope(self);
        }
        prepared
    }
    pub(super) fn ast_completion_capability(&self, id: CapabilityId) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ast
            .as_ref()
            .and_then(|ast| ast.continuation.as_ref())
            .is_some_and(|continuation| continuation.completion == id)
    }
    pub(super) fn complete_ast_native(
        &self,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle, ValueError> {
        let run = self.run.as_ref().and_then(Weak::upgrade).ok_or(ValueError::ExpiredCapability(self.completion))?;
        run.check_live().map_err(|_| ValueError::ExpiredCapability(self.completion))?;
        let heap = self.request.values.heap();
        if arguments.len() != 2 {
            return Err(ValueError::WrongRuntimeReply("AST scope completion arity"));
        }
        let ValueSnapshot::Number(kind) = heap.snapshot(&arguments[0])? else {
            return Err(ValueError::WrongRuntimeReply("AST scope completion kind"));
        };
        heap.adopt(&arguments[1])?;
        let (job, kernel_resolve, kernel_reject, result_resolve, result_reject) = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.phase != Phase::Running {
                return Err(ValueError::ExpiredCapability(self.completion));
            }
            let continuation = state
                .ast
                .as_mut()
                .and_then(|ast| ast.continuation.as_mut())
                .ok_or(ValueError::ExpiredCapability(self.completion))?;
            if !receiver.same_identity(&continuation.port) {
                return Err(ValueError::ExpiredCapability(continuation.completion));
            }
            if kind == 1.0 || kind == 2.0 {
                if continuation.kernel_completed {
                    return Err(ValueError::ExpiredCapability(continuation.completion));
                }
                continuation.kernel_completed = true;
            } else if kind == 3.0 || kind == 4.0 {
                if !continuation.kernel_completed || continuation.result_completed {
                    return Err(ValueError::ExpiredCapability(continuation.completion));
                }
                continuation.result_completed = true;
            } else {
                return Err(ValueError::WrongRuntimeReply("AST scope completion kind"));
            }
            (
                continuation.job,
                continuation.kernel.resolve.clone(),
                continuation.kernel.reject.clone(),
                continuation.result.resolve.clone(),
                continuation.result.reject.clone(),
            )
        };
        if kind == 1.0 || kind == 2.0 {
            heap.call(
                if kind == 1.0 { &kernel_resolve } else { &kernel_reject },
                &heap.undefined()?,
                &[arguments[1].clone()],
                effective,
            )?;
            return heap.undefined();
        }
        let next = if kind == 4.0 {
            let error = self.ast_apply_failure(&arguments[1], effective);
            self.close_ast_scope(effective);
            Err(error)
        } else if self.has_ast_scope() {
            self.resume_ast_native(effective).map_err(|error| match error {
                ActualProviderError::Value(error) => error,
                other => {
                    run.fail(other);
                    ValueError::UnsupportedOperation("AST scope infrastructure failure retained")
                }
            })
        } else {
            self.finish_ast_output(&arguments[1], effective)
        };
        match next {
            Ok(value) => {
                heap.call(&result_resolve, &heap.undefined()?, &[value], effective)?;
            }
            Err(ValueError::Thrown(reason)) => {
                heap.call(&result_reject, &heap.undefined()?, &[reason], effective)?;
            }
            Err(error) => {
                run.fail(error.clone().into());
                return Err(error);
            }
        }
        run.finish_graph(job);
        heap.undefined()
    }
}

pub(super) async fn scope_step(
    tool: Arc<AstEditTool>,
    step: AstOsStep,
    receipt: ScopeReceiptSlot,
    cancel: CancellationToken,
) -> Result<ToolOutput, ToolError> {
    if !matches!(&step,AstOsStep::Compute {dry_run:false,..}|AstOsStep::PrepareApply(_)) && cancel.is_cancelled() {
        return Err(ToolError("Aborted: Cancelled".into()));
    }
    let value = match step {
        AstOsStep::Expand(input) => {
            let ctx = tool.ctx.clone();
            let split = tokio::task::spawn_blocking(move || {
                paths::split_prepared_delimited_entry(&ctx, &input, paths::Splitter::Search)
            })
            .await
            .map_err(|error| ToolError(error.to_string()))?
            .map_err(ToolError)?;
            ScopeReceipt::Expanded(split)
        }
        AstOsStep::Resolve(input) => {
            ScopeReceipt::Resolved(ara_tools::ast_edit::resolve_ast_scope_entry(&tool.ctx, &input, cancel).await?)
        }
        AstOsStep::AcquireGrammars(languages) => {
            ScopeReceipt::Grammars(ara_tools::grammars::ensure_missing(&tool.ctx, &languages, cancel).await?)
        }
        AstOsStep::PrepareApply(stage)=>{
            ScopeReceipt::Prepared(stage.fresh_apply_scope(&tool.ctx).await?)
        }
        AstOsStep::Prepare(inputs) => {
            let ctx = tool.ctx.clone();
            ScopeReceipt::Prepared(
                tokio::task::spawn_blocking(move || ara_tools::ast_edit::resolve_prepared_ast_scope(&ctx, &inputs))
                    .await
                    .map_err(|e| ToolError(e.to_string()))??,
            )
        }
        AstOsStep::Compute { target, rewrites, dry_run, max_files } => {
            // Queued apply is executed once. Preview's outer source-shaped
            // program performs grammar acquisition and at most one whole-scope
            // rerun, retaining original Graph reads before each native target.
            let token = if dry_run { cancel } else { CancellationToken::new() };
            ScopeReceipt::Computed(tokio::task::spawn_blocking(move || {
                ast_edit_engine::run_native_target(target, rewrites, dry_run, max_files, token)
            }).await.map_err(|error| ToolError(error.to_string()))??)
        }
    };
    *receipt.lock().unwrap_or_else(|e| e.into_inner()) = Some(value);
    Ok(ToolOutput::text(""))
}
pub(super) async fn execute_finish(
    tool: Arc<AstEditTool>,
    step: AstFinishStep,
    receipt: ScopeReceiptSlot,
) -> Result<ToolOutput, ToolError> {
    let (output, model, display) = match step {
        AstFinishStep::Preview {scope,max_files,result}=>{
            *receipt.lock().unwrap_or_else(|e|e.into_inner())=Some(ScopeReceipt::UnpublishedPreview {scope,max_files,result,scope_inputs:None});
            return Ok(ToolOutput::text(""));
        }
        AstFinishStep::PreviewWithScopeInputs {scope,max_files,result,scope_inputs}=>{
            *receipt.lock().unwrap_or_else(|e|e.into_inner())=Some(ScopeReceipt::UnpublishedPreview {scope,max_files,result,scope_inputs:Some(scope_inputs)});
            return Ok(ToolOutput::text(""));
        }
        AstFinishStep::Apply { selection, queue, reason, result } => {
            let output = queue.finish_native_ast(&selection, "resolve", &reason, selection.stage.finish_apply_with_context(result,&tool.ctx));
            let model = output
                .content
                .iter()
                .filter_map(|part| match part {
                    ara_ai::UserBlock::Text(text) => Some(text.text.as_str()),
                    ara_ai::UserBlock::Image(_) => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
                .encode_utf16()
                .collect();
            (output, model, None)
        }
        AstFinishStep::Discard { selection, queue, reason } => {
            let output = queue.discard_native_ast(&selection, &reason);
            // Existing queue owns discard's output and exact removal semantics.
            let model = output
                .content
                .iter()
                .filter_map(|part| match part {
                    ara_ai::UserBlock::Text(text) => Some(text.text.as_str()),
                    ara_ai::UserBlock::Image(_) => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
                .encode_utf16()
                .collect();
            (output, model, None)
        }
    };
    *receipt.lock().unwrap_or_else(|e| e.into_inner()) = Some(ScopeReceipt::Preview { model, display });
    Ok(output)
}

/// Exact napi3.12.2 map.rs69-87/object.rs831-865,897-915/string.rs51-86.
/// Keys are captured once before any value Get. UTF8 key recreation precedes
/// Get; undefined skips String conversion; raw values never use ToString.
pub(super) fn native_engine_input(
    heap: &Arc<ara_rpc::value_heap::ValueHeap>,
    rewrites: &ValueHandle,
    module: &NativeAstModule,
    effective: &dyn ValueRuntime,
) -> Result<Vec<(String, String)>, ActualProviderError> {
    if matches!(heap.kind(rewrites)?, ara_rpc::value_heap::ValueKind::Null | ara_rpc::value_heap::ValueKind::Undefined)
    {
        return Ok(Vec::new());
    }
    // napi_get_property_names includes ancestor enumerable names even when
    // the receiver has a nonenumerable shadow; the later Get then reads that
    // own shadow. This differs from ordinary ECMAScript for-in enumeration.
    // Native reference: abi-probe-receipt.json shadow-nonenumerable, plus the
    // pinned napi3.12.2 Object::keys. Primitive strings expose own indices.
    let mut keys = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    let mut current = Some(rewrites.clone());
    while let Some(value) = current {
        if !visited.insert(value.node_id()) {
            return Err(ValueError::TypeError("Cyclic native property enumeration".into()).into());
        }
        for key in heap.own_keys(&value, effective)? {
            let PropertyKey::String(text) = &key else {
                continue;
            };
            if heap.own_descriptor(&value, &key, effective)?.is_some_and(|descriptor| descriptor.enumerable())
                && names.insert(text.clone())
            {
                keys.push(text.clone());
            }
        }
        current = heap.prototype(&value, effective)?;
    }
    let keys: Vec<_> = keys.iter().map(|key| String::from_utf16_lossy(key.units())).collect();
    let mut map = std::collections::HashMap::new();
    for key in keys {
        let value = heap.get(rewrites, &ara_rpc::WireString::from(key.as_str()).into(), effective)?;
        match heap.snapshot(&value)? {
            ValueSnapshot::Undefined => continue,
            ValueSnapshot::String(text) => {
                map.insert(key, String::from_utf16_lossy(text.units()));
            }
            _ => return Err(napi_string_error(heap, &value, module, effective)?.into()),
        }
    }
    let mut pairs: Vec<_> = map.into_iter().collect();
    // Native normalize_rewrite_map performs this after the conversion, including
    // last-wins collisions caused by UTF8 key replacement.
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(pairs)
}

#[cfg(test)]
#[path = "ast_edit_native_tests.rs"]
mod tests;

/// Port napi3.12.2 error.rs1741-1789 diagnostics at this native origin.
/// Its JSON/function-name diagnostic observations can themselves throw, and
/// those original reasons must propagate before a new Native Error is made.
fn napi_string_error(
    heap: &Arc<ara_rpc::value_heap::ValueHeap>,
    value: &ValueHandle,
    module: &NativeAstModule,
    effective: &dyn ValueRuntime,
) -> Result<ValueError, ValueError> {
    let description = match heap.kind(value)? {
        ara_rpc::value_heap::ValueKind::Function => {
            let name = heap.get(value, &"name".into(), effective)?;
            let ValueSnapshot::String(name) = heap.snapshot(&name)? else {
                return Err(ValueError::TypeError("Failed to read native function name".into()));
            };
            let name = String::from_utf16_lossy(name.units());
            format!("function {}(..) ", if name.is_empty() { "anonymous" } else { &name })
        }
        ara_rpc::value_heap::ValueKind::Object | ara_rpc::value_heap::ValueKind::Array => {
            let json = heap.get(&module.environment, &"JSON".into(), effective)?;
            let stringify = heap.get(&json, &"stringify".into(), effective)?;
            let rendered = heap.call(&stringify, &json, std::slice::from_ref(value), effective)?;
            let ValueSnapshot::String(text) = heap.snapshot(&rendered)? else {
                return Err(ValueError::TypeError("Failed to read native JSON diagnostic".into()));
            };
            format!("Object {}", String::from_utf16_lossy(text.units()))
        }
        ara_rpc::value_heap::ValueKind::Number => {
            format!("Number {} ", String::from_utf16_lossy(heap.to_string(value, effective)?.units()))
        }
        ara_rpc::value_heap::ValueKind::Boolean => {
            format!("Boolean {} ", String::from_utf16_lossy(heap.to_string(value, effective)?.units()))
        }
        ara_rpc::value_heap::ValueKind::BigInt => {
            format!("BigInt {} ", String::from_utf16_lossy(heap.to_string(value, effective)?.units()))
        }
        ara_rpc::value_heap::ValueKind::Null => "Null".into(),
        ara_rpc::value_heap::ValueKind::Symbol => "Symbol".into(),
        ara_rpc::value_heap::ValueKind::Undefined => "Undefined".into(),
        ara_rpc::value_heap::ValueKind::String => unreachable!(),
    };
    module.native_error(
        format!(
            "Failed to convert JavaScript value `{description}` into rust type `String` on AstReplaceOptions.rewrites"
        )
        .into(),
        effective,
    )
}
