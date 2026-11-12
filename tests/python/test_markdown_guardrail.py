"""Exercise the documentation guardrail against real linter/LSP regressions."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(
    shutil.which('markdownlint-cli2') and shutil.which('marksman'),
    'Markdown guardrail integration requires markdownlint-cli2 and marksman',
)
class MarkdownGuardrailTest(unittest.TestCase):
    def test_clean_ambiguous_broken_and_lint_diagnostics(self):
        with tempfile.TemporaryDirectory(prefix='qoda-markdown-') as directory:
            root = Path(directory)
            subprocess.run(['git', 'init', '--quiet', str(root)], check=True)
            (root / 'docs/research').mkdir(parents=True)
            (root / 'docs/report.md').write_text('# Report\n', encoding='utf-8')
            (root / 'docs/research/report.md').write_text('# Research\n', encoding='utf-8')
            source = root / 'docs/index.md'
            # Files are deliberately untracked, matching newly authored docs.
            source.write_text('# Index\n\n[Report](./report.md)\n', encoding='utf-8')

            def check():
                return subprocess.run(
                    [sys.executable, str(ROOT / 'tools/check-markdown.py'), '--root', str(root)],
                    text=True, capture_output=True, timeout=45,
                )

            clean = check()
            self.assertEqual(clean.returncode, 0, clean.stdout + clean.stderr)
            source.write_text('# Index\n\n[Report](report.md)\n', encoding='utf-8')
            ambiguous = check()
            self.assertNotEqual(ambiguous.returncode, 0)
            self.assertIn('Ambiguous link', ambiguous.stdout)

            source.write_text('# Index\n\n[Missing](./missing.md)\n', encoding='utf-8')
            broken = check()
            self.assertNotEqual(broken.returncode, 0)
            self.assertIn('docs/index.md:3:', broken.stdout)

            source.write_text('# Index\n\n\nText\n', encoding='utf-8')
            lint = check()
            self.assertNotEqual(lint.returncode, 0)
            self.assertIn('MD012', lint.stdout + lint.stderr)
