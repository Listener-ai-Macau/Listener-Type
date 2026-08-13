from __future__ import annotations

import argparse
import json
import math
import pathlib
import random
import shutil

from generate_wake_104_matrix import (
    SAMPLE_RATE,
    firmware_level,
    limit,
    metrics,
    read_pcm16,
    resample_linear,
    sha256,
    write_pcm16,
)


CATEGORIES = ("normal", "soft", "slow", "fast", "far")
CROP_MS = (0, 50, 100, 150, 200, 0, 100, 200)
SPEEDS = {
    "normal": (0.95, 1.0, 1.05, 1.0, 0.95, 1.05, 1.0, 1.0),
    "soft": (0.9, 1.0, 1.1, 0.95, 1.05, 1.0, 0.9, 1.1),
    "slow": (0.8, 0.82, 0.85, 0.88, 0.9, 0.8, 0.85, 0.9),
    "fast": (1.15, 1.18, 1.2, 1.22, 1.25, 1.15, 1.2, 1.25),
    "far": (0.9, 1.0, 1.1, 0.95, 1.05, 1.0, 0.9, 1.1),
}
DISTANCES_M = {
    "normal": (0.3, 0.4, 0.5, 0.6, 0.7, 0.3, 0.5, 0.7),
    "soft": (0.3, 0.4, 0.5, 0.6, 0.7, 0.3, 0.5, 0.7),
    "slow": (0.3, 0.4, 0.5, 0.6, 0.7, 0.3, 0.5, 0.7),
    "fast": (0.3, 0.4, 0.5, 0.6, 0.7, 0.3, 0.5, 0.7),
    "far": (0.8, 0.85, 0.9, 0.95, 1.0, 0.8, 0.9, 1.0),
}
OVERLAP_INTERFERENCE_DB = (-12.0, -9.0, -6.0, -3.0, 0.0, 3.0, -9.0, -3.0)
OVERLAP_ONSET_OFFSETS_MS = (-600, -400, -200, 0, 150, 300, 450, 600)


