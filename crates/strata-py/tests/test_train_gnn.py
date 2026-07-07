# The neurosymbolic loop, end to end: the training example must run and learn.
# torch missing → skipped locally; STRATA_REQUIRE_ORACLES=1 (CI) hard-fails.
import os
import pathlib
import subprocess
import sys

import pytest

REQUIRE = os.environ.get("STRATA_REQUIRE_ORACLES") == "1"
try:
    import torch  # noqa: F401  (presence check only)

    HAVE_TORCH = True
except ImportError:
    HAVE_TORCH = False

if REQUIRE:
    assert HAVE_TORCH, "STRATA_REQUIRE_ORACLES=1 but the torch package is missing"

pytestmark = pytest.mark.skipif(not HAVE_TORCH, reason="torch not installed")

EXAMPLE = pathlib.Path(__file__).resolve().parents[3] / "examples" / "python" / "train_gnn.py"


def test_training_example_learns():
    out = subprocess.run(
        [sys.executable, str(EXAMPLE)],
        capture_output=True,
        text=True,
        timeout=600,
        check=False,
    )
    assert out.returncode == 0, out.stdout + "\n" + out.stderr
    # the example asserts loss decreased and both queries learned, then prints:
    assert "TRAINED" in out.stdout
