"""Actual isolated Node environment setup with a self-authored, dependency-free wheel.

No Internet package source, global installation, administrator operation or
existing service is used. A controlled provider makes exactly one run_script
call and validates its real wheel, venv/import and loopback-process receipts.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import threading
import time
import zipfile

import verify_context as context
from verify_host import require, stop, validate_loop_events

PACKAGE = "handwave_env_fixture_731"
VALUE = "HANDWAVE_SELF_AUTHORED_WHEEL_731"
MARKER = "ENVIRONMENT_INSTALL_AND_LOOPBACK_VERIFIED_731"

SCRIPT = r'''
import base64,csv,hashlib,io,json,os,pathlib,socket,subprocess,sys,venv,zipfile
root=pathlib.Path.cwd().resolve()
package="handwave_env_fixture_731"
value="HANDWAVE_SELF_AUTHORED_WHEEL_731"
wheel=root/(package+"-0.0.1-py3-none-any.whl")
environment=root/".venv"
assert not wheel.exists() and not environment.exists(), "Fresh fixture environment required; no replay"
report={"fixture_origin":"self-authored local test package; no third-party source", "work":str(root), "package":package}
child=None
try:
    dist=package+"-0.0.1.dist-info"
    files={package+"/__init__.py":("VALUE = "+repr(value)+"\n").encode(),
           dist+"/METADATA":("Metadata-Version: 2.1\nName: handwave-env-fixture-731\nVersion: 0.0.1\n\nSelf-authored fixture only.\n").encode(),
           dist+"/WHEEL":b"Wheel-Version: 1.0\nGenerator: handwave-isolated-test\nRoot-Is-Purelib: true\nTag: py3-none-any\n"}
    records=io.StringIO(newline="")
    writer=csv.writer(records,lineterminator="\n")
    for name,data in files.items():
        writer.writerow([name,"sha256="+base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode(),len(data)])
    writer.writerow([dist+"/RECORD","",""])
    files[dist+"/RECORD"]=records.getvalue().encode()
    with zipfile.ZipFile(wheel,"x",compression=zipfile.ZIP_DEFLATED) as archive:
        for name,data in files.items(): archive.writestr(name,data)
    report["wheel"]={"path":str(wheel),"sha256":hashlib.sha256(wheel.read_bytes()).hexdigest(),"bytes":wheel.stat().st_size}
    clean_env={k:v for k,v in os.environ.items() if not k.upper().startswith(("PIP_","PYTHON","CONDA")) and k.upper()!="VIRTUAL_ENV"}
    clean_env["PIP_CONFIG_FILE"]=os.devnull
    venv.EnvBuilder(with_pip=True).create(environment)
    python=environment/("Scripts/python.exe" if os.name=="nt" else "bin/python")
    def command(args,timeout=30):
        run=subprocess.run([str(python),"-I"]+args,cwd=root,env=clean_env,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=timeout)
        receipt={"exit_code":run.returncode,"stdout":run.stdout.decode("utf-8",errors="replace")[:4096],"stderr":run.stderr.decode("utf-8",errors="replace")[:4096]}
        assert run.returncode==0, "Isolated interpreter command failed: "+json.dumps(receipt)
        return receipt
    report["pip_version"]=command(["-m","pip","--isolated","--version"])
    paths=command(["-c","import json,sys,sysconfig; print(json.dumps({'prefix':sys.prefix,'base_prefix':sys.base_prefix,'purelib':sysconfig.get_paths()['purelib']}))"])
    paths=json.loads(paths["stdout"])
    assert pathlib.Path(paths["prefix"]).resolve()==environment.resolve() and paths["prefix"]!=paths["base_prefix"]
    purelib=pathlib.Path(paths["purelib"]).resolve()
    purelib.relative_to(environment.resolve())
    # Explicit target keeps even machine-level pip configuration from selecting
    # a global installation path. This is this venv's own normal site-packages.
    report["pip_install"]=command(["-m","pip","--isolated","--disable-pip-version-check","install","--no-index","--no-deps","--no-cache-dir","--target",str(purelib),str(wheel)])
    report["environment"]={"path":str(environment),"python":str(python),"purelib":str(purelib),"isolated":True}
    report["import_probe"]=command(["-c","import json,"+package+" as p; print(json.dumps({'value':p.VALUE,'module_path':p.__file__}))"])
    imported=json.loads(report["import_probe"]["stdout"])
    assert imported["value"]==value
    pathlib.Path(imported["module_path"]).resolve().relative_to(environment.resolve())
    server_code="""import http.server,json,threading
