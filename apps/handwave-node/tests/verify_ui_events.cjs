const fs = require('node:fs');
const vm = require('node:vm');
const crypto = require('node:crypto');
const [appFile, transcriptFile, outputFile] = process.argv.slice(2);
const source = fs.readFileSync(appFile, 'utf8');
function extract(name, next) {
  const start = source.indexOf('function ' + name + '(');
  const end = source.indexOf('\nfunction ' + next + '(', start);
  if (start < 0 || end < 0) throw new Error('Missing exact deployed reducer: ' + name);
  return source.slice(start, end);
}
const reducers = extract('agentIdentity', 'recordAgentEvent') + '\n'
  + extract('recordAgentEvent', 'completeCoordinator') + '\n'
  + extract('onLocalAgentEvent', 'formatTaskDur');
const transcript = JSON.parse(fs.readFileSync(transcriptFile, 'utf8'));
const pending = new Map();
const runs = [];
for (const frame of transcript) {
  if (!frame.id) continue;
  if (!pending.has(frame.id)) pending.set(frame.id, []);
  const frames = pending.get(frame.id);
  frames.push(frame);
  // Same wire ID may be retried after completion. Split distinct attempts at
  // an actual terminal end followed by its out, keeping earlier bad-packet
  // denial out packets inside the still-active original attempt.
  if (frame.k === 'out' && frame.done && frames.some(f => f.event?.type === 'agent_end')) {
    runs.push([frame.id, frames]);
    pending.delete(frame.id);
  }
}
const checked = [];
for (const [id, frames] of runs) {
  const terminal = frames.find(f => f.k === 'agent-event' && f.event?.type === 'agent_end');
  if (!terminal) continue;
  if (frames.filter(f => f.event?.type === 'agent_end').length !== 1) throw new Error('Multiple local end events for ' + id);
  const ctx = {
    agentRun: {agents: new Map(), events: []}, localAnswer: '', localPhase: '',
    localStream: null, localTasks: new Map(), localBash: '', pendingAsk: null,
    TERMINAL_TASK: {done: 1, failed: 1, canceled: 1},
    beginAssistant: () => ({append() {}, finalize() {}}), finishLocalRun() {},
    renderAgentDock() {}, $() { return null; },
  };
  vm.createContext(ctx); vm.runInContext(reducers, ctx);
  ctx.recordAgentEvent({type: 'agent_start', status: 'running', agent_id: 'local', agent_name: '本机', agent_role: 'operator', task_id: id});
  let ended = false;
  for (const frame of frames) {
    if (frame.k !== 'agent-event') continue;
    const event = frame.event;
    if (event.agent_id !== 'local' || event.agent_name !== '本机' || event.agent_role !== 'operator' || event.task_id !== id) {
      throw new Error('Incorrect native event identity for ' + id + ': ' + event.type);
    }
    ctx.recordAgentEvent(event);
    if (event.type === 'agent_end') ended = true;
    if (!ended && ctx.agentRun.agents.get('local').status !== 'running') {
      throw new Error('Main card became terminal before agent_end: ' + id + ': ' + event.type);
    }
    if (event.type === 'tool_end' && !ctx.agentRun.events.some(e => e === event && e.status === event.status)) {
      throw new Error('Tool timeline status was lost for ' + id);
    }
  }
  const card = ctx.agentRun.agents.get('local');
  if (ctx.agentRun.agents.size !== 1 || card.status !== terminal.event.status || !['completed', 'failed', 'canceled'].includes(card.status)) {
    throw new Error('Deployed reducer left wrong card status for ' + id);
  }
  const endIndex = frames.indexOf(terminal);
  if (frames.slice(endIndex + 1).some(f => f.k === 'agent-event')) throw new Error('Progress after terminal local end for ' + id);
  const outIndex = frames.findIndex(f => f.k === 'out' && f.done && !f.event);
  // Same-ID malformed-packet fixture emits unrelated rejected out packets while
  // the original job is still active; its actual terminal out is the last one.
  const lastOutIndex = frames.reduce((last, f, i) => f.k === 'out' && f.done ? i : last, -1);
  if (lastOutIndex < endIndex) throw new Error('Terminal out preceded local end for ' + id);
  // Force the concurrent late-notification ordering that is hard to time with a
  // network collector. It must preserve the already-settled main card status.
  for (const type of ['retry', 'steer']) {
    ctx.recordAgentEvent({type, status: type === 'retry' ? 'running' : 'queued', agent_id: 'local', agent_name: '本机', agent_role: 'operator', task_id: id});
    if (card.status !== terminal.event.status) throw new Error('Late notification reopened settled main card: ' + id + ': ' + type);
  }
  checked.push({run: id, status: card.status, cards: [...ctx.agentRun.agents.keys()], end_before_terminal: true});
}
if (!checked.length) throw new Error('No real terminal events to verify');
const result = {status: 'PASS', scope: 'Exact deployed JavaScript reducers with real Rust/Hub event receipts; DOM rendering is stubbed', app_sha256: crypto.createHash('sha256').update(source).digest('hex'), transcript_sha256: crypto.createHash('sha256').update(fs.readFileSync(transcriptFile)).digest('hex'), checked};
fs.writeFileSync(outputFile, JSON.stringify(result, null, 2) + '\n');
console.log(JSON.stringify(result, null, 2));
