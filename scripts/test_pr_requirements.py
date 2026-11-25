# SPDX-License-Identifier: MIT OR Apache-2.0
"""Exercise the PR policy against real temporary Git histories."""
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

CHECK = Path(__file__).with_name("check_pr_requirements.py").resolve()
SUITE = "ui/manual.test.cjs"
REQ = "docs/requirements/manual.toml"


class PolicyTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.repo = Path(temp.name)
        self.git("init", "-q")
        self.git("config", "user.name", "Test")
        self.git("config", "user.email", "test@example.invalid")
        self.write("Cargo.toml", '[workspace]\nmembers = []\n')
        self.write("README.md", "# Manual\n")
        self.write(SUITE, "const test = require('node:test');\n")
        self.write(REQ, self.record("The help shall show the input format."))
        self.base = self.commit()

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.repo, text=True).strip()

    def write(self, path, text):
        file = self.repo / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text)

    def record(self, statement, suite=SUITE):
        return f'[BF-DOC-001]\nstatement = "{statement}"\ntests = ["{suite}"]\n'

    def commit(self):
        self.git("add", "-A")
        self.git("commit", "-qm", "test change")
        return self.git("rev-parse", "HEAD")

    def verdict(self, base=None):
        return subprocess.run([sys.executable, CHECK, base or self.base, "HEAD"], cwd=self.repo, capture_output=True, text=True)

    def test_requires_both_parts_on_docs_prs(self):
        self.write("README.md", "# Updated manual\n")
        self.commit()
        self.assertIn("requirement statement", self.verdict().stderr)
        self.write(REQ, self.record("The help shall show an executable example."))
        self.commit()
        self.assertIn("linked test suite", self.verdict().stderr)
        self.write(SUITE, "// only a comment\n")
        self.commit()
        self.assertIn("linked test suite", self.verdict().stderr)
        self.write(SUITE, "test('example appears in help', verifyHelp);\n")
        self.commit()
        result = self.verdict()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_every_changed_statement_needs_a_linked_change(self):
        self.write(REQ, self.record("The help shall show examples."))
        self.write("ui/unrelated.test.cjs", "test('unrelated', run);\n")
        self.commit()
        self.assertIn("linked test suite", self.verdict().stderr)

    def test_unsupported_missing_and_duplicate(self):
        for path in ["README.md", "ui/missing.test.cjs"]:
            self.write(REQ, self.record("The help shall show examples.", path))
            self.commit()
            self.assertIn("unsupported CI test suite", self.verdict().stderr)
        self.write("docs/requirements/duplicate.toml", self.record("Another requirement."))
        self.commit()
        self.assertIn("duplicate requirement ID", self.verdict().stderr)

    def test_base_only_changes_do_not_count_toward_the_pr(self):
        self.git("checkout", "-qb", "other")
        self.write(REQ, self.record("The help shall show examples."))
        advanced_base = self.commit()
        self.git("checkout", "-q", self.base)
        self.write(SUITE, "test('help', verifyHelp);\n")
        self.commit()
        self.assertIn("requirement statement", self.verdict(advanced_base).stderr)


if __name__ == "__main__":
    unittest.main()
