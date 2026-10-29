"""Normal Cargo ara binary -> held local SSE -> actual ConPTY/cells -> same-binary Session restore."""
from __future__ import annotations
import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import threading
import time

ADAPTER_SHA = "130833ffa4edd084b9d41be6de32135a900d9f93cd35e03f37cd4081ea480f58"
WASM_SHA = "e85c89f6d22231c4ad753c3d6ed30f69b4526d6f5fd4fdc84ef16ebc8ee2dec1"
TEST_NAME = "normal_cli_native_markdown_progressive_restore"
PTY_SHA = "d9df5deb3ff9f7eb44fae5dd665b0e38a22136b15dcbdfd9b4e8f01289671015"
PROMPT = b"> \x1b[7m \x1b[27m"
STAGES = [
    {"language":"python", "color":"syntaxString", "markers":["PY_INSIDE","PY_PART","PY_SECOND","PY_PART2"],
     "chunks":["```python\nvalue = \"\"\"open\nPY_INSIDE\nPY_PART", "\nPY_SECOND\nPY_PART2", "\n\"\"\"\n```\n"]},
    {"language":"javascript", "color":"syntaxComment", "markers":["JS_INSIDE","JS_PART","JS_SECOND","JS_PART2"],
     "chunks":["```javascript\n/* open\nJS_INSIDE\nJS_PART", "\nJS_SECOND\nJS_PART2", "\n*/\n```\n"]},
]

def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()

class ChildBudget:
    def __init__(self, child, module):
        self.end=time.monotonic()+60; self.expired=threading.Event()
        def terminate():
            self.expired.set()
            if child.exit_code(0) is None: module.k32.TerminateProcess(child.pi.hProcess,98)
        self.timer=threading.Timer(60,terminate);self.timer.daemon=True;self.timer.start()
    def limit(self, seconds):
        remaining=self.end-time.monotonic()
        if self.expired.is_set() or remaining<=0:raise TimeoutError("60s total child bound")
        return min(seconds,remaining)
    def close(self):self.timer.cancel()

def wait_for(predicate, timeout: float, description: str, budget):
    end=time.monotonic()+budget.limit(timeout)
    while time.monotonic()<end:
        value=predicate()
        if value: return value
        time.sleep(.025)
    raise TimeoutError(description)

def wait_event(event, seconds, description, budget):
    if not event.wait(budget.limit(seconds)):raise TimeoutError(description)

def prompt_count(pty):
    with pty.lock:return bytes(pty.raw).count(PROMPT)

def normal_artifact(cargo_json: Path, binary: Path) -> dict:
    records=[json.loads(line) for line in cargo_json.read_text(encoding="utf8").splitlines() if line.strip()]
    artifacts=[record for record in records if record.get("reason")=="compiler-artifact"
               and record.get("target",{}).get("name")=="ara" and "bin" in record.get("target",{}).get("kind",[])
               and record.get("profile",{}).get("test") is False and record.get("executable")]
    unique={str(Path(record["executable"]).resolve()):record for record in artifacts}
    assert len(unique)==1, "Cargo JSON must select one unique normal ara bin, never libtest"
    selected=next(iter(unique.values()))
    assert Path(selected["executable"]).resolve()==binary and binary.name.lower()=="ara.exe"
    assert any(record.get("reason")=="build-finished" and record.get("success") is True for record in records)
    return selected

class HeldUpstream:
    def __init__(self, root: Path):
        self.root=root; self.lock=threading.Lock(); self.calls=0; self.records=[]; self.errors=[]
        self.ready=[[threading.Event() for _ in range(3)] for _ in STAGES]
        self.release=[[threading.Event() for _ in range(3)] for _ in STAGES]
        self.finished=[threading.Event() for _ in STAGES]
        owner=self
        class Handler(BaseHTTPRequestHandler):
            protocol_version="HTTP/1.1"
            def log_message(self, *_): pass
            def do_POST(self):
                try:
                    length=int(self.headers.get("Content-Length","0")); assert 0<length<=2_000_000
                    body=json.loads(self.rfile.read(length)); assert body["stream"] is True
                    assert self.path=="/v1/chat/completions" and body["model"]=="fake-model"
                    assert self.headers.get("Authorization")=="Bearer test"
                    with owner.lock:
                        index=owner.calls; owner.calls+=1
                        owner.records.append({"index":index,"path":self.path,"monotonic":time.monotonic(),"body":body})
                    assert index<len(STAGES), "more than two actual model calls"
                    self.send_response(200); self.send_header("Content-Type","text/event-stream")
                    self.send_header("Connection","close"); self.end_headers()
                    def emit(delta, finish=None):
                        event={"id":f"native-{index}","object":"chat.completion.chunk","model":"fake-model",
                               "choices":[{"index":0,"delta":delta,"finish_reason":finish}]}
                        self.wfile.write(("data: "+json.dumps(event)+"\n\n").encode()); self.wfile.flush()
                    emit({"role":"assistant"})
                    for phase, chunk in enumerate(STAGES[index]["chunks"]):
                        emit({"content":chunk}); owner.ready[index][phase].set()
                        assert owner.release[index][phase].wait(30), "held phase not observed/released"
                    emit({},"stop")
                    self.wfile.write(b'data: {"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":20,"total_tokens":30}}\n\ndata: [DONE]\n\n')
                    self.wfile.flush(); owner.finished[index].set(); self.close_connection=True
                except Exception as error:
                    with owner.lock: owner.errors.append(repr(error))
                    self.close_connection=True
        self.server=ThreadingHTTPServer(("127.0.0.1",0),Handler)
        self.server.daemon_threads=True
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
    def save(self):
        with self.lock:
            (self.root/"upstream.json").write_text(json.dumps({"calls":self.calls,"requests":self.records,"errors":self.errors},indent=2)+"\n",encoding="utf8")
    def close(self):
        for phases in self.release:
            for event in phases: event.set()
        self.server.shutdown();self.server.server_close();self.thread.join(timeout=5);self.save()

