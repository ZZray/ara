#!/usr/bin/env python3
"""Collect resolved Cargo license texts without copying local configuration.

Requires Python 3.11+ and cached Cargo sources. The bundle includes all packages
in the locked dependency resolution (including build/target dependencies), not
an assertion that every resolved package is linked into the executable.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib


PREFIXES = ("LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "NOTICE", "UNLICENSE")
SENSITIVE = re.compile(r"-----BEGIN (?:[A-Z ]+ )?PRIVATE KEY-----|\bsk-[A-Za-z0-9_-]{32,}|\b(?:api[_-]?key|access[_-]?token|secret[_-]?key)\s*[:=]\s*['\"]?[A-Za-z0-9_-]{24,}", re.I)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def legal_text(path: Path) -> bytes:
    data = path.read_bytes()
    if len(data) > 1024 * 1024:
        raise RuntimeError(f"license text exceeds 1 MiB: {path.name}")
    text = data.decode("utf-8-sig")
    if SENSITIVE.search(text):
        raise RuntimeError(f"sensitive-looking content in license input: {path.name}")
    return data


def repository_root(manifest: Path) -> Path:
    """Only a Git dependency may inherit repository-root license notices."""
    for directory in [manifest.parent, *manifest.parent.parents]:
        if (directory / ".git").exists():
            return directory
    return manifest.parent


def candidates(package: dict) -> list[tuple[Path, str]]:
    manifest = Path(package["manifest_path"]).resolve()
    base = manifest.parent
    legal_root = repository_root(manifest) if (package.get("source") or "").startswith("git+") else base
    paths: dict[Path, str] = {}
    declared = package.get("license_file")
    if declared:
        path = (base / declared).resolve()
        if not path.is_relative_to(legal_root):
            raise RuntimeError(f"license_file escapes source root: {package['name']}")
        if path.is_file():
            paths[path] = str(path.relative_to(legal_root))
    roots = list(dict.fromkeys([base, legal_root]))
    for root in roots:
        for file in root.iterdir():
            if file.is_file() and file.name.upper().startswith(PREFIXES) and file.resolve().is_relative_to(legal_root):
                paths[file.resolve()] = str(file.relative_to(legal_root))
        for name in ["licenses", "license", "LICENSES"]:
            directory = root / name
            if directory.is_dir():
                for file in directory.rglob("*"):
                    resolved = file.resolve()
                    if file.is_file() and resolved.is_relative_to(legal_root) and len(file.relative_to(directory).parts) <= 3:
                        paths[resolved] = str(file.relative_to(legal_root))
    return sorted(paths.items(), key=lambda item: item[1])


def readme_copyright(package: dict) -> str:
    base = Path(package["manifest_path"]).resolve().parent
    names = [package.get("readme"), "README.md", "README", "README.txt"]
    lines: set[str] = set()
    for name in names:
        if not name:
            continue
        path = (base / name).resolve()
        if not path.is_relative_to(base) or not path.is_file() or path.stat().st_size > 1024 * 1024:
            continue
        for line in path.read_text(encoding="utf-8-sig", errors="replace").splitlines():
            if re.search(r"copyright|©|\(c\)", line, re.I) and len(line) <= 1000:
                if SENSITIVE.search(line):
                    raise RuntimeError(f"sensitive-looking README attribution: {package['name']}")
                lines.add(line.strip())
    return "\n".join(sorted(lines))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=Path(__file__).resolve().parents[1] / "Cargo.toml")
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument("--rustc", default="rustc")
    parser.add_argument("--cargo-home", type=Path)
    parser.add_argument("--metadata", type=Path, help="Use existing locked Cargo metadata instead of invoking Cargo")
    args = parser.parse_args()
    manifest = args.manifest.resolve()
    root = manifest.parent
    lock_path = root / "Cargo.lock"
    lock = tomllib.loads(lock_path.read_text(encoding="utf-8"))
    lock_keys = {(row["name"], row["version"], row.get("source")) for row in lock["package"]}
    if args.metadata:
        metadata = json.loads(args.metadata.read_text(encoding="utf-8-sig"))
    else:
        environment = os.environ.copy()
        if args.cargo_home:
            environment["CARGO_HOME"] = str(args.cargo_home.resolve())
        result = subprocess.run([args.cargo, "metadata", "--manifest-path", str(manifest), "--locked", "--offline", "--format-version", "1"], capture_output=True, text=True, encoding="utf-8", env=environment, timeout=180)
        if result.returncode:
            raise RuntimeError("locked offline Cargo metadata failed; populate public source caches first")
        metadata = json.loads(result.stdout)
    metadata_root = Path(metadata["workspace_root"]).resolve()
    if metadata_root != root:
        # An isolated build snapshot is allowed only for the identical locked
        # manifest; host source/configuration never enters this collection.
        if digest((metadata_root / "Cargo.toml").read_bytes()) != digest(manifest.read_bytes()) or digest((metadata_root / "Cargo.lock").read_bytes()) != digest(lock_path.read_bytes()):
            raise RuntimeError("Cargo metadata manifest/lock differs from the requested workspace")
    resolved = {node["id"] for node in metadata["resolve"]["nodes"]}
    packages = [row for row in metadata["packages"] if row["id"] in resolved and row.get("source")]
    output = root / "licenses" / "third-party"
    output.mkdir(parents=True, exist_ok=True)
    entries = []
    supplements_path = root / "licenses" / "upstream-supplements.json"
    supplements = json.loads(supplements_path.read_text(encoding="utf-8-sig")) if supplements_path.exists() else {}
    for package in sorted(packages, key=lambda row: (row["name"], row["version"], row.get("source", ""))):
        key = (package["name"], package["version"], package.get("source"))
        if key not in lock_keys:
            raise RuntimeError(f"metadata package not in Cargo.lock: {package['name']} {package['version']}")
        directory = output / f"{package['name']}-{package['version']}"
        directory.mkdir(exist_ok=True)
        files = []
        for source, relative in candidates(package):
            data = legal_text(source)
            target_name = relative.replace("\\", "/").replace("/", "__")
            target = directory / target_name
            target.write_bytes(data)
            files.append({"file": str(target.relative_to(root)).replace("\\", "/"), "source_relative": relative.replace("\\", "/"), "sha256": digest(data)})
        copyright_text = readme_copyright(package)
        if copyright_text:
            target = directory / "README-COPYRIGHT.txt"
            data = (copyright_text + "\n").encode()
            target.write_bytes(data)
            files.append({"file": str(target.relative_to(root)).replace("\\", "/"), "source_relative": "README copyright lines", "sha256": digest(data)})
        for supplement in supplements.get(f"{package['name']} {package['version']}", []):
            source = (root / supplement["file"]).resolve()
            if not source.is_relative_to((root / "licenses" / "source-supplements").resolve()):
                raise RuntimeError("supplemental license escapes its dedicated directory")
            data = legal_text(source)
            if digest(data) != supplement["sha256"]:
                raise RuntimeError("supplemental original license digest changed")
            target = directory / source.name
            target.write_bytes(data)
            files.append({"file": str(target.relative_to(root)).replace("\\", "/"), "source_relative": supplement.get("kind", "upstream original omitted from crate archive"), "source_url": supplement["url"], "revision": supplement["revision"], "sha256": digest(data)})
        entries.append({"name": package["name"], "version": package["version"], "source": package["source"], "license": package.get("license"), "repository": package.get("repository"), "authors": package.get("authors", []), "files": files})
    missing = [f"{row['name']} {row['version']}" for row in entries if not row["files"]]
    toolchain = []
    sysroot = subprocess.run([args.rustc, "--print", "sysroot"], capture_output=True, text=True, encoding="utf-8", timeout=90, check=True).stdout.strip()
    rust_docs = Path(sysroot) / "share" / "doc" / "rust"
    for name in ["LICENSE-MIT", "LICENSE-APACHE", "COPYRIGHT", "COPYRIGHT-library.html"]:
        source = rust_docs / name
        if source.is_file():
            data = legal_text(source)
            target = root / "licenses" / "rust-toolchain" / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
            toolchain.append({"file": str(target.relative_to(root)).replace("\\", "/"), "sha256": digest(data)})
    elections = ["eventsource-stream 0.2.3", "r-efi 5.3.0", "r-efi 6.0.0", "winapi-i686-pc-windows-gnu 0.4.0", "winapi-x86_64-pc-windows-gnu 0.4.0"]
    receipt = {"cargo_lock_sha256": digest(lock_path.read_bytes()), "package_count": len(entries), "scope": "all locked resolved third-party packages, including build and target dependencies", "missing_original_texts": missing, "packages_without_standalone_license_in_archive": elections, "distribution_license_elections": {name: "Apache-2.0" for name in elections}, "rust_toolchain_notices": toolchain, "packages": entries}
    (root / "licenses" / "manifest.json").write_text(json.dumps(receipt, indent=2, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")
    lines = ["# Third-party notices", "", "This bundle records all third-party packages in the locked Cargo resolution,", "including build and target dependencies. It does not assert that every package", "is linked into the executable. Original license and notice text is retained.", "", f"Cargo.lock SHA256: `{receipt['cargo_lock_sha256']}`.", "", "Rust standard-library/toolchain notices are retained under `licenses/rust-toolchain`.", "Operating-system runtime libraries are supplied by the OS. Local credentials", "and configuration are excluded from this source-license collection.", "", "Regenerate with `python scripts/collect-licenses.py` using cached Cargo sources.", "", "| Package | License expression | Original texts |", "| --- | --- | --- |"]
    for row in entries:
        links = " · ".join(f"[{Path(file['file']).name}]({file['file']})" for file in row["files"]) or "Not supplied in cached package"
        lines.append(f"| {row['name']} {row['version']} | {row['license'] or 'Unspecified'} | {links} |")
    if missing:
        lines += ["", "Packages whose cached source does not supply original license/notice text:", ""] + ["- " + name for name in missing]
    lines += ["", "`eventsource-stream 0.2.3`, `r-efi 5.3.0/6.0.0`, and the two winapi GNU", "import-library packages 0.4.0 offer Apache-2.0 in their exact source-package", "declarations but omit separate license files from their crate archives.", "This distribution elects Apache-2.0 for these packages and includes its", "complete standard terms, the exact declaration/revision or archive checksum,", "and the original r-efi AUTHORS. No copyright owner or year has been invented.", "`licenses/upstream-supplements.json` records every source and SHA256."]
    (root / "THIRD-PARTY-NOTICES.md").write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print(json.dumps({"packages": len(entries), "notice_files": sum(len(row["files"]) for row in entries), "missing_original_texts": missing, "lock_sha256": receipt["cargo_lock_sha256"]}, ensure_ascii=False))


if __name__ == "__main__":
    main()
