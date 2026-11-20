#!/usr/bin/env python3
"""Generate the notices accompanying Sansho release binaries from a populated Cargo cache.

Provenance: original implementation using the Cargo metadata v1 format and Python stdlib.
"""

import argparse
from collections import defaultdict
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "THIRD-PARTY-LICENSES.md"
TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
)
BINARIES = {"sansho", "sansho-mcp"}

# These Wasmtime packages omit the workspace license when published. The pinned
# cranelift-codegen package includes the complete license from the same revision.
WASMTIME_OMISSIONS = {
    "cranelift-assembler-x64", "cranelift-bitset", "wasmtime-internal-core",
    "wasmtime-internal-jit-icache-coherence",
}


def shipped_dependencies():
    packages = {}
    for target in TARGETS:
        result = subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--locked", "--offline",
             "--all-features", "--filter-platform", target], cwd=ROOT,
        )
        graph = json.loads(result)
        by_id = {p["id"]: p for p in graph["packages"]}
        nodes = {n["id"]: n for n in graph["resolve"]["nodes"]}
        members = set(graph["workspace_members"])
        roots = {
            t["name"]: i for i in members for t in by_id[i]["targets"]
            if "bin" in t["kind"] and t["name"] in BINARIES
        }
        if roots.keys() != BINARIES:
            raise ValueError(f"{target}: missing release binary in metadata")
        pending = list(roots.values())
        visited = set()
        while pending:
            i = pending.pop()
            if i in visited:
                continue
            visited.add(i)
            if i not in members:
                packages[i] = by_id[i]
            for dep in nodes[i]["deps"]:
                kinds = dep["dep_kinds"]
                if not kinds or any(k["kind"] not in ("dev", "build") for k in kinds):
                    pending.append(dep["pkg"])
    return sorted(packages.values(), key=lambda p: (p["name"], p["version"]))


def directory(package):
    return Path(package["manifest_path"]).parent


def read(path):
    return path.read_text(encoding="utf-8")


def notices(package):
    root = directory(package)
    prefixes = ("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT")
    paths = {p for p in root.rglob("*") if p.name.upper().startswith(prefixes) and p.is_file()}
    if package["license_file"]:
        paths.add(root / package["license_file"])
    return dict(sorted((p.relative_to(root).as_posix(), read(p)) for p in paths))


def upstream_license(package, donor):
    def revision(p):
        return json.loads(read(directory(p) / ".cargo_vcs_info.json"))["git"]["sha1"]

    if (not donor or package["license"] != "Apache-2.0 WITH LLVM-exception"
            or donor["license"] != package["license"] or revision(package) != revision(donor)):
        raise ValueError(f"{package['name']}: cannot verify matching Wasmtime license")
    license_text = read(directory(donor) / "LICENSE")
    if "LLVM Exceptions" not in license_text:
        raise ValueError("cranelift-codegen LICENSE is missing the LLVM exceptions")
    label = f"{donor['name']} {donor['version']} LICENSE (same upstream revision)"
    return {label: license_text}


def standard_licenses(package):
    expression = package["license"].replace("/", " OR ")
    ids = [part.strip() for part in expression.split(" OR ")]
    if not set(ids) <= {"MIT", "Apache-2.0"}:
        raise ValueError(f"{package['name']}: missing license file for {expression}")
    root = directory(package)
    statements = []
    for path in sorted(root.iterdir()):
        if path.is_file() and path.name.upper().startswith(("README", "AUTHORS", "COPYRIGHT", "CARGO.TOML")):
            for line in read(path).splitlines():
                if re.search(r"SPDX-FileCopyrightText:|\bcopyright\s+(?:\(c\)|©|\d{4})|©\s*\d{4}", line, re.I):
                    if line.strip() not in statements:
                        statements.append(line.strip())
    texts = {}
    for spdx in ids:
        if spdx == "MIT":
            # The repository's license contains the standard MIT grant. Its
            # project-specific attribution belongs only to Sansho.
            _, grant = read(ROOT / "LICENSE-MIT").split("\n\nPermission is hereby granted,", 1)
            body = "MIT License\n\nPermission is hereby granted," + grant
        else:
            body = read(ROOT / "LICENSE-APACHE")
        texts[f"{spdx} standard text"] = "\n\n".join([*statements, body])
    return texts


def generate():
    packages = shipped_dependencies()
    donor = next((p for p in packages if p["name"] == "cranelift-codegen"), None)
    groups = defaultdict(list)
    texts = {}
    missing = []
    for p in packages:
        label = f"{p['name']} {p['version']}"
        files = notices(p)
        if not p["license"] and not p["license_file"]:
            raise ValueError(f"{label}: no declared license")
        # Some crates ship only component-specific COPYING files. Preserve
        # those as well as providing the crate's own declared license.
        if not p["license_file"] and not any(
            name.upper().startswith(("LICENSE", "LICENCE")) or name.upper() == "COPYING"
            for name in files
        ):
            extras = upstream_license(p, donor) if p["name"] in WASMTIME_OMISSIONS else standard_licenses(p)
            reason = f"{label} — " + ", ".join(extras)
            print(f"Missing crate license file: {reason}", file=sys.stderr)
            missing.append(reason)
            files.update(extras)
        refs = []
        for name, body in files.items():
            if not body.strip():
                raise ValueError(f"{label}: empty {name}")
            body = re.sub(r"[ \t]+$", "", body, flags=re.M)
            key = hashlib.sha256(body.encode()).hexdigest()
            texts[key] = body
            refs.append(f"[{name}](#text-{key[:16]})")
        groups[p["license"] or p["license_file"]].append((label, refs))

    lines = [
        "# Third-party licenses", "",
        "Generated by `python3 tools/third_party_licenses.py` from Cargo.lock. The inventory",
        "covers normal dependency edges from both release binaries with all features across",
        "the four release targets. It includes procedural macros and their normal dependencies.",
        "The headings give Cargo's declared expressions. Each file link leads to its full text.",
        "Identical texts share an entry. Component notices are included alongside crate licenses.",
        "", "## Packages without a packaged crate license", "",
        *[f"- {case}" for case in missing], "",
    ]
    for expression, entries in sorted(groups.items()):
        lines.extend([f"## {expression}", "", "| Crate and version | Files |", "| --- | --- |"])
        for label, refs in entries:
            lines.append(f"| {label} | {' · '.join(refs)} |")
        lines.append("")
    lines.extend(["## Full texts", ""])
    for key, body in sorted(texts.items()):
        fence = "`" * max(3, 1 + max((len(s) for s in re.findall(r"`+", body)), default=0))
        lines.extend([f"### Text {key[:16]}", "", fence, body.removesuffix("\n"), fence, ""])
    return "\n".join(lines).encode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail if the committed file is stale")
    args = parser.parse_args()
    content = generate()
    if args.check:
        if not OUTPUT.is_file() or OUTPUT.read_bytes() != content:
            sys.exit("THIRD-PARTY-LICENSES.md is stale. Run python3 tools/third_party_licenses.py.")
    else:
        OUTPUT.write_bytes(content)


if __name__ == "__main__":
    main()
