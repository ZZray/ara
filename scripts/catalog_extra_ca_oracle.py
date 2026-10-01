"""Fixed original TLS helper/test oracle; controlled filesystem dependencies."""
import argparse, datetime, hashlib, json, pathlib, shutil, subprocess, uuid
SHA = "596f2da7101178214aa27a753529d15e6b7ad91d"
BUN_SHA = "627d2e4775c24bdedee2cd7ccc18dcadae061e5345274ab6e3c4c797927bfb8f"

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--upstream", required=True)
    p.add_argument("--source-run", required=True, type=pathlib.Path)
    p.add_argument("--bun", required=True, type=pathlib.Path)
    p.add_argument("--output", required=True, type=pathlib.Path)
    a = p.parse_args()
    run = a.output / ("run-" + datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-" + uuid.uuid4().hex[:8])
    run.mkdir(parents=True)
    if hashlib.sha256(a.bun.read_bytes()).hexdigest() != BUN_SHA:
        raise RuntimeError("wrong fixed Bun runtime")
    tree = subprocess.check_output(["git", "-C", a.upstream, "ls-tree", "-r", "--format=%(objectname) %(path)", SHA, "packages/utils/src", "packages/utils/package.json", "packages/utils/test/tls-fetch.test.ts", "LICENSE", "packages/utils/LICENSE"]).decode().splitlines()
    source_manifest = []
    for line in tree:
        blob, file = line.split(" ", 1)
        previous = a.source_run / "upstream" / file
        content = previous.read_bytes() if previous.exists() else subprocess.check_output(["git", "-C", a.upstream, "show", SHA + ":" + file])
        if hashlib.sha1(b"blob " + str(len(content)).encode() + b"\0" + content).hexdigest() != blob:
            raise RuntimeError("source drift: " + file)
        destination = run / "upstream" / file
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(content)
        source_manifest.append({"path": file, "gitBlob": blob, "sha256": hashlib.sha256(content).hexdigest()})
    package = run / "upstream/node_modules/@oh-my-pi/pi-utils"
    shutil.copytree(run / "upstream/packages/utils", package)
    original = subprocess.run([str(a.bun), "test", str((run / "upstream/packages/utils/test/tls-fetch.test.ts").resolve())], cwd=run, capture_output=True, timeout=40)
    (run / "original-tests.log").write_bytes(original.stdout + original.stderr)
    original.check_returncode()
    runner = pathlib.Path(__file__).with_suffix(".mjs").read_bytes()
    (run / "runner.mjs").write_bytes(runner)
    reference_probes = pathlib.Path(__file__).with_name("catalog_extra_ca_reference_probes.mjs").read_bytes()
    (run / "reference-probes.mjs").write_bytes(reference_probes)
    manifest = {"upstreamCommit": SHA, "bunVersion": "1.4.0", "bunSha256": BUN_SHA, "runnerSha256": hashlib.sha256(runner).hexdigest(), "referenceProbesSha256": hashlib.sha256(reference_probes).hexdigest(), "sourceManifest": source_manifest, "originalAssertionsExecuted": True}
    (run / "manifest.json").write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    result = subprocess.run([str(a.bun), str((run / "runner.mjs").resolve()), str((run / "manifest.json").resolve())], cwd=run, capture_output=True, timeout=40)
    (run / "stdout.log").write_bytes(result.stdout)
    (run / "stderr.log").write_bytes(result.stderr)
    result.check_returncode()
    artifact = run / "oracle.json"
    data = json.loads(artifact.read_text(encoding="utf-8"))
    receipt = {"artifact": str(artifact.resolve()), "sha256": hashlib.sha256(artifact.read_bytes()).hexdigest(), "cases": len(data["cases"]), "originalTestsPassed": True, "scope": "All exported TLS helpers with controlled fs/env; not native TLS acceptance"}
    (run / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(receipt))

if __name__ == "__main__":
    main()
