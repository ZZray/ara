// Transparent stdio relay that records the kind of every frame in both directions.
// `node kind_proxy.mjs <sidecar binary> <log>`. Bytes pass unchanged; the exit
// code is the sidecar's. Each relayed frame appends {"from":"host"|"sidecar","kind":...}
// to <log>, noted before it is relayed so a reaction is always noted later.
import { spawn } from 'node:child_process';
import { appendFileSync } from 'node:fs';

const [binary, log] = process.argv.slice(2);
const child = spawn(binary, [], { stdio: ['pipe', 'pipe', 'inherit'], windowsHide: true, env: process.env });
const note = (from, line) => {
  let kind;
  try { kind = JSON.parse(line).kind; } catch { kind = '<invalid>'; }
  appendFileSync(log, `${JSON.stringify({ from, kind })}\n`);
};
const relay = (input, from, write, end) => {
  let buffered = Buffer.alloc(0);
  input.on('data', (chunk) => {
    buffered = Buffer.concat([buffered, chunk]);
    let newline;
    while ((newline = buffered.indexOf(10)) >= 0) {
      const line = buffered.subarray(0, newline + 1); buffered = buffered.subarray(newline + 1);
      note(from, line.toString('utf8')); write(line);
    }
  });
  input.on('end', () => { if (buffered.length) write(buffered); end(); });
};
child.stdin.on('error', () => {});
relay(process.stdin, 'host', (line) => { if (!child.stdin.destroyed) child.stdin.write(line); }, () => child.stdin.end());
relay(child.stdout, 'sidecar', (line) => process.stdout.write(line), () => {});
child.on('close', (code) => { process.stdout.end(() => process.exit(code ?? 1)); });
