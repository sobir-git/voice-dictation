# Host-only token-map experiment

Run the meaningful simulator checks with:

```sh
python3 token_map_sim.py --manifest /tmp/canary-vulkan-research/human/manifest.json \
  --output /tmp/canary-multitoken-luna/token-map-fold0.json
python3 -m unittest token_map_sim.py -v
```

The simulator hashes each case ID into a deterministic five-way split. The map
is built only from the other four folds, and all five folds are run for the
reported aggregate. This avoids case-level train/eval reuse, but the clips
share speaker 1272 and source-book material. It uses uppercase words because the
provided manifest has reference text but no extracted Canary token IDs. Thus
`context` is a word context and `k` is a word proposal length, not a SentencePiece
configuration.

`accepted` counts the matching proposal prefix. `effective_tokens_per_round`
counts the verifier's guaranteed progress, which is `accepted + 1` on a
mismatch and `accepted` when a proposal consumes EOS. A map miss at final EOS
still commits one EOS token. A map miss has zero coverage and exactly one token
per round, so it cannot create a false speedup in the accounting.

The measured microseconds cover Python map construction and host simulation,
not Canary inference. They are lookup-overhead indicators only. The result does
not establish target-model acceptance, KV-cache correctness, WER, or inference
speed. Use separate folds or a second speaker before treating the confidence
interval as generalization evidence. The current 73 clips all use speaker 1272,
so bootstrap intervals describe clip variability from one speaker, not a
population confidence interval.
