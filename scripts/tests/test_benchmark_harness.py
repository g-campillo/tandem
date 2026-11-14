from pathlib import Path
import json
import subprocess
import sys
import tempfile
import unittest

HARNESS=Path(__file__).resolve().parents[2]/'crates/tandem-checks/src/benchmark.py'

class BenchmarkHarness(unittest.TestCase):
    def run_fixture(self,candidate,cases=None):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);(root/'benchmarks').mkdir()
            (root/'logic.py').write_text('def double(value):\n    return value + value\n')
            (root/'benchmarks/tandem_compare.py').write_text('from logic import double\ndef cases():\n    return [("zero", 0), ("positive", 21)]\ndef baseline(value):\n    return double(value)\ndef candidate(value):\n    return '+candidate+'\n')
            if cases is not None:
                file=root/'benchmarks/tandem_compare.py'
                file.write_text(file.read_text()+'\n'+cases)
            return subprocess.run([sys.executable,'-I','-B',str(HARNESS)],cwd=root,capture_output=True,text=True,timeout=10)
    def test_isolated_harness_imports_captured_project_modules(self):
        result=self.run_fixture('value * 2')
        self.assertEqual(result.returncode,0,result.stderr)
        receipt=json.loads(result.stdout.split('TANDEM_BENCHMARK_V1=',1)[1]);self.assertEqual(len(receipt['cases']),2);self.assertTrue(all(len(c['candidate_ns'])==7 for c in receipt['cases']))
    def test_case_iterator_is_bounded_before_workload_setup(self):
        result=self.run_fixture('value * 2', 'def cases():\n    for i in range(13):\n        yield (str(i), i)\n    raise RuntimeError("read past case limit")\n')
        self.assertNotEqual(result.returncode,0);self.assertIn('Provide 1–12',result.stderr);self.assertNotIn('read past case limit',result.stderr);self.assertNotIn('TANDEM_BENCHMARK_V1=',result.stdout)
    def test_unequal_outputs_never_produce_a_measurement_receipt(self):
        result=self.run_fixture('value + 1')
        self.assertNotEqual(result.returncode,0);self.assertIn('Behavior differs',result.stderr);self.assertNotIn('TANDEM_BENCHMARK_V1=',result.stdout)
if __name__=='__main__':unittest.main()
