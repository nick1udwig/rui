#!/usr/bin/env python3
"""Run browser/GTK integration only on the harness-owned authenticated display."""
import argparse
from pathlib import Path
import sys
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from fixtures.gtk.mirror_check import run_private
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--xvfb', required=True, type=Path)
parser.add_argument('--timeout', type=int, default=45)
args = parser.parse_args()
if not 45 <= args.timeout <= 1800:
    parser.error('timeout must be between 45 and 1800 seconds')
print(run_private(str(args.xvfb.resolve()), ['node', str(ROOT / 'demos/web/check.mjs')], timeout=args.timeout).strip())
