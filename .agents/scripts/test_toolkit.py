import contextlib
import importlib.util
import io
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def load(name):
    spec = importlib.util.spec_from_file_location(f"weft_toolkit_{name}", HERE / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


checker = load("check_toolkit")
verify = load("verify")

SKILL = "---\nname: example\ndescription: An example skill.\n---\n[Contract](../../../AGENTS.md)\n"
COMMAND = "---\nname: run\ndescription: An example command.\n---\n# Run\n"
ROLE = "---\nname: role\ndescription: An example role.\n---\n# Role\n"
CATALOG = ("[Example](skills/example/SKILL.md)\n[Run](commands/run.md)\n"
           "[Role](agents/role.md)\n")
FORM = ("name: Change\ndescription: A change.\nbody:\n"
        "  - type: markdown\n    attributes:\n      value: Intro\n"
        "  - type: textarea\n    id: observed\n    attributes:\n      label: Observed\n")


class ToolkitCheck(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        for name in checker.TOP_LEVEL:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("# Contract\n\n## 2. Evidence levels\n", encoding="utf-8")
        self.skill = self.write(".agents/skills/example/SKILL.md", SKILL)
        self.write(".agents/commands/run.md", COMMAND)
        self.write(".agents/agents/role.md", ROLE)
        self.catalog = self.write(".agents/README.md", CATALOG)
        self.form = self.write(".github/ISSUE_TEMPLATE/change.yml", FORM)
        self.write(".github/ISSUE_TEMPLATE/config.yml", "blank_issues_enabled: false\n")

    def write(self, relative, text):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return path

    def errors_containing(self, text):
        return [error for error in checker.check(self.root) if text in error]

    def append_to_skill(self, text):
        self.skill.write_text(SKILL + text + "\n", encoding="utf-8")

    def test_valid_toolkit_passes(self):
        self.assertEqual(checker.check(self.root), [])

    def test_missing_contract_breaks_links(self):
        (self.root / "AGENTS.md").unlink()
        self.assertTrue(self.errors_containing("broken local link"))

    def test_link_outside_repository_is_rejected(self):
        self.append_to_skill("[Outside](../../../../outside.md)")
        self.assertTrue(self.errors_containing("broken local link"))

    def test_existing_and_missing_anchors(self):
        self.append_to_skill("[Levels](../../../AGENTS.md#2-evidence-levels)")
        self.assertEqual(checker.check(self.root), [])
        self.append_to_skill("[Levels](../../../AGENTS.md#3-missing)")
        self.assertTrue(self.errors_containing("missing anchor"))

    def test_same_document_anchor(self):
        self.skill.write_text(SKILL + "## Local `code` heading\n[Here](#local-code-heading)\n",
                              encoding="utf-8")
        self.assertEqual(checker.check(self.root), [])
        self.skill.write_text(SKILL + "[Here](#absent)\n", encoding="utf-8")
        self.assertTrue(self.errors_containing("missing anchor"))

    def test_duplicate_headings_receive_numbered_anchors(self):
        self.skill.write_text(SKILL + "## Step\n## Step\n[Second](#step-1)\n", encoding="utf-8")
        self.assertEqual(checker.check(self.root), [])

    def test_literal_examples_are_not_links(self):
        for example in ["`[X](missing.md)`", "```md\n[X](missing.md)\n```",
                        "<!-- [X](missing.md) -->", r"\[X](missing.md)"]:
            with self.subTest(example=example):
                self.append_to_skill(example)
                self.assertEqual(checker.check(self.root), [])

    def test_reference_links_are_checked(self):
        self.append_to_skill("[Missing][doc]\n\n[doc]: missing.md")
        self.assertTrue(self.errors_containing("broken local link"))

    def test_metadata_must_match_path_and_be_unique_strings(self):
        for fields in ["name: other\ndescription: Example.", "name: example\ndescription:",
                       "name: example\ndescription: 42", "name: example\nname: example\n"
                       "description: Example.", "name: example\ndescription: [unterminated"]:
            with self.subTest(fields=fields):
                self.skill.write_text(f"---\n{fields}\n---\n", encoding="utf-8")
                self.assertTrue(self.errors_containing("name must match path"))

    def test_missing_front_matter_is_rejected(self):
        self.skill.write_text("# No metadata\n", encoding="utf-8")
        self.assertTrue(self.errors_containing("missing front matter"))

    def test_uncatalogued_document_is_rejected(self):
        self.catalog.write_text("[Example](skills/example/SKILL.md)\n", encoding="utf-8")
        errors = self.errors_containing("absent from toolkit catalog")
        self.assertEqual(len(errors), 2)

    def test_catalog_needs_a_visible_link(self):
        self.catalog.write_text("`[Example](skills/example/SKILL.md)`\n"
                                "[Run](commands/run.md)\n[Role](agents/role.md)\n",
                                encoding="utf-8")
        self.assertTrue(self.errors_containing("absent from toolkit catalog"))

    def test_empty_toolkit_directory_is_rejected(self):
        (self.root / ".agents/agents/role.md").unlink()
        self.catalog.write_text("[Example](skills/example/SKILL.md)\n[Run](commands/run.md)\n",
                                encoding="utf-8")
        self.assertTrue(self.errors_containing("no documents in .agents/agents"))

    def test_issue_forms_are_validated(self):
        self.form.write_text("name: Change\nbody: []\n", encoding="utf-8")
        self.assertTrue(self.errors_containing("issue form needs"))
        self.form.write_text(FORM + "  - type: input\n    id: observed\n", encoding="utf-8")
        self.assertTrue(self.errors_containing("unique id"))
        self.form.write_text("name: [unterminated\n", encoding="utf-8")
        self.assertTrue(self.errors_containing("invalid YAML"))

    def test_repository_toolkit_passes(self):
        self.assertEqual(checker.check(ROOT), [])

    def test_missing_dependencies_fail_with_instructions(self):
        result = subprocess.run([sys.executable, "-I", "-S", "-B", str(HERE / "check_toolkit.py")],
                                capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 1)
        self.assertIn("requirements.txt", result.stderr)


class Runner(unittest.TestCase):
    def run_quietly(self, argv):
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            return verify.main(argv)

    def test_dry_run_from_another_directory_executes_nothing(self):
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run(
                [sys.executable, str(HERE / "verify.py"), "portable", "demos", "--dry-run"],
                cwd=directory, capture_output=True, text=True, check=False,
            )
            self.assertEqual(list(Path(directory).iterdir()), [])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("cargo fmt --all --check", result.stdout)
        self.assertIn("--target wasm32-wasip2", result.stdout)

    def test_dry_run_calls_no_subprocess(self):
        with patch.object(verify.subprocess, "run") as run:
            self.assertEqual(self.run_quietly(list(verify.PROFILES) + ["--dry-run"]), 0)
        run.assert_not_called()

    def test_commands_run_from_repository_root(self):
        with patch.object(verify.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 0)
            self.assertEqual(self.run_quietly(["servo-embed"]), 0)
        self.assertEqual(run.call_count, 2)
        for call in run.call_args_list:
            self.assertEqual(call.kwargs["cwd"], ROOT)
            self.assertEqual(call.kwargs["env"]["RUSTUP_AUTO_INSTALL"], "0")
            self.assertIsInstance(call.args[0], list)

    def test_failure_stops_remaining_checks_and_keeps_status(self):
        with patch.object(verify.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 7)
            self.assertEqual(self.run_quietly(["portable", "linux"]), 7)
        self.assertEqual(run.call_count, 1)

    def test_failure_in_later_command_stops_profile(self):
        statuses = [subprocess.CompletedProcess([], 0), subprocess.CompletedProcess([], 101)]
        with patch.object(verify.subprocess, "run", side_effect=statuses) as run:
            self.assertEqual(self.run_quietly(["portable", "linux"]), 101)
        self.assertEqual(run.call_count, 2)

    def test_signal_termination_is_failure(self):
        with patch.object(verify.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], -9)
            self.assertEqual(self.run_quietly(["toolkit"]), 1)

    def test_missing_executable_is_failure(self):
        with patch.object(verify.subprocess, "run", side_effect=FileNotFoundError("cargo")):
            self.assertEqual(self.run_quietly(["linux"]), 1)

    def test_real_failing_command_propagates(self):
        failing = {"failing": ("fails", [[sys.executable, "-c", "raise SystemExit(3)"]])}
        with patch.dict(verify.PROFILES, failing):
            self.assertEqual(self.run_quietly(["failing"]), 3)

    def test_unknown_profile_is_rejected_before_execution(self):
        with patch.object(verify.subprocess, "run") as run:
            with self.assertRaises(SystemExit) as error:
                self.run_quietly(["unknown"])
        self.assertEqual(error.exception.code, 2)
        run.assert_not_called()

    def test_repeated_profiles_run_once(self):
        with patch.object(verify.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 0)
            self.run_quietly(["servo-embed", "servo-embed"])
        self.assertEqual(run.call_count, 2)

    def test_profiles_never_mutate_install_or_publish(self):
        forbidden = {"install", "add", "update", "fix", "publish", "push", "commit", "sign",
                     "uninstall", "--bless", "--fix", "--allow-dirty"}
        for name, (_, commands) in verify.PROFILES.items():
            for command in commands:
                with self.subTest(profile=name, command=command):
                    self.assertTrue(all(isinstance(part, str) for part in command))
                    self.assertFalse(forbidden.intersection(command))
                    if command[:2] == ["cargo", "fmt"]:
                        self.assertIn("--check", command)

    def test_cargo_checks_use_the_locked_graph(self):
        for name, (_, commands) in verify.PROFILES.items():
            for command in commands:
                if command[0] == "cargo" and command[1] != "fmt":
                    with self.subTest(profile=name, command=command):
                        self.assertIn("--locked", command)


if __name__ == "__main__":
    unittest.main()
