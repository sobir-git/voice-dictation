# Source register

Checked 2026-09-10. Paper claims below are summaries of the primary paper
abstracts and methods. Runtime claims come from the pinned local source.

1. [Token Map Drafting, arXiv:2507.21522](https://arxiv.org/abs/2507.21522)
2. [Whisper in Medusa's Ear, arXiv:2409.15869](https://arxiv.org/abs/2409.15869)
3. [SpecASR, arXiv:2507.18181](https://arxiv.org/abs/2507.18181)
4. [Medusa, arXiv:2401.10774](https://arxiv.org/abs/2401.10774)
5. [Speculative Decoding for Seq2seq Generation, arXiv:2203.16487](https://arxiv.org/abs/2203.16487)
6. [Canary family porting notes](https://github.com/handy-computer/transcribe.cpp/blob/main/docs/porting/families/canary.md)
7. [Canary 180M Flash model notes](https://github.com/handy-computer/transcribe.cpp/blob/main/docs/models/canary-180m-flash.md)

Local primary source:

`/home/fire/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/transcribe-cpp-sys-0.2.3/src/arch/canary/`

Relevant files are `decoder.h`, `decoder.cpp`, `model.cpp`, `canary.h`, and
`weights.h`. The shared decode loop is in `src/transcribe-batch-util.{h,cpp}`.
