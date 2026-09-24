"""Decide the required ubuntu-latest and macos-latest checks from the selected jobs.

The main ruleset requires those two contexts. They pass only when change
selection succeeded, every selected job succeeded, and every unselected job
was skipped.
"""
import os
import sys

JOBS = ('core', 'tauri')


def problems(selection, jobs):
    """Return why the gate fails; an empty list means it passes.

    `jobs` maps a job to (selected, result): the `changes` output ('true' or
    'false') and the job's `needs.<job>.result`.
    """
    if selection != 'success':
        return [f'change selection ended as {selection or "unknown"}']
    found = []
    for job, (selected, result) in jobs.items():
        expected = 'success' if selected == 'true' else 'skipped'
        if result != expected:
            found.append(f'{job} ended as {result or "unknown"}, expected {expected}')
    return found


def main():
    env = os.environ
    found = problems(env.get('SELECTION', ''), {
        job: (env.get(f'{job.upper()}_SELECTED', ''), env.get(f'{job.upper()}_RESULT', ''))
        for job in JOBS
    })
    for problem in found:
        print(f'::error::{problem}')
    if not found:
        print('Every selected check passed.')
    return 1 if found else 0


if __name__ == '__main__':
    sys.exit(main())
