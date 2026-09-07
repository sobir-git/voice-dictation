import logging
import os
import threading
from typing import List, Optional

from faster_whisper import WhisperModel

logger = logging.getLogger(__name__)


class Transcriber:
    def __init__(
        self,
        model_name: str = 'tiny.en',
        compute_type: str = 'int8',
        language: str = 'en',
        beam_size: int = 1,
        vad_filter: bool = True,
    ):
        self.model_name = model_name
        self.compute_type = compute_type
        self.language = language
        self.beam_size = beam_size
        self.vad_filter = vad_filter
        self.model: Optional[WhisperModel] = None
        self._model_lock = threading.Lock()

    def load_model(self):
        with self._model_lock:
            if self.model is not None:
                return
            logger.info('Loading Whisper model: %s (compute_type: %s)', self.model_name, self.compute_type)
            try:
                self.model = WhisperModel(self.model_name, compute_type=self.compute_type, local_files_only=True)
            except FileNotFoundError:
                logger.info('Model is not cached; downloading %s', self.model_name)
                self.model = WhisperModel(self.model_name, compute_type=self.compute_type)
            logger.info('Model loaded successfully')

    def _validate_audio_file(self, file_path: str) -> None:
        if not os.path.exists(file_path):
            raise FileNotFoundError(f"Audio file not found: {file_path}")

        file_size = os.path.getsize(file_path)
        if file_size < 1000:
            raise ValueError(
                f"Audio file too small ({file_size} bytes), likely empty or corrupt"
            )

    def transcribe_file_segments(self, file_path: str) -> List[str]:
        if self.model is None:
            self.load_model()

        self._validate_audio_file(file_path)

        try:
            logger.info("Transcribing audio file: %s", file_path)
            segments, info = self.model.transcribe(
                file_path,
                language=self.language,
                beam_size=self.beam_size,
                vad_filter=self.vad_filter,
            )

            results: List[str] = []
            for segment in segments:
                text = segment.text.strip()
                if text:
                    results.append(text)


            if not results:
                logger.warning("No speech detected in audio")
                return []

            logger.info("Transcription complete: %d segments", len(results))
            return results
        except Exception as e:
            logger.error("Transcription failed: %s", e)
            raise

    def transcribe_file(self, file_path: str) -> str:
        segments = self.transcribe_file_segments(file_path)
        return ' '.join(segments).strip()
