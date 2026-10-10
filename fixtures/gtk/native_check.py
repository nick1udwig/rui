#!/usr/bin/env python3
"""Run the common-ledger native bridge on owned displays and a service-free bus."""
import argparse
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from fixtures.gtk.mirror_check import run_private


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--xvfb', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--case', '--scenario', choices=['semantic', 'unicode', 'restart', 'whole-native', 'native-context', 'native-fence', 'editor'], default='semantic')
    args = parser.parse_args()
    command = [str(Path('target/debug/gtk_command_check').resolve()), str(args.output)]
    if args.case != 'semantic':
        command.append(args.case)
    print(run_private(args.xvfb, command).strip())


if __name__ == '__main__':
    main()
