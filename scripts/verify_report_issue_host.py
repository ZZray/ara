"""Bounded delivered Native report_issue process/SQLite/localhost trials.
No Cargo, CAS, real account configuration or production QA endpoint calls.
Run once with exact delivered binary/build/manifest/runner hashes on fresh D.
"""
from __future__ import annotations
import argparse, hashlib, json, os, re, sqlite3, subprocess, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
OMP="596f2da7101178214aa27a753529d15e6b7ad91d"
def sha(p):return hashlib.sha256(Path(p).read_bytes()).hexdigest()
def read(p):return json.loads(Path(p).read_text(encoding="utf-8-sig"))
def dump(p,v):Path(p).write_text(json.dumps(v,ensure_ascii=False,indent=2)+"\n",encoding="utf-8")
def require(v,s):
    if not v:raise AssertionError(s)
def main():
    parser=argparse.ArgumentParser(description=__doc__)
    for flag in ["binary","build","delivery","output"]:parser.add_argument("--"+flag,type=Path,required=True)
    for flag in ["binary-sha256","build-sha256","delivery-sha256","runner-sha256"]:parser.add_argument("--"+flag,required=True)
    args=parser.parse_args()
    require(os.name=="nt" and args.output.resolve().drive.upper()=="D:","fresh D Windows artifact root required")
    require(not args.output.exists(),"preserve old receipts; no replay")
    def binding():
        require(sha(__file__)==args.runner_sha256,"runner changed")
        require(sha(args.binary)==args.binary_sha256 and sha(args.build)==args.build_sha256 and sha(args.delivery)==args.delivery_sha256,"delivered artifact changed")
        build,delivery=read(args.build),read(args.delivery)
        require(build["exit_code"]==0 and build["source_unchanged"] is True,"delivered build incomplete")
        require(delivery["fixed_omp_commit"]==OMP and delivery["state"]=="DELIVERED_SOURCE_ONLY","source delivery differs")
        require(build["delivery"]["sha256"]==args.delivery_sha256,"build used another delivery")
        require(sha(ROOT/".git/index")==delivery["index_sha256"],"index changed")
        normal=[a for a in build["artifacts"] if a["target"]=="ara" and not a.get("test",a.get("profile_test")) and a.get("kind")==["bin"]]
        require(any(Path(a["executable"]).resolve()==args.binary.resolve() and a["sha256"]==args.binary_sha256 for a in normal),"supplied binary is not normal delivered Host")
        for row in delivery["files"]:require(sha(ROOT/row["path"])==row["delivered_sha256"],"source changed "+row["path"])
        return {"binary":args.binary_sha256,"build":args.build_sha256,"delivery":args.delivery_sha256,"index":delivery["index_sha256"],"source_files":len(delivery["files"])}
    pins=binding();output=args.output.resolve();output.mkdir(parents=True);all_receipts=[]
    cases=["default-no-handler","enabled-malformed","disabled-malformed","granted-200","granted-500","headless-200","http-timeout"]
    started=time.monotonic()
    for case in cases:
        directory=output/case;work=directory/"workspace";home=directory/"profile";sessions=directory/"sessions";temp=directory/"temp"
        for p in [work,home/"agent",sessions,temp]:p.mkdir(parents=True)
        model_calls=[];http_calls=[];faults=[]
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*a):pass
            def do_POST(self):
                try:
                    body=json.loads(self.rfile.read(int(self.headers.get("Content-Length","0"))))
                    if self.path=="/autoqa":
                        require(case in ["granted-200","granted-500","headless-200","http-timeout"],"unexpected report network effect")
                        http_calls.append({"body":body,"headers":{k.lower():v for k,v in self.headers.items()}});dump(directory/"http.json",http_calls)
                        if case=="http-timeout":time.sleep(6);return
                        self.send_response(500 if case=="granted-500" else 200);self.send_header("Content-Length","0");self.end_headers();return
                    require(self.path=="/model/v1/chat/completions","non-loopback fixture route")
                    require(body.get("model")=="report-issue-fixture" and body.get("stream") is True,"model protocol differs")
                    require(len(model_calls)<2,"two-call model bound exceeded");model_calls.append(body);dump(directory/"requests.json",model_calls)
                    if len(model_calls)==1:
                        content="not a report" if case.endswith("malformed") else "proxy_read: native process report"
                        delta={"role":"assistant","tool_calls":[{"index":0,"id":"report","type":"function","function":{"name":"write","arguments":json.dumps({"path":"xd://report_issue","content":content})}}]};finish="tool_calls"
                    else:
                        receipts=[m for m in body.get("messages",[]) if m.get("role")=="tool" and m.get("tool_call_id")=="report"]
                        require(len(receipts)==1,"model lost original tool receipt")
                        require(("Invalid report format." if case=="enabled-malformed" else "Noted, thanks!") in receipts[0]["content"],"unexpected actual report result")
                        delta={"role":"assistant","content":"Report fixture complete."};finish="stop"
                    chunk={"id":"report-fixture","object":"chat.completion.chunk","created":0,"model":"report-issue-fixture","choices":[{"index":0,"delta":delta,"finish_reason":None}]}
                    end=dict(chunk,choices=[{"index":0,"delta":{},"finish_reason":finish}]);payload=("data: "+json.dumps(chunk)+"\n\ndata: "+json.dumps(end)+"\n\ndata: [DONE]\n\n").encode()
                    self.send_response(200);self.send_header("Content-Type","text/event-stream");self.send_header("Content-Length",str(len(payload)));self.end_headers();self.wfile.write(payload)
                except Exception as error:
                    faults.append(str(error));dump(directory/"faults.json",faults)
                    try:self.send_response(500);self.send_header("Content-Length","0");self.end_headers()
                    except OSError:pass
        server=ThreadingHTTPServer(("127.0.0.1",0),Handler);server.daemon_threads=True;server.block_on_close=False
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start();base=f"http://127.0.0.1:{server.server_port}"
        models=home/"agent/models.json";dump(models,{"providers":{"fixture":{"api":"openai-completions","baseUrl":base+"/model/v1","apiKey":"ARA_CONTROLLED_FIXTURE_KEY","models":[{"id":"report-issue-fixture","contextWindow":131072,"maxTokens":128,"reasoning":False}]}}})
        config={"retry":{"enabled":False,"modelFallback":False},"toolFormat":"native","appendOnlyContext":"on","defaultThinkingLevel":"off"}
        if case in ["granted-200","granted-500","http-timeout"]:config["dev"]={"autoqaConsent":"granted","autoqaPush":{"endpoint":base+"/autoqa","token":"fixture-token"}}
        if case=="disabled-malformed":config["dev"]={"autoqa":False}
        dump(home/"agent/config.yml",config)
        env={k:v for k,v in os.environ.items() if k.upper() in {"PATH","PATHEXT","SYSTEMROOT","WINDIR","SYSTEMDRIVE","COMSPEC"}}
        env.update(ARA_HOME=str(home),HOME=str(home),USERPROFILE=str(home),TEMP=str(temp),TMP=str(temp),ARA_CONTROLLED_FIXTURE_KEY="loopback-only",ARA_AUTO_QA_PUSH_URL=base+"/autoqa")
        if case=="headless-200":env["ARA_AUTO_QA_PUSH"]="1"
        command=[str(args.binary.resolve()),"--mode","json","--provider","fixture","--model","report-issue-fixture","--models-config",str(models),"--cwd",str(work),"--session-dir",str(sessions),"--max-model-calls","2","--max-time","12","--max-tokens","128","--no-skills","--tools","read,write","--compact-threshold","0","--system-prompt","Use only the supplied fixture report call.","Report process fixture."]
        before=time.monotonic();stdout=stderr=b"";code=None;failure=None
        try:
            proc=subprocess.run(command,cwd=work,env=env,capture_output=True,timeout=20);stdout,stderr,code=proc.stdout,proc.stderr,proc.returncode
        except subprocess.TimeoutExpired as error:stdout=error.stdout or b"";stderr=error.stderr or b"";failure="deadline; inspect existing side effects, do not replay"
        finally:
            (directory/"native.jsonl").write_bytes(stdout);(directory/"native.stderr").write_bytes(stderr);server.shutdown();server.server_close();thread.join(1)
        rows=[];events=[];settlements=[]
        try:
            require(code==0 and not faults and len(model_calls)==2,"Native process/fake sequence failed")
            events=[json.loads(x) for x in stdout.decode("utf-8-sig").splitlines()]
            ends=[e for e in events if e.get("type")=="tool_execution_end"]
            require(len(ends)==1 and ends[0]["toolCallId"]=="report","original report receipt cardinality")
            require(ends[0]["isError"] is (case=="enabled-malformed"),"actual error boundary")
            result=ends[0]["result"]
            if case!="enabled-malformed":require(result["details"]["xdev"]["args"]=={"report":"not a report" if case=="disabled-malformed" else "proxy_read: native process report"},"original plain metadata changed")
            require(sum(e.get("type")=="agent_end" for e in events)==1,"Run did not settle")
            for line in stderr.decode("utf-8-sig").splitlines():
                prefix="ara: autoqa background settlement "
                if line.startswith(prefix):settlements.extend(json.loads(line[len(prefix):]))
            db=home/"autoqa.db"
            if case in ["default-no-handler","enabled-malformed","disabled-malformed"]:
                require(not db.exists() and not (home/"install-id").exists() and not http_calls,"no-consent/disabled report created external effects")
            else:
                require(db.exists() and len(http_calls)==1,"one accepted background HTTP effect required")
                with sqlite3.connect(db) as connection:rows=[dict(zip(["model","version","tool","report","pushed"],r)) for r in connection.execute("SELECT model,version,tool,report,pushed FROM grievances ORDER BY id")]
                require(len(rows)==1 and rows[0]["tool"]=="read" and rows[0]["report"]=="native process report" and rows[0]["model"]=="fixture/report-issue-fixture","real DB canonical report/model differs")
                require(rows[0]["pushed"]==(1 if case in ["granted-200","headless-200"] else 0),"pending row retention differs")
                payload=http_calls[0]["body"];headers=http_calls[0]["headers"]
                require(payload["agent"]["name"]=="omp" and isinstance(payload["agent"]["version"],str),"fixed wire name/version")
                require(set(payload)=={"agent","installId","platform","arch","entries"},"wire metadata fields differ/hostname leak")
                require(len(payload["entries"])==1 and payload["entries"][0]["tool"]=="read","exact outbound entry")
                require(headers.get("content-type","").startswith("application/json"),"JSON header")
                require(headers.get("authorization")==("Bearer fixture-token" if case!="headless-200" else None),"bearer header contract")
                require(settlements,"background completion was silently lost")
                if case=="http-timeout":require(any(r["outcome"]=="UnknownHttpOutcome" for r in settlements),"sent HTTP timeout must be Unknown")
            require(not list(work.iterdir()),"device report wrote a filesystem target")
            journals=list(sessions.glob("*.jsonl"));require(len(journals)==1,"Session receipt absent")
            journal=[json.loads(x) for x in journals[0].read_text(encoding="utf-8-sig").splitlines()]
            tool=[r["message"] for r in journal if r.get("type")=="message" and r.get("message",{}).get("role")=="toolResult"]
            require(len(tool)==1 and tool[0]["content"]==result["content"] and tool[0]["details"]==result["details"] and tool[0]["isError"]==ends[0]["isError"],"durable receipt differs")
        except Exception as error:failure=failure or f"{type(error).__name__}: {error}"
        receipt={"case":case,"state":"PASS" if failure is None else "FAIL_OR_UNKNOWN","failure":failure,"exit_code":code,"elapsed_seconds":round(time.monotonic()-before,3),"fake_model_calls":len(model_calls),"CAS_calls":0,"http_calls":len(http_calls),"rows":rows,"settlements":settlements,"command":command,"stdout_sha256":sha(directory/"native.jsonl"),"stderr_sha256":sha(directory/"native.stderr"),"journals":[{"path":str(p),"sha256":sha(p)} for p in sessions.glob("*.jsonl")]}
        dump(directory/"receipt.json",receipt);all_receipts.append(receipt)
        if failure:break
    unchanged=binding()==pins
    result={"state":"PASS" if len(all_receipts)==len(cases) and all(r["state"]=="PASS" for r in all_receipts) and unchanged else "FAIL_OR_UNKNOWN","binding":pins,"source_unchanged":unchanged,"elapsed_seconds":round(time.monotonic()-started,3),"receipts":all_receipts,"bounds":{"processes":7,"fake_model_calls":14,"CAS_calls":0,"HTTP_routes":"127.0.0.1 only; production endpoint overridden even on no-handler cases"},"boundary":"Selected fixed report_issue WRITE Host dependency; no original29/full MISC/full WRITE acceptance by this runner alone. No external-effect replay after unknown."}
    dump(output/"result.json",result);print(json.dumps({"state":result["state"],"result":str(output/"result.json"),"seconds":result["elapsed_seconds"]}));return 0 if result["state"]=="PASS" else 1
if __name__=="__main__":raise SystemExit(main())
