#!/usr/bin/env python3
# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Sweeps every mesy input's pulser through channels/positions/amplitudes."""

import argparse
import json
import subprocess
import sys
import time

# module type -> channel count, other module types are skipped
CHANNELS_BY_MODTYPE = {
    'mpsd8sadc': 8, 'mpsd8': 8, 'mpsd8p': 8,
    'mstd16': 16, 'mstd16p': 16,
}

DEFAULT_AMPS = [30, 60]
DEFAULT_POSITIONS = ['left', 'middle', 'right']
DEFAULT_DWELL = 0.125


def umami_ctl(ipc, *args):
    result = subprocess.run(['umami-ctl', '--ipc', ipc, *args],  # noqa: S607
                            capture_output=True, text=True, check=False)
    if result.returncode != 0:
        raise RuntimeError(
            f'umami-ctl {" ".join(args)} failed: {result.stderr.strip()}')
    return result.stdout


def get_params(ipc):
    return json.loads(umami_ctl(ipc, 'get-params', '--full'))


def set_pulser(ipc, input_name, slot, chan, pos, amp, on):
    payload = {f'{input_name}.pulser': {str(slot): {
        'chan': chan, 'pos': pos, 'amp': amp, 'on': on,
    }}}
    umami_ctl(ipc, 'set-params', json.dumps(payload))


def discover_targets(params, only_inputs):
    """Yield (input_name, slot, n_channels) for every real mpsd/mstd slot."""
    for key, info in sorted(params.items()):
        if (not key.endswith('._info') or info['kind'] != 'input'
                or info['type'] != 'mesy'):
            continue
        input_name = key[:-len('._info')]
        if only_inputs and input_name not in only_inputs:
            continue
        found = params[f'{input_name}.found']['value']
        for slot, module in enumerate(found):
            n_channels = CHANNELS_BY_MODTYPE.get(module['mod_type'])
            if n_channels is not None:
                yield input_name, slot, n_channels


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ipc', default='umami',
                        help='IPC name of the running instance')
    parser.add_argument('--dwell', type=float, default=DEFAULT_DWELL,
                        help=f'seconds to hold each step (default: {DEFAULT_DWELL})')
    parser.add_argument('--amps', default=','.join(map(str, DEFAULT_AMPS)),
                        help=f'comma-separated amplitudes to test '
                        f'(default: {DEFAULT_AMPS})')
    parser.add_argument('--positions', default=','.join(DEFAULT_POSITIONS),
                        help='comma-separated positions to test, '
                        f'from left/middle/right (default: {DEFAULT_POSITIONS})')
    parser.add_argument('--inputs', default='',
                        help='comma-separated input names to restrict to '
                        '(default: all mesy inputs)')
    parser.add_argument('--dry-run', action='store_true',
                        help='print the planned sequence, '
                        "don't actually send anything")
    args = parser.parse_args()

    amps = [int(a) for a in args.amps.split(',') if a]
    positions = [p for p in args.positions.split(',') if p]
    only_inputs = {n for n in args.inputs.split(',') if n}

    params = get_params(args.ipc)
    targets = list(discover_targets(params, only_inputs))
    if not targets:
        sys.exit('No mesy inputs with a detected MPSD/MSTD module found.')

    steps = [(input_name, slot, chan, pos, amp)
             for input_name, slot, n_channels in targets
             for chan in range(n_channels)
             for pos in positions
             for amp in amps]
    print(f'{len(steps)} steps planned across {len(targets)} module(s), '
          f'{len(steps) * args.dwell:.0f}s total at {args.dwell}s dwell')

    if args.dry_run:
        for input_name, slot, chan, pos, amp in steps:
            print(f'{input_name} slot {slot}: chan {chan}, pos {pos}, amp {amp}')
        return

    current = None
    try:
        for input_name, slot, chan, pos, amp in steps:
            print(f'{input_name} slot {slot}: chan {chan}, pos {pos}, amp {amp} ON')
            set_pulser(args.ipc, input_name, slot, chan, pos, amp, True)
            current = (input_name, slot)
            time.sleep(args.dwell)
            set_pulser(args.ipc, input_name, slot, 0, 'middle', 0, False)
            current = None
    finally:
        if current is not None:
            input_name, slot = current
            print(f'Interrupted -- turning off {input_name} slot {slot}')
            set_pulser(args.ipc, input_name, slot, 0, 'middle', 0, False)


if __name__ == '__main__':
    main()
