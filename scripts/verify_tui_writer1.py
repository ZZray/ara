"""Explicit auxiliary libtest emitter -> native 80x6 ConPTY -> fixed cell oracle."""
from __future__ import annotations
import argparse, hashlib, importlib.util, json, os, pathlib, shutil, subprocess, time
TEST="terminal::markdown::writer_original_emitter::emit_writer1_vt"
ADAPTER_SHA="130833ffa4edd084b9d41be6de32135a900d9f93cd35e03f37cd4081ea480f58"
WASM_SHA="e85c89f6d22231c4ad753c3d6ed30f69b4526d6f5fd4fdc84ef16ebc8ee2dec1"
def sha(path): return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()
def main():
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--libtest",required=True); ap.add_argument("--out",required=True)
    ap.add_argument("--repo",required=True); ap.add_argument("--dependency",required=True)
    ap.add_argument("--bun",required=True); ap.add_argument("--oracle-entry",required=True)
    a=ap.parse_args(); out=pathlib.Path(a.out).resolve()
    if out.exists(): raise RuntimeError("fresh evidence directory required")
    out.mkdir(parents=True)
    dep=pathlib.Path(a.dependency).resolve(); adapter=dep/"test/virtual-terminal.ts"
    if sha(adapter)!=ADAPTER_SHA: raise RuntimeError("fixed adapter pin mismatch")
    if sha(dep/"node_modules/kitty-vt-wasm/dist/kitty-vt.wasm")!=WASM_SHA: raise RuntimeError("fixed WASM pin mismatch")
    oracle=dep/"test/ara-writer1-oracle.ts"
    # Only a new test entry is copied; the fixed adapter and package stay byte-identical.
    if oracle.exists() and sha(oracle)!=sha(a.oracle_entry): raise RuntimeError("existing oracle entry differs")
    if not oracle.exists(): shutil.copyfile(a.oracle_entry,oracle)
    spec=importlib.util.spec_from_file_location("conpty",pathlib.Path(a.repo)/"scripts/windows_conpty_ctrl_c.py")
    mod=importlib.util.module_from_spec(spec); spec.loader.exec_module(mod)
    env=dict(os.environ,ARA_WRITER1_EMITTER_OUT=str(out)); started=time.monotonic()
    command=subprocess.list2cmdline([str(pathlib.Path(a.libtest).resolve()),"--exact",TEST,"--ignored","--nocapture","--test-threads=1"])
    pty=mod.Pty(command,str(pathlib.Path(a.repo).resolve()),env,columns=80,rows=6)
    observed=None; exitcode=None; oracle_run=None; error=None
    try:
        end=time.monotonic()+30
        while not (out/"frame-ready.json").exists():
            if pty.exit_code(0) is not None: raise RuntimeError("emitter exited before frame-ready")
            if time.monotonic()>end: raise RuntimeError("frame-ready timeout")
            time.sleep(.01)
        # Flush is complete in the emitter. Wait for bounded ConPTY transport drain
        # while emitter remains held, before any libtest suffix can be emitted.
        end=time.monotonic()+5; previous=None; stable_since=time.monotonic()
        while time.monotonic()<end:
            with pty.lock: candidate=bytes(pty.raw)
            if candidate!=previous: previous=candidate; stable_since=time.monotonic()
            if b"INPUT" in candidate and time.monotonic()-stable_since>=.2:
                observed=candidate; break
            time.sleep(.01)
        if observed is None: raise RuntimeError("raw VT transport did not settle with INPUT")
        (out/"observed.raw.vt").write_bytes(observed)
        oracle_run=subprocess.run([a.bun,"run",str(oracle),str(out/"observed.raw.vt"),str(out/"frame-ready.json"),str(out/"cell-oracle.json")],cwd=str(dep),capture_output=True,timeout=20)
        (out/"oracle.stdout").write_bytes(oracle_run.stdout); (out/"oracle.stderr").write_bytes(oracle_run.stderr)
        if oracle_run.returncode: raise RuntimeError("cell oracle failed; inspect artifacts")
    except Exception as e:
        error=repr(e)
    finally:
        (out/"release-emitter").write_text("release\n",encoding="utf-8")
        exitcode=pty.exit_code(5)
        pty.close()
        (out/"full-after-exit.raw.vt").write_bytes(bytes(pty.raw))
    receipt={"originalID":"B-deacfbb9e3","testHelper":TEST,"helperIgnoredByDefault":True,
        "helperExecutedExplicitly":True,"command":command,"columns":80,"rows":6,
        "sourcePins":{"libtest":sha(a.libtest),"Pty":sha(pathlib.Path(a.repo)/"scripts/windows_conpty_ctrl_c.py"),"oracle":sha(oracle),"adapter":sha(adapter),"WASM":WASM_SHA},
        "rawObservedBytes":len(observed) if observed is not None else None,
        "rawObservedSHA256":hashlib.sha256(observed).hexdigest() if observed is not None else None,
        "fullRawSHA256":sha(out/"full-after-exit.raw.vt"),"emitterExit":exitcode,
        "oracleExit":oracle_run.returncode if oracle_run is not None else None,
        "wallSeconds":time.monotonic()-started,"error":error,
        "passed":error is None and exitcode==0 and oracle_run is not None and oracle_run.returncode==0}
    (out/"receipt.json").write_text(json.dumps(receipt,indent=2),encoding="utf-8")
    print(json.dumps(receipt,indent=2)); return 0 if receipt["passed"] else 1
if __name__=="__main__": raise SystemExit(main())
