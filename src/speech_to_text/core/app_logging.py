import logging
import os
import sys
import threading
from logging.handlers import RotatingFileHandler
from pathlib import Path


def log_path(config, role='daemon'):
    path = Path(config.get('logging', 'file')).expanduser()
    return path if role == 'daemon' else path.with_name(path.stem + '-' + role + path.suffix)


def setup_logging(config, role='daemon'):
    level = getattr(logging, config.get('logging', 'level', 'INFO'))
    path = log_path(config, role)
    path.parent.mkdir(parents=True, exist_ok=True)
    root = logging.getLogger()
    root.setLevel(level)
    for handler in list(root.handlers):
        root.removeHandler(handler)
        handler.close()
    fmt = logging.Formatter('%(asctime)s %(levelname)s pid=%(process)d thread=%(threadName)s %(name)s: %(message)s')
    file_handler = RotatingFileHandler(path, maxBytes=config.get('logging', 'max_size_mb', 10)*1024*1024,
                                       backupCount=3, encoding='utf-8')
    os.chmod(path, 0o600)
    for handler in (file_handler, logging.StreamHandler(sys.stderr)):
        handler.setFormatter(fmt)
        root.addHandler(handler)
    # Third-party HTTP DEBUG logs can include credentials and large payloads.
    for name in ('httpx', 'httpcore', 'huggingface_hub', 'urllib3'):
        logging.getLogger(name).setLevel(logging.WARNING)

    def thread_exception(args):
        logging.getLogger('uncaught').critical('Unhandled exception in %s', args.thread.name,
            exc_info=(args.exc_type, args.exc_value, args.exc_traceback))
    threading.excepthook = thread_exception
    def exception(kind, value, tb):
        logging.getLogger('uncaught').critical('Unhandled exception', exc_info=(kind, value, tb))
    sys.excepthook = exception
