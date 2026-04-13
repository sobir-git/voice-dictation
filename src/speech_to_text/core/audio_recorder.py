import logging
import os
import subprocess
import time

logger = logging.getLogger(__name__)


class AudioRecorder:
    def __init__(
        self,
        sample_rate: int = 16000,
        format: str = 'S16_LE',
        channels: int = 1,
        output_file: str = '/tmp/stt_recording.wav',
    ):
        self.sample_rate = sample_rate
        self.format = format
        self.channels = channels
        self.output_file = output_file
        self.process: subprocess.Popen | None = None

    def start(self) -> None:
        if self.process is not None:
            return

        os.makedirs(os.path.dirname(self.output_file) or '.', exist_ok=True)
        try:
            if os.path.exists(self.output_file):
                os.unlink(self.output_file)
        except Exception:
            pass

        cmd = [
            'arecord',
            '-q',
            '-f',
            str(self.format),
            '-r',
            str(self.sample_rate),
            '-c',
            str(self.channels),
            self.output_file,
        ]

        logger.info("Starting arecord: %s", ' '.join(cmd))
        self.process = subprocess.Popen(
            cmd,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        time.sleep(0.1)

    def stop(self) -> bool:
        if self.process is None:
            return False

        try:
            self.process.terminate()
            try:
                self.process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=2)
        except Exception as e:
            logger.warning("Failed stopping arecord: %s", e)
        finally:
            self.process = None

        return os.path.exists(self.output_file) and os.path.getsize(self.output_file) > 1000
