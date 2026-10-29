// Run only inside the reviewed isolated fixed VT dependency/test directory.
// Reuses the byte-identical fixed adapter and its kitty-vt-wasm engine.
import { readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { VirtualTerminal } from "./virtual-terminal.ts";
import { CELL_U32, KittyTerminal, loadModuleSync } from "kitty-vt-wasm";
const [rawPath, metaPath, resultPath] = Bun.argv.slice(2);
if (!rawPath || !metaPath || !resultPath) throw new Error("raw/meta/result arguments required");
const raw = readFileSync(rawPath);
const data = new TextDecoder("utf-8", {fatal:true}).decode(raw);
const meta = JSON.parse(readFileSync(metaPath, "utf8"));
if (meta.columns !== 80 || meta.rows !== 30) throw new Error("fixed actual Host viewport must be 80x30");
const terminal = new VirtualTerminal(meta.columns, meta.rows);
terminal.write(data); await terminal.flush();
const viewport = terminal.getViewport();
const bufferPosition = terminal.getBufferPosition();
const scrollBuffer = terminal.getScrollBuffer();
const wasm = loadModuleSync(Bun.resolveSync("kitty-vt-wasm/kitty-vt.wasm", import.meta.dir));
const engine = KittyTerminal.createSync({columns:meta.columns, rows:meta.rows, scrollback:1000, wasm, onEvent:()=>{}});
// Same documented grid-neutral stripping as the fixed VirtualTerminal adapter.
engine.write(data.replaceAll("\x1b[?2026h", "").replaceAll("\x1b[?2026l", "").replace(/\x1b\][\s\S]*?(?:\x07|\x1b\\)/g, ""));
const cellWords = Array.from({length:meta.rows}, (_,row)=>Array.from(engine.lineCells(row) ?? []));
const failures:string[] = [];
const observations:any[] = [];
if (bufferPosition.viewportY !== bufferPosition.baseY) failures.push("fixed adapter is not following current viewport tail");
const engineViewport = Array.from({length:meta.rows},(_,row)=>engine.line(row));
if (JSON.stringify(viewport) !== JSON.stringify(engineViewport)) failures.push("fixed adapter/current native engine viewport text mismatch");
const cappedHistory = Math.min(engine.scrollbackLength,1000);
const historyStart = engine.scrollbackLength-cappedHistory;
const scrollCellWords = Array.from({length:cappedHistory+meta.rows},(_,row)=>Array.from(
    row<cappedHistory ? engine.scrollbackLineCells(historyStart+row) ?? [] : engine.lineCells(row-cappedHistory) ?? []));
const engineScrollBuffer = Array.from({length:cappedHistory+meta.rows},(_,row)=>
    row<cappedHistory ? engine.scrollbackLine(historyStart+row) ?? "" : engine.line(row-cappedHistory));
if (JSON.stringify(scrollBuffer) !== JSON.stringify(engineScrollBuffer)) failures.push("fixed adapter/native engine complete scrollbuffer text mismatch");
if (meta.scope !== "viewport" && meta.scope !== "scrollbuffer") throw new Error("explicit observation scope required");
const presentedText = meta.scope === "scrollbuffer" ? scrollBuffer : viewport;
const presentedCells = meta.scope === "scrollbuffer" ? scrollCellWords : cellWords;
for (const check of meta.checks) {
    const found:{row:number,column:number}[] = [];
    for (let row=0;row<presentedText.length;row++) {
        let column=presentedText[row]!.indexOf(check.marker);
        while (column>=0) {
            const before=presentedText[row]![column-1] ?? "";
            const after=presentedText[row]![column+check.marker.length] ?? "";
            if (!/[A-Za-z0-9_]/.test(before) && !/[A-Za-z0-9_]/.test(after)) found.push({row,column});
            column=presentedText[row]!.indexOf(check.marker,column+check.marker.length);
        }
    }
    if (found.length!==1) { failures.push(`${check.marker}: expected one ${meta.scope} occurrence, got ${found.length}`); continue; }
    const {row,column}=found[0]!;
    // Fixture code-body prefixes and markers are ASCII; text index == cell column.
    const characters = Array.from(check.marker, (_,index)=>presentedCells[row]?.[(column+index)*CELL_U32]);
    if (characters.some((word,index)=>word!==check.marker.codePointAt(index))) failures.push(`${check.marker}: cell characters do not match text columns`);
    const observed = Array.from(check.marker, (_,index)=>presentedCells[row]?.[(column+index)*CELL_U32+3]);
    const expected = (0x02000000 | parseInt(check.rgb.slice(1),16)) >>> 0;
    if (observed.some(word=>word!==expected)) failures.push(`${check.marker}: actual foreground differs from ${check.palette} ${check.rgb}`);
    observations.push({marker:check.marker, palette:check.palette, rgb:check.rgb, row,column,expected,foregroundWords:observed,characterWords:characters});
}
const result={phase:meta.phase,rawBytes:raw.length,rawSHA256:createHash("sha256").update(raw).digest("hex"),
    columns:meta.columns,rows:meta.rows,scope:meta.scope,bufferPosition,viewport,cellWords,scrollBuffer,scrollCellWords,observations,failures,passed:failures.length===0,
    boundary:"Actual normal CLI ConPTY bytes; fixed cell engine, no highlighter emitter"};
writeFileSync(resultPath, JSON.stringify(result,null,2));
engine.dispose(); terminal.stop();
if (failures.length) throw new Error(failures.join("; "));
console.log("ARA_NATIVE_MARKDOWN_HOST_CELLS "+JSON.stringify({phase:meta.phase,passed:true,rawBytes:raw.length}));
