#!/usr/bin/env python3
"""Give two cognitive resources the same work and record what comes back.

This measures *models*, not Kamimusuhi. It exists because a capability
declaration — `latency`, `quality` — is an operator's assertion, and an
assertion that nobody ever checked against the endpoint is just a guess with
better formatting. Run this, read the numbers, then decide what to declare.

    ./scripts/compare-cognitive-resources.py \
        --a j72=http://llm-machine:8081/v1:j72-30m \
        --b grokbot=http://<HOST>:8080/v1:qwen2.5-3b-instruct

The bearer token, if the endpoints need one, comes from the environment:

    export KAMIMUSUHI_GROKBOT_API_KEY=...
    ./scripts/compare-cognitive-resources.py --auth-env KAMIMUSUHI_GROKBOT_API_KEY ...

What it does NOT do, deliberately: ask either model which model should handle
a task. Routing authority belongs to the Cognitive Action Router. These
prompts describe work, never a choice of who does the work.

Scoring is deliberately crude — substring and prefix checks on a normalized
reply. A crude scorer that anyone can audit beats a clever one nobody
re-reads, and the failure mode that matters here ("did it follow the shape of
the instruction at all") is exactly what a crude scorer catches.
"""

import argparse
import json
import os
import re
import statistics
import sys
import time
import urllib.error
import urllib.request

# --- the task set -----------------------------------------------------------
#
# `shallow` tasks are the ones a small model is plausibly *for*: classify,
# extract, label, compress. `deep` tasks are here to find the boundary, not to
# embarrass anyone — failing them is a measurement, not a verdict.
#
# `accept` is a list of substrings; a reply scores if it contains any of them
# (case-insensitively, punctuation-insensitively).

TASKS = [
    # 1. binary / ternary classification
    dict(id="classify-sentiment", depth="shallow",
         prompt='Classify the sentiment as exactly one word: positive, negative, or neutral.\n'
                'Text: "The delivery arrived two days late and the box was crushed."\n'
                'Answer with one word only.',
         accept=["negative"]),
    # 2. intent classification
    dict(id="classify-intent", depth="shallow",
         prompt='Classify this message as exactly one of: question, statement, request.\n'
                'Message: "Please summarize this paragraph."\n'
                'Answer with one word only.',
         accept=["request"]),
    # 3. salience
    dict(id="salience", depth="shallow",
         prompt='Which sentence carries information worth remembering about the user?\n'
                'A: "It is raining today."\n'
                'B: "I am allergic to peanuts."\n'
                'Answer with one letter only: A or B.',
         accept=["b"]),
    # 4. short extraction
    dict(id="extract-date", depth="shallow",
         prompt='Extract the date from this sentence. Answer with the date only.\n'
                'Sentence: "The meeting was moved to 2026-03-14 at the request of the client."',
         accept=["2026-03-14"]),
    # 5. provenance labelling
    dict(id="provenance", depth="shallow",
         prompt='Label the source of this statement as exactly one of: user_said, model_generated, document.\n'
                'Statement: "According to the attached PDF, revenue grew 12%."\n'
                'Answer with one label only.',
         accept=["document"]),
    # 6. short summarization
    dict(id="summarize", depth="shallow",
         prompt='Summarize in one short sentence:\n'
                '"Hojicha is roasted at high temperature, which removes most of its '
                'caffeine and gives it a toasted flavour. It is often served to children '
                'and in the evening for that reason."',
         accept=["roast", "caffeine", "toast"]),
    # 7. simple contradiction
    dict(id="contradiction", depth="shallow",
         prompt='Do these two statements contradict each other? Answer yes or no.\n'
                'A: "The store closes at 6pm."\n'
                'B: "The store is open until 8pm."\n'
                'Answer with one word only.',
         accept=["yes"]),
    # 8. trivial transformation
    dict(id="transform-case", depth="shallow",
         prompt='Convert the following to uppercase. Answer with the result only.\n'
                'Text: kamimusuhi',
         accept=["kamimusuhi"]),
    # --- boundary probes ---
    dict(id="multi-step", depth="deep",
         prompt='A shelf holds 3 boxes. Each box holds 4 jars. Two jars are removed from '
                'the shelf entirely. How many jars remain? Answer with a number only.',
         accept=["10"]),
    dict(id="ambiguous", depth="deep",
         prompt='A user says only: "it broke again". What is the single most useful '
                'clarifying question to ask? Answer with one question.',
         accept=["what", "which", "when", "?"]),
    dict(id="longer-summary", depth="deep",
         prompt='Summarize the following in two sentences:\n'
                '"A continuity head records which canonical commit an individual is '
                'currently at. Restarting the process does not move it. Resources may be '
                'replaced between runs; the head is unaffected, because a resource call '
                'is not a canonical mutation. Only an activated proposal moves it."',
         accept=["head", "commit", "restart", "resource"]),
    dict(id="knowledge", depth="deep",
         prompt='In which country is the city of Kanazawa? Answer with the country only.',
         accept=["japan", "日本"]),
]


def normalize(text):
    return re.sub(r"[^a-z0-9%\-:぀-ヿ一-鿿?]+", " ", text.lower()).strip()


