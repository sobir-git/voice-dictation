import os
from copy import deepcopy
from pathlib import Path
from typing import Any, Dict

import yaml


class Config:
    def __init__(self):
        self.config_dir = Path(os.path.expanduser('~/.config/speech-to-text'))
        self.data_dir = Path(os.path.expanduser('~/.local/share/speech-to-text'))
        self.config_file = self.config_dir / 'config.yaml'

        self.defaults: Dict[str, Any] = {
            'audio': {
                'sample_rate': 16000,
                'format': 'S16_LE',
                'channels': 1,
                'temp_file': '/tmp/stt_recording.wav',
            },
            'transcription': {
                'model': 'base.en',
                'compute_type': 'int8',
                'language': 'en',
                'beam_size': 1,
                'vad_filter': True,
            },
            'input': {
                'device_path': '',
                'trigger_key': 'KEY_F16',
            },
            'output': {
                'method': 'auto',
                'add_space': True,
                'type_interval': 0.0,
            },
            'notifications': {
                'enabled': True,
                'audio_feedback': True,
            },
            'logging': {
                'level': 'INFO',
                'file': '~/.local/share/speech-to-text/app.log',
                'max_size_mb': 10,
            },
        }
        self.data: Dict[str, Any] = deepcopy(self.defaults)

        self._ensure_dirs()
        self.load()

    def _ensure_dirs(self) -> None:
        self.config_dir.mkdir(parents=True, exist_ok=True)
        self.data_dir.mkdir(parents=True, exist_ok=True)

    def load(self) -> None:
        self.data = deepcopy(self.defaults)
        if not self.config_file.exists():
            return
        with open(self.config_file, 'r', encoding='utf-8') as f:
            loaded = yaml.safe_load(f) or {}
        self._deep_update(self.data, loaded)

    def save(self) -> None:
        self._ensure_dirs()
        with open(self.config_file, 'w', encoding='utf-8') as f:
            yaml.safe_dump(self.data, f, sort_keys=False)

    def get(self, section: str, key: str, default=None):
        return self.data.get(section, {}).get(key, default)

    def _deep_update(self, dst: Dict[str, Any], src: Dict[str, Any]) -> None:
        for k, v in src.items():
            if isinstance(v, dict) and isinstance(dst.get(k), dict):
                self._deep_update(dst[k], v)  # type: ignore[index]
            else:
                dst[k] = v
