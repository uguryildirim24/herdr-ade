"""Names shared by the host and guests; zero preserves the original wall."""
from dataclasses import dataclass
from pathlib import Path

AUTH = Path('/var/lib/herdr-wall-auth')
STAGES = Path('/var/lib/herdr-wall-builds')


@dataclass(frozen=True)
class Instance:
    number: int = 0

    def __post_init__(self):
        if not 0 <= self.number <= 8:
            raise ValueError('instance must be 1..8 (omit for the default)')

    @property
    def home(self):
        return Path('/home/wall' + (f'-{self.number}' if self.number else ''))

    @property
    def base(self):
        return Path('/var/lib/herdr-wall' + (f'-{self.number}' if self.number else ''))

    @property
    def user(self):
        return 'wall' + (str(self.number) if self.number else '')

    @property
    def box_user(self):
        return 'wallbox' + (str(self.number) if self.number else '')

    @property
    def port(self):
        return 22285 + 2 * self.number

    @property
    def box_port(self):
        return self.port + 1

    @property
    def unit(self):
        return 'herdr-wall' + (f'-{self.number}' if self.number else '')

    @property
    def slice(self):
        # No hyphens: systemd must not nest numbered slots under the default.
        return f'herdrwall{self.number}.slice'

    @property
    def machine(self):
        return 'wall-box' + (f'-{self.number}' if self.number else '')