import handwave_env_fixture_731 as fixture
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        shutdown=self.path=='/__fixture_shutdown_731'
        body=b'FIXTURE_SERVER_STOPPING_731' if shutdown else fixture.VALUE.encode()
        self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
        if shutdown:threading.Thread(target=self.server.shutdown,daemon=True).start()
    def log_message(self,*args):pass
server=http.server.HTTPServer(('127.0.0.1',0),Handler)
print(json.dumps({'port':server.server_port}),flush=True)
server.serve_forever()
server.server_close()
"""
    child=subprocess.Popen([str(python),"-I","-u","-c",server_code],cwd=root,env=clean_env,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
    port=json.loads(child.stdout.readline())["port"]
    assert isinstance(port,int) and 0<port<65536 and child.poll() is None
    import urllib.request
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open("http://127.0.0.1:"+str(port)+"/healthz",timeout=5) as response:
        body=response.read(4096).decode("utf-8")
        report["http"]={"host":"127.0.0.1","port":port,"status":response.status,"body":body,"child_pid":child.pid}
        assert response.status==200 and body==value
finally:
    if child is not None:
        if child.poll() is None and "http" in report:
            try:
                with opener.open("http://127.0.0.1:"+str(report["http"]["port"])+"/__fixture_shutdown_731",timeout=3) as response:
                    report["shutdown_http"]={"status":response.status,"body":response.read(256).decode("utf-8")}
                child.wait(timeout=5)
            except Exception as error:report["shutdown_error_type"]=type(error).__name__
        if child.poll() is None:
            child.terminate()
            try:child.wait(timeout=5)
            except subprocess.TimeoutExpired:child.kill();child.wait(timeout=5)
        report["child_exit_code"]=child.returncode
        assert child.returncode is not None
        if child.stdout:child.stdout.close()
        if child.stderr:child.stderr.close()
        if "http" in report:
            try:
                connection=socket.create_connection(("127.0.0.1",report["http"]["port"]),timeout=.5)
            except OSError:report["listener_closed"]=True
            else:connection.close();report["listener_closed"]=False
    (root/"environment-result.json").write_text(json.dumps(report,indent=2),encoding="utf-8")
assert report.get("listener_closed") is True
print("ENVIRONMENT_INSTALL_AND_LOOPBACK_VERIFIED_731",flush=True)
'''


def validate_artifacts(work):
    report = json.loads((work / "environment-result.json").read_text(encoding="utf-8"))
    require(report["fixture_origin"].startswith("self-authored"), "Fixture provenance missing")
    wheel = Path(report["wheel"]["path"])
    require(wheel.parent.resolve() == work.resolve(), "Wheel is outside fresh work")
    require(hashlib.sha256(wheel.read_bytes()).hexdigest() == report["wheel"]["sha256"], "Original wheel hash mismatch")
    with zipfile.ZipFile(wheel) as archive:
        require(set(archive.namelist()) == {PACKAGE + "/__init__.py", PACKAGE + "-0.0.1.dist-info/METADATA", PACKAGE + "-0.0.1.dist-info/WHEEL", PACKAGE + "-0.0.1.dist-info/RECORD"}, "Unexpected wheel files")
    environment = (work / ".venv").resolve()
    require(Path(report["environment"]["path"]).resolve() == environment, "Wrong venv")
    Path(report["environment"]["python"]).resolve().relative_to(environment)
    require(report["pip_version"]["exit_code"] == 0 and "pip " in report["pip_version"]["stdout"], "Actual venv pip version missing")
    require(report["pip_install"]["exit_code"] == report["import_probe"]["exit_code"] == 0, "Actual pip or Python process failed")
    imported = json.loads(report["import_probe"]["stdout"])
    require(imported["value"] == VALUE, "Installed fixture import returned incorrect value")
    Path(imported["module_path"]).resolve().relative_to(environment)
    require(report["http"]["host"] == "127.0.0.1" and report["http"]["status"] == 200 and report["http"]["body"] == VALUE, "Actual loopback response differs")
    require(report.get("shutdown_http") == {"status": 200, "body": "FIXTURE_SERVER_STOPPING_731"}, "Graceful loopback shutdown receipt missing")
    require(report["http"]["child_pid"] > 0 and report["child_exit_code"] == 0 and report["listener_closed"] is True, "Child cleanup receipt missing")
    return report


class EnvironmentFixture(context.ContextFixture):
    def reply(self, handler, body):
        self.agent_requests.append(body)
        messages = body["messages"]
        last = max(i for i, row in enumerate(messages) if row.get("role") == "user")
        require(messages[last]["content"] == "ENVIRONMENT_LOCAL_WHEEL_731", "Unexpected environment request")
        receipts = [row for row in messages[last + 1:] if row.get("role") == "tool"]
        if not receipts:
            require(any(row["function"]["name"] == "run_script" for row in body.get("tools", [])), "run_script not declared")
            return self.emit(handler, tool="run_script", arguments={"language": "python", "script": SCRIPT, "timeout": 90, "output_limit": 2048})
        require(len(receipts) == 1 and MARKER in receipts[0]["content"], "Environment setup failed or was replayed")
        validate_artifacts(self.root)
        return self.emit(handler, text=MARKER)


def run_environment(host):
    run_id = "isolated-environment-731"
    host.controller.frame({"k": "agent-loop", "id": run_id, "msg": "ENVIRONMENT_LOCAL_WHEEL_731", "model": "default"})
    events, output = [], bytearray()
    deadline = time.monotonic() + 105
    while time.monotonic() < deadline:
        packet = host.controller.receive(deadline - time.monotonic())
        if packet.get("k") != "dc": continue
        frame = json.loads(base64.b64decode(packet["d"]))
        host.transcript.append(frame)
        if frame.get("id") != run_id: continue
        if frame.get("k") == "agent-event": events.append(frame["event"])
        if frame.get("k") == "out":
            output.extend(base64.b64decode(frame.get("b64", "")))
            if frame.get("done"):
                validate_loop_events(events, run_id, frame)
                return frame, events, output.decode("utf-8", errors="replace")
    raise TimeoutError("Environment Run did not settle before its bounded deadline")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--hub-binary", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    root = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="handwave-environment-"))
    root.mkdir(parents=True, exist_ok=True)
    require(not any(root.iterdir()), "Use a fresh empty evidence directory")
    context.ContextFixture = EnvironmentFixture
    host = context.Host(args, root / "host", owner="isolated-environment-owner", max_calls=3)
    host.env["PATH"] = str(Path(sys.executable).parent) + os.pathsep + host.env.get("PATH", "")
    expired = threading.Event()
    def expire():
        expired.set()
        stop(host.node)
        stop(host.hub)
    watchdog = threading.Timer(120, expire)
    watchdog.daemon = True
    watchdog.start()
    started = time.monotonic()
    result = {"status": "FAIL", "scope": "actual isolated Node/Hub/controlled provider; self-authored local wheel only", "watchdog_seconds": 120}
    try:
        host.start()
        terminal, events, text = run_environment(host)
        require(not terminal.get("err") and terminal.get("code", 0) == 0 and MARKER in text, "Controller did not receive verified environment result")
        require(len(host.fixture.agent_requests) == 2 and not host.fixture.errors, "Unexpected model request count or fixture assertion failure")
        require(any(event.get("type") == "tool_end" and event.get("title") == "run_script" and event.get("status") == "completed" for event in events), "Actual completed script receipt missing")
        result.update(status="PASS", artifact_receipt=validate_artifacts(host.work), passed=["self_authored_wheel_hash", "fresh_venv_local_no_deps_install", "real_venv_python_import", "real_loopback_http_body", "child_exit_and_listener_closed", "two_model_calls_actual_tool_and_controller_receipts"])
    except Exception as error:
        result["error"] = f"{type(error).__name__}: {error}"
    finally:
        watchdog.cancel()
        host.close()
        if expired.is_set(): result["status"] = "FAIL"
        result.update(elapsed_seconds=round(time.monotonic() - started, 3), watchdog_triggered=expired.is_set(), binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest(), hub_sha256=hashlib.sha256(args.hub_binary.read_bytes()).hexdigest(), model_requests=len(host.fixture.agent_requests), not_run=["Internet package installation", "global/admin installation", "third-party dependency compatibility", "production deployment", "real-model environment reasoning"])
        (root / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(result, ensure_ascii=False, indent=2))
    if result["status"] != "PASS": raise SystemExit(1)


if __name__ == "__main__":
    main()
