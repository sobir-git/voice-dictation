import os
import math
import tempfile
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
                'device': 'default',
                'pipewire_node': '',
                'preprocess': True,
            },
            'transcription': {
                'model': 'base.en',
                'compute_type': 'int8',
                'language': 'en',
                'beam_size': 1,
                'vad_filter': True,
            },
            'input': {
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
            'ui': {
                'cursor_indicator': False,
            },
        }
        self.data: Dict[str, Any] = deepcopy(self.defaults)

        self._ensure_dirs()
        self.load()

    def _ensure_dirs(self) -> None:
        self.config_dir.mkdir(parents=True, exist_ok=True)
        self.data_dir.mkdir(parents=True, exist_ok=True)

    def load(self) -> None:
        candidate = deepcopy(self.defaults)
        if self.config_file.exists():
            with open(self.config_file, 'r', encoding='utf-8') as f:
                loaded = yaml.safe_load(f)
            if loaded is None:
                loaded = {}
            if not isinstance(loaded, dict):
                raise ValueError('Config must contain named sections, not a list or scalar')
            self._deep_update(candidate, loaded)
        self.validate(candidate)
        self.data = candidate

    @staticmethod
    def validate(data):
        from evdev import ecodes
        for section in ('audio', 'transcription', 'input', 'output', 'logging', 'notifications', 'ui'):
            if not isinstance(data.get(section), dict):
                raise ValueError(f'Config section {section} must be a mapping')
        for section, key, low, high in (
            ('audio', 'sample_rate', 8000, 192000), ('audio', 'channels', 1, 8),
            ('transcription', 'beam_size', 1, 10), ('logging', 'max_size_mb', 1, 1000)):
            value = data[section][key]
            if type(value) is not int or not low <= value <= high:
                raise ValueError(f'{section}.{key} must be an integer between {low} and {high}')
        for section, key in (('audio', 'preprocess'), ('transcription', 'vad_filter'),
                             ('output', 'add_space'), ('notifications', 'enabled'),
                             ('notifications', 'audio_feedback'), ('ui', 'cursor_indicator')):
            if type(data[section][key]) is not bool:
                raise ValueError(f'{section}.{key} must be true or false')
        key = data['input']['trigger_key']
        if not isinstance(key, str) or key not in ecodes.ecodes or not key.startswith(('KEY_', 'BTN_')):
            raise ValueError(f'Unknown hotkey: {key}')
        if data['output']['method'] not in ('auto', 'xdotool', 'ydotool', 'dotool', 'wtype', 'xclip', 'none'):
            raise ValueError('Unknown output method')
        interval = data['output']['type_interval']
        if type(interval) not in (int, float) or not math.isfinite(interval) or not 0 <= interval <= 1:
            raise ValueError('output.type_interval must be between 0 and 1 seconds')
        for section, key in (('audio', 'temp_file'), ('audio', 'device'), ('audio', 'format'),
                             ('transcription', 'model'), ('transcription', 'compute_type'), ('logging', 'file')):
            if not isinstance(data[section][key], str) or not data[section][key].strip():
                raise ValueError(f'{section}.{key} must be a nonempty string')
        if not isinstance(data['audio']['pipewire_node'], str):
            raise ValueError('audio.pipewire_node must be a string')
        if data['transcription']['language'] is not None and not isinstance(data['transcription']['language'], str):
            raise ValueError('transcription.language must be a language code or null')
        if data['logging']['level'] not in ('DEBUG', 'INFO', 'WARNING', 'ERROR', 'CRITICAL'):
            raise ValueError('Unknown logging level')

    def save(self) -> None:
        self.validate(self.data)
        self._ensure_dirs()
        fd, path = tempfile.mkstemp(prefix='.config-', dir=self.config_dir)
        try:
            with os.fdopen(fd, 'w', encoding='utf-8') as f:
                yaml.safe_dump(self.data, f, sort_keys=False)
                f.flush()
                os.fsync(f.fileno())
            os.replace(path, self.config_file)
        finally:
            if os.path.exists(path):
                os.unlink(path)

    def get(self, section: str, key: str, default=None):
        return self.data.get(section, {}).get(key, default)

    def _deep_update(self, dst: Dict[str, Any], src: Dict[str, Any]) -> None:
        for k, v in src.items():
            if isinstance(v, dict) and isinstance(dst.get(k), dict):
                self._deep_update(dst[k], v)  # type: ignore[index]
            else:
                dst[k] = v
