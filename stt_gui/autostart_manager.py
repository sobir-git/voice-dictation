import os
import subprocess
from pathlib import Path


class AutostartManager:
    def __init__(self):
        self.app_name = 'speech-to-text'
        self.systemd_unit = f'{self.app_name}.service'
        self.autostart_dir = Path(os.path.expanduser('~/.config/autostart'))
        self.autostart_file = self.autostart_dir / f'{self.app_name}.desktop'

    def is_enabled(self) -> bool:
        try:
            r = subprocess.run(['systemctl', '--user', 'is-enabled', self.systemd_unit], capture_output=True)
            if r.returncode == 0:
                return True
        except Exception:
            pass
        return self.autostart_file.exists()

    def enable(self, use_gui: bool = False) -> None:
        try:
            subprocess.run(['systemctl', '--user', 'enable', '--now', self.systemd_unit], check=False)
            return
        except Exception:
            pass

        self.autostart_dir.mkdir(parents=True, exist_ok=True)
        if not self.autostart_file.exists():
            self.autostart_file.write_text('')

    def disable(self) -> None:
        try:
            subprocess.run(['systemctl', '--user', 'disable', '--now', self.systemd_unit], check=False)
        except Exception:
            pass
        try:
            if self.autostart_file.exists():
                self.autostart_file.unlink()
        except Exception:
            pass
