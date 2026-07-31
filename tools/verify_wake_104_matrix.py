from __future__ import annotations

import argparse
import array
import hashlib
import json
import math
import pathlib
import re
import wave


KWS_LINE = re.compile(
    r"^diag (?P<name>\S+) .* combined_hit=(?P<hit>true|false) "
)


def sha256(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def voiced_reference(path: pathlib.Path) -> float:
    with wave.open(str(path), "rb") as wav:
        samples = array.array("h")
        samples.frombytes(wav.readframes(wav.getnframes()))
    frame_size = 320
    values = []
    for start in range(0, len(samples) - frame_size + 1, frame_size):
        frame = samples[start : start + frame_size]
        values.append(math.sqrt(sum(value * value for value in frame) / frame_size))
    values.sort()
    return values[min(len(values) - 1, math.ceil(len(values) * 0.85) - 1)]


def parse_kws(path: pathlib.Path) -> dict[str, bool]:
    result = {}
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        match = KWS_LINE.match(line)
        if match:
            result[match["name"]] = match["hit"] == "true"
    return result


def parse_helper(path: pathlib.Path, names: list[str], prefix: str) -> dict[str, bool]:
    indexed = {}
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        request_id = value.get("request_id", "")
        if request_id.startswith(prefix):
            indexed[int(request_id.rsplit("-", 1)[1])] = bool(value.get("matched"))
    return {
        name: indexed.get(index, False)
        for index, name in enumerate(sorted(names), start=1)
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=pathlib.Path, required=True)
    parser.add_argument("--positive-kws-log", type=pathlib.Path, required=True)
    parser.add_argument("--negative-kws-log", type=pathlib.Path, required=True)
    parser.add_argument("--positive-helper-log", type=pathlib.Path, required=True)
    parser.add_argument("--negative-helper-log", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()

    manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    positive = [case for case in manifest["cases"] if case["kind"] == "positive"]
    negative = [case for case in manifest["cases"] if case["kind"] == "negative"]
    positive_names = [pathlib.Path(case["path"]).name for case in positive]
    negative_names = [pathlib.Path(case["path"]).name for case in negative]

    hash_errors = []
    for case in manifest["cases"]:
        path = pathlib.Path(case["path"])
        if sha256(path) != case["sha256"]:
            hash_errors.append(path.name)
        if case["kind"] == "positive":
            raw_path = pathlib.Path(case["raw_path"])
            if sha256(raw_path) != case["raw_sha256"]:
                hash_errors.append(raw_path.name)

    positive_kws = parse_kws(args.positive_kws_log)
    negative_kws = parse_kws(args.negative_kws_log)
    positive_helper = parse_helper(args.positive_helper_log, positive_names, "wake-")
    negative_helper = parse_helper(args.negative_helper_log, negative_names, "neg-")

    category = {}
    misses = []
    for name in positive_names:
        case = next(case for case in positive if pathlib.Path(case["path"]).name == name)
        hit = positive_kws.get(name, False) or positive_helper.get(name, False)
        totals = category.setdefault(case["category"], {"hits": 0, "total": 0})
        totals["total"] += 1
        totals["hits"] += int(hit)
        if not hit:
            misses.append(name)

    negative_false_hits = sorted(
        name
        for name in negative_names
        if negative_kws.get(name, False) or negative_helper.get(name, False)
    )
    paired_spreads = []
    for index in range(1, manifest["cases_per_positive_category"] + 1):
        levels = []
        for label in ("normal", "soft", "loud"):
            case = next(case for case in positive if case["id"] == f"{label}-{index:02d}")
            levels.append(voiced_reference(pathlib.Path(case["path"])))
        paired_spreads.append(20.0 * math.log10(max(levels) / min(levels)))

    peak_limit = manifest["firmware_level_model"]["limiter_peak"]
    clipped_samples = sum(case["metrics"]["clipped_samples"] for case in positive)
    max_peak = max(case["metrics"]["peak"] for case in positive)
    positive_hits = len(positive) - len(misses)
    checks = {
        "manifest_hashes": not hash_errors,
        "positive_overall_at_least_95_percent": positive_hits * 100 >= len(positive) * 95,
        "each_category_at_least_90_percent": all(
            value["hits"] * 100 >= value["total"] * 90 for value in category.values()
        ),
        "negative_false_hits_zero": not negative_false_hits,
        "processed_volume_spread_at_most_6_db": max(paired_spreads) <= 6.0,
        "processed_clipped_samples_zero": clipped_samples == 0,
        "processed_peak_within_limiter": max_peak <= peak_limit,
    }
    report = {
        "schema": "listener.wake-matrix-verification.v1",
        "manifest_sha256": sha256(args.manifest),
        "positive": {
            "hits": positive_hits,
            "total": len(positive),
            "misses": misses,
            "categories": category,
        },
        "negative": {
            "false_hits": len(negative_false_hits),
            "total": len(negative),
            "names": negative_false_hits,
        },
        "audio": {
            "max_paired_volume_spread_db": round(max(paired_spreads), 3),
            "clipped_samples": clipped_samples,
            "max_peak": max_peak,
            "limiter_peak": peak_limit,
        },
        "hash_errors": hash_errors,
        "checks": checks,
        "passed": all(checks.values()),
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
