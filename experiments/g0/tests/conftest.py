import sys
from pathlib import Path

# allow `import experiments.g0...` when pytest is invoked from the
# worktree root OR from experiments/g0/
ROOT = Path(__file__).resolve().parents[3]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