def scores(reply, accept):
    flat = normalize(reply)
    return any(normalize(a) in flat for a in accept)


def call(base_url, model, prompt, auth_env, timeout, max_tokens):
    """One chat completion. Returns (latency_s, reply, usage, error)."""
    body = json.dumps({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": max_tokens,
        "temperature": 0.0,
    }).encode()
    headers = {"Content-Type": "application/json"}
    if auth_env:
        token = os.environ.get(auth_env)
        if not token:
            return (0.0, "", None, f"environment variable {auth_env} is not set")
        # Read at call time, never stored or printed.
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(
        base_url.rstrip("/") + "/chat/completions", data=body, headers=headers)
    started = time.monotonic()
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            payload = json.load(response)
    except urllib.error.HTTPError as error:
        return (time.monotonic() - started, "", None, f"HTTP {error.code}")
    except Exception as error:  # transport, timeout, malformed JSON
        return (time.monotonic() - started, "", None, type(error).__name__)
    elapsed = time.monotonic() - started
    try:
        reply = payload["choices"][0]["message"]["content"]
    except (KeyError, IndexError, TypeError):
        return (elapsed, "", payload.get("usage"), "MALFORMED_RESPONSE")
    return (elapsed, reply, payload.get("usage"), None)


def parse_resource(text):
    """`name=http://host:port/v1:model` -> (name, base_url, model)."""
    name, _, rest = text.partition("=")
    base_url, _, model = rest.rpartition(":")
    if not (name and base_url and model):
        raise argparse.ArgumentTypeError(
            f"expected name=BASE_URL:MODEL, got {text!r}")
    return (name, base_url, model)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--a", required=True, type=parse_resource)
    parser.add_argument("--b", required=True, type=parse_resource)
    parser.add_argument("--auth-env", default=None,
                        help="NAME of the environment variable holding the bearer token")
    parser.add_argument("--timeout", type=float, default=180.0)
    parser.add_argument("--max-tokens", type=int, default=64)
    parser.add_argument("--repeat", type=int, default=1,
                        help="runs per task, for stability")
    parser.add_argument("--json", default=None, help="write the full record here")
    args = parser.parse_args()

    resources = [args.a, args.b]
    record = {"tasks": len(TASKS), "repeat": args.repeat, "resources": {}}

    for name, base_url, model in resources:
        print(f"\n=== {name}  {base_url}  ({model})", flush=True)
        latencies, results = [], []
        for task in TASKS:
            replies, oks, errors, task_latencies = [], 0, [], []
            for _ in range(args.repeat):
                elapsed, reply, usage, error = call(
                    base_url, model, task["prompt"], args.auth_env,
                    args.timeout, args.max_tokens)
                task_latencies.append(elapsed)
                if error:
                    errors.append(error)
                    continue
                latencies.append(elapsed)
                replies.append(reply.strip())
                oks += 1 if scores(reply, task["accept"]) else 0
            stable = len(set(replies)) <= 1 if len(replies) > 1 else None
            results.append(dict(
                id=task["id"], depth=task["depth"],
                passed=oks, attempts=args.repeat,
                errors=errors, stable=stable,
                latency_s=round(statistics.median(task_latencies), 3),
                reply=(replies[0][:200] if replies else ""),
            ))
            mark = "ok " if oks == args.repeat else ("~  " if oks else "MISS")
            print(f"  {mark} {task['id']:<18} {results[-1]['latency_s']:>7.2f}s  "
                  f"{results[-1]['reply'][:70]!r}", flush=True)

        shallow = [r for r in results if r["depth"] == "shallow"]
        deep = [r for r in results if r["depth"] == "deep"]
        summary = dict(
            shallow_passed=sum(r["passed"] for r in shallow),
            shallow_total=sum(r["attempts"] for r in shallow),
            deep_passed=sum(r["passed"] for r in deep),
            deep_total=sum(r["attempts"] for r in deep),
            failures=sum(len(r["errors"]) for r in results),
        )
        if latencies:
            latencies.sort()
            summary.update(
                latency_min_s=round(latencies[0], 3),
                latency_median_s=round(statistics.median(latencies), 3),
                latency_p95_s=round(latencies[min(len(latencies) - 1,
                                                  int(len(latencies) * 0.95))], 3),
                latency_max_s=round(latencies[-1], 3),
                samples=len(latencies),
            )
        record["resources"][name] = dict(
            base_url=base_url, model=model, summary=summary, results=results)
        print(f"  -- shallow {summary['shallow_passed']}/{summary['shallow_total']}"
              f"  deep {summary['deep_passed']}/{summary['deep_total']}"
              f"  call failures {summary['failures']}")
        if latencies:
            print(f"  -- latency min {summary['latency_min_s']}s  "
                  f"median {summary['latency_median_s']}s  "
                  f"p95 {summary['latency_p95_s']}s  max {summary['latency_max_s']}s")

    if args.json:
        with open(args.json, "w") as handle:
            json.dump(record, handle, indent=2, ensure_ascii=False)
        print(f"\nwrote {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
