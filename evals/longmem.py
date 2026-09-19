#!/usr/bin/env python3
"""Turn LongMemEval instances into morpho eval scenarios, and report the runs back.

The haystack's assistant turns are dropped: morpho writes its own replies, so only the user
side of each session is replayed. Instances whose evidence sits only in assistant turns are
skipped, which is why the score is not comparable to a published LongMemEval number.

  python3 evals/longmem.py longmemeval_oracle.json --n 20
  python3 evals/longmem.py --report --label lme
"""

import argparse
import json
import pathlib
import random
from collections import defaultdict

HERE = pathlib.Path(__file__).resolve().parent
SKIP_TYPES = {"single-session-assistant"}


def rubric(inst):
    if inst["question_id"].endswith("_abs"):
        return (
            "PASS if the reply says it does not have that information, or asks for it. "
            "FAIL if it asserts an answer."
        )
    return (
        f'PASS if the reply states this, in any wording: "{inst["answer"]}". '
        "FAIL if it contradicts it, leaves it out, or says it does not know."
    )


def scenario(inst):
    turns, refs = [], []
    sessions = zip(
        inst["haystack_session_ids"], inst["haystack_dates"], inst["haystack_sessions"]
    )
    for sid, date, session in sessions:
        dated = True
        for t in session:
            if t["role"] != "user":
                continue
            # The benchmark hands its baselines a timestamp per session; this is where it goes.
            text = f"[{date}] {t['content']}" if dated else t["content"]
            dated = False
            if t.get("has_answer"):
                refs.append(len(turns))
            turns.append({"text": text, "session": sid, "tag": "fact"})
    abstain = inst["question_id"].endswith("_abs")
    if not turns or (not refs and not abstain):
        return None
    turns.append(
        {
            "text": f'[{inst["question_date"]}] {inst["question"]}',
            "session": "probe",
            "tag": "probe",
            "judge": rubric(inst),
            "refs": refs,
        }
    )
    return {"cycle_every": 5, "turns": turns}


def generate(args):
    data = json.loads(pathlib.Path(args.dataset).read_text())
    pool = [x for x in data if x["question_type"] not in SKIP_TYPES]
    if args.ids:
        want = set(pathlib.Path(args.ids).read_text().split())
        pool = [x for x in pool if x["question_id"] in want]
    else:
        random.Random(args.seed).shuffle(pool)
    out = HERE / "scenarios"
    written, kinds = [], defaultdict(int)
    for inst in pool:
        if len(written) == args.n:
            break
        built = scenario(inst)
        if built is None:
            continue
        name = f"{args.prefix}-{inst['question_id']}"
        (out / f"{name}.json").write_text(json.dumps(built, indent=1) + "\n")
        (out / f"{name}.meta.json").write_text(
            json.dumps({"question_type": inst["question_type"], "answer": inst["answer"]})
            + "\n"
        )
        kinds[inst["question_type"]] += 1
        written.append((name, len(built["turns"])))
    turns = sum(n for _, n in written)
    print(f"{len(written)} scenarios, {turns} turns, {turns / max(len(written), 1):.1f} per instance")
    for kind, n in sorted(kinds.items()):
        print(f"  {kind:28} {n}")


def report(args):
    results = sorted((HERE / "results").glob(f"{args.prefix}-*.{args.label}.json"))
    if not results:
        raise SystemExit(f"no evals/results/{args.prefix}-*.{args.label}.json")
    by_kind, rows = defaultdict(list), []
    for path in results:
        name = path.name.split(f".{args.label}.json")[0]
        meta_path = HERE / "scenarios" / f"{name}.meta.json"
        kind = json.loads(meta_path.read_text())["question_type"] if meta_path.exists() else "unknown"
        data = json.loads(path.read_text())
        m = data.get("metrics", data)
        score = m.get("judge_accuracy")
        if score is None:
            continue
        by_kind[kind].append(score)
        rows.append((name, kind, score, m.get("retrieval_hit"), m.get("ctx_tokens")))
    overall = [s for scores in by_kind.values() for s in scores]
    print(f"judge_accuracy {sum(overall) / len(overall):.4f} over {len(overall)} instances")
    for kind, scores in sorted(by_kind.items()):
        print(f"  {kind:28} {sum(scores) / len(scores):.4f}  n={len(scores)}")
    if args.verbose:
        for name, kind, score, hit, toks in rows:
            print(f"  {name:44} {kind:28} {score} hit={hit} ctx={toks}")


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("dataset", nargs="?", help="longmemeval_*.json")
    p.add_argument("--n", type=int, default=20)
    p.add_argument("--seed", type=int, default=0)
    p.add_argument("--report", action="store_true")
    p.add_argument("--label", default="lme")
    p.add_argument("--prefix", default="longmem")
    p.add_argument("--ids", help="file of question_ids, one per line: the same questions as another split")
    p.add_argument("--verbose", action="store_true")
    a = p.parse_args()
    if a.report:
        report(a)
    elif a.dataset:
        generate(a)
    else:
        p.error("give a dataset, or --report")
