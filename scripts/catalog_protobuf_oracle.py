"""Capture exact fixed source codecs and original tests; no Rust implementation."""
import argparse,datetime,hashlib,json,pathlib,subprocess,uuid
SHA="596f2da7101178214aa27a753529d15e6b7ad91d"
FILES=["LICENSE","packages/catalog/LICENSE","packages/catalog/src/discovery/protobuf.ts","packages/catalog/src/discovery/cursor-proto.ts","packages/catalog/src/discovery/devin-proto.ts","packages/catalog/test/protobuf.test.ts","packages/utils/src/type-guards.ts"]
def main():
 p=argparse.ArgumentParser();p.add_argument("--upstream",required=True);p.add_argument("--bun",required=True);p.add_argument("--output",required=True);a=p.parse_args()
 root=pathlib.Path(a.output)/("run-"+datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")+"-"+uuid.uuid4().hex[:8]);ref=root/"reference";manifest=[]
 for file in FILES:
  b=subprocess.check_output(["git","-C",a.upstream,"show",SHA+":"+file]);dest=ref/file;dest.parent.mkdir(parents=True,exist_ok=True);dest.write_bytes(b);manifest.append({"path":file,"sha256":hashlib.sha256(b).hexdigest()})
 utils=ref/"node_modules/@oh-my-pi/pi-utils";utils.mkdir(parents=True);(utils/"package.json").write_text('{"name":"@oh-my-pi/pi-utils","exports":"./index.ts"}');(utils/"index.ts").write_text('export * from "../../../packages/utils/src/type-guards.ts";')
 # The module binding reexports the actual unchanged type-guards, not a fake predicate.
 test=subprocess.run([a.bun,"test",str(ref/"packages/catalog/test/protobuf.test.ts")],capture_output=True,text=True,encoding="utf-8",timeout=30);(root/"original-tests.log").write_text(test.stdout+test.stderr,encoding="utf-8");test.check_returncode()
 ir=pathlib.Path("crates/ara-cli/src/catalog_proto_schemas/ir.json").resolve();result=subprocess.run([a.bun,str(pathlib.Path(__file__).with_suffix(".mjs")),str(ref),str(ir)],capture_output=True,text=True,encoding="utf-8",timeout=45);result.check_returncode();data=json.loads(result.stdout);data["sourceManifest"]=manifest;data["originalAssertionsExecuted"]=True
 artifact=root/"oracle.json";artifact.write_text(json.dumps(data,ensure_ascii=True,separators=(",",":")),encoding="utf-8");print(json.dumps({"artifact":str(artifact),"sha256":hashlib.sha256(artifact.read_bytes()).hexdigest(),"cases":len(data["cases"]),"originalTestsPassed":test.returncode==0}))
if __name__=="__main__":main()
