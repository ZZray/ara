"""Actual native REPL summary consumer through existing Windows ConPTY.

Caller supplies one delivered binary. Reuses existing loopback upstream and
native Session preparation; never builds or reads local provider secrets.
Raw VT is interaction evidence, not a visual reference or screen-state oracle.
"""
from __future__ import annotations
import argparse
import ctypes
import datetime
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time
import verify_snapcompact_host as host
import windows_conpty_ctrl_c as console

ROOT = Path(__file__).resolve().parents[1]
SOURCES = [
    "crates/ara-cli/src/main.rs", "crates/ara-cli/src/rpc_host_settings.rs",
    "crates/ara-cli/src/agent_message_projection.rs",
    "crates/ara-cli/src/terminal/evaluation.rs", "crates/ara-cli/src/terminal/event_controller.rs",
    "crates/ara-cli/src/terminal/session_observation.rs", "crates/ara-cli/src/terminal/session_transcript.rs",
    "crates/ara-cli/src/terminal/session_consumer_observations.rs", "crates/ara-cli/src/terminal/transcript.rs",
    "crates/ara-cli/src/terminal/driver.rs", "crates/ara-cli/src/terminal/input.rs",
    "crates/ara-cli/src/terminal/render_utils.rs", "crates/ara-cli/src/terminal/mod.rs",
    "crates/ara-cli/src/terminal/summary_message.rs", "crates/ara-cli/src/terminal/markdown.rs",
    "crates/ara-cli/src/terminal/markdown_lexer.rs", "crates/ara-cli/src/terminal/markdown_math.rs",
    "crates/ara-cli/src/terminal/box_component.rs", "crates/ara-cli/src/terminal/theme.rs",
    "scripts/verify_summary_terminal_host.py", "scripts/windows_conpty_ctrl_c.py",
    "scripts/verify_snapcompact_host.py", "scripts/verify_remote_host.py",
    "scripts/verify_handoff_host.py", "scripts/verify_local_reduction_host.py",
]
def digest(path): return hashlib.sha256(Path(path).read_bytes()).hexdigest()
def pins(): return {name:digest(ROOT/name) for name in SOURCES}
def require(condition, message):
    if not condition: raise AssertionError(message)
def raw(pty):
    with pty.lock: return bytes(pty.raw)
def seed():
    stamp="2026-07-09T00:00:00.000Z"
    def user(identity,parent,text):
        return {"type":"message","id":identity,"parentId":parent,"timestamp":stamp,
                "message":{"role":"user","content":[{"type":"text","text":text}],"timestamp":1}}
    return [user("u1",None,"BEFORE_COLLAPSE_SENTINEL"),user("u2","u1","kept real user"),
        {"type":"compaction","id":"c1","parentId":"u2","timestamp":stamp,
         "summary":"# DETAIL_ONLY_SENTINEL\n\nA **real** native Session summary.","tokensBefore":256000,
         "tokensAfter":20000,"method":"remote","warning":"REAL_WARNING_SENTINEL","firstKeptEntryId":"u2"},
        user("u3","c1","AFTER_COMPACTION_SENTINEL")]

