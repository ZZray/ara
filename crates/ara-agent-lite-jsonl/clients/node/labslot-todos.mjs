// Candidate LabSlot Host binding. Real product tools/authorization/DB services
// are injected; this module does not implement TodoService or start Nest/DB.
import { createHash, randomUUID } from 'node:crypto';

const digest = (value) => createHash('sha256').update(JSON.stringify(value)).digest('hex');
const requireValue = (condition, message) => { if (!condition) throw new Error(message); };
const plain = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const nonempty = (value) => typeof value === 'string' && value.length > 0;
const clone = (value) => JSON.parse(JSON.stringify(value));
const freeze = (value) => {
  if (value && typeof value === 'object') { Object.values(value).forEach(freeze); Object.freeze(value); }
  return value;
};

/**
 * tools are the actual StructuredTool instances from createTodoTools.
 * schemaToJson must convert each original tool.schema, not a second handwritten
 * tool implementation. assertAuthorization closes over the product's captured
 * authority and calls its current assertBoundAuthorization(db, authority).
 *
 * readback() returns {target: actual DB row, protected_rows: actual DB rows}.
 * operations.started/finished/unknown commit product-owned durable receipts.
 * settleTool inspects the real operation/result/readback and returns
 * {is_error:boolean, settlement:'Settled'|'Unknown'}; Chinese success/error text
 * alone is insufficient. commitReview must commit the supplied actual DB proof
 * before returning a nonempty evidence reference.
 */
