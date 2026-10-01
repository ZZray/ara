// Generated from fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d; do not edit.
// MIT License
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
#![allow(non_upper_case_globals, non_camel_case_types)]
use crate::catalog_protobuf::{ProtoMessage, SchemaHandle};
pub type AfterAgentResponseRequestQuery = ProtoMessage;
pub const AfterAgentResponseRequestQuerySchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.AfterAgentResponseRequestQuery");
pub type AfterAgentResponseRequestResponse = ProtoMessage;
pub const AfterAgentResponseRequestResponseSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.AfterAgentResponseRequestResponse");
pub type AfterAgentThoughtRequestQuery = ProtoMessage;
pub const AfterAgentThoughtRequestQuerySchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.AfterAgentThoughtRequestQuery");
pub type AfterAgentThoughtRequestResponse = ProtoMessage;
pub const AfterAgentThoughtRequestResponseSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.AfterAgentThoughtRequestResponse");
pub type AgentClientMessage = ProtoMessage;
pub const AgentClientMessageSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.AgentClientMessage");
pub type AgentConversationTurnStructure = ProtoMessage;
pub const AgentConversationTurnStructureSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.AgentConversationTurnStructure");
pub type AgentRunRequest = ProtoMessage;
pub const AgentRunRequestSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.AgentRunRequest");
pub type AgentServerMessage = ProtoMessage;
pub const AgentServerMessageSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.AgentServerMessage");
pub type AgentStoreConflictArgs = ProtoMessage;
pub const AgentStoreConflictArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.AgentStoreConflictArgs");
pub type AgentStoreConflictCursor = ProtoMessage;
pub const AgentStoreConflictCursorSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.AgentStoreConflictCursor");
pub type AgentStoreConflictError = ProtoMessage;
pub const AgentStoreConflictErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.AgentStoreConflictError");
pub type AgentStoreConflictEvent = ProtoMessage;
pub const AgentStoreConflictEventSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.AgentStoreConflictEvent");
pub type AgentStoreConflictResult = ProtoMessage;
pub const AgentStoreConflictResultSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.AgentStoreConflictResult");
pub type AgentStoreConflictSuccess = ProtoMessage;
pub const AgentStoreConflictSuccessSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.AgentStoreConflictSuccess");
pub type ApiKeyCredentials = ProtoMessage;
pub const ApiKeyCredentialsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ApiKeyCredentials");
pub type AppliedAgentChange = ProtoMessage;
pub const AppliedAgentChangeSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.AppliedAgentChange");
pub type ApplyAgentDiffArgs = ProtoMessage;
pub const ApplyAgentDiffArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ApplyAgentDiffArgs");
pub type ApplyAgentDiffError = ProtoMessage;
pub const ApplyAgentDiffErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ApplyAgentDiffError");
pub type ApplyAgentDiffResult = ProtoMessage;
pub const ApplyAgentDiffResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ApplyAgentDiffResult");
pub type ApplyAgentDiffSuccess = ProtoMessage;
pub const ApplyAgentDiffSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ApplyAgentDiffSuccess");
pub type ApplyAgentDiffToolCall = ProtoMessage;
pub const ApplyAgentDiffToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ApplyAgentDiffToolCall");
pub type AskQuestionArgs = ProtoMessage;
pub const AskQuestionArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.AskQuestionArgs");
pub type AskQuestionArgs_Option = ProtoMessage;
pub const AskQuestionArgs_OptionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.AskQuestionArgs_Option");
pub type AskQuestionArgs_Question = ProtoMessage;
pub const AskQuestionArgs_QuestionSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.AskQuestionArgs_Question");
pub type AskQuestionAsync = ProtoMessage;
pub const AskQuestionAsyncSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.AskQuestionAsync");
pub type AskQuestionError = ProtoMessage;
pub const AskQuestionErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.AskQuestionError");
pub type AskQuestionInteractionQuery = ProtoMessage;
pub const AskQuestionInteractionQuerySchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.AskQuestionInteractionQuery");
pub type AskQuestionInteractionResponse = ProtoMessage;
pub const AskQuestionInteractionResponseSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.AskQuestionInteractionResponse");
pub type AskQuestionRejected = ProtoMessage;
pub const AskQuestionRejectedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.AskQuestionRejected");
pub type AskQuestionResult = ProtoMessage;
pub const AskQuestionResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.AskQuestionResult");
pub type AskQuestionSuccess = ProtoMessage;
pub const AskQuestionSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.AskQuestionSuccess");
pub type AskQuestionSuccess_Answer = ProtoMessage;
pub const AskQuestionSuccess_AnswerSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.AskQuestionSuccess_Answer");
pub type AskQuestionToolCall = ProtoMessage;
pub const AskQuestionToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.AskQuestionToolCall");
pub type AssistantMessage = ProtoMessage;
pub const AssistantMessageSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.AssistantMessage");
pub type AsyncAskQuestionCompletionAction = ProtoMessage;
pub const AsyncAskQuestionCompletionActionSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.AsyncAskQuestionCompletionAction");
pub type AzureCredentials = ProtoMessage;
pub const AzureCredentialsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.AzureCredentials");
pub type BackgroundShellSpawnArgs = ProtoMessage;
pub const BackgroundShellSpawnArgsSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.BackgroundShellSpawnArgs");
pub type BackgroundShellSpawnError = ProtoMessage;
pub const BackgroundShellSpawnErrorSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.BackgroundShellSpawnError");
pub type BackgroundShellSpawnResult = ProtoMessage;
pub const BackgroundShellSpawnResultSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.BackgroundShellSpawnResult");
pub type BackgroundShellSpawnSuccess = ProtoMessage;
pub const BackgroundShellSpawnSuccessSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.BackgroundShellSpawnSuccess");
pub type BedrockCredentials = ProtoMessage;
pub const BedrockCredentialsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.BedrockCredentials");
pub type BeforeSubmitPromptRequestQuery = ProtoMessage;
pub const BeforeSubmitPromptRequestQuerySchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.BeforeSubmitPromptRequestQuery");
pub type BeforeSubmitPromptRequestResponse = ProtoMessage;
pub const BeforeSubmitPromptRequestResponseSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.BeforeSubmitPromptRequestResponse");
pub type CallFrame = ProtoMessage;
pub const CallFrameSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.CallFrame");
pub type CancelAction = ProtoMessage;
pub const CancelActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.CancelAction");
pub type CanvasDiagnosticsArgs = ProtoMessage;
pub const CanvasDiagnosticsArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.CanvasDiagnosticsArgs");
pub type CanvasDiagnosticsError = ProtoMessage;
pub const CanvasDiagnosticsErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.CanvasDiagnosticsError");
pub type CanvasDiagnosticsResult = ProtoMessage;
pub const CanvasDiagnosticsResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.CanvasDiagnosticsResult");
pub type CanvasDiagnosticsSuccess = ProtoMessage;
pub const CanvasDiagnosticsSuccessSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.CanvasDiagnosticsSuccess");
pub type ClickAction = ProtoMessage;
pub const ClickActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ClickAction");
pub type ClientHeartbeat = ProtoMessage;
pub const ClientHeartbeatSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ClientHeartbeat");
pub type CommandClassifierResult = ProtoMessage;
pub const CommandClassifierResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.CommandClassifierResult");
pub type CommandClassifierResult_ClassifiedCommand = ProtoMessage;
pub const CommandClassifierResult_ClassifiedCommandSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.CommandClassifierResult_ClassifiedCommand");
pub type ComputerUseAction = ProtoMessage;
pub const ComputerUseActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ComputerUseAction");
pub type ComputerUseArgs = ProtoMessage;
pub const ComputerUseArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ComputerUseArgs");
pub type ComputerUseError = ProtoMessage;
pub const ComputerUseErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ComputerUseError");
pub type ComputerUseResult = ProtoMessage;
pub const ComputerUseResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ComputerUseResult");
pub type ComputerUseSuccess = ProtoMessage;
pub const ComputerUseSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ComputerUseSuccess");
pub type ComputerUseToolCall = ProtoMessage;
pub const ComputerUseToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ComputerUseToolCall");
pub type ConnectScmArgs = ProtoMessage;
pub const ConnectScmArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ConnectScmArgs");
pub type ConnectScmError = ProtoMessage;
pub const ConnectScmErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ConnectScmError");
pub type ConnectScmGithub = ProtoMessage;
pub const ConnectScmGithubSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ConnectScmGithub");
pub type ConnectScmGithubRepository = ProtoMessage;
pub const ConnectScmGithubRepositorySchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ConnectScmGithubRepository");
pub type ConnectScmRejected = ProtoMessage;
pub const ConnectScmRejectedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ConnectScmRejected");
pub type ConnectScmResult = ProtoMessage;
pub const ConnectScmResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ConnectScmResult");
pub type ConnectScmSuccess = ProtoMessage;
pub const ConnectScmSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ConnectScmSuccess");
pub type ConnectScmToolCall = ProtoMessage;
pub const ConnectScmToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ConnectScmToolCall");
pub type ConversationAction = ProtoMessage;
pub const ConversationActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ConversationAction");
pub type ConversationPlan = ProtoMessage;
pub const ConversationPlanSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ConversationPlan");
pub type ConversationSearchArgs = ProtoMessage;
pub const ConversationSearchArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ConversationSearchArgs");
pub type ConversationSearchError = ProtoMessage;
pub const ConversationSearchErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ConversationSearchError");
pub type ConversationSearchHit = ProtoMessage;
pub const ConversationSearchHitSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ConversationSearchHit");
pub type ConversationSearchResult = ProtoMessage;
pub const ConversationSearchResultSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ConversationSearchResult");
pub type ConversationSearchSuccess = ProtoMessage;
pub const ConversationSearchSuccessSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ConversationSearchSuccess");
pub type ConversationStateStructure = ProtoMessage;
pub const ConversationStateStructureSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ConversationStateStructure");
pub type ConversationStep = ProtoMessage;
pub const ConversationStepSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ConversationStep");
pub type ConversationTokenDetails = ProtoMessage;
pub const ConversationTokenDetailsSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ConversationTokenDetails");
pub type ConversationTurnStructure = ProtoMessage;
pub const ConversationTurnStructureSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ConversationTurnStructure");
pub type Coordinate = ProtoMessage;
pub const CoordinateSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.Coordinate");
pub type CreatePlanArgs = ProtoMessage;
pub const CreatePlanArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.CreatePlanArgs");
pub type CreatePlanError = ProtoMessage;
pub const CreatePlanErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.CreatePlanError");
pub type CreatePlanRequestQuery = ProtoMessage;
pub const CreatePlanRequestQuerySchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.CreatePlanRequestQuery");
pub type CreatePlanRequestResponse = ProtoMessage;
pub const CreatePlanRequestResponseSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.CreatePlanRequestResponse");
pub type CreatePlanResult = ProtoMessage;
pub const CreatePlanResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.CreatePlanResult");
pub type CreatePlanSuccess = ProtoMessage;
pub const CreatePlanSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.CreatePlanSuccess");
pub type CreatePlanToolCall = ProtoMessage;
pub const CreatePlanToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.CreatePlanToolCall");
pub type CursorPositionAction = ProtoMessage;
pub const CursorPositionActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.CursorPositionAction");
pub type CursorRule = ProtoMessage;
pub const CursorRuleSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.CursorRule");
pub type CursorRuleType = ProtoMessage;
pub const CursorRuleTypeSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.CursorRuleType");
pub type CursorRuleTypeAgentFetched = ProtoMessage;
pub const CursorRuleTypeAgentFetchedSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.CursorRuleTypeAgentFetched");
pub type CursorRuleTypeFileGlobs = ProtoMessage;
pub const CursorRuleTypeFileGlobsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.CursorRuleTypeFileGlobs");
pub type CursorRuleTypeGlobal = ProtoMessage;
pub const CursorRuleTypeGlobalSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.CursorRuleTypeGlobal");
pub type CursorRuleTypeManuallyAttached = ProtoMessage;
pub const CursorRuleTypeManuallyAttachedSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.CursorRuleTypeManuallyAttached");
pub type CustomSubagent = ProtoMessage;
pub const CustomSubagentSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.CustomSubagent");
pub type DebugModeConfig = ProtoMessage;
pub const DebugModeConfigSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DebugModeConfig");
pub type DeleteArgs = ProtoMessage;
pub const DeleteArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DeleteArgs");
pub type DeleteError = ProtoMessage;
pub const DeleteErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DeleteError");
pub type DeleteFileBusy = ProtoMessage;
pub const DeleteFileBusySchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DeleteFileBusy");
pub type DeleteFileNotFound = ProtoMessage;
pub const DeleteFileNotFoundSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DeleteFileNotFound");
pub type DeleteNotFile = ProtoMessage;
pub const DeleteNotFileSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DeleteNotFile");
pub type DeletePermissionDenied = ProtoMessage;
pub const DeletePermissionDeniedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DeletePermissionDenied");
pub type DeleteRejected = ProtoMessage;
pub const DeleteRejectedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DeleteRejected");
pub type DeleteResult = ProtoMessage;
pub const DeleteResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DeleteResult");
pub type DeleteSuccess = ProtoMessage;
pub const DeleteSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DeleteSuccess");
pub type DeleteToolCall = ProtoMessage;
pub const DeleteToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DeleteToolCall");
pub type Diagnostic = ProtoMessage;
pub const DiagnosticSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.Diagnostic");
pub type DiagnosticItem = ProtoMessage;
pub const DiagnosticItemSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DiagnosticItem");
pub type DiagnosticRange = ProtoMessage;
pub const DiagnosticRangeSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DiagnosticRange");
pub type DiagnosticsArgs = ProtoMessage;
pub const DiagnosticsArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DiagnosticsArgs");
pub type DiagnosticsError = ProtoMessage;
pub const DiagnosticsErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DiagnosticsError");
pub type DiagnosticsFileNotFound = ProtoMessage;
pub const DiagnosticsFileNotFoundSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DiagnosticsFileNotFound");
pub type DiagnosticsPermissionDenied = ProtoMessage;
pub const DiagnosticsPermissionDeniedSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.DiagnosticsPermissionDenied");
pub type DiagnosticsRejected = ProtoMessage;
pub const DiagnosticsRejectedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DiagnosticsRejected");
pub type DiagnosticsResult = ProtoMessage;
pub const DiagnosticsResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DiagnosticsResult");
pub type DiagnosticsSuccess = ProtoMessage;
pub const DiagnosticsSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DiagnosticsSuccess");
pub type DragAction = ProtoMessage;
pub const DragActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.DragAction");
pub type EditArgs = ProtoMessage;
pub const EditArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.EditArgs");
pub type EditError = ProtoMessage;
pub const EditErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.EditError");
pub type EditFileNotFound = ProtoMessage;
pub const EditFileNotFoundSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.EditFileNotFound");
pub type EditReadPermissionDenied = ProtoMessage;
pub const EditReadPermissionDeniedSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.EditReadPermissionDenied");
pub type EditRejected = ProtoMessage;
pub const EditRejectedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.EditRejected");
pub type EditResult = ProtoMessage;
pub const EditResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.EditResult");
pub type EditSuccess = ProtoMessage;
pub const EditSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.EditSuccess");
pub type EditToolCall = ProtoMessage;
pub const EditToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.EditToolCall");
pub type EditToolCallDelta = ProtoMessage;
pub const EditToolCallDeltaSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.EditToolCallDelta");
pub type EditWritePermissionDenied = ProtoMessage;
pub const EditWritePermissionDeniedSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.EditWritePermissionDenied");
pub type Error = ProtoMessage;
pub const ErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.Error");
pub type ExaFetchArgs = ProtoMessage;
pub const ExaFetchArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExaFetchArgs");
pub type ExaFetchContent = ProtoMessage;
pub const ExaFetchContentSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExaFetchContent");
pub type ExaFetchError = ProtoMessage;
pub const ExaFetchErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExaFetchError");
pub type ExaFetchRejected = ProtoMessage;
pub const ExaFetchRejectedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExaFetchRejected");
pub type ExaFetchRequestQuery = ProtoMessage;
pub const ExaFetchRequestQuerySchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExaFetchRequestQuery");
pub type ExaFetchRequestResponse = ProtoMessage;
pub const ExaFetchRequestResponseSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExaFetchRequestResponse");
pub type ExaFetchRequestResponse_Approved = ProtoMessage;
pub const ExaFetchRequestResponse_ApprovedSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ExaFetchRequestResponse_Approved");
pub type ExaFetchRequestResponse_Rejected = ProtoMessage;
pub const ExaFetchRequestResponse_RejectedSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ExaFetchRequestResponse_Rejected");
pub type ExaFetchResult = ProtoMessage;
pub const ExaFetchResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExaFetchResult");
pub type ExaFetchSuccess = ProtoMessage;
pub const ExaFetchSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExaFetchSuccess");
pub type ExaFetchToolCall = ProtoMessage;
pub const ExaFetchToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExaFetchToolCall");
pub type ExaSearchArgs = ProtoMessage;
pub const ExaSearchArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExaSearchArgs");
pub type ExaSearchError = ProtoMessage;
pub const ExaSearchErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExaSearchError");
pub type ExaSearchReference = ProtoMessage;
pub const ExaSearchReferenceSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExaSearchReference");
pub type ExaSearchRejected = ProtoMessage;
pub const ExaSearchRejectedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExaSearchRejected");
pub type ExaSearchRequestQuery = ProtoMessage;
pub const ExaSearchRequestQuerySchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExaSearchRequestQuery");
pub type ExaSearchRequestResponse = ProtoMessage;
pub const ExaSearchRequestResponseSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ExaSearchRequestResponse");
pub type ExaSearchRequestResponse_Approved = ProtoMessage;
pub const ExaSearchRequestResponse_ApprovedSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ExaSearchRequestResponse_Approved");
pub type ExaSearchRequestResponse_Rejected = ProtoMessage;
pub const ExaSearchRequestResponse_RejectedSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ExaSearchRequestResponse_Rejected");
pub type ExaSearchResult = ProtoMessage;
pub const ExaSearchResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExaSearchResult");
pub type ExaSearchSuccess = ProtoMessage;
pub const ExaSearchSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExaSearchSuccess");
pub type ExaSearchToolCall = ProtoMessage;
pub const ExaSearchToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExaSearchToolCall");
pub type ExecClientControlMessage = ProtoMessage;
pub const ExecClientControlMessageSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ExecClientControlMessage");
pub type ExecClientHeartbeat = ProtoMessage;
pub const ExecClientHeartbeatSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExecClientHeartbeat");
pub type ExecClientMessage = ProtoMessage;
pub const ExecClientMessageSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExecClientMessage");
pub type ExecClientStreamClose = ProtoMessage;
pub const ExecClientStreamCloseSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExecClientStreamClose");
pub type ExecClientThrow = ProtoMessage;
pub const ExecClientThrowSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExecClientThrow");
pub type ExecServerAbort = ProtoMessage;
pub const ExecServerAbortSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExecServerAbort");
pub type ExecServerControlMessage = ProtoMessage;
pub const ExecServerControlMessageSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ExecServerControlMessage");
pub type ExecServerMessage = ProtoMessage;
pub const ExecServerMessageSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExecServerMessage");
pub type ExecuteHookArgs = ProtoMessage;
pub const ExecuteHookArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExecuteHookArgs");
pub type ExecuteHookRequest = ProtoMessage;
pub const ExecuteHookRequestSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExecuteHookRequest");
pub type ExecuteHookResponse = ProtoMessage;
pub const ExecuteHookResponseSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExecuteHookResponse");
pub type ExecuteHookResult = ProtoMessage;
pub const ExecuteHookResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExecuteHookResult");
pub type ExecutePlanAction = ProtoMessage;
pub const ExecutePlanActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExecutePlanAction");
pub type ExtraContextEntry = ProtoMessage;
pub const ExtraContextEntrySchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ExtraContextEntry");
pub type FetchArgs = ProtoMessage;
pub const FetchArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.FetchArgs");
pub type FetchError = ProtoMessage;
pub const FetchErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.FetchError");
pub type FetchResult = ProtoMessage;
pub const FetchResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.FetchResult");
pub type FetchSuccess = ProtoMessage;
pub const FetchSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.FetchSuccess");
pub type FetchToolCall = ProtoMessage;
pub const FetchToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.FetchToolCall");
pub type FileDiagnostics = ProtoMessage;
pub const FileDiagnosticsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.FileDiagnostics");
pub type FileDiff = ProtoMessage;
pub const FileDiffSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.FileDiff");
pub type FileDiff_Chunk = ProtoMessage;
pub const FileDiff_ChunkSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.FileDiff_Chunk");
pub type FileStateStructure = ProtoMessage;
pub const FileStateStructureSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.FileStateStructure");
pub type ForceBackgroundShellArgs = ProtoMessage;
pub const ForceBackgroundShellArgsSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ForceBackgroundShellArgs");
pub type ForceBackgroundShellResult = ProtoMessage;
pub const ForceBackgroundShellResultSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ForceBackgroundShellResult");
pub type ForceBackgroundSubagentArgs = ProtoMessage;
pub const ForceBackgroundSubagentArgsSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ForceBackgroundSubagentArgs");
pub type ForceBackgroundSubagentResult = ProtoMessage;
pub const ForceBackgroundSubagentResultSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ForceBackgroundSubagentResult");
pub type GenerateImageArgs = ProtoMessage;
pub const GenerateImageArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GenerateImageArgs");
pub type GenerateImageError = ProtoMessage;
pub const GenerateImageErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GenerateImageError");
pub type GenerateImageResult = ProtoMessage;
pub const GenerateImageResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GenerateImageResult");
pub type GenerateImageSuccess = ProtoMessage;
pub const GenerateImageSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GenerateImageSuccess");
pub type GenerateImageToolCall = ProtoMessage;
pub const GenerateImageToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GenerateImageToolCall");
pub type GetBlobArgs = ProtoMessage;
pub const GetBlobArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GetBlobArgs");
pub type GetBlobResult = ProtoMessage;
pub const GetBlobResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GetBlobResult");
pub type GetDiffRequest = ProtoMessage;
pub const GetDiffRequestSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GetDiffRequest");
pub type GetDiffResponse = ProtoMessage;
pub const GetDiffResponseSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GetDiffResponse");
pub type GetDiffResponse_SubmoduleDiff = ProtoMessage;
pub const GetDiffResponse_SubmoduleDiffSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.GetDiffResponse_SubmoduleDiff");
pub type GetUsableModelsRequest = ProtoMessage;
pub const GetUsableModelsRequestSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GetUsableModelsRequest");
pub type GetUsableModelsResponse = ProtoMessage;
pub const GetUsableModelsResponseSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GetUsableModelsResponse");
pub type GitDiff = ProtoMessage;
pub const GitDiffSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GitDiff");
pub type GitRepoInfo = ProtoMessage;
pub const GitRepoInfoSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GitRepoInfo");
pub type GlobToolCall = ProtoMessage;
pub const GlobToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GlobToolCall");
pub type GlobToolError = ProtoMessage;
pub const GlobToolErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GlobToolError");
pub type GlobToolResult = ProtoMessage;
pub const GlobToolResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GlobToolResult");
pub type GlobToolSuccess = ProtoMessage;
pub const GlobToolSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GlobToolSuccess");
pub type GrepArgs = ProtoMessage;
pub const GrepArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GrepArgs");
pub type GrepContentMatch = ProtoMessage;
pub const GrepContentMatchSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GrepContentMatch");
pub type GrepContentResult = ProtoMessage;
pub const GrepContentResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GrepContentResult");
pub type GrepCountResult = ProtoMessage;
pub const GrepCountResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GrepCountResult");
pub type GrepError = ProtoMessage;
pub const GrepErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GrepError");
pub type GrepFileCount = ProtoMessage;
pub const GrepFileCountSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GrepFileCount");
pub type GrepFileMatch = ProtoMessage;
pub const GrepFileMatchSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GrepFileMatch");
pub type GrepFilesResult = ProtoMessage;
pub const GrepFilesResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GrepFilesResult");
pub type GrepResult = ProtoMessage;
pub const GrepResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GrepResult");
pub type GrepSuccess = ProtoMessage;
pub const GrepSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GrepSuccess");
pub type GrepToolCall = ProtoMessage;
pub const GrepToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GrepToolCall");
pub type GrepUnionResult = ProtoMessage;
pub const GrepUnionResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.GrepUnionResult");
pub type HeartbeatUpdate = ProtoMessage;
pub const HeartbeatUpdateSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.HeartbeatUpdate");
pub type HookAdditionalContext = ProtoMessage;
pub const HookAdditionalContextSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.HookAdditionalContext");
pub type InteractionQuery = ProtoMessage;
pub const InteractionQuerySchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.InteractionQuery");
pub type InteractionResponse = ProtoMessage;
pub const InteractionResponseSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.InteractionResponse");
pub type InteractionUpdate = ProtoMessage;
pub const InteractionUpdateSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.InteractionUpdate");
pub type InvocationContext = ProtoMessage;
pub const InvocationContextSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.InvocationContext");
pub type InvocationContext_GithubPR = ProtoMessage;
pub const InvocationContext_GithubPRSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.InvocationContext_GithubPR");
pub type InvocationContext_IdeState = ProtoMessage;
pub const InvocationContext_IdeStateSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.InvocationContext_IdeState");
pub type InvocationContext_IdeState_File = ProtoMessage;
pub const InvocationContext_IdeState_FileSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.InvocationContext_IdeState_File");
pub type InvocationContext_IdeState_File_CursorPosition = ProtoMessage;
pub const InvocationContext_IdeState_File_CursorPositionSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.InvocationContext_IdeState_File_CursorPosition");
pub type InvocationContext_IdeState_ViewedPullRequest = ProtoMessage;
pub const InvocationContext_IdeState_ViewedPullRequestSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.InvocationContext_IdeState_ViewedPullRequest");
pub type InvocationContext_SlackThread = ProtoMessage;
pub const InvocationContext_SlackThreadSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.InvocationContext_SlackThread");
pub type KeyAction = ProtoMessage;
pub const KeyActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.KeyAction");
pub type KvClientMessage = ProtoMessage;
pub const KvClientMessageSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.KvClientMessage");
pub type KvServerMessage = ProtoMessage;
pub const KvServerMessageSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.KvServerMessage");
pub type ListMcpResourcesError = ProtoMessage;
pub const ListMcpResourcesErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ListMcpResourcesError");
pub type ListMcpResourcesExecArgs = ProtoMessage;
pub const ListMcpResourcesExecArgsSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ListMcpResourcesExecArgs");
pub type ListMcpResourcesExecResult = ProtoMessage;
pub const ListMcpResourcesExecResultSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ListMcpResourcesExecResult");
pub type ListMcpResourcesExecResult_McpResource = ProtoMessage;
pub const ListMcpResourcesExecResult_McpResourceSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ListMcpResourcesExecResult_McpResource");
pub type ListMcpResourcesRejected = ProtoMessage;
pub const ListMcpResourcesRejectedSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ListMcpResourcesRejected");
pub type ListMcpResourcesSuccess = ProtoMessage;
pub const ListMcpResourcesSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ListMcpResourcesSuccess");
pub type ListMcpResourcesToolCall = ProtoMessage;
pub const ListMcpResourcesToolCallSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ListMcpResourcesToolCall");
pub type LsArgs = ProtoMessage;
pub const LsArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.LsArgs");
pub type LsDirectoryTreeNode = ProtoMessage;
pub const LsDirectoryTreeNodeSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.LsDirectoryTreeNode");
pub type LsDirectoryTreeNode_File = ProtoMessage;
pub const LsDirectoryTreeNode_FileSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.LsDirectoryTreeNode_File");
pub type LsError = ProtoMessage;
pub const LsErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.LsError");
pub type LsRejected = ProtoMessage;
pub const LsRejectedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.LsRejected");
pub type LsResult = ProtoMessage;
pub const LsResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.LsResult");
pub type LsSuccess = ProtoMessage;
pub const LsSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.LsSuccess");
pub type LsTimeout = ProtoMessage;
pub const LsTimeoutSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.LsTimeout");
pub type LsToolCall = ProtoMessage;
pub const LsToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.LsToolCall");
pub type McpAllowlistPrecheckArgs = ProtoMessage;
pub const McpAllowlistPrecheckArgsSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.McpAllowlistPrecheckArgs");
pub type McpAllowlistPrecheckResult = ProtoMessage;
pub const McpAllowlistPrecheckResultSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.McpAllowlistPrecheckResult");
pub type McpApproved = ProtoMessage;
pub const McpApprovedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpApproved");
pub type McpArgs = ProtoMessage;
pub const McpArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpArgs");
pub type McpDescriptor = ProtoMessage;
pub const McpDescriptorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpDescriptor");
pub type McpError = ProtoMessage;
pub const McpErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpError");
pub type McpFileSystemOptions = ProtoMessage;
pub const McpFileSystemOptionsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpFileSystemOptions");
pub type McpImageContent = ProtoMessage;
pub const McpImageContentSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpImageContent");
pub type McpInstructions = ProtoMessage;
pub const McpInstructionsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpInstructions");
pub type McpPermissionDenied = ProtoMessage;
pub const McpPermissionDeniedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpPermissionDenied");
pub type McpRejected = ProtoMessage;
pub const McpRejectedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpRejected");
pub type McpResult = ProtoMessage;
pub const McpResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpResult");
pub type McpServerNotFound = ProtoMessage;
pub const McpServerNotFoundSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpServerNotFound");
pub type McpStateError = ProtoMessage;
pub const McpStateErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpStateError");
pub type McpStateExecArgs = ProtoMessage;
pub const McpStateExecArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpStateExecArgs");
pub type McpStateExecResult = ProtoMessage;
pub const McpStateExecResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpStateExecResult");
pub type McpStateRejected = ProtoMessage;
pub const McpStateRejectedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpStateRejected");
pub type McpStateServer = ProtoMessage;
pub const McpStateServerSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpStateServer");
pub type McpStateSuccess = ProtoMessage;
pub const McpStateSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpStateSuccess");
pub type McpSuccess = ProtoMessage;
pub const McpSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpSuccess");
pub type McpTextContent = ProtoMessage;
pub const McpTextContentSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpTextContent");
pub type McpToolCall = ProtoMessage;
pub const McpToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpToolCall");
pub type McpToolDefinition = ProtoMessage;
pub const McpToolDefinitionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpToolDefinition");
pub type McpToolDescriptor = ProtoMessage;
pub const McpToolDescriptorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpToolDescriptor");
pub type McpToolError = ProtoMessage;
pub const McpToolErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpToolError");
pub type McpToolNotFound = ProtoMessage;
pub const McpToolNotFoundSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpToolNotFound");
pub type McpToolResult = ProtoMessage;
pub const McpToolResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpToolResult");
pub type McpToolResultContentItem = ProtoMessage;
pub const McpToolResultContentItemSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.McpToolResultContentItem");
pub type McpTools = ProtoMessage;
pub const McpToolsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.McpTools");
pub type ModelDetails = ProtoMessage;
pub const ModelDetailsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ModelDetails");
pub type MouseDownAction = ProtoMessage;
pub const MouseDownActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.MouseDownAction");
pub type MouseMoveAction = ProtoMessage;
pub const MouseMoveActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.MouseMoveAction");
pub type MouseUpAction = ProtoMessage;
pub const MouseUpActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.MouseUpAction");
pub type OutputLocation = ProtoMessage;
pub const OutputLocationSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.OutputLocation");
pub type PartialToolCallUpdate = ProtoMessage;
pub const PartialToolCallUpdateSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PartialToolCallUpdate");
pub type Phase = ProtoMessage;
pub const PhaseSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.Phase");
pub type PiBashExecArgs = ProtoMessage;
pub const PiBashExecArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiBashExecArgs");
pub type PiBashExecError = ProtoMessage;
pub const PiBashExecErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiBashExecError");
pub type PiBashExecResult = ProtoMessage;
pub const PiBashExecResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiBashExecResult");
pub type PiBashExecSuccess = ProtoMessage;
pub const PiBashExecSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiBashExecSuccess");
pub type PiBashToolCall = ProtoMessage;
pub const PiBashToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiBashToolCall");
pub type PiEditExecArgs = ProtoMessage;
pub const PiEditExecArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiEditExecArgs");
pub type PiEditExecError = ProtoMessage;
pub const PiEditExecErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiEditExecError");
pub type PiEditExecRejected = ProtoMessage;
pub const PiEditExecRejectedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiEditExecRejected");
pub type PiEditExecResult = ProtoMessage;
pub const PiEditExecResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiEditExecResult");
pub type PiEditExecSuccess = ProtoMessage;
pub const PiEditExecSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiEditExecSuccess");
pub type PiEditReplacement = ProtoMessage;
pub const PiEditReplacementSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiEditReplacement");
pub type PiEditToolCall = ProtoMessage;
pub const PiEditToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiEditToolCall");
pub type PiFindExecArgs = ProtoMessage;
pub const PiFindExecArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiFindExecArgs");
pub type PiFindExecError = ProtoMessage;
pub const PiFindExecErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiFindExecError");
pub type PiFindExecResult = ProtoMessage;
pub const PiFindExecResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiFindExecResult");
pub type PiFindExecSuccess = ProtoMessage;
pub const PiFindExecSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiFindExecSuccess");
pub type PiFindToolCall = ProtoMessage;
pub const PiFindToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiFindToolCall");
pub type PiGrepExecArgs = ProtoMessage;
pub const PiGrepExecArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiGrepExecArgs");
pub type PiGrepExecError = ProtoMessage;
pub const PiGrepExecErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiGrepExecError");
pub type PiGrepExecResult = ProtoMessage;
pub const PiGrepExecResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiGrepExecResult");
pub type PiGrepExecSuccess = ProtoMessage;
pub const PiGrepExecSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiGrepExecSuccess");
pub type PiGrepToolCall = ProtoMessage;
pub const PiGrepToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiGrepToolCall");
pub type PiLsExecArgs = ProtoMessage;
pub const PiLsExecArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiLsExecArgs");
pub type PiLsExecError = ProtoMessage;
pub const PiLsExecErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiLsExecError");
pub type PiLsExecResult = ProtoMessage;
pub const PiLsExecResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiLsExecResult");
pub type PiLsExecSuccess = ProtoMessage;
pub const PiLsExecSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiLsExecSuccess");
pub type PiLsToolCall = ProtoMessage;
pub const PiLsToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiLsToolCall");
pub type PiReadExecArgs = ProtoMessage;
pub const PiReadExecArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiReadExecArgs");
pub type PiReadExecError = ProtoMessage;
pub const PiReadExecErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiReadExecError");
pub type PiReadExecResult = ProtoMessage;
pub const PiReadExecResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiReadExecResult");
pub type PiReadExecSuccess = ProtoMessage;
pub const PiReadExecSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiReadExecSuccess");
pub type PiReadToolCall = ProtoMessage;
pub const PiReadToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiReadToolCall");
pub type PiTruncation = ProtoMessage;
pub const PiTruncationSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiTruncation");
pub type PiWriteExecArgs = ProtoMessage;
pub const PiWriteExecArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiWriteExecArgs");
pub type PiWriteExecError = ProtoMessage;
pub const PiWriteExecErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiWriteExecError");
pub type PiWriteExecRejected = ProtoMessage;
pub const PiWriteExecRejectedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiWriteExecRejected");
pub type PiWriteExecResult = ProtoMessage;
pub const PiWriteExecResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiWriteExecResult");
pub type PiWriteExecSuccess = ProtoMessage;
pub const PiWriteExecSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiWriteExecSuccess");
pub type PiWriteToolCall = ProtoMessage;
pub const PiWriteToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PiWriteToolCall");
pub type Position = ProtoMessage;
pub const PositionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.Position");
pub type PostToolUseFailureRequestQuery = ProtoMessage;
pub const PostToolUseFailureRequestQuerySchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.PostToolUseFailureRequestQuery");
pub type PostToolUseFailureRequestResponse = ProtoMessage;
pub const PostToolUseFailureRequestResponseSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.PostToolUseFailureRequestResponse");
pub type PostToolUseRequestQuery = ProtoMessage;
pub const PostToolUseRequestQuerySchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PostToolUseRequestQuery");
pub type PostToolUseRequestResponse = ProtoMessage;
pub const PostToolUseRequestResponseSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.PostToolUseRequestResponse");
pub type PreCompactRequestQuery = ProtoMessage;
pub const PreCompactRequestQuerySchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PreCompactRequestQuery");
pub type PreCompactRequestResponse = ProtoMessage;
pub const PreCompactRequestResponseSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.PreCompactRequestResponse");
pub type PreToolUseRequestQuery = ProtoMessage;
pub const PreToolUseRequestQuerySchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PreToolUseRequestQuery");
pub type PreToolUseRequestResponse = ProtoMessage;
pub const PreToolUseRequestResponseSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.PreToolUseRequestResponse");
pub type PrewarmRequest = ProtoMessage;
pub const PrewarmRequestSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.PrewarmRequest");
pub type Range = ProtoMessage;
pub const RangeSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.Range");
pub type ReadArgs = ProtoMessage;
pub const ReadArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadArgs");
pub type ReadError = ProtoMessage;
pub const ReadErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadError");
pub type ReadFileNotFound = ProtoMessage;
pub const ReadFileNotFoundSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadFileNotFound");
pub type ReadInvalidFile = ProtoMessage;
pub const ReadInvalidFileSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadInvalidFile");
pub type ReadLintsToolArgs = ProtoMessage;
pub const ReadLintsToolArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadLintsToolArgs");
pub type ReadLintsToolCall = ProtoMessage;
pub const ReadLintsToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadLintsToolCall");
pub type ReadLintsToolError = ProtoMessage;
pub const ReadLintsToolErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadLintsToolError");
pub type ReadLintsToolResult = ProtoMessage;
pub const ReadLintsToolResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadLintsToolResult");
pub type ReadLintsToolSuccess = ProtoMessage;
pub const ReadLintsToolSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadLintsToolSuccess");
pub type ReadMcpResourceError = ProtoMessage;
pub const ReadMcpResourceErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadMcpResourceError");
pub type ReadMcpResourceExecArgs = ProtoMessage;
pub const ReadMcpResourceExecArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadMcpResourceExecArgs");
pub type ReadMcpResourceExecResult = ProtoMessage;
pub const ReadMcpResourceExecResultSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ReadMcpResourceExecResult");
pub type ReadMcpResourceNotFound = ProtoMessage;
pub const ReadMcpResourceNotFoundSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadMcpResourceNotFound");
pub type ReadMcpResourceRejected = ProtoMessage;
pub const ReadMcpResourceRejectedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadMcpResourceRejected");
pub type ReadMcpResourceSuccess = ProtoMessage;
pub const ReadMcpResourceSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadMcpResourceSuccess");
pub type ReadMcpResourceToolCall = ProtoMessage;
pub const ReadMcpResourceToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadMcpResourceToolCall");
pub type ReadPermissionDenied = ProtoMessage;
pub const ReadPermissionDeniedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadPermissionDenied");
pub type ReadRange = ProtoMessage;
pub const ReadRangeSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadRange");
pub type ReadRejected = ProtoMessage;
pub const ReadRejectedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadRejected");
pub type ReadResult = ProtoMessage;
pub const ReadResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadResult");
pub type ReadSuccess = ProtoMessage;
pub const ReadSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadSuccess");
pub type ReadTodosArgs = ProtoMessage;
pub const ReadTodosArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadTodosArgs");
pub type ReadTodosError = ProtoMessage;
pub const ReadTodosErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadTodosError");
pub type ReadTodosResult = ProtoMessage;
pub const ReadTodosResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadTodosResult");
pub type ReadTodosSuccess = ProtoMessage;
pub const ReadTodosSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadTodosSuccess");
pub type ReadTodosToolCall = ProtoMessage;
pub const ReadTodosToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadTodosToolCall");
pub type ReadToolArgs = ProtoMessage;
pub const ReadToolArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadToolArgs");
pub type ReadToolCall = ProtoMessage;
pub const ReadToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadToolCall");
pub type ReadToolError = ProtoMessage;
pub const ReadToolErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadToolError");
pub type ReadToolResult = ProtoMessage;
pub const ReadToolResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadToolResult");
pub type ReadToolSuccess = ProtoMessage;
pub const ReadToolSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReadToolSuccess");
pub type RecordScreenArgs = ProtoMessage;
pub const RecordScreenArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.RecordScreenArgs");
pub type RecordScreenDiscardSuccess = ProtoMessage;
pub const RecordScreenDiscardSuccessSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.RecordScreenDiscardSuccess");
pub type RecordScreenFailure = ProtoMessage;
pub const RecordScreenFailureSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.RecordScreenFailure");
pub type RecordScreenResult = ProtoMessage;
pub const RecordScreenResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.RecordScreenResult");
pub type RecordScreenSaveSuccess = ProtoMessage;
pub const RecordScreenSaveSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.RecordScreenSaveSuccess");
pub type RecordScreenStartSuccess = ProtoMessage;
pub const RecordScreenStartSuccessSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.RecordScreenStartSuccess");
pub type RecordScreenToolCall = ProtoMessage;
pub const RecordScreenToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.RecordScreenToolCall");
pub type ReflectArgs = ProtoMessage;
pub const ReflectArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReflectArgs");
pub type ReflectError = ProtoMessage;
pub const ReflectErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReflectError");
pub type ReflectResult = ProtoMessage;
pub const ReflectResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReflectResult");
pub type ReflectSuccess = ProtoMessage;
pub const ReflectSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReflectSuccess");
pub type ReflectToolCall = ProtoMessage;
pub const ReflectToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ReflectToolCall");
pub type RepositoryIndexingInfo = ProtoMessage;
pub const RepositoryIndexingInfoSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.RepositoryIndexingInfo");
pub type RequestContext = ProtoMessage;
pub const RequestContextSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.RequestContext");
pub type RequestContextArgs = ProtoMessage;
pub const RequestContextArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.RequestContextArgs");
pub type RequestContextEnv = ProtoMessage;
pub const RequestContextEnvSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.RequestContextEnv");
pub type RequestContextError = ProtoMessage;
pub const RequestContextErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.RequestContextError");
pub type RequestContextRejected = ProtoMessage;
pub const RequestContextRejectedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.RequestContextRejected");
pub type RequestContextResult = ProtoMessage;
pub const RequestContextResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.RequestContextResult");
pub type RequestContextSuccess = ProtoMessage;
pub const RequestContextSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.RequestContextSuccess");
pub type RequestedModel = ProtoMessage;
pub const RequestedModelSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.RequestedModel");
pub type RequestedModel_ModelParameterbytes = ProtoMessage;
pub const RequestedModel_ModelParameterbytesSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.RequestedModel_ModelParameterbytes");
pub type ResumeAction = ProtoMessage;
pub const ResumeActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ResumeAction");
pub type SandboxPolicy = ProtoMessage;
pub const SandboxPolicySchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SandboxPolicy");
pub type ScreenshotAction = ProtoMessage;
pub const ScreenshotActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ScreenshotAction");
pub type ScrollAction = ProtoMessage;
pub const ScrollActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ScrollAction");
pub type SearchConversationsToolCall = ProtoMessage;
pub const SearchConversationsToolCallSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SearchConversationsToolCall");
pub type SelectedCodeSelection = ProtoMessage;
pub const SelectedCodeSelectionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SelectedCodeSelection");
pub type SelectedConsoleLog = ProtoMessage;
pub const SelectedConsoleLogSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SelectedConsoleLog");
pub type SelectedContext = ProtoMessage;
pub const SelectedContextSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SelectedContext");
pub type SelectedCursorCommand = ProtoMessage;
pub const SelectedCursorCommandSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SelectedCursorCommand");
pub type SelectedCursorRule = ProtoMessage;
pub const SelectedCursorRuleSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SelectedCursorRule");
pub type SelectedDocumentation = ProtoMessage;
pub const SelectedDocumentationSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SelectedDocumentation");
pub type SelectedExternalLink = ProtoMessage;
pub const SelectedExternalLinkSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SelectedExternalLink");
pub type SelectedFile = ProtoMessage;
pub const SelectedFileSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SelectedFile");
pub type SelectedFolder = ProtoMessage;
pub const SelectedFolderSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SelectedFolder");
pub type SelectedGitCommit = ProtoMessage;
pub const SelectedGitCommitSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SelectedGitCommit");
pub type SelectedGitDiff = ProtoMessage;
pub const SelectedGitDiffSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SelectedGitDiff");
pub type SelectedGitDiffFromBranchToMain = ProtoMessage;
pub const SelectedGitDiffFromBranchToMainSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SelectedGitDiffFromBranchToMain");
pub type SelectedGitPRDiffSelection = ProtoMessage;
pub const SelectedGitPRDiffSelectionSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SelectedGitPRDiffSelection");
pub type SelectedImage = ProtoMessage;
pub const SelectedImageSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SelectedImage");
pub type SelectedImage_BlobIdWithData = ProtoMessage;
pub const SelectedImage_BlobIdWithDataSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SelectedImage_BlobIdWithData");
pub type SelectedImage_Dimension = ProtoMessage;
pub const SelectedImage_DimensionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SelectedImage_Dimension");
pub type SelectedPastChat = ProtoMessage;
pub const SelectedPastChatSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SelectedPastChat");
pub type SelectedPullRequest = ProtoMessage;
pub const SelectedPullRequestSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SelectedPullRequest");
pub type SelectedSubagent = ProtoMessage;
pub const SelectedSubagentSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SelectedSubagent");
pub type SelectedTerminal = ProtoMessage;
pub const SelectedTerminalSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SelectedTerminal");
pub type SelectedTerminalSelection = ProtoMessage;
pub const SelectedTerminalSelectionSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SelectedTerminalSelection");
pub type SelectedUIElement = ProtoMessage;
pub const SelectedUIElementSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SelectedUIElement");
pub type SemSearchToolArgs = ProtoMessage;
pub const SemSearchToolArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SemSearchToolArgs");
pub type SemSearchToolCall = ProtoMessage;
pub const SemSearchToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SemSearchToolCall");
pub type SemSearchToolError = ProtoMessage;
pub const SemSearchToolErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SemSearchToolError");
pub type SemSearchToolResult = ProtoMessage;
pub const SemSearchToolResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SemSearchToolResult");
pub type SemSearchToolSuccess = ProtoMessage;
pub const SemSearchToolSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SemSearchToolSuccess");
pub type SetBlobArgs = ProtoMessage;
pub const SetBlobArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SetBlobArgs");
pub type SetBlobResult = ProtoMessage;
pub const SetBlobResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SetBlobResult");
pub type SetupVmEnvironmentArgs = ProtoMessage;
pub const SetupVmEnvironmentArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SetupVmEnvironmentArgs");
pub type SetupVmEnvironmentResult = ProtoMessage;
pub const SetupVmEnvironmentResultSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SetupVmEnvironmentResult");
pub type SetupVmEnvironmentSuccess = ProtoMessage;
pub const SetupVmEnvironmentSuccessSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SetupVmEnvironmentSuccess");
pub type SetupVmEnvironmentToolCall = ProtoMessage;
pub const SetupVmEnvironmentToolCallSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SetupVmEnvironmentToolCall");
pub type ShellAllowlistPrecheckArgs = ProtoMessage;
pub const ShellAllowlistPrecheckArgsSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ShellAllowlistPrecheckArgs");
pub type ShellAllowlistPrecheckResult = ProtoMessage;
pub const ShellAllowlistPrecheckResultSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ShellAllowlistPrecheckResult");
pub type ShellArgs = ProtoMessage;
pub const ShellArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellArgs");
pub type ShellCommand = ProtoMessage;
pub const ShellCommandSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellCommand");
pub type ShellCommandAction = ProtoMessage;
pub const ShellCommandActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellCommandAction");
pub type ShellCommandParsingResult = ProtoMessage;
pub const ShellCommandParsingResultSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ShellCommandParsingResult");
pub type ShellCommandParsingResult_ExecutableCommand = ProtoMessage;
pub const ShellCommandParsingResult_ExecutableCommandSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ShellCommandParsingResult_ExecutableCommand");
pub type ShellCommandParsingResult_ExecutableCommandArg = ProtoMessage;
pub const ShellCommandParsingResult_ExecutableCommandArgSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ShellCommandParsingResult_ExecutableCommandArg");
pub type ShellCommandParsingResult_Redirect = ProtoMessage;
pub const ShellCommandParsingResult_RedirectSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ShellCommandParsingResult_Redirect");
pub type ShellConversationTurnStructure = ProtoMessage;
pub const ShellConversationTurnStructureSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ShellConversationTurnStructure");
pub type ShellFailure = ProtoMessage;
pub const ShellFailureSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellFailure");
pub type ShellHookApprovalRequirement = ProtoMessage;
pub const ShellHookApprovalRequirementSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ShellHookApprovalRequirement");
pub type ShellOutputDeltaUpdate = ProtoMessage;
pub const ShellOutputDeltaUpdateSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellOutputDeltaUpdate");
pub type ShellOutputNotificationConfig = ProtoMessage;
pub const ShellOutputNotificationConfigSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ShellOutputNotificationConfig");
pub type ShellPermissionDenied = ProtoMessage;
pub const ShellPermissionDeniedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellPermissionDenied");
pub type ShellRejected = ProtoMessage;
pub const ShellRejectedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellRejected");
pub type ShellResult = ProtoMessage;
pub const ShellResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellResult");
pub type ShellSpawnError = ProtoMessage;
pub const ShellSpawnErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellSpawnError");
pub type ShellStream = ProtoMessage;
pub const ShellStreamSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellStream");
pub type ShellStreamBackgrounded = ProtoMessage;
pub const ShellStreamBackgroundedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellStreamBackgrounded");
pub type ShellStreamExit = ProtoMessage;
pub const ShellStreamExitSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellStreamExit");
pub type ShellStreamHookContext = ProtoMessage;
pub const ShellStreamHookContextSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellStreamHookContext");
pub type ShellStreamStart = ProtoMessage;
pub const ShellStreamStartSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellStreamStart");
pub type ShellStreamStderr = ProtoMessage;
pub const ShellStreamStderrSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellStreamStderr");
pub type ShellStreamStdout = ProtoMessage;
pub const ShellStreamStdoutSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellStreamStdout");
pub type ShellSuccess = ProtoMessage;
pub const ShellSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellSuccess");
pub type ShellTimeout = ProtoMessage;
pub const ShellTimeoutSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellTimeout");
pub type ShellToolCall = ProtoMessage;
pub const ShellToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellToolCall");
pub type ShellToolCallDelta = ProtoMessage;
pub const ShellToolCallDeltaSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ShellToolCallDelta");
pub type ShellToolCallStderrDelta = ProtoMessage;
pub const ShellToolCallStderrDeltaSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ShellToolCallStderrDelta");
pub type ShellToolCallStdoutDelta = ProtoMessage;
pub const ShellToolCallStdoutDeltaSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.ShellToolCallStdoutDelta");
pub type SkillDescriptor = ProtoMessage;
pub const SkillDescriptorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SkillDescriptor");
pub type SkillOptions = ProtoMessage;
pub const SkillOptionsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SkillOptions");
pub type SmartModeApproval = ProtoMessage;
pub const SmartModeApprovalSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SmartModeApproval");
pub type SmartModeClassifierArgs = ProtoMessage;
pub const SmartModeClassifierArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SmartModeClassifierArgs");
pub type SmartModeClassifierConversationMessage = ProtoMessage;
pub const SmartModeClassifierConversationMessageSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SmartModeClassifierConversationMessage");
pub type SmartModeClassifierError = ProtoMessage;
pub const SmartModeClassifierErrorSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SmartModeClassifierError");
pub type SmartModeClassifierResult = ProtoMessage;
pub const SmartModeClassifierResultSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SmartModeClassifierResult");
pub type SmartModeClassifierSuccess = ProtoMessage;
pub const SmartModeClassifierSuccessSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SmartModeClassifierSuccess");
pub type SmartModeRiskTarget = ProtoMessage;
pub const SmartModeRiskTargetSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SmartModeRiskTarget");
pub type SpanContext = ProtoMessage;
pub const SpanContextSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SpanContext");
pub type StackTrace = ProtoMessage;
pub const StackTraceSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.StackTrace");
pub type StartGrindExecutionArgs = ProtoMessage;
pub const StartGrindExecutionArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.StartGrindExecutionArgs");
pub type StartGrindExecutionError = ProtoMessage;
pub const StartGrindExecutionErrorSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.StartGrindExecutionError");
pub type StartGrindExecutionResult = ProtoMessage;
pub const StartGrindExecutionResultSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.StartGrindExecutionResult");
pub type StartGrindExecutionSuccess = ProtoMessage;
pub const StartGrindExecutionSuccessSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.StartGrindExecutionSuccess");
pub type StartGrindExecutionToolCall = ProtoMessage;
pub const StartGrindExecutionToolCallSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.StartGrindExecutionToolCall");
pub type StartGrindPlanningArgs = ProtoMessage;
pub const StartGrindPlanningArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.StartGrindPlanningArgs");
pub type StartGrindPlanningError = ProtoMessage;
pub const StartGrindPlanningErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.StartGrindPlanningError");
pub type StartGrindPlanningResult = ProtoMessage;
pub const StartGrindPlanningResultSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.StartGrindPlanningResult");
pub type StartGrindPlanningSuccess = ProtoMessage;
pub const StartGrindPlanningSuccessSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.StartGrindPlanningSuccess");
pub type StartGrindPlanningToolCall = ProtoMessage;
pub const StartGrindPlanningToolCallSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.StartGrindPlanningToolCall");
pub type StartPlanAction = ProtoMessage;
pub const StartPlanActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.StartPlanAction");
pub type StepCompletedUpdate = ProtoMessage;
pub const StepCompletedUpdateSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.StepCompletedUpdate");
pub type StepStartedUpdate = ProtoMessage;
pub const StepStartedUpdateSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.StepStartedUpdate");
pub type StepTiming = ProtoMessage;
pub const StepTimingSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.StepTiming");
pub type StopRequestQuery = ProtoMessage;
pub const StopRequestQuerySchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.StopRequestQuery");
pub type StopRequestResponse = ProtoMessage;
pub const StopRequestResponseSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.StopRequestResponse");
pub type SubagentArgs = ProtoMessage;
pub const SubagentArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SubagentArgs");
pub type SubagentAwaitArgs = ProtoMessage;
pub const SubagentAwaitArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SubagentAwaitArgs");
pub type SubagentAwaitComplete = ProtoMessage;
pub const SubagentAwaitCompleteSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SubagentAwaitComplete");
pub type SubagentAwaitError = ProtoMessage;
pub const SubagentAwaitErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SubagentAwaitError");
pub type SubagentAwaitNotFound = ProtoMessage;
pub const SubagentAwaitNotFoundSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SubagentAwaitNotFound");
pub type SubagentAwaitResult = ProtoMessage;
pub const SubagentAwaitResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SubagentAwaitResult");
pub type SubagentAwaitStillRunning = ProtoMessage;
pub const SubagentAwaitStillRunningSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SubagentAwaitStillRunning");
pub type SubagentError = ProtoMessage;
pub const SubagentErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SubagentError");
pub type SubagentPersistedState = ProtoMessage;
pub const SubagentPersistedStateSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SubagentPersistedState");
pub type SubagentResult = ProtoMessage;
pub const SubagentResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SubagentResult");
pub type SubagentStartRequestQuery = ProtoMessage;
pub const SubagentStartRequestQuerySchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SubagentStartRequestQuery");
pub type SubagentStartRequestResponse = ProtoMessage;
pub const SubagentStartRequestResponseSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SubagentStartRequestResponse");
pub type SubagentStopRequestQuery = ProtoMessage;
pub const SubagentStopRequestQuerySchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SubagentStopRequestQuery");
pub type SubagentStopRequestResponse = ProtoMessage;
pub const SubagentStopRequestResponseSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SubagentStopRequestResponse");
pub type SubagentSuccess = ProtoMessage;
pub const SubagentSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SubagentSuccess");
pub type SubagentType = ProtoMessage;
pub const SubagentTypeSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SubagentType");
pub type SubagentTypeComputerUse = ProtoMessage;
pub const SubagentTypeComputerUseSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SubagentTypeComputerUse");
pub type SubagentTypeCustom = ProtoMessage;
pub const SubagentTypeCustomSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SubagentTypeCustom");
pub type SubagentTypeExplore = ProtoMessage;
pub const SubagentTypeExploreSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SubagentTypeExplore");
pub type SubagentTypeUnspecified = ProtoMessage;
pub const SubagentTypeUnspecifiedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SubagentTypeUnspecified");
pub type SummarizeAction = ProtoMessage;
pub const SummarizeActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SummarizeAction");
pub type SummaryCompletedUpdate = ProtoMessage;
pub const SummaryCompletedUpdateSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SummaryCompletedUpdate");
pub type SummaryStartedUpdate = ProtoMessage;
pub const SummaryStartedUpdateSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SummaryStartedUpdate");
pub type SummaryUpdate = ProtoMessage;
pub const SummaryUpdateSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SummaryUpdate");
pub type SwitchModeArgs = ProtoMessage;
pub const SwitchModeArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SwitchModeArgs");
pub type SwitchModeError = ProtoMessage;
pub const SwitchModeErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SwitchModeError");
pub type SwitchModeRejected = ProtoMessage;
pub const SwitchModeRejectedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SwitchModeRejected");
pub type SwitchModeRequestQuery = ProtoMessage;
pub const SwitchModeRequestQuerySchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SwitchModeRequestQuery");
pub type SwitchModeRequestResponse = ProtoMessage;
pub const SwitchModeRequestResponseSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SwitchModeRequestResponse");
pub type SwitchModeRequestResponse_Approved = ProtoMessage;
pub const SwitchModeRequestResponse_ApprovedSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SwitchModeRequestResponse_Approved");
pub type SwitchModeRequestResponse_Rejected = ProtoMessage;
pub const SwitchModeRequestResponse_RejectedSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.SwitchModeRequestResponse_Rejected");
pub type SwitchModeResult = ProtoMessage;
pub const SwitchModeResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SwitchModeResult");
pub type SwitchModeSuccess = ProtoMessage;
pub const SwitchModeSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SwitchModeSuccess");
pub type SwitchModeToolCall = ProtoMessage;
pub const SwitchModeToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.SwitchModeToolCall");
pub type TaskArgs = ProtoMessage;
pub const TaskArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.TaskArgs");
pub type TaskError = ProtoMessage;
pub const TaskErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.TaskError");
pub type TaskResult = ProtoMessage;
pub const TaskResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.TaskResult");
pub type TaskSuccess = ProtoMessage;
pub const TaskSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.TaskSuccess");
pub type TaskToolCall = ProtoMessage;
pub const TaskToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.TaskToolCall");
pub type TaskToolCallDelta = ProtoMessage;
pub const TaskToolCallDeltaSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.TaskToolCallDelta");
pub type TerminalMetadata = ProtoMessage;
pub const TerminalMetadataSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.TerminalMetadata");
pub type TerminalMetadata_Command = ProtoMessage;
pub const TerminalMetadata_CommandSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.TerminalMetadata_Command");
pub type TextDeltaUpdate = ProtoMessage;
pub const TextDeltaUpdateSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.TextDeltaUpdate");
pub type ThinkingCompletedUpdate = ProtoMessage;
pub const ThinkingCompletedUpdateSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ThinkingCompletedUpdate");
pub type ThinkingDeltaUpdate = ProtoMessage;
pub const ThinkingDeltaUpdateSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ThinkingDeltaUpdate");
pub type ThinkingDetails = ProtoMessage;
pub const ThinkingDetailsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ThinkingDetails");
pub type ThinkingMessage = ProtoMessage;
pub const ThinkingMessageSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ThinkingMessage");
pub type TodoItem = ProtoMessage;
pub const TodoItemSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.TodoItem");
pub type TokenDeltaUpdate = ProtoMessage;
pub const TokenDeltaUpdateSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.TokenDeltaUpdate");
pub type ToolCall = ProtoMessage;
pub const ToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ToolCall");
pub type ToolCallCompletedUpdate = ProtoMessage;
pub const ToolCallCompletedUpdateSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ToolCallCompletedUpdate");
pub type ToolCallDelta = ProtoMessage;
pub const ToolCallDeltaSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ToolCallDelta");
pub type ToolCallDeltaUpdate = ProtoMessage;
pub const ToolCallDeltaUpdateSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ToolCallDeltaUpdate");
pub type ToolCallStartedUpdate = ProtoMessage;
pub const ToolCallStartedUpdateSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.ToolCallStartedUpdate");
pub type TruncatedToolCall = ProtoMessage;
pub const TruncatedToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.TruncatedToolCall");
pub type TruncatedToolCallArgs = ProtoMessage;
pub const TruncatedToolCallArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.TruncatedToolCallArgs");
pub type TruncatedToolCallError = ProtoMessage;
pub const TruncatedToolCallErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.TruncatedToolCallError");
pub type TruncatedToolCallResult = ProtoMessage;
pub const TruncatedToolCallResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.TruncatedToolCallResult");
pub type TruncatedToolCallSuccess = ProtoMessage;
pub const TruncatedToolCallSuccessSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.TruncatedToolCallSuccess");
pub type TurnEndedUpdate = ProtoMessage;
pub const TurnEndedUpdateSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.TurnEndedUpdate");
pub type TypeAction = ProtoMessage;
pub const TypeActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.TypeAction");
pub type UpdateTodosArgs = ProtoMessage;
pub const UpdateTodosArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.UpdateTodosArgs");
pub type UpdateTodosError = ProtoMessage;
pub const UpdateTodosErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.UpdateTodosError");
pub type UpdateTodosResult = ProtoMessage;
pub const UpdateTodosResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.UpdateTodosResult");
pub type UpdateTodosSuccess = ProtoMessage;
pub const UpdateTodosSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.UpdateTodosSuccess");
pub type UpdateTodosToolCall = ProtoMessage;
pub const UpdateTodosToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.UpdateTodosToolCall");
pub type UserMessage = ProtoMessage;
pub const UserMessageSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.UserMessage");
pub type UserMessageAction = ProtoMessage;
pub const UserMessageActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.UserMessageAction");
pub type UserMessageAppendedUpdate = ProtoMessage;
pub const UserMessageAppendedUpdateSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.UserMessageAppendedUpdate");
pub type WaitAction = ProtoMessage;
pub const WaitActionSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WaitAction");
pub type WebFetchAllowlistPrecheckArgs = ProtoMessage;
pub const WebFetchAllowlistPrecheckArgsSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.WebFetchAllowlistPrecheckArgs");
pub type WebFetchAllowlistPrecheckResult = ProtoMessage;
pub const WebFetchAllowlistPrecheckResultSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.WebFetchAllowlistPrecheckResult");
pub type WebFetchRequestQuery = ProtoMessage;
pub const WebFetchRequestQuerySchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WebFetchRequestQuery");
pub type WebFetchRequestResponse = ProtoMessage;
pub const WebFetchRequestResponseSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WebFetchRequestResponse");
pub type WebFetchRequestResponse_Approved = ProtoMessage;
pub const WebFetchRequestResponse_ApprovedSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.WebFetchRequestResponse_Approved");
pub type WebFetchRequestResponse_Rejected = ProtoMessage;
pub const WebFetchRequestResponse_RejectedSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.WebFetchRequestResponse_Rejected");
pub type WebSearchArgs = ProtoMessage;
pub const WebSearchArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WebSearchArgs");
pub type WebSearchError = ProtoMessage;
pub const WebSearchErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WebSearchError");
pub type WebSearchReference = ProtoMessage;
pub const WebSearchReferenceSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WebSearchReference");
pub type WebSearchRejected = ProtoMessage;
pub const WebSearchRejectedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WebSearchRejected");
pub type WebSearchRequestQuery = ProtoMessage;
pub const WebSearchRequestQuerySchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WebSearchRequestQuery");
pub type WebSearchRequestResponse = ProtoMessage;
pub const WebSearchRequestResponseSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.WebSearchRequestResponse");
pub type WebSearchRequestResponse_Approved = ProtoMessage;
pub const WebSearchRequestResponse_ApprovedSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.WebSearchRequestResponse_Approved");
pub type WebSearchRequestResponse_Rejected = ProtoMessage;
pub const WebSearchRequestResponse_RejectedSchema: SchemaHandle =
    SchemaHandle::new("cursor", "agent.v1.WebSearchRequestResponse_Rejected");