def run_case(trial,collapsed,theme):
    name=f"native-{theme}-{'collapsed' if collapsed else 'full'}"
    directory,work,session=trial.setup(name,seed())
    result={"case":name,"theme":theme,"collapsed":collapsed,"viewportStates":[],"physicalReader":"existing Windows ConPTY"}
    result["stage"]="setup"
    with host.remote.RemoteUpstream(directory,generic=True) as fake:
        models=host.configure(directory,fake)
        config=models.parent/"config.yml"
        config.write_text(config.read_text(encoding="utf-8")+f"display:\n  collapseCompacted: {'true' if collapsed else 'false'}\n",encoding="utf-8")
        command=host.command(trial,directory,work,session,models,repl=True)
        env=trial.environment(directory);env["ARA_TERMINAL_THEME"]=theme
        pty=console.Pty(subprocess.list2cmdline(command),str(work),env)
        try:
            require(pty.wait_text("interactive session",20)>=0,"actual REPL did not initialize")
            require(pty.wait_text("AFTER_COMPACTION_SENTINEL",20)>=0,"actual Session transcript was not consumed")
            require(pty.wait_text("remote-compacted",20)>=0,"real divider absent")
            require(pty.wait_text("256K→20K",20)>=0,"actual token amount absent")
            require(pty.wait_text("ctrl+o",20)>=0,"actual expand affordance absent")
            first=pty.text()
            require("DETAIL_ONLY_SENTINEL" not in first,"collapsed detail leaked before expansion")
            require(("BEFORE_COLLAPSE_SENTINEL" in first)==(not collapsed),"Full/Collapsed native chronology differs")
            require(not fake.requests,"display resume unexpectedly called a model")
            result["stage"]="native-display-observed"
            before=session.read_bytes()
            # Draft and ticket are retained while the idle reader sends the same
            # ordered control used during a Run. Enter later proves the draft.
            pty.send(b"draft-preserved")
            pty.send(b"\x0f")
            require(pty.wait_text("DETAIL_ONLY_SENTINEL",20)>=0,"idle Ctrl+O did not reveal actual Markdown summary")
            require(pty.wait_text("REAL_WARNING_SENTINEL",20)>=0,"actual warning detail absent")
            require(session.read_bytes()==before,"Ctrl+O changed native Session persistence")
            require(not fake.requests,"Ctrl+O unexpectedly called a model")
            result["stage"]="expanded-with-draft-retained"
            console.k32.ResizePseudoConsole.argtypes=[ctypes.c_void_p,console.COORD]
            console.k32.ResizePseudoConsole.restype=ctypes.c_long
            for width in (40,80,120):
                offset=len(raw(pty));status=console.k32.ResizePseudoConsole(pty.hpc,console.COORD(width,30))
                require(status==0,f"ResizePseudoConsole failed {status}")
                deadline=time.monotonic()+3
                while len(raw(pty))==offset and time.monotonic()<deadline:time.sleep(.05)
                require(len(raw(pty))>offset,"actual terminal produced no resize output")
                result["viewportStates"].append({"columns":width,"rows":30,"outputBytes":len(raw(pty))-offset})
            result["stage"]="resize-observed"
            pty.send(b"\r")
            answer="Continued work."
            answer_index=pty.wait_text(answer,20)
            require(answer_index>=0,"draft submission did not settle in the actual Host")
            deadline=time.monotonic()+5
            while len(fake.requests)<1 and time.monotonic()<deadline:time.sleep(.05)
            require(len(fake.requests)==1 and not fake.errors,"bounded supplied route expected exactly one successful call")
            messages=[row.get("message",{}) for row in host.shared.journal(session).values()]
            found=False
            for message in messages:
                content=message.get("content")
                text=content if isinstance(content,str) else "\n".join(block.get("text","") for block in content or [] if block.get("type")=="text")
                if message.get("role")=="user" and text=="draft-preserved":found=True
            require(found,"Ctrl+O changed or submitted the retained draft")
            result["stage"]="draft-persisted-and-route-returned"
            prompt_index=pty.wait_text("> ",20,start=answer_index+len(answer))
            require(prompt_index>=0,"actual REPL did not activate the next empty prompt")
            result.update(stage="next-prompt-observed",answerIndex=answer_index,nextPromptIndex=prompt_index)
            pty.send(b"\x04")
            result["stage"]="eof-requested"
            result["eofExitCode"]=pty.exit_code(10)
            require(result["eofExitCode"]==0,"actual REPL EOF did not exit cleanly")
            result["stage"]="clean-eof-observed"
            result.update(exitCode=0,zeroCallsBeforeEnter=True,modelCalls=len(fake.requests),sourceSession=str(session),
                nativeSessionSha256=digest(session),draftPreserved=True,
                screenOracle="not run; VT capture does not prove final screen or visual fidelity")
        except Exception as error:
            result["failure"]=f"{type(error).__name__}: {error}"
            raise
        finally:
            data=raw(pty)
            (directory/"terminal.vt").write_bytes(data)
            (directory/"terminal-visible-log.txt").write_text(pty.text(),encoding="utf-8")
            result["terminalVtSha256"]=hashlib.sha256(data).hexdigest()
            result["terminalVtBytes"]=len(data)
            result["exitCodeBeforeCleanup"]=pty.exit_code(0)
            result["cleanup"]="existing Pty.close terminates a still-live process with99; EOF success requires observed exit0 before cleanup"
            host.shared.dump(directory/"case-receipt.json",result)
            pty.close()
    return result

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary",type=Path,required=True)
    parser.add_argument("--output",type=Path,required=True)
    args=parser.parse_args()
    require(os.name=="nt","actual ConPTY acceptance requires Windows")
    require(args.binary.is_file(),"caller must supply the delivered binary")
    args.output.mkdir(parents=True,exist_ok=True)
    stamp=datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    output=args.output/('run-'+stamp);output.mkdir()
    before=pins();binary=digest(args.binary);started=time.monotonic()
    trial=host.shared.Trial(args.binary.resolve(),output.resolve(),30)
    cases=[];failure=None
    try:
        for collapsed,theme in [(True,"dark"),(False,"dark"),(True,"light"),(False,"light")]:
            cases.append(run_case(trial,collapsed,theme))
    except Exception as error:failure=f"{type(error).__name__}: {error}"
    after=pins();unchanged=before==after and digest(args.binary)==binary
    receipt={"status":"PASS" if failure is None and unchanged else "FAIL","seconds":round(time.monotonic()-started,3),
        "binary":str(args.binary.resolve()),"binarySha256":binary,"sourcePinsBefore":before,"sourcePinsAfter":after,
        "sourceUnchanged":unchanged,"cases":cases,"failure":failure,
        "coverage":"real native Session resume/chronology, idle prompt key/draft, resize and one bounded fake route call per case",
        "uncovered":["fixed reference visual comparison","ConPTY final-screen oracle","live model/image decode","mid-run maintenance cancel/fault and physical replacement ack fault injection; grouped Rust tests are separate evidence"],
        "acceptance":"consumer interaction only; full OMP TUI/Markdown/product acceptance not implied"}
    host.shared.dump(output/"receipt.json",receipt)
    print(json.dumps({"status":receipt["status"],"receipt":str(output/"receipt.json"),"cases":len(cases),"failure":failure}))
    return 0 if receipt["status"]=="PASS" else 1

if __name__=="__main__":raise SystemExit(main())
