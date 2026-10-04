"""Real sealed member and reviewer start; no provider-backed throwaway pane."""
import json
from pathlib import Path
import sys
import tomllib

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
import guest


def main():
    guest.open_project(strict=True)
    guest.HOME.joinpath('control.json').write_text(json.dumps({'hold': []}))
    member = guest.lane()
    def sealed():
        events = [tomllib.loads(p.read_text()) for p in (guest.PROJECT / '.state/events').glob('*.toml')]
        return any(e.get('thread') == member['id'] and e.get('payload', {}).get('done', {}).get('sha')
                   for e in events)
    guest.poll('member seal', sealed)
    guest.HOME.joinpath('control.json').write_text(json.dumps({'hold': ['mid-review']}))
    config = guest.HOME / '.config/herdr-ade/config.toml'
    with config.open('a') as out:
        out.write(f'\n[dispatch]\nmachine = "{guest.INSTANCE.machine}"\n')
    guest.run('ssh', guest.INSTANCE.machine, '"$HOME/bin/wall-pi" --wall-check --wall-lane')
    project = guest.PROJECT / 'PROJECT.md'
    project.write_text(project.read_text().replace(
        '[[repos]]\n', '[[repos]]\nreview_machine = "local"\ngates = [{ command = "git diff --check" }]\n'))
    guest.run('ha', 'review', 'wall')
    review = tomllib.loads(next((guest.PROJECT / '.state/reviews').glob('*.toml')).read_text())
    reviewer = guest.target(review['reviewer'])
    actual = reviewer.get('machine') or 'local'
    print('EXPECTED: reviewer and pile gates explicitly local despite ready dispatch box', flush=True)
    print(f'ACTUAL: reviewer={actual}, record={review.get("review_machine")}', flush=True)
    if actual != 'local' or review.get('review_machine') != 'local':
        raise RuntimeError('D60 review_machine ignored')
    guest.poll('reviewer brief submission', lambda: guest.target(reviewer['id']).get('brief_submitted'), seconds=60)
    reviewer = guest.target(reviewer['id'])
    packet = Path(reviewer['thread_dir']).joinpath('brief.md').read_text()
    if 'Reviewer machine: `local`' not in packet or '- Machine: local.' not in packet:
        raise RuntimeError('D60 reviewer packet omitted machine')
    print('EXPECTED: packet names local review machine; ACTUAL: local named in frozen packet', flush=True)


if __name__ == '__main__':
    main()