export async function createLabslotTodoBinding({ tools, schemaToJson, assertAuthorization,
  readback, operations, settleTool, commitReview, turn, criteria }) {
  requireValue(Array.isArray(tools) && typeof schemaToJson === 'function', 'actual product tools and schema converter required');
  requireValue([assertAuthorization, readback, settleTool, commitReview].every((fn) => typeof fn === 'function'), 'actual authorization/readback/settlement/review callbacks required');
  requireValue(plain(operations) && ['started','finished','unknown'].every((key) => typeof operations[key] === 'function'), 'durable product operation callbacks required');
  requireValue(plain(turn) && nonempty(turn.actor_id) && nonempty(turn.conversation_id)
    && Number.isSafeInteger(turn.user_message_ordinal) && turn.user_message_ordinal >= 0, 'captured real user turn required');
  requireValue(plain(criteria) && nonempty(criteria.todo_id)
    && ['urgent','high','medium','low'].includes(criteria.expected_priority), 'Host-selected todo criteria required');
  // This is a continuity declaration, not proof that the caller is authorized.
  // Actual authority remains in the product closure and domain service.
  const capturedTurn = freeze(clone(turn));
  const capturedCriteria = freeze(clone(criteria));
  const registry = new Map(); const configurations = [];
  for (const name of ['list_user_todos','update_todo']) {
    const matches = tools.filter((tool) => tool.name === name);
    requireValue(matches.length === 1 && typeof matches[0].invoke === 'function', `one actual ${name} StructuredTool required`);
    const tool = matches[0]; const original = await schemaToJson(tool.schema);
    requireValue(plain(original) && original.type === 'object' && plain(original.properties), 'product schema conversion did not produce an object');
    const parameters = clone(original);
    if (name === 'update_todo') {
      requireValue(plain(parameters.properties.todoId) && plain(parameters.properties.priority), 'current LabSlot todo schema fields missing');
      // Narrow the real schema to this user-authorized assignment. We neither
      // duplicate domain validation nor expose title/status/batch mutations.
      parameters.properties = { todoId: { ...parameters.properties.todoId, enum: [capturedCriteria.todo_id] },
        priority: parameters.properties.priority };
      parameters.required = ['todoId','priority']; parameters.additionalProperties = false;
    }
    registry.set(name, tool);
    configurations.push({ name, description: `${tool.description ?? name}\nHost scope: only the captured user's selected todo priority; no device or code operations.`,
      parameters, allow_in_plan: name === 'list_user_todos' });
  }
  await assertAuthorization();
  const baseline = clone(await readback());
  requireValue(plain(baseline?.target) && baseline.target.id === capturedCriteria.todo_id
    && baseline.target.userId === capturedTurn.actor_id && Array.isArray(baseline.protected_rows), 'baseline must be actual owned target and protected rows');
  await assertAuthorization();
  let unknown = false;
  const preservedFields = Object.keys(baseline.target).filter((key) => !['priority','updatedAt'].includes(key));
  const context = (frame, signal) => ({ signal, turn: capturedTurn, criteria: capturedCriteria,
    task_id: frame.task_id, run_id: frame.run_id, rpc_id: frame.rpc_id,
    native_call_id: frame.native_call_id, args_sha256: frame.args_sha256 });
  const callbacks = {
    async tool(frame, { signal }) {
      const ctx = context(frame, signal);
      const started = await operations.started({ ...ctx, signal: undefined,
        tool_name: frame.tool_name, arguments: clone(frame.arguments) });
      requireValue(nonempty(started), 'product operation start receipt missing');
      let value; let error = null; let invoked = false; let settlement;
      try {
        signal.throwIfAborted(); await assertAuthorization();
        requireValue(!unknown && registry.has(frame.tool_name), 'tool is not authorized for this task');
        requireValue(plain(frame.arguments), 'tool arguments must be an object');
        const keys = Object.keys(frame.arguments);
        if (frame.tool_name === 'update_todo') requireValue(keys.length === 2
          && keys.includes('todoId') && keys.includes('priority')
          && frame.arguments.todoId === capturedCriteria.todo_id
          && ['urgent','high','medium','low'].includes(frame.arguments.priority), 'update is outside the exact task scope');
        else requireValue(keys.every((key) => ['status','priority','search'].includes(key)), 'unexpected list argument');
        signal.throwIfAborted(); invoked = true;
        // Await the original service promise. Passing signal into a generic
        // LangChain race wrapper would not prove its inner DB query joined.
        value = await registry.get(frame.tool_name).invoke(frame.arguments);
        await assertAuthorization();
      } catch (failure) { error = failure; value = { error: 'Product operation refused or failed; consult its original receipt' }; }
      if (!invoked) settlement = { is_error: true, settlement: 'Settled' };
      else settlement = await settleTool({ ...ctx, tool_name: frame.tool_name,
        arguments: clone(frame.arguments), value, error, readback });
      requireValue(plain(settlement) && typeof settlement.is_error === 'boolean'
        && ['Settled','Unknown'].includes(settlement.settlement), 'product settlement declaration required');
      if (settlement.settlement === 'Unknown') unknown = true;
      let receipt;
      try {
        receipt = await operations.finished(started, { ...ctx, signal: undefined,
          tool_name: frame.tool_name, result_sha256: digest(value), invoked, ...settlement });
      } catch {
        unknown = true;
        receipt = await operations.unknown(started, { category: 'operation_result_persistence_failed' });
        settlement = { is_error: true, settlement: 'Unknown' };
      }
      requireValue(nonempty(receipt), 'product completion/unknown receipt missing');
      return { value, ...settlement, operation_receipt: receipt };
    },
    async verify(candidate, { signal, frame }) {
      signal.throwIfAborted(); await assertAuthorization();
      const actual = clone(await readback());
      await assertAuthorization(); signal.throwIfAborted();
      const checks = {
        correct_owner: actual?.target?.userId === capturedTurn.actor_id,
        correct_target: actual?.target?.id === capturedCriteria.todo_id,
        desired_priority: actual?.target?.priority === capturedCriteria.expected_priority,
        other_target_fields_unchanged: preservedFields.every((key) => digest(actual?.target?.[key] ?? null) === digest(baseline.target[key] ?? null)),
        protected_rows_unchanged: digest(actual?.protected_rows ?? null) === digest(baseline.protected_rows),
      };
      const passed = !unknown && Object.values(checks).every(Boolean);
      const verdict = unknown ? 'Unknown' : passed ? 'Verified'
        : { NeedsChanges: { feedback: 'Direct product database readback does not satisfy the selected todo priority/ownership/preservation criteria. Inspect the authorized todo list and correct only the selected priority.' } };
      const proof = { basis: clone(candidate.basis), checks, actual, baseline_sha256: digest(baseline),
        turn: capturedTurn, criteria: capturedCriteria, settlement: unknown ? 'Unknown' : 'Settled', verdict };
      const evidence = await commitReview(proof, context(frame, signal));
      requireValue(nonempty(evidence), 'actual DB review evidence was not committed');
      return { basis: candidate.basis, committed: true, settlement: proof.settlement, verdict, evidence: [evidence] };
    },
    async prepare(continuation, { signal }) {
      signal.throwIfAborted(); await assertAuthorization();
      requireValue(!unknown && continuation.receipt?.committed === true
        && continuation.receipt.settlement === 'Settled'
        && nonempty(continuation.receipt.verdict?.NeedsChanges?.feedback), 'no automatic continuation after unknown or unreviewed effects');
      // No actor, conversation, ordinal, confirmation, tool or route fields are
      // returned. An automatic Run is never a new human confirmation turn.
      return { run_id_next: `labslot-todo-${randomUUID()}`,
        objective: `Correct the selected todo priority using only list_user_todos and update_todo. Independent Host feedback: ${continuation.receipt.verdict.NeedsChanges.feedback}. The real user turn and all original authority/budgets remain fixed.` };
    },
  };
  return { tools: configurations, callbacks, captured_turn: capturedTurn,
    acceptance_scope: 'candidate binding; actual TodoService/PostgreSQL acceptance requires a separately executed product fixture' };
}
