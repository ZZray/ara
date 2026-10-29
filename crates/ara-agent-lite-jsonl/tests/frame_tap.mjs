// Transparent stdio relay that records the raw bytes of every frame in both directions.
// `node frame_tap.mjs <sidecar binary> <log>`. Bytes pass unchanged; the exit code is
// the sidecar's. Each relayed line (LF included) appends
// {"from":"host"|"sidecar","base64":...} to <log> before it is relayed.
import { spawn } from 'node:child_process';
import { appendFileSync } from 'node:fs';

const [binary, log] = process.argv.slice(2);
const child = spawn(binary, [], { stdio: ['pipe', 'pipe', 'inherit'], windowsHide: true, env: process.env });
const relay = (input, from, write, end) => {
  let buffered = Buffer.alloc(0);
  input.on('data', (chunk) => {
    buffered = Buffer.concat([buffered, chunk]);
    let newline;
    while ((newline = buffered.indexOf(10)) >= 0) {
      const line = buffered.subarray(0, newline + 1); buffered = buffered.subarray(newline + 1);
      appendFileSync(log, `${JSON.stringify({ from, base64: line.toString('base64') })}\n`); write(line);
    }
  });
  input.on('end', () => {
    if (buffered.length) {
      appendFileSync(log, `${JSON.stringify({ from, base64: buffered.toString('base64') })}\n`); write(buffered);
    }
    end();
  });
};
child.stdin.on('error', () => {});
relay(process.stdin, 'host', (line) => { if (!child.stdin.destroyed) child.stdin.write(line); }, () => child.stdin.end());
relay(child.stdout, 'sidecar', (line) => process.stdout.write(line), () => {});
child.on('close', (code) => { process.stdout.end(() => process.exit(code ?? 1)); });