def session_receipts(sessions: Path, expected: list[str]):
    matches=[]
    for path in sessions.rglob("*.jsonl"):
        try: rows=[json.loads(line) for line in path.read_text(encoding="utf8").splitlines() if line.strip()]
        except (OSError,ValueError): continue
        actual=[]
        for row in rows:
            message=row.get("message",{})
            if message.get("role")=="assistant" and message.get("stopReason")=="stop":
                actual.append("".join(block.get("text","") for block in message.get("content",[]) if block.get("type")=="text"))
        if all(source in actual for source in expected): matches.append((path,rows))
    assert len(matches)<=1, "ambiguous actual Session file"
    return matches[0] if matches else None

def capture_cells(pty, root: Path, phase: str, checks: list[dict], args, oracle: Path, held: dict, budget, scope="viewport"):
    # Observe bytes while the upstream is still held; do not release on capture failure.
    assert pty.exit_code(0) is None
    marker_started=time.monotonic()
    wait_for(lambda: all(check["marker"] in pty.text() for check in checks),15,f"{phase} actual terminal text",budget)
    marker_elapsed=time.monotonic()-marker_started
    quiet=time.monotonic();previous=None;end=time.monotonic()+budget.limit(5);settled=False
    while time.monotonic()<end:
        with pty.lock: raw=bytes(pty.raw)
        if raw!=previous:previous=raw;quiet=time.monotonic()
        elif time.monotonic()-quiet>=.2:
            try:raw.decode("utf8",errors="strict")
            except UnicodeDecodeError:pass
            else:settled=True;break
        time.sleep(.025)
    assert pty.exit_code(0) is None
    assert settled,"terminal output did not settle as complete UTF-8"
    raw_path=root/f"{phase}.raw.vt";raw_path.write_bytes(raw)
    meta={"phase":phase,"columns":80,"rows":30,"checks":checks,"held":held,
          "monotonic":time.monotonic(),"marker_wait_seconds":marker_elapsed,"pid":pty.pi.dwProcessId,"child_alive":True,"scope":scope}
    meta_path=root/f"{phase}.meta.json";meta_path.write_text(json.dumps(meta,indent=2)+"\n",encoding="utf8")
    command=[str(args.bun),"run",str(oracle),str(raw_path),str(meta_path),str(root/f"{phase}.cells.json")]
    run=subprocess.run(command,cwd=args.dependency,capture_output=True,timeout=budget.limit(15))
    (root/f"{phase}.oracle.stdout").write_bytes(run.stdout);(root/f"{phase}.oracle.stderr").write_bytes(run.stderr)
    assert run.returncode==0,f"{phase} actual cell oracle failed"
    return json.loads((root/f"{phase}.cells.json").read_text(encoding="utf8"))

