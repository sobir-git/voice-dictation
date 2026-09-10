#!/usr/bin/env python3
"""Leakage-free word-level Token Map Drafting proxy.

This does not run Canary. References stand in for target token sequences, so
the result estimates text continuation predictability only.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import random
import time
import unittest
from collections import Counter, defaultdict
from pathlib import Path


def words(text: str) -> list[str]:
    return text.split()


def split_cases(cases: list[dict], fold: int = 0, folds: int = 5) -> tuple[list[dict], list[dict]]:
    """Deterministic case split by case-ID hash.

    The clips come from one reader and the same source book, so this does not
    remove shared-book or speaker leakage between folds.
    """
    if folds < 2 or not 0 <= fold < folds:
        raise ValueError("fold must be in [0, folds) and folds must be >= 2")
    train, test = [], []
    for case in sorted(cases, key=lambda x: x["case"]):
        digest = hashlib.sha256(case["case"].encode()).digest()[0] % folds
        (test if digest == fold else train).append(case)
    return train, test


def make_map(train: list[dict], context: int, k: int) -> dict[tuple[str, ...], tuple[str, ...]]:
    counts: dict[tuple[str, ...], Counter[tuple[str, ...]]] = defaultdict(Counter)
    for case in train:
        seq = words(case["reference"])
        for pos in range(context, len(seq)):
            key = tuple(seq[pos - context : pos])
            continuation = tuple(seq[pos : pos + k])
            counts[key][continuation] += 1
    return {key: counter.most_common(1)[0][0] for key, counter in counts.items()}


def verify_round(target: list[str], pos: int, proposal: tuple[str, ...], eos: str = "<eos>") -> dict:
    """Return accepted draft prefix and the verifier's guaranteed progress."""
    accepted = 0
    while accepted < len(proposal) and pos + accepted < len(target):
        proposed = proposal[accepted]
        actual = target[pos + accepted]
        # EOS is an ordinary target token for accounting. A draft EOS before
        # the target EOS mismatches and cannot terminate the transcription.
        if proposed == eos and actual != eos:
            break
        if proposed != actual:
            break
        accepted += 1
    # The target verifier emits the next target token after a mismatch or an
    # empty proposal. If the proposal consumed EOS, no extra token is needed.
    consumed_all = pos + accepted >= len(target)
    committed = accepted if consumed_all else accepted + 1
    eos_selected = consumed_all or target[pos + accepted] == eos
    return {"accepted": accepted, "committed": committed, "eos": eos_selected}


def evaluate(cases: list[dict], token_map: dict[tuple[str, ...], tuple[str, ...]], context: int, k: int) -> list[dict]:
    rows = []
    for case in cases:
        target = words(case["reference"]) + ["<eos>"]
        pos = 0
        rounds = 0
        covered = 0
        accepted = 0
        while pos < len(target):
            key = tuple(target[max(0, pos - context) : pos])
            proposal = token_map.get(key, ()) if len(key) == context else ()
            result = verify_round(target, pos, proposal[:k])
            rounds += 1
            if proposal:
                covered += 1
            accepted += result["accepted"]
            advance = result["committed"]
            if advance <= 0:
                raise AssertionError("verification round made no progress")
            pos += advance
        rows.append(
            {
                "case": case["case"],
                "tokens": len(target),
                "rounds": rounds,
                "covered_rounds": covered,
                "accepted": accepted,
                "coverage": covered / rounds if rounds else 0.0,
                "accepted_per_round": accepted / rounds if rounds else 0.0,
                "effective_tokens_per_round": len(target) / rounds if rounds else 0.0,
            }
        )
    return rows


def bootstrap(rows: list[dict], key: str, seed: int = 20260910, samples: int = 2000) -> dict:
    rng = random.Random(seed)
    values = [row[key] for row in rows]
    if not values:
        return {"mean": 0.0, "p05": 0.0, "p95": 0.0, "n": 0}
    means = []
    for _ in range(samples):
        means.append(sum(rng.choice(values) for _ in values) / len(values))
    means.sort()
    return {
        "mean": sum(values) / len(values),
        "p05": means[max(0, math.floor(0.05 * samples) - 1)],
        "p95": means[min(samples - 1, math.floor(0.95 * samples))],
        "n": len(values),
    }


