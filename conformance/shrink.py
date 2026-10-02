"""Deterministic delta debugging for exact-wire generated replay failures.

The predicate must replay each candidate from a fresh state and return true only
for the same failure class. Bytes inside retained operations remain exact, so a
result can be handed directly to both decoders without re-encoding differences.
"""


def shrink_transcript(entries, reproduces, maximum_runs=512):
    current = list(entries)
    runs = 1
    if not reproduces(current):
        raise ValueError('input does not reproduce the failure')
    granularity = 2
    while current and runs < maximum_runs:
        width = max(1, (len(current) + granularity - 1) // granularity)
        reduced = False
        for start in range(0, len(current), width):
            if runs >= maximum_runs:
                break
            candidate = current[:start] + current[start + width:]
            runs += 1
            if reproduces(candidate):
                current = candidate
                granularity = max(2, granularity - 1)
                reduced = True
                break
        if not reduced:
            if granularity >= len(current):
                break
            granularity = min(len(current), granularity * 2)
    return current, runs
