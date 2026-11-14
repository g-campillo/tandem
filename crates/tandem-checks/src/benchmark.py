"""Tandem comparison harness v1. Project callbacks are trusted like test code."""
import copy
import importlib.util
import itertools
import json
import pathlib
import sys
import time

path = pathlib.Path.cwd() / "benchmarks/tandem_compare.py"
# The runner sets cwd to its read-only captured project. -I still ignores
# inherited PYTHONPATH/user site settings; only this explicit snapshot is added.
sys.path.insert(0, str(path.parent.parent))
spec = importlib.util.spec_from_file_location("tandem_comparison", path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
cases = list(itertools.islice(module.cases(), 13))
assert 1 <= len(cases) <= 12, "Provide 1–12 named comparison cases"
rows = []
labels = set()
for entry in cases:
    assert isinstance(entry, tuple) and len(entry) == 2, "Each case must be (label, input)"
    label, case = entry
    assert isinstance(label, str) and 0 < len(label) <= 160 and label not in labels
    labels.add(label)
    values = {"baseline": [], "candidate": []}
    # Copy/setup costs are excluded. Each callback receives a fresh input.
    for sample in range(9):
        outputs = {}
        for name in (["baseline", "candidate"] if sample % 2 == 0 else ["candidate", "baseline"]):
            argument = copy.deepcopy(case)
            callback = getattr(module, name)
            started = time.perf_counter_ns()
            outputs[name] = callback(argument)
            elapsed = time.perf_counter_ns() - started
            if sample >= 2:
                values[name].append(elapsed)
        assert outputs["baseline"] == outputs["candidate"], f"Behavior differs for {label}"
    rows.append({"label": label, "baseline_ns": values["baseline"], "candidate_ns": values["candidate"]})
print("TANDEM_BENCHMARK_V1=" + json.dumps({"method": "perf_counter_ns; alternating order; deepcopy/setup excluded", "warmups": 2, "samples": 7, "cases": rows}))