def speech_onset(samples: list[int]) -> int:
    """Return the first sustained 20 ms voiced frame, preserving a small lead."""
    frame = SAMPLE_RATE // 50
    threshold = max(80.0, max((abs(value) for value in samples), default=0) * 0.015)
    for start in range(0, max(0, len(samples) - frame), frame):
        window = samples[start : start + frame]
        rms = math.sqrt(sum(float(value) * float(value) for value in window) / len(window))
        if rms >= threshold:
            return max(0, start - SAMPLE_RATE // 100)
    raise RuntimeError("positive base WAV has no voiced onset")


def first_phrase_end(samples: list[int], onset: int) -> int:
    """Find the first >=200 ms silence separating the wake phrase from body."""
    frame = SAMPLE_RATE // 50
    silence_frames = 10
    peak = max((abs(value) for value in samples), default=0)
    threshold = max(60.0, peak * 0.006)
    quiet_run = 0
    for start in range(onset + SAMPLE_RATE // 5, len(samples) - frame, frame):
        window = samples[start : start + frame]
        rms = math.sqrt(sum(float(value) * float(value) for value in window) / len(window))
        if rms < threshold:
            quiet_run += 1
            if quiet_run >= silence_frames:
                return start - (silence_frames - 1) * frame
        else:
            quiet_run = 0
    raise RuntimeError("positive base WAV has no phrase/body silence boundary")


def frame_reference_rms(samples: list[int]) -> float:
    frame = SAMPLE_RATE // 50
    values = []
    for start in range(0, len(samples), frame):
        window = samples[start : start + frame]
        if window:
            values.append(
                math.sqrt(
                    sum(float(value) * float(value) for value in window) / len(window)
                )
            )
    if not values:
        return 0.0
    values.sort()
    return values[min(len(values) - 1, len(values) * 85 // 100)]


def mix_overlap(
    wake_raw: list[int],
    interference: list[int],
    interference_db: float,
    onset_offset_ms: int,
) -> tuple[list[int], list[int], dict[str, float | int]]:
    phrase_start = SAMPLE_RATE
    phrase_reference = frame_reference_rms(
        wake_raw[phrase_start : phrase_start + SAMPLE_RATE]
    )
    interference_reference = frame_reference_rms(interference)
    if phrase_reference <= 0.0 or interference_reference <= 0.0:
        raise RuntimeError("overlap source has no measurable speech energy")
    scale = (
        phrase_reference
        * math.pow(10.0, interference_db / 20.0)
        / interference_reference
    )
    interference_start = max(
        0, phrase_start + onset_offset_ms * SAMPLE_RATE // 1000
    )
    mixed_len = max(len(wake_raw), interference_start + len(interference))
    mixed = [float(value) for value in wake_raw] + [0.0] * (
        mixed_len - len(wake_raw)
    )
    for index, value in enumerate(interference):
        mixed[interference_start + index] += float(value) * scale
    raw = limit(mixed)
    processed, firmware_gain = firmware_level(raw)
    return raw, processed, {
        "interference_db_relative_to_wake": interference_db,
        "interference_onset_offset_ms": onset_offset_ms,
        "interference_input_scale": round(scale, 6),
        "firmware_agc_gain": round(firmware_gain, 6),
    }


def transform(
    base: list[int], category: str, case_index: int, seed: int
) -> tuple[list[int], list[int], dict[str, float | int]]:
    rng = random.Random(seed)
    crop_ms = CROP_MS[case_index]
    speed = SPEEDS[category][case_index]
    distance_m = DISTANCES_M[category][case_index]
    onset = speech_onset(base)
    phrase_end = first_phrase_end(base, onset)
    cropped = base[min(len(base) - 1, onset + crop_ms * SAMPLE_RATE // 1000) :]
    working = resample_linear(cropped, speed)

    gain = 1.0
    if category == "soft":
        gain = 0.22
    elif category == "far":
        gain = max(0.22, 0.55 * 0.8 / distance_m)

    values = [float(value) * gain for value in working]
    if category == "far":
        dry = list(values)
        for index in range(len(values)):
            echo = 0.0
            if index >= 83:
                echo += dry[index - 83] * 0.25
            if index >= 211:
                echo += dry[index - 211] * 0.14
            values[index] += echo + rng.uniform(-20.0, 20.0)
    else:
        noise = 1.0 if category != "soft" else 0.5
        values = [value + rng.uniform(-noise, noise) for value in values]

    # The product requires at least one second of device pre-roll at the wake
    # boundary. Keep that invariant in every deterministic matrix case.
    raw = [0] * SAMPLE_RATE + limit(values)
    processed, firmware_gain = firmware_level(raw)
    retained_phrase_samples = max(
        1, phrase_end - min(phrase_end - 1, onset + crop_ms * SAMPLE_RATE // 1000)
    )
    phrase_duration_ms = retained_phrase_samples * 1000.0 / SAMPLE_RATE / speed
    return raw, processed, {
        "speed": speed,
        "start_crop_ms": crop_ms,
        "distance_m": distance_m,
        "source_onset_ms": round(onset * 1000.0 / SAMPLE_RATE, 3),
        "source_phrase_end_ms": round(phrase_end * 1000.0 / SAMPLE_RATE, 3),
        "wake_phrase_start_ms": 1000.0,
        "wake_phrase_end_ms": round(1000.0 + phrase_duration_ms, 3),
        "input_gain": gain,
        "firmware_agc_gain": round(firmware_gain, 6),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-wav", required=True)
    parser.add_argument("--negative-dir", required=True)
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--product-version", default="1.0.5")
    args = parser.parse_args()

    base_path = pathlib.Path(args.base_wav).resolve()
    negative_dir = pathlib.Path(args.negative_dir).resolve()
    output_dir = pathlib.Path(args.output_dir).resolve()
    base = read_pcm16(base_path)
    negative_paths = sorted(negative_dir.glob("*.wav"))
    if len(negative_paths) < 100:
        raise RuntimeError(f"need at least 100 negative WAVs, found {len(negative_paths)}")
    cases: list[dict[str, object]] = []

    for category_index, category in enumerate(CATEGORIES):
        for case_index in range(8):
            seed = 105000 + category_index * 100 + case_index
            raw, processed, parameters = transform(base, category, case_index, seed)
            case_id = f"{category}-{case_index + 1:02d}"
            path = output_dir / f"wake_{case_id}.wav"
            raw_path = output_dir / "raw" / f"wake_{case_id}.wav"
            write_pcm16(raw_path, raw)
            write_pcm16(path, processed)
            cases.append(
                {
                    "id": case_id,
                    "kind": "positive",
                    "category": category,
                    "expected_gate": "accept",
                    "path": str(path),
                    "sha256": sha256(path),
                    "raw_path": str(raw_path),
                    "raw_sha256": sha256(raw_path),
                    "source_sha256": sha256(base_path),
                    "seed": seed,
                    **parameters,
                    "raw_metrics": metrics(raw),
                    "metrics": metrics(processed),
                }
            )

    # The clean-positive matrix missed the owner's actual failure condition:
    # another voice or television speech overlapping the wake utterance. Select
    # eight energetic retained non-wake candidates deterministically, mix them
    # at controlled relative levels/offsets, and send the result through the
    # same firmware-level model and production gate evaluator.
    ranked_interference = sorted(
        (
            (frame_reference_rms(read_pcm16(path)), path)
            for path in negative_paths
        ),
        key=lambda item: (-item[0], item[1].name),
    )
    overlap_sources = [path for reference, path in ranked_interference if reference >= 45.0][
        : len(OVERLAP_INTERFERENCE_DB)
    ]
    if len(overlap_sources) != len(OVERLAP_INTERFERENCE_DB):
        raise RuntimeError("need eight energetic retained negative overlap sources")
    overlap_wake_raw, _, overlap_wake_parameters = transform(base, "normal", 0, 106000)
    for case_index, source in enumerate(overlap_sources):
        interference = read_pcm16(source)
        raw, processed, overlap_parameters = mix_overlap(
            overlap_wake_raw,
            interference,
            OVERLAP_INTERFERENCE_DB[case_index],
            OVERLAP_ONSET_OFFSETS_MS[case_index],
        )
        case_id = f"overlap-{case_index + 1:02d}"
        path = output_dir / f"wake_{case_id}.wav"
        raw_path = output_dir / "raw" / f"wake_{case_id}.wav"
        write_pcm16(raw_path, raw)
        write_pcm16(path, processed)
        cases.append(
            {
                "id": case_id,
                "kind": "positive",
                "category": "overlap",
                "expected_gate": "accept",
                "path": str(path),
                "sha256": sha256(path),
                "raw_path": str(raw_path),
                "raw_sha256": sha256(raw_path),
                "source_sha256": sha256(base_path),
                "interference_source_sha256": sha256(source),
                "seed": 106000 + case_index,
                "speed": overlap_wake_parameters["speed"],
                "start_crop_ms": overlap_wake_parameters["start_crop_ms"],
                "distance_m": overlap_wake_parameters["distance_m"],
                "wake_phrase_start_ms": overlap_wake_parameters[
                    "wake_phrase_start_ms"
                ],
                "wake_phrase_end_ms": overlap_wake_parameters["wake_phrase_end_ms"],
                **overlap_parameters,
                "raw_metrics": metrics(raw),
                "metrics": metrics(processed),
            }
        )

    for case_index, source in enumerate(negative_paths):
        path = output_dir / f"neg_{case_index + 1:03d}.wav"
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, path)
        cases.append(
            {
                "id": f"negative-{case_index + 1:03d}",
                "kind": "negative",
                "category": "saved-live-non-match",
                "expected_gate": "reject",
                "path": str(path),
                "sha256": sha256(path),
                "source_sha256": sha256(source),
                "metrics": metrics(read_pcm16(path)),
            }
        )

    manifest = {
        "schema": "listener.mature-wake-matrix.v1",
        "product_version": args.product_version,
        "phrase": "开始录音",
        "source_sha256": sha256(base_path),
        "positive_case_count": len([case for case in cases if case["kind"] == "positive"]),
        "negative_case_count": len(negative_paths),
        "positive_categories": [*CATEGORIES, "overlap"],
        "required_overall_recall": 0.95,
        "required_category_recall": 0.90,
        "required_negative_accepts": 0,
        "cases": cases,
    }
    manifest_path = output_dir / "manifest.json"
    manifest_path.parent.mkdir(parents=True, exist_ok=True)
    manifest_path.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(f"manifest={manifest_path}")
    print(f"manifest_sha256={sha256(manifest_path)}")
    print(f"positive_cases={manifest['positive_case_count']}")
    print(f"negative_cases={len(negative_paths)}")


if __name__ == "__main__":
    main()
