"""Small offline fixtures: counting, subsystem isolation and fail-closed joining."""
import importlib.util
from pathlib import Path
import tempfile
import hashlib
import json
from unittest.mock import patch
import unittest

spec = importlib.util.spec_from_file_location('gaps', Path(__file__).with_name('gaps.py'))
gaps = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gaps)


class ReportTests(unittest.TestCase):
    def test_join_counts_and_unexplained(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'never-reached.tsv'
            path.write_text('class\tmethod\tline\tmissed branches\n'
                            'test/belt/A\trun()V\t10\t2\n'
                            'test/belt/A\trun()V\t11\t1\n'
                            'test/fluid/A\trun()V\t10\t9\n')
            rule = dict(zip(['class', 'method', 'first', 'last', 'category', 'why'],
                            ['test/belt/A', 'run()V', '10', '10', 'harness-gap', 'Missing input']))
            provenance = dict(fuzzer='f', engine='e', create='c', command='cover 1', reporter='r')
            report, missing = gaps.generate(path, {'belts': ['test/belt/']}, [rule], 'belts', provenance)
            self.assertEqual(missing, 1)
            self.assertIn('| harness-gap | 1 | 2 |', report)
            self.assertNotIn('test/fluid', report)
            with self.assertRaisesRegex(ValueError, 'ambiguous'):
                gaps.generate(path, {'belts': ['test/belt/']}, [rule, rule], 'belts', provenance)
            rule['last'] = '11'
            _, missing = gaps.generate(path, {'belts': ['test/belt/']}, [rule], 'belts', provenance)
            self.assertEqual(missing, 0)


    def test_class_method_and_line_precedence(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'never-reached.tsv'
            path.write_text('class\tmethod\tline\tmissed branches\n'
                            'test/A\trun()V\t10\t1\n'
                            'test/A\trun(I)V\t11\t2\n'
                            'test/A\tother()V\t12\t3\n')
            base = dict(zip(['class', 'method', 'first', 'last', 'category', 'why'],
                            ['test/A', '*', '1', '100', 'harness-gap', 'Missing world']))
            method = dict(base, method='run', category='client', why='Client run')
            line = dict(base, first='11', last='11', category='error-path', why='Invalid argument')
            provenance = dict(fuzzer='f', engine='e', create='c', command='cover 1', reporter='r')
            report, missing = gaps.generate(path, {'test': ['test/']}, [base, method, line], 'test', provenance)
            self.assertEqual(missing, 0)
            self.assertIn('| client | 1 | 1 |', report)
            self.assertIn('| error-path | 1 | 2 |', report)
            self.assertIn('| harness-gap | 1 | 3 |', report)


    def test_command_requires_completed_matching_campaign_and_all_packages(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'coverage').mkdir()
            (root / 'coverage/subsystems.json').write_text(json.dumps({'belts': ['test/belt/', 'test/depot/']}))
            (root / 'coverage/review.json').write_text(json.dumps({'create': 'create-commit'}))
            (root / 'coverage/reasons.tsv').write_text(
                'class\tmethod\tfirst\tlast\tcategory\twhy\tsource_sha256\n'
                'test/belt/A\t*\t1\t100\tharness-gap\tMissing input\thash\n')
            data = root / 'input'
            data.mkdir()
            missed = data / 'never-reached.tsv'
            missed.write_text('class\tmethod\tline\tmissed branches\n'
                              'test/belt/A\trun()V\t10\t2\n')
            classes = data / 'classes.tsv'
            classes.write_text('class\tcovered\ttotal\ntest/belt/A\t0\t2\n')
            campaign = data / 'campaign.json'
            metadata = dict(fuzzer='f', engine='e', create='create-commit', command='cover 1', complete=True,
                            never_reached_sha256=hashlib.sha256(missed.read_bytes()).hexdigest())
            campaign.write_text(json.dumps(metadata))
            argv = ['gaps', '--input', str(missed), '--output', str(root / 'reports')]
            with patch.object(gaps, 'ROOT', root), patch.object(gaps, 'revision', return_value='r'), patch('sys.argv', argv):
                self.assertEqual(gaps.main(), 1)
                classes.write_text(classes.read_text() + 'test/depot/A\t0\t0\n')
                self.assertEqual(gaps.main(), 0)
                metadata['complete'] = False
                campaign.write_text(json.dumps(metadata))
                with self.assertRaises(SystemExit):
                    gaps.main()
                metadata['complete'] = True
                metadata['never_reached_sha256'] = 'wrong'
                campaign.write_text(json.dumps(metadata))
                with self.assertRaises(SystemExit):
                    gaps.main()


if __name__ == '__main__':
    unittest.main()