def run(manifest: Path, fold: int, folds: int, contexts: list[int], ks: list[int]) -> dict:
    cases = json.loads(manifest.read_text())
    train, test = split_cases(cases, fold, folds)
    if not test:
        raise ValueError("evaluation fold is empty")
    output = {"manifest": str(manifest), "fold": fold, "folds": folds, "train_cases": len(train), "eval_cases": len(test), "results": []}
    for context in contexts:
        for k in ks:
            started = time.perf_counter_ns()
            token_map = make_map(train, context, k)
            map_build_us = (time.perf_counter_ns() - started) / 1000
            started = time.perf_counter_ns()
            rows = evaluate(test, token_map, context, k)
            eval_us = (time.perf_counter_ns() - started) / 1000
            result = {
                "context": context,
                "k": k,
                "map_entries": len(token_map),
                "map_build_us": map_build_us,
                "eval_us": eval_us,
                "eval_us_per_case": eval_us / len(test) if test else 0.0,
                "mean_coverage": sum(r["coverage"] for r in rows) / len(rows),
                "mean_accepted_per_round": sum(r["accepted_per_round"] for r in rows) / len(rows),
                "mean_effective_tokens_per_round": sum(r["effective_tokens_per_round"] for r in rows) / len(rows),
                "bootstrap": {
                    "coverage": bootstrap(rows, "coverage"),
                    "accepted_per_round": bootstrap(rows, "accepted_per_round"),
                    "effective_tokens_per_round": bootstrap(rows, "effective_tokens_per_round"),
                },
            }
            output["results"].append(result)
    return output


class SimulatorTests(unittest.TestCase):
    def test_mismatch_first_still_commits_verifier_token(self):
        self.assertEqual(verify_round(["A", "B"], 0, ("X", "B")), {"accepted": 0, "committed": 1, "eos": False})

    def test_partial_accept_commits_one_verified_token_after_prefix(self):
        self.assertEqual(verify_round(["A", "B", "C", "<eos>"], 0, ("A", "X", "C")), {"accepted": 1, "committed": 2, "eos": False})

    def test_all_accept_counts_bonus_tokens(self):
        self.assertEqual(verify_round(["A", "B", "C", "<eos>"], 0, ("A", "B")), {"accepted": 2, "committed": 3, "eos": False})

    def test_eos_is_verified_after_accepted_prefix(self):
        self.assertEqual(verify_round(["A", "<eos>"], 1, ("<eos>",)), {"accepted": 1, "committed": 1, "eos": True})

    def test_premature_eos_is_rejected(self):
        self.assertEqual(verify_round(["A", "B", "<eos>"], 0, ("<eos>",)), {"accepted": 0, "committed": 1, "eos": False})

    def test_final_eos_on_map_miss_makes_progress(self):
        self.assertEqual(verify_round(["<eos>"], 0, ()), {"accepted": 0, "committed": 1, "eos": True})

    def test_zero_coverage_has_no_false_speedup(self):
        rows = evaluate([{"case": "x", "reference": "A B C"}], {}, 1, 4)
        self.assertEqual(rows[0]["coverage"], 0.0)
        self.assertEqual(rows[0]["effective_tokens_per_round"], 1.0)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--fold", type=int, default=0)
    parser.add_argument("--folds", type=int, default=5)
    parser.add_argument("--contexts", type=int, nargs="+", default=[1, 2, 3])
    parser.add_argument("--ks", type=int, nargs="+", default=[1, 2, 4])
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.write_text(json.dumps(run(args.manifest, args.fold, args.folds, args.contexts, args.ks), indent=2) + "\n")


if __name__ == "__main__":
    main()
