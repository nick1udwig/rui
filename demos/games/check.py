#!/usr/bin/env python3
"""Check a live MAME/RUI/browser session on an owned authenticated display."""
import argparse
from pathlib import Path
import sys
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from fixtures.gtk.mirror_check import run_private
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--xvfb', type=Path, required=True)
args = parser.parse_args()
print(run_private(str(args.xvfb.resolve()), ['node', str(ROOT/'demos/games/check.mjs')], timeout=180).strip())
