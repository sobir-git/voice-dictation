import logging
import os
import sys
from logging.handlers import RotatingFileHandler


def setup_logging(config) -> None:
    level_name = config.get('logging', 'level', default='INFO')
    level = getattr(logging, str(level_name).upper(), logging.INFO)

    log_file = config.get('logging', 'file', default='~/.local/share/speech-to-text/app.log')
    log_file = os.path.expanduser(log_file)
    os.makedirs(os.path.dirname(log_file), exist_ok=True)

    root = logging.getLogger()
    root.setLevel(level)

    for h in list(root.handlers):
        root.removeHandler(h)

    fmt = logging.Formatter('%(asctime)s [%(levelname)s] %(name)s: %(message)s')

    file_handler = RotatingFileHandler(
        log_file,
        maxBytes=int(config.get('logging', 'max_size_mb', default=10)) * 1024 * 1024,
        backupCount=3,
    )
    file_handler.setFormatter(fmt)
    file_handler.setLevel(level)

    stream_handler = logging.StreamHandler(sys.stderr)
    stream_handler.setFormatter(fmt)
    stream_handler.setLevel(level)

    root.addHandler(file_handler)
    root.addHandler(stream_handler)
