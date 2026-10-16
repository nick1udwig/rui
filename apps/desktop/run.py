#!/usr/bin/env python3
"""Share existing desktop applications through the mobile web client."""
import importlib.util
from pathlib import Path

spec = importlib.util.spec_from_file_location('rui_launcher', Path(__file__).resolve().parents[1] / 'browser/run.py')
launcher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(launcher)

if __name__ == '__main__':
    raise SystemExit(launcher.main(workspace='desktop'))
