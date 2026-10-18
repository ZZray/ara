"""Create a reproducible, secret-free source release from an explicit allowlist."""
import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import tarfile

parser = argparse.ArgumentParser()
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
names = ['Cargo.toml', 'Cargo.lock', 'README.md', 'LICENSE',
         'THIRD-PARTY-NOTICES.md', 'node.conf.example']
for directory, pattern in [('src', '*.rs'), ('tests', '*.py'),
                           ('docs/knowledge', '*.md')]:
    names.extend(p.relative_to(root).as_posix() for p in (root / directory).glob(pattern) if p.is_file())
names.extend(p.relative_to(root).as_posix() for p in (root / 'licenses').rglob('*') if p.is_file())
# Compile-time prompt and builtin-skill payloads are part of the Rust sources.
names.extend(['src/web_research_prompt.md', 'src/environment_prompt.md',
              'src/builtin_skills/skill-creator/SKILL.md',
              'src/builtin_skills/skill-installer/SKILL.md'])
names.extend(['scripts/install-linux.sh', 'scripts/test-install-linux.sh', 'scripts/package-source.py'])
pins = {}
archive = io.BytesIO()
with tarfile.open(fileobj=archive, mode='w', format=tarfile.USTAR_FORMAT) as tar:
    for name in sorted(set(names)):
        path = root / name
        assert path.is_file() and not path.is_symlink(), name
        data = path.read_bytes()
        pins[name] = hashlib.sha256(data).hexdigest()
        entry = tarfile.TarInfo('handwave-node/' + name)
        entry.size, entry.mode, entry.mtime = len(data), 0o644, 0
        tar.addfile(entry, io.BytesIO(data))
compressed = gzip.compress(archive.getvalue(), mtime=0)
digest = hashlib.sha256(compressed).hexdigest()
args.output.mkdir(parents=True, exist_ok=True)
source_name = 'handwave-node-source-' + digest[:16] + '.tar.gz'
script = (root / 'scripts/install-linux.sh').read_bytes()
assert b'\r' not in script, 'Installer must use Unix newlines'
script_sha = hashlib.sha256(script).hexdigest()
script_name = 'install-linux-' + script_sha[:16] + '.sh'
(args.output / source_name).write_bytes(compressed)
(args.output / script_name).write_bytes(script)
result = {'source_name': source_name, 'source_sha256': digest, 'source_bytes': len(compressed),
          'script_name': script_name, 'script_sha256': script_sha, 'files': pins}
(args.output / 'source-manifest.json').write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
print(json.dumps({k: v for k, v in result.items() if k != 'files'}))