def main() -> int:
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--binary",type=Path,required=True);ap.add_argument("--binary-sha256",required=True)
    ap.add_argument("--cargo-json",type=Path,required=True);ap.add_argument("--repo",type=Path,required=True)
    ap.add_argument("--dependency",type=Path,required=True);ap.add_argument("--bun",type=Path,required=True)
    ap.add_argument("--oracle-entry",type=Path,required=True);ap.add_argument("--out",type=Path,required=True)
    args=ap.parse_args()
    for name in ("binary","cargo_json","repo","dependency","bun","oracle_entry"):
        setattr(args,name,getattr(args,name).resolve(strict=True))
    args.out=args.out.resolve();assert args.out.drive.lower()=="d:","fresh runtime artifacts must stay D"
    assert args.dependency.drive.lower()=="d:","isolated fixed dependency must stay D"
    args.out.mkdir(parents=True,exist_ok=False)
    report={"entry":TEST_NAME,"state":"NOT_EXECUTED","accepted":False,"model_protocol":"openai-completions","actual_model":False}
    upstream=None;pty=None;restored=None;code=1;started=time.monotonic();budgets=[]
    report["bounds"]={"model_calls":2,"per_run_seconds":30,"per_child_seconds":60,"oracle_seconds":15,"child_count":2}
    try:
        assert sys.platform=="win32", "actual ConPTY gate requires Windows"
        binary=args.binary.resolve(strict=True);assert sha(binary)==args.binary_sha256.lower(), "normal binary SHA mismatch"
        artifact=normal_artifact(args.cargo_json,binary)
        report["cargo_artifact"]=artifact;report["binary_sha256"]=sha(binary);report["cargo_json_sha256"]=sha(args.cargo_json)
        adapter=args.dependency/"test/virtual-terminal.ts";wasm=args.dependency/"node_modules/kitty-vt-wasm/dist/kitty-vt.wasm"
        assert sha(adapter)==ADAPTER_SHA and sha(wasm)==WASM_SHA,"fixed VT dependencies mismatch"
        oracle=args.dependency/"test/ara-native-markdown-host-oracle-root-v3.ts"
        if oracle.exists():assert sha(oracle)==sha(args.oracle_entry),"existing oracle differs"
        else:shutil.copyfile(args.oracle_entry,oracle)
        harness=args.repo/"scripts/windows_conpty_ctrl_c.py"
        assert sha(harness)==PTY_SHA,"reviewed existing ConPTY harness pin mismatch"
        spec=importlib.util.spec_from_file_location("native_md_conpty",harness);mod=importlib.util.module_from_spec(spec);spec.loader.exec_module(mod)
        report["pins"]={"Pty":sha(harness),"oracle":sha(oracle),"fixedAdapter":sha(adapter),"wasm":sha(wasm),"script":sha(Path(__file__))}
        for name in ("home","work","sessions"): (args.out/name).mkdir()
        theme_path=args.repo/"crates/ara-cli/src/terminal/themes/dark.json";theme=json.loads(theme_path.read_text(encoding="utf8"))
        report["pins"]["theme"]=sha(theme_path)
        def rgb(key):
            value=theme["colors"][key];value=theme["vars"].get(value,value)
            assert isinstance(value,str) and len(value)==7 and value.startswith("#")
            return value
        def checks(stage, phase):
            markers=stage["markers"]
            selected=[(markers[0],stage["color"]),(markers[1],"mdCodeBlock")] if phase==0 else [(markers[0],stage["color"]),(markers[2],stage["color"]),(markers[3],"mdCodeBlock")] if phase==1 else [(marker,stage["color"]) for marker in markers]
            return [{"marker":marker,"palette":palette,"rgb":rgb(palette)} for marker,palette in selected]
        upstream=HeldUpstream(args.out)
        blocked={"OPENROUTER_API_KEY","ANTHROPIC_API_KEY","ARA_API_KEY","ARA_TEST_API_KEY","ARA_MODEL","ARA_BASE_URL","ARA_TEST_BASE_URL","OPENROUTER_BASE_URL","ARA_TEST_MODEL_ID","NO_COLOR","CLAUDE_CONFIG_DIR","COPILOT_HOME","CODEX_HOME"}
        env={key:value for key,value in os.environ.items() if key not in blocked}
        env.update({"HOME":str(args.out/"home"),"USERPROFILE":str(args.out/"home"),"ARA_HOME":str(args.out/"home"),"NATIVE_MD_TEST_KEY":"test","ARA_TERMINAL_THEME":"dark","TERM":"xterm-256color","COLORTERM":"truecolor"})
        arguments=[str(binary),"--model","fake-model","--base-url",f"http://127.0.0.1:{upstream.server.server_port}/v1","--api","openai-completions","--api-key-env","NATIVE_MD_TEST_KEY","--session-dir",str(args.out/"sessions"),"--cwd",str(args.out/"work"),"--max-time","30","--max-model-calls","1"]
        report["command"]=arguments;report["state"]="FAIL"
        pty=mod.Pty(subprocess.list2cmdline(arguments),str(args.out/"work"),env,columns=80,rows=30)
        live_budget=ChildBudget(pty,mod);budgets.append(live_budget)
        wait_for(lambda:prompt_count(pty)>0,10,"initial actual idle prompt",live_budget)
        phases=[];sources=[]
        for index,stage in enumerate(STAGES):
            prior_prompt=prompt_count(pty)
            pty.send(f"Render native fixture {stage['language']}\r".encode())
            for phase in range(3):
                wait_event(upstream.ready[index][phase],15,"actual model request/held phase missing",live_budget)
                upstream.save()
                if phase<2:
                    assert not upstream.finished[index].is_set() and not upstream.release[index][phase].is_set()
                    observed=capture_cells(pty,args.out,f"{stage['language']}-active-{phase+1}",checks(stage,phase),args,oracle,{"request":index,"phase":phase,"eos_sent":False,"release_set":False},live_budget)
                    assert not upstream.finished[index].is_set() and not upstream.release[index][phase].is_set()
                    assert pty.exit_code(0) is None,"child exited before active cell observation completed"
                    phases.append(observed)
                upstream.release[index][phase].set()
            wait_event(upstream.finished[index],5,"held upstream did not finish",live_budget)
            sources.append("".join(stage["chunks"]))
            path,entries=wait_for(lambda:session_receipts(args.out/"sessions",sources),10,"final actual persisted assistant receipt",live_budget)
            wait_for(lambda:prompt_count(pty)>prior_prompt,10,"new actual idle prompt after assistant completion",live_budget)
            (args.out/f"{stage['language']}-session.json").write_text(json.dumps({"file":str(path),"entries":entries},indent=2)+"\n",encoding="utf8")
            phases.append(capture_cells(pty,args.out,f"{stage['language']}-final",checks(stage,2),args,oracle,{"request":index,"eos_sent":True,"idle_prompt_count":prompt_count(pty)},live_budget))
        assert upstream.calls==2 and not upstream.errors
        pty.send(b"/exit\r");assert pty.exit_code(live_budget.limit(10))==0,"normal CLI idle exit"
        with pty.lock:(args.out/"full-live-after-exit.raw.vt").write_bytes(bytes(pty.raw))
        pty.close();pty=None;live_budget.close()
        # Same exact normal bin and original Session; restoration must emit no model request.
        path,entries=session_receipts(args.out/"sessions",sources)
        resume=arguments+["--resume",str(path)];report["restore_command"]=resume
        restored=mod.Pty(subprocess.list2cmdline(resume),str(args.out/"work"),env,columns=80,rows=30)
        restore_budget=ChildBudget(restored,mod);budgets.append(restore_budget)
        wait_for(lambda:prompt_count(restored)>0,10,"restored actual idle prompt",restore_budget)
        restore=capture_cells(restored,args.out,"restored",checks(STAGES[0],2)+checks(STAGES[1],2),args,oracle,{"eos_sent":True,"requests_before":upstream.calls},restore_budget,"scrollbuffer")
        assert upstream.calls==2 and not upstream.errors,"restore cannot request a new model conversation"
        final={item["marker"]:item["foregroundWords"] for receipt in phases if receipt["phase"].endswith("-final") for item in receipt["observations"]}
        assert all(final[item["marker"]]==item["foregroundWords"] for item in restore["observations"]),"same source final/restore cell color mismatch"
        restored.send(b"/exit\r");assert restored.exit_code(restore_budget.limit(10))==0
        with restored.lock:(args.out/"full-restore-after-exit.raw.vt").write_bytes(bytes(restored.raw))
        restored.close();restored=None;restore_budget.close()
        report.update({"state":"EXEC_PASS_AUDIT_PENDING","phases":[receipt["phase"] for receipt in phases],"restore":True,"calls":upstream.calls,"session_file":str(path),"source_bodies":sources});code=0
    except Exception as error:
        report["error"]=f"{type(error).__name__}: {error}"
    finally:
        for budget in budgets:budget.close()
        cleanup_errors=[]
        for label,child in (("live",pty),("restore",restored)):
            if child is not None:
                try:
                    with child.lock:(args.out/f"{label}-failure.raw.vt").write_bytes(bytes(child.raw))
                    report[f"{label}_exit_before_cleanup"]=child.exit_code(0)
                except Exception as error:cleanup_errors.append(f"{label} capture: {error!r}")
                try:child.close()
                except Exception as error:cleanup_errors.append(f"{label} close: {error!r}")
        if upstream is not None:
            try:upstream.close()
            except Exception as error:cleanup_errors.append(f"upstream close: {error!r}")
        if cleanup_errors:
            report["cleanup_errors"]=cleanup_errors;report["state"]="FAIL";code=1
        report["wall_seconds"]=time.monotonic()-started
        report["artifacts"]=[{"path":str(path.relative_to(args.out)),"sha256":sha(path),"bytes":path.stat().st_size} for path in sorted(args.out.rglob("*")) if path.is_file()]
        (args.out/"receipt.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf8");print(json.dumps(report))
    return code

if __name__=="__main__":raise SystemExit(main())