pub type WebSearchResult = ProtoMessage;
pub const WebSearchResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WebSearchResult");
pub type WebSearchSuccess = ProtoMessage;
pub const WebSearchSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WebSearchSuccess");
pub type WebSearchToolCall = ProtoMessage;
pub const WebSearchToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WebSearchToolCall");
pub type WriteArgs = ProtoMessage;
pub const WriteArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WriteArgs");
pub type WriteError = ProtoMessage;
pub const WriteErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WriteError");
pub type WriteNoSpace = ProtoMessage;
pub const WriteNoSpaceSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WriteNoSpace");
pub type WritePermissionDenied = ProtoMessage;
pub const WritePermissionDeniedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WritePermissionDenied");
pub type WriteRejected = ProtoMessage;
pub const WriteRejectedSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WriteRejected");
pub type WriteResult = ProtoMessage;
pub const WriteResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WriteResult");
pub type WriteShellStdinArgs = ProtoMessage;
pub const WriteShellStdinArgsSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WriteShellStdinArgs");
pub type WriteShellStdinError = ProtoMessage;
pub const WriteShellStdinErrorSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WriteShellStdinError");
pub type WriteShellStdinResult = ProtoMessage;
pub const WriteShellStdinResultSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WriteShellStdinResult");
pub type WriteShellStdinSuccess = ProtoMessage;
pub const WriteShellStdinSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WriteShellStdinSuccess");
pub type WriteShellStdinToolCall = ProtoMessage;
pub const WriteShellStdinToolCallSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WriteShellStdinToolCall");
pub type WriteSuccess = ProtoMessage;
pub const WriteSuccessSchema: SchemaHandle = SchemaHandle::new("cursor", "agent.v1.WriteSuccess");
pub mod CommandClassifierResult_SuggestedSandboxMode {
    pub const UNSPECIFIED: i32 = 0;
    pub const SANDBOX: i32 = 1;
    pub const NO_SANDBOX: i32 = 2;
    pub const UNDETERMINED: i32 = 3;
}
pub mod ConversationSearchSource {
    pub const UNSPECIFIED: i32 = 0;
    pub const LOCAL: i32 = 1;
    pub const CLOUD_CACHE: i32 = 2;
}
pub mod CursorRuleSource {
    pub const UNSPECIFIED: i32 = 0;
    pub const TEAM: i32 = 1;
    pub const USER: i32 = 2;
}
pub mod DiagnosticSeverity {
    pub const UNSPECIFIED: i32 = 0;
    pub const ERROR: i32 = 1;
    pub const WARNING: i32 = 2;
    pub const INFORMATION: i32 = 3;
    pub const HINT: i32 = 4;
}
pub mod ForceBackgroundShellStatus {
    pub const UNSPECIFIED: i32 = 0;
    pub const ACCEPTED: i32 = 1;
    pub const NOT_FOUND: i32 = 2;
}
pub mod ForceBackgroundSubagentStatus {
    pub const UNSPECIFIED: i32 = 0;
    pub const ACCEPTED: i32 = 1;
    pub const NOT_FOUND: i32 = 2;
}
pub mod GetDiffRequest_OutputFormat {
    pub const UNSPECIFIED: i32 = 0;
    pub const NAME_STATUS: i32 = 1;
    pub const NAME_STATUS_AND_NUMSTAT: i32 = 2;
    pub const FILE_DIFFS: i32 = 3;
    pub const DIFFS_WITH_BEFORE_AND_AFTER: i32 = 4;
}
pub mod GitDiff_DiffType {
    pub const UNSPECIFIED: i32 = 0;
    pub const DIFF_TO_HEAD: i32 = 1;
    pub const DIFF_FROM_BRANCH_TO_MAIN: i32 = 2;
}
pub mod ShellBackgroundReason {
    pub const UNSPECIFIED: i32 = 0;
    pub const TIMEOUT: i32 = 1;
    pub const USER_REQUEST: i32 = 2;
}
pub mod ShellHookApprovalRequirement_Kind {
    pub const UNSPECIFIED: i32 = 0;
    pub const FORCE_PROMPT: i32 = 1;
}
pub mod SmartModeClassifierDecision {
    pub const UNSPECIFIED: i32 = 0;
    pub const ALLOW: i32 = 1;
    pub const BLOCK: i32 = 2;
}
pub mod SubagentBackgroundReason {
    pub const UNSPECIFIED: i32 = 0;
    pub const AGENT_REQUEST: i32 = 1;
    pub const USER_REQUEST: i32 = 2;
    pub const QUEUED_FOLLOW_UP: i32 = 3;
}
