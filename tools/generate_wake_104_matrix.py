from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import pathlib
import random
import shutil
import wave


SAMPLE_RATE = 16000
TARGET_PEAK = 23197
TARGET_VOICED_RMS = 4096.0
MAX_AGC_GAIN = 10.0 ** (48.0 / 20.0)
MIN_AGC_GAIN = 10.0 ** (-12.0 / 20.0)
ACTUAL_PREROLL_MS = 1000
CATEGORIES = ("normal", "soft", "loud", "fast", "far")


def read_pcm16(path: pathlib.Path) -> list[int]:
    with wave.open(str(path), "rb") as wav:
        if (
            wav.getnchannels() != 1
            or wav.getsampwidth() != 2
            or wav.getframerate() != SAMPLE_RATE
        ):
            raise RuntimeError(f"unsupported WAV geometry: {path}")
        raw = wav.readframes(wav.getnframes())
    return [
        int.from_bytes(raw[index : index + 2], "little", signed=True)
        for index in range(0, len(raw), 2)
    ]


def write_pcm16(path: pathlib.Path, samples: list[int]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    raw = bytearray()
    for sample in samples:
        raw.extend(int(sample).to_bytes(2, "little", signed=True))
    with wave.open(str(path), "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(SAMPLE_RATE)
        wav.writeframes(raw)


def resample_linear(samples: list[int], speed: float) -> list[int]:
    output_len = max(1, int(round(len(samples) / speed)))
    output = []
    for output_index in range(output_len):
        source = output_index * speed
        left = min(int(source), len(samples) - 1)
        right = min(left + 1, len(samples) - 1)
        fraction = source - left
        output.append(
            int(round(samples[left] * (1.0 - fraction) + samples[right] * fraction))
        )
    return output


def limit(samples: list[float]) -> list[int]:
    peak = max((abs(value) for value in samples), default=0.0)
    scale = min(1.0, TARGET_PEAK / peak) if peak > 0.0 else 1.0
    return [
        max(-32768, min(32767, int(round(value * scale))))
        for value in samples
    ]


def firmware_level(samples: list[int]) -> tuple[list[int], float]:
    frame_samples = SAMPLE_RATE // 50
    frame_rms = []
    for start in range(0, len(samples), frame_samples):
        frame = samples[start : start + frame_samples]
        if not frame:
            continue
        frame_rms.append(
            math.sqrt(sum(float(value) * float(value) for value in frame) / len(frame))
        )
    frame_rms.sort()
    reference = (
        frame_rms[min(len(frame_rms) - 1, len(frame_rms) * 85 // 100)]
        if frame_rms
        else 0.0
    )
    gain = (
        max(MIN_AGC_GAIN, min(MAX_AGC_GAIN, TARGET_VOICED_RMS / reference))
        if reference > 0.0
        else 1.0
    )
    return limit([float(sample) * gain for sample in samples]), gain


def transform(samples: list[int], category: str, seed: int) -> list[int]:
    rng = random.Random(seed)
    working = list(samples)
    gain = 1.0
    if category == "soft":
        gain = 0.25
    elif category == "loud":
        gain = 4.0
    elif category == "fast":
        working = resample_linear(working, 1.22)
    elif category == "far":
        gain = 0.35

    values = [float(sample) * gain for sample in working]
    if category == "far":
        dry = list(values)
        for index in range(len(values)):
            echo = 0.0
            if index >= 83:
                echo += dry[index - 83] * 0.28
            if index >= 211:
                echo += dry[index - 211] * 0.16
            values[index] += echo + rng.uniform(-18.0, 18.0)
    elif category in {"normal", "soft", "loud"}:
        noise = 2.0 if category != "soft" else 0.75
        values = [value + rng.uniform(-noise, noise) for value in values]

    prefix_ms = ACTUAL_PREROLL_MS + (seed % 4) * 20
    return [0] * (prefix_ms * SAMPLE_RATE // 1000) + limit(values)


def metrics(samples: list[int]) -> dict[str, float | int]:
    peak = max((abs(value) for value in samples), default=0)
    rms = math.sqrt(
        sum(float(value) * float(value) for value in samples) / max(1, len(samples))
    )
    clipped = sum(value in (-32768, 32767) for value in samples)
    return {
        "samples": len(samples),
        "duration_ms": round(len(samples) * 1000.0 / SAMPLE_RATE, 3),
        "rms": round(rms, 3),
        "peak": peak,
        "clipped_samples": clipped,
    }


def sha256(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--output-dir",
        default=".artifacts/listener-1.0.4-recording-polish/wake-matrix",
    )
    parser.add_argument("--cases-per-category", type=int, default=20)
    args = parser.parse_args()

    output_dir = pathlib.Path(args.output_dir).resolve()
    positive_dir = output_dir
    negative_dir = output_dir
    local_app_data = pathlib.Path(os.environ["LOCALAPPDATA"])
    diagnostic_dir = local_app_data / "Listener Type" / "Logs" / "wake-diag-live"
    accepted = sorted(diagnostic_dir.glob("*accepted.wav"))
    airborne = (
        pathlib.Path(__file__).resolve().parents[2]
        / "Listener-Firmware"
        / ".artifacts"
        / "listener-1.0.4-recording-polish"
        / "airborne"
        / "wake-phrase-start-recording-16k-mono.wav"
    )
    bases = accepted + ([airborne] if airborne.exists() else [])
    if len(bases) < 4:
        raise RuntimeError(f"need at least four accepted wake WAVs, found {len(bases)}")

    cases: list[dict[str, object]] = []
    for category_index, category in enumerate(CATEGORIES):
        for case_index in range(args.cases_per_category):
            source = bases[case_index % len(bases)]
            seed = 104000 + category_index * 100 + case_index
            raw_samples = transform(read_pcm16(source), category, seed)
            samples, firmware_agc_gain = firmware_level(raw_samples)
            case_id = f"{category}-{case_index + 1:02d}"
            path = positive_dir / f"wake_{case_id}.wav"
            raw_path = positive_dir / "raw" / f"wake_{case_id}.wav"
            write_pcm16(raw_path, raw_samples)
            write_pcm16(path, samples)
            cases.append(
                {
                    "id": case_id,
                    "kind": "positive",
                    "category": category,
                    "expected_hit": True,
                    "path": str(path),
                    "sha256": sha256(path),
                    "raw_path": str(raw_path),
                    "raw_sha256": sha256(raw_path),
                    "source_sha256": sha256(source),
                    "seed": seed,
                    "firmware_agc_gain": round(firmware_agc_gain, 6),
                    "raw_metrics": metrics(raw_samples),
                    "metrics": metrics(samples),
                }
            )

    negative_sources = sorted(
        path
        for path in diagnostic_dir.glob("*.wav")
        if "accepted" not in path.name
    )
    for case_index, source in enumerate(negative_sources):
        path = negative_dir / f"neg_{case_index + 1:03d}.wav"
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, path)
        samples = read_pcm16(path)
        cases.append(
            {
                "id": f"negative-{case_index + 1:03d}",
                "kind": "negative",
                "category": "saved-non-match",
                "expected_hit": False,
                "path": str(path),
                "sha256": sha256(path),
                "source_sha256": sha256(source),
                "metrics": metrics(samples),
            }
        )

    manifest = {
        "schema": "listener.wake-matrix.v1",
        "product_version": "1.0.4",
        "phrase": "开始录音",
        "runtime_score": 3.5,
        "runtime_threshold": 0.05,
        "actual_preroll_ms": ACTUAL_PREROLL_MS,
        "firmware_level_model": {
            "target_voiced_rms": TARGET_VOICED_RMS,
            "max_gain_db": 48.0,
            "min_gain_db": -12.0,
            "limiter_peak": TARGET_PEAK,
            "reference": "85th_percentile_of_20ms_frame_rms",
        },
        "cases_per_positive_category": args.cases_per_category,
        "positive_categories": list(CATEGORIES),
        "positive_case_count": sum(case["kind"] == "positive" for case in cases),
        "negative_case_count": sum(case["kind"] == "negative" for case in cases),
        "cases": cases,
    }
    manifest_path = output_dir / "manifest.json"
    manifest_path.parent.mkdir(parents=True, exist_ok=True)
    manifest_path.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print(f"manifest={manifest_path}")
    print(f"manifest_sha256={sha256(manifest_path)}")
    print(f"positive_cases={manifest['positive_case_count']}")
    print(f"negative_cases={manifest['negative_case_count']}")


if __name__ == "__main__":
    main()
