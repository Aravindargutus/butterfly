#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Check the requirement-to-test links introduced by one pull request."""
import argparse
import re
import subprocess
import sys
import tomllib
from functools import cache


def git(*args):
    return subprocess.check_output(["git", *args])


def files_at(revision):
    records = git("ls-tree", "-rz", "--end-of-options", revision).split(b"\0")
    files = {}
    for record in filter(None, records):
        metadata, path = record.split(b"\t", 1)
        mode, _, oid = metadata.split()
        if mode in (b"100644", b"100755"):
            files[path.decode("utf-8", "surrogateescape")] = oid.decode()
    return files


def requirements(files):
    result = {}
    for path, oid in files.items():
        if not path.startswith("docs/requirements/") or not path.endswith(".toml"):
            continue
        for req_id, spec in tomllib.loads(git("cat-file", "--end-of-options", "blob", oid).decode()).items():
            if not re.fullmatch(r"BF-[A-Z0-9]+(?:-[A-Z0-9]+)*-[0-9]+", req_id) or req_id in result:
                raise ValueError(f"{path}: invalid or duplicate requirement ID {req_id}")
            if not isinstance(spec, dict) or not isinstance(spec.get("statement"), str) or not spec["statement"].strip():
                raise ValueError(f"{path}: {req_id} needs a requirement statement")
            tests = spec.get("tests")
            if not isinstance(tests, list) or not tests or not all(isinstance(p, str) for p in tests):
                raise ValueError(f"{path}: {req_id} needs a list of test suites")
            result[req_id] = (" ".join(spec["statement"].split()), set(tests))
    return result


def ci_suite(path, files, members):
    if path not in files:
        return False
    # This script is exercised explicitly in full CI, alongside the Cargo and Node suites.
    # The browser checks are the ones ui/browser/run.cjs runs there.
    if path == "scripts/test_pr_requirements.py" or re.fullmatch(r"ui/[^/]+\.test\.cjs|ui/browser/checks/[^/]+\.cjs", path):
        return True
    match = re.fullmatch(r"crates/([^/]+)/tests/[^/]+\.rs", path)
    return bool(match and f"crates/{match[1]}/Cargo.toml" in files and
                f"crates/{match[1]}" in members)


def code_lines(path, blob):
    """The lines of a suite that are code: not blank, and not comment only.

    A `*` opens a comment line only inside a `/* ... */` block; elsewhere it is
    code (`*flag = true;`, a generator method), so the block is followed
    through the file rather than guessed from each line's first character.
    """
    result = []
    in_block = False
    for line in blob.splitlines():
        line = line.strip()
        if path.endswith(".py"):
            is_code = bool(line) and not line.startswith(b"#")
        elif in_block:
            is_code = False
            in_block = b"*/" not in line
        elif line.startswith(b"/*"):
            is_code = False
            in_block = b"*/" not in line[2:]
        else:
            is_code = bool(line) and not line.startswith(b"//")
        result.append(is_code)
    return result


def check(base, head):
    # Keep local refs like origin/main usable; only resolved commit IDs reach other commands.
    base, head = [git("rev-parse", "--verify", "--end-of-options", ref + "^{commit}").decode().strip() for ref in (base, head)]
    ancestor = git("merge-base", "--end-of-options", base, head).decode().strip()
    before = files_at(ancestor)
    after = files_at(head)
    old = requirements(before)
    new = requirements(after)
    # A statement or its list of suites changing is a change to verify; so is
    # a requirement going away with the behavior it described.
    changes = [req_id for req_id, spec in new.items() if req_id not in old or spec != old[req_id]]
    removed = [req_id for req_id in old if req_id not in new]
    if not changes and not removed:
        raise ValueError("add or update a requirement statement under docs/requirements/ for this PR")
    if ancestor == base:
        merged = after
    else:
        try:
            tree = git("merge-tree", "--write-tree", "--end-of-options", base, head)
        except subprocess.CalledProcessError as error:
            if error.returncode != 1:
                raise
            raise ValueError("cannot build the prospective merge tree: the branch conflicts with its base. "
                             "Merge or rebase onto the base branch, resolve the conflicts, and run this again") from None
        merged = files_at(tree.decode().splitlines()[0])
    registry = new if merged is after else requirements(merged)
    members = tomllib.loads(git("cat-file", "--end-of-options", "blob", merged["Cargo.toml"]).decode())["workspace"]["members"]

    def blob(files, path):
        return git("cat-file", "--end-of-options", "blob", files[path])

    @cache
    def changed_test(path):
        if path not in after or before.get(path) == after[path]:
            return False
        was = code_lines(path, blob(before, path)) if path in before else []
        now = code_lines(path, blob(after, path))
        patch = git("diff", "--unified=0", "--ignore-all-space", "--no-color", "--no-ext-diff", "--no-textconv",
                    "--no-renames", "--end-of-options", ancestor, head, "--", path)
        removed_code = False
        for line in patch.splitlines():
            hunk = re.match(rb"@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@", line)
            if not hunk:
                continue
            old_start, old_count, new_start, new_count = (int(n) if n is not None else 1 for n in hunk.groups())
            if any(now[new_start - 1:new_start - 1 + new_count]):
                return True
            removed_code = removed_code or any(was[old_start - 1:old_start - 1 + old_count])
        return removed_code and any(now)

    for req_id, (_, tests) in registry.items():
        for path in tests:
            if not ci_suite(path, merged, members):
                raise ValueError(f"{req_id}: missing or unsupported CI test suite {path!r}")
    for req_id in changes:
        tests = registry[req_id][1]
        if not any(changed_test(path) for path in tests):
            raise ValueError(f"{req_id}: add or update a linked test suite in this PR")
        print(f"{req_id}: " + ", ".join(sorted(tests)))
    for req_id in removed:
        tests = old[req_id][1]
        if not any(path not in after or changed_test(path) for path in tests):
            raise ValueError(f"{req_id}: removed, so remove or update a test suite it linked in this PR")
        print(f"{req_id}: removed with " + ", ".join(sorted(tests)))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("base")
    parser.add_argument("head")
    args = parser.parse_args()
    try:
        check(args.base, args.head)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
