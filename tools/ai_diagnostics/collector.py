from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable

SCHEMA_ID = "listener_type.ai_diagnostic_bundle"
SCHEMA_VERSION = 1
MANIFEST_SCHEMA_ID = "listener_type.ai_diagnostic_manifest"
TEXT_EXTENSIONS = {
    ".json",
    ".jsonl",
    ".log",
    ".md",
    ".ps1",
    ".rs",
    ".toml",
    ".txt",
    ".yaml",
    ".yml",
}
ARTIFACT_ROOTS = ("tests/artifacts", "artifacts", ".artifacts")
HASH_SIZE_LIMIT_BYTES = 64 * 1024 * 1024
COPY_SIZE_LIMIT_BYTES = 512 * 1024
READ_TEXT_LIMIT_BYTES = 2 * 1024 * 1024
ARTIFACT_SCAN_LIMIT = 5000
TEXT_FIELD_NAMES = {
    "asr_text_updates",
    "expected_text",
    "final_text",
    "inserted_text",
    "last_partial_preview",
    "normalized_expected",
    "normalized_transcript",
    "sentence",
    "text_updates",
    "transcript",
}
WARN_ERROR_RE = re.compile(
    r"\b(error|warn|warning|failed|failure|panic|exception|timeout|cccd|no notification)\b",
    re.IGNORECASE,
)
SECRET_REPLACEMENTS = (
    (re.compile(r"sk-[A-Za-z0-9_\-]{12,}"), "sk-<redacted>"),
    (re.compile(r"(?i)(api[_\- ]?key|token|secret|password)\s*[:=]\s*['\"]?[^'\"\s,;]+"), r"\1=<redacted>"),
)


def utc_now() -> datetime:
    return datetime.now(timezone.utc).replace(microsecond=0)


def utc_string(value: datetime | None = None) -> str:
    value = value or utc_now()
    return value.isoformat().replace("+00:00", "Z")


def timestamp_string(value: datetime | None = None) -> str:
    value = value or utc_now()
    return value.strftime("%Y%m%dT%H%M%SZ")


def stable_json(path: Path, payload: Any) -> None:
    path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def resolve_input_path(raw: str | None, repo_root: Path) -> Path | None:
    if not raw:
        return None
    path = Path(raw).expanduser()
    if not path.is_absolute():
        path = repo_root / path
    return path.resolve()


def relative_path(path: Path, root: Path) -> str:
    try:
        return path.resolve().relative_to(root.resolve()).as_posix()
    except ValueError:
        return str(path)


def relative_to(path: Path, root: Path) -> str:
    try:
        return path.resolve().relative_to(root.resolve()).as_posix()
    except ValueError:
        return relative_path(path, root)


def is_within(path: Path, root: Path) -> bool:
    try:
        path.resolve().relative_to(root.resolve())
        return True
    except ValueError:
        return False


def is_generated_ai_diagnostics_path(path: Path, repo_root: Path) -> bool:
    try:
        parts = path.resolve().relative_to(repo_root.resolve()).parts
    except ValueError:
        return False
    if len(parts) < 3:
        return False
    return parts[0] == "tests" and parts[1] == "artifacts" and parts[2].startswith("ai_diagnostics")


def file_mtime_utc(path: Path) -> str | None:
    try:
        return datetime.fromtimestamp(path.stat().st_mtime, timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")
    except OSError:
        return None


def sha256_file(path: Path, size_limit: int = HASH_SIZE_LIMIT_BYTES) -> dict[str, Any]:
    try:
        size = path.stat().st_size
    except OSError as exc:
        return {"sha256": None, "hash_status": "error", "error": str(exc)}
    if size > size_limit:
        return {"sha256": None, "hash_status": "skipped_size_limit", "size_limit_bytes": size_limit}
    digest = hashlib.sha256()
    try:
        with path.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
    except OSError as exc:
        return {"sha256": None, "hash_status": "error", "error": str(exc)}
    return {"sha256": digest.hexdigest(), "hash_status": "ok"}


def sha256_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8", errors="replace")).hexdigest()


def redact_text(value: str) -> str:
    redacted = value
    for pattern, replacement in SECRET_REPLACEMENTS:
        redacted = pattern.sub(replacement, redacted)
    return redacted


def truncate(value: str, limit: int = 300) -> str:
    if len(value) <= limit:
        return value
    return value[: limit - 3] + "..."


def run_command(command: list[str], cwd: Path, timeout_seconds: int = 20) -> dict[str, Any]:
    try:
        completed = subprocess.run(
            command,
            cwd=str(cwd),
            text=True,
            encoding="utf-8",
            errors="replace",
            capture_output=True,
            timeout=timeout_seconds,
            check=False,
        )
    except Exception as exc:
        return {
            "command": command,
            "exit_code": None,
            "status": "error",
            "error": str(exc),
            "stdout": "",
            "stderr": "",
        }
    return {
        "command": command,
        "exit_code": completed.returncode,
        "status": "ok" if completed.returncode == 0 else "failed",
        "stdout": completed.stdout.strip(),
        "stderr": completed.stderr.strip(),
    }


def git_metadata(repo_root: Path) -> dict[str, Any]:
    def git(args: list[str]) -> dict[str, Any]:
        return run_command(["git", "-C", str(repo_root), *args], cwd=repo_root)

    head = git(["rev-parse", "HEAD"])
    branch = git(["branch", "--show-current"])
    describe = git(["describe", "--tags", "--dirty", "--always"])
    status = git(["status", "--short", "--branch"])
    porcelain = git(["status", "--porcelain=v1"])
    status_lines = status["stdout"].splitlines() if status["stdout"] else []
    porcelain_lines = porcelain["stdout"].splitlines() if porcelain["stdout"] else []
    return {
        "branch": branch["stdout"] or None,
        "commit": head["stdout"] or None,
        "describe": describe["stdout"] or None,
        "dirty": bool(porcelain_lines),
        "status_short": status_lines[:200],
        "status_truncated": len(status_lines) > 200,
    }


def environment_summary() -> dict[str, Any]:
    safe_env_names = (
        "AI_AGENT_ID",
        "CI",
        "GITHUB_ACTIONS",
        "LISTENER_TYPE_DIAGNOSTIC_WRAPPER",
        "LOCALAPPDATA",
        "OS",
        "PROCESSOR_ARCHITECTURE",
    )
    return {
        "platform": {
            "machine": platform.machine(),
            "platform": platform.platform(),
            "python_executable": sys.executable,
            "python_version": sys.version.split()[0],
            "system": platform.system(),
            "version": platform.version(),
        },
        "environment": {name: os.environ.get(name) for name in safe_env_names if os.environ.get(name)},
    }


def make_bundle_dir(output_dir: Path, generated_at: datetime) -> Path:
    output_dir.mkdir(parents=True, exist_ok=True)
    base_name = timestamp_string(generated_at)
    candidate = output_dir / base_name
    index = 1
    while candidate.exists():
        candidate = output_dir / f"{base_name}-{index:02d}"
        index += 1
    candidate.mkdir(parents=True)
    return candidate


def classify_file(path: Path, repo_root: Path, kind_hint: str) -> str:
    rel = relative_path(path, repo_root).lower()
    name = path.name.lower()
    if kind_hint == "app_log" or name.endswith(".log"):
        return "log"
    if "validation" in rel and path.suffix.lower() == ".md":
        return "validation_doc"
    if any(term in rel for term in ("ble", "embedded", "audio", "stream", "wav", "pcm", "asr")):
        return "ble_audio_artifact"
    if path.suffix.lower() in (".json", ".jsonl"):
        return "structured_artifact"
    return kind_hint


def should_copy_file(path: Path, category: str, no_copy: bool) -> tuple[bool, str]:
    if no_copy:
        return False, "disabled"
    if path.is_symlink():
        return False, "symlink"
    try:
        size = path.stat().st_size
    except OSError as exc:
        return False, f"stat_error:{exc}"
    if size > COPY_SIZE_LIMIT_BYTES:
        return False, "size_limit"
    suffix = path.suffix.lower()
    if suffix not in TEXT_EXTENSIONS:
        return False, "non_text_extension"
    rel_lower = str(path).lower()
    if any(term in rel_lower for term in ("history", "transcript", "recording_archive")):
        return False, "possible_user_text"
    if category in ("validation_doc", "structured_artifact", "log"):
        return True, "copied"
    return False, "referenced"


def copy_source_file(path: Path, repo_root: Path, bundle_dir: Path, category: str) -> str:
    rel = relative_path(path, repo_root)
    safe_rel = rel.replace(":", "_").replace("\\", "/")
    if safe_rel.startswith("..") or Path(safe_rel).is_absolute():
        safe_rel = path.name.replace(":", "_")
    target = bundle_dir / "source_files" / category / safe_rel
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(path, target)
    return relative_to(target, bundle_dir)


def summarize_text_value(value: str) -> dict[str, Any]:
    return {
        "present": bool(value),
        "length": len(value),
        "sha256": sha256_text(value),
    }


def summarize_json_value(key: str, value: Any) -> Any:
    if key in TEXT_FIELD_NAMES:
        if isinstance(value, str):
            return summarize_text_value(value)
        if isinstance(value, list):
            joined = "\n".join(str(item) for item in value)
            return {
                "item_count": len(value),
                "joined_text": summarize_text_value(joined),
            }
    if isinstance(value, (str, int, float, bool)) or value is None:
        if isinstance(value, str) and len(value) > 240:
            return {"length": len(value), "sha256": sha256_text(value), "preview": truncate(redact_text(value), 120)}
        return value
    if isinstance(value, list):
        return {"item_count": len(value)}
    if isinstance(value, dict):
        return {"keys": sorted(str(k) for k in value.keys())[:50], "key_count": len(value)}
    return str(type(value).__name__)


def summarize_json_artifact(path: Path) -> dict[str, Any] | None:
    if path.suffix.lower() != ".json":
        return None
    try:
        if path.stat().st_size > READ_TEXT_LIMIT_BYTES:
            return {"parse_status": "skipped_size_limit", "size_limit_bytes": READ_TEXT_LIMIT_BYTES}
        payload = json.loads(path.read_text(encoding="utf-8", errors="replace"))
    except Exception as exc:
        return {"parse_status": "error", "error": str(exc)}
    if not isinstance(payload, dict):
        return {"parse_status": "ok", "top_level_type": type(payload).__name__}

    keys_of_interest = (
        "report_schema",
        "schema_id",
        "schema_version",
        "status",
        "trigger",
        "audio_profile",
        "started_at_utc",
        "log_path",
        "serial_log_path",
        "recording_archive_path",
        "history_path",
        "pcm_bytes",
        "missing_packets",
        "verification_errors",
        "expected_text",
        "transcript",
        "final_text",
        "asr_text_updates",
        "history_session",
        "serial_report",
    )
    summary: dict[str, Any] = {
        "parse_status": "ok",
        "top_level_keys": sorted(str(key) for key in payload.keys())[:80],
        "top_level_key_count": len(payload),
    }
    for key in keys_of_interest:
        if key in payload:
            summary[key] = summarize_json_value(key, payload[key])
    return summary


def file_record(path: Path, repo_root: Path, bundle_dir: Path, kind_hint: str, no_copy: bool) -> dict[str, Any]:
    category = classify_file(path, repo_root, kind_hint)
    record: dict[str, Any] = {
        "path": relative_path(path, repo_root),
        "category": category,
        "extension": path.suffix.lower(),
        "mtime_utc": file_mtime_utc(path),
    }
    try:
        record["size_bytes"] = path.stat().st_size
    except OSError as exc:
        record["size_bytes"] = None
        record["stat_error"] = str(exc)
    record.update(sha256_file(path))
    copy_allowed, copy_status = should_copy_file(path, category, no_copy)
    record["copy_status"] = copy_status
    if copy_allowed:
        try:
            record["bundle_copy_path"] = copy_source_file(path, repo_root, bundle_dir, category)
        except Exception as exc:
            record["copy_status"] = "copy_error"
            record["copy_error"] = str(exc)
    artifact_summary = summarize_json_artifact(path)
    if artifact_summary is not None:
        record["artifact_summary"] = artifact_summary
    return record


def iter_files_bounded(root: Path, repo_root: Path | None = None) -> Iterable[Path]:
    scanned = 0
    for current_root, dirnames, filenames in os.walk(root):
        current_path = Path(current_root)
        if repo_root is not None:
            dirnames[:] = [
                name
                for name in dirnames
                if not is_generated_ai_diagnostics_path(current_path / name, repo_root)
            ]
        for filename in filenames:
            scanned += 1
            if scanned > ARTIFACT_SCAN_LIMIT:
                return
            path = current_path / filename
            if path.is_file():
                yield path


def discover_candidate_files(repo_root: Path, bundle_dir: Path) -> list[tuple[Path, str]]:
    candidates: list[tuple[Path, str]] = []
    for relative_root in ARTIFACT_ROOTS:
        root = repo_root / relative_root
        if not root.exists():
            continue
        for path in iter_files_bounded(root, repo_root):
            if is_within(path, bundle_dir):
                continue
            if is_generated_ai_diagnostics_path(path, repo_root):
                continue
            candidates.append((path, "repo_artifact"))

    validation_root = repo_root / "docs" / "validation"
    if validation_root.exists():
        for path in iter_files_bounded(validation_root, repo_root):
            candidates.append((path, "validation_doc"))

    local_appdata = os.environ.get("LOCALAPPDATA")
    if local_appdata:
        log_root = Path(local_appdata) / "Listener Type" / "Logs"
        if log_root.exists():
            for path in iter_files_bounded(log_root):
                candidates.append((path, "app_log"))
    return candidates


def discover_files(repo_root: Path, bundle_dir: Path, max_files: int, no_copy: bool) -> list[dict[str, Any]]:
    candidates = discover_candidate_files(repo_root, bundle_dir)
    candidates.sort(key=lambda item: item[0].stat().st_mtime if item[0].exists() else 0, reverse=True)
    return [
        file_record(path, repo_root, bundle_dir, kind_hint, no_copy)
        for path, kind_hint in candidates[:max_files]
    ]


def collect_warning_error_refs(records: list[dict[str, Any]], repo_root: Path, limit: int) -> list[dict[str, Any]]:
    refs: list[dict[str, Any]] = []
    for record in records:
        extension = str(record.get("extension") or "").lower()
        if extension not in TEXT_EXTENSIONS:
            continue
        path_text = str(record.get("path") or "")
        path = Path(path_text)
        if not path.is_absolute():
            path = repo_root / path
        if not path.exists() or not path.is_file():
            continue
        try:
            size = path.stat().st_size
        except OSError:
            continue
        if size > READ_TEXT_LIMIT_BYTES:
            continue
        matches: list[dict[str, Any]] = []
        try:
            with path.open("r", encoding="utf-8", errors="replace") as handle:
                for line_number, line in enumerate(handle, 1):
                    if WARN_ERROR_RE.search(line):
                        matches.append(
                            {
                                "path": relative_path(path, repo_root),
                                "line": line_number,
                                "message": truncate(redact_text(line.strip())),
                            }
                        )
        except OSError:
            continue
        refs.extend(matches[-20:])
    refs.sort(key=lambda item: (str(item["path"]), int(item["line"])))
    return refs[-limit:]


def read_json_file(path: Path) -> tuple[Any | None, str | None]:
    try:
        return json.loads(path.read_text(encoding="utf-8", errors="replace")), None
    except Exception as exc:
        return None, str(exc)


def branch_agent_id(branch: str | None) -> str | None:
    if not branch:
        return None
    match = re.match(r"^ai/([^-]+)-", branch)
    if not match:
        return None
    return match.group(1)


def iter_workflow_steps(payload: dict[str, Any]) -> Iterable[dict[str, Any]]:
    steps = payload.get("steps")
    if isinstance(steps, list):
        for step in steps:
            if isinstance(step, dict):
                yield step
    phases = payload.get("phases")
    if isinstance(phases, list):
        for phase in phases:
            if not isinstance(phase, dict):
                continue
            phase_steps = phase.get("steps")
            if not isinstance(phase_steps, list):
                continue
            for step in phase_steps:
                if isinstance(step, dict):
                    yield step


def find_workflow_metadata(repo_root: Path, git_info: dict[str, Any]) -> dict[str, Any]:
    env_agent_id = os.environ.get("AI_AGENT_ID")
    branch_agent = branch_agent_id(git_info.get("branch"))
    metadata: dict[str, Any] = {
        "env_agent_id": env_agent_id,
        "branch_agent_id": branch_agent,
        "effective_agent_id": branch_agent or env_agent_id,
        "branch": git_info.get("branch"),
    }
    workflow_root = repo_root.parent / "ai-collaboration-workflow"
    status_path = workflow_root / "docs" / "plans" / "listener-type-ai-diagnostic-bundle_status.json"
    metadata["workflow_root"] = str(workflow_root) if workflow_root.exists() else None
    metadata["status_path"] = str(status_path) if status_path.exists() else None
    if status_path.exists():
        payload, error = read_json_file(status_path)
        if error:
            metadata["status_parse_error"] = error
        elif isinstance(payload, dict):
            metadata["plan"] = payload.get("plan") or payload.get("slug") or "listener-type-ai-diagnostic-bundle"
            for step in iter_workflow_steps(payload):
                if str(step.get("id")) == "1.1":
                    metadata["step"] = {
                        "id": step.get("id"),
                        "title": step.get("title"),
                        "status": step.get("status"),
                        "assignee": step.get("assignee"),
                        "branch": step.get("branch"),
                        "worktree_path": step.get("worktree_path"),
                    }
                    break
    return metadata


def summarize_jsonl(path: Path) -> dict[str, Any]:
    summary: dict[str, Any] = {
        "line_count": 0,
        "parsed_json_lines": 0,
        "parse_errors": 0,
        "observed_keys": [],
        "observed_event_fields": {},
    }
    keys: set[str] = set()
    events: dict[str, set[str]] = {"event": set(), "evt": set(), "type": set(), "code": set(), "op": set()}
    try:
        with path.open("r", encoding="utf-8", errors="replace") as handle:
            for line in handle:
                summary["line_count"] += 1
                text = line.strip()
                if not text:
                    continue
                try:
                    payload = json.loads(text)
                except json.JSONDecodeError:
                    summary["parse_errors"] += 1
                    continue
                if isinstance(payload, dict):
                    summary["parsed_json_lines"] += 1
                    keys.update(str(key) for key in payload.keys())
                    for event_key in events:
                        if event_key in payload:
                            events[event_key].add(str(payload[event_key]))
    except OSError as exc:
        summary["read_error"] = str(exc)
    summary["observed_keys"] = sorted(keys)[:80]
    summary["observed_event_fields"] = {key: sorted(value)[:40] for key, value in events.items() if value}
    return summary


def detect_firmware_tool(firmware_repo: Path) -> Path | None:
    for candidate in (
        firmware_repo / "tools" / "collect_ai_diagnostics.ps1",
        firmware_repo / "tools" / "collect_ai_diagnostic_bundle.ps1",
        firmware_repo / "tools" / "ai" / "collect_ai_diagnostics.ps1",
    ):
        if candidate.exists():
            return candidate
    return None


def invoke_firmware_tool(tool_path: Path, output_dir: Path, firmware_repo: Path) -> dict[str, Any]:
    pwsh = shutil.which("pwsh") or shutil.which("powershell")
    if not pwsh:
        return {"status": "not_invoked", "reason": "powershell_not_found", "tool_path": str(tool_path)}
    tool_output = output_dir / "firmware_tool"
    tool_output.mkdir(parents=True, exist_ok=True)
    result = run_command(
        [pwsh, "-NoProfile", "-File", str(tool_path), "-OutputDir", str(tool_output)],
        cwd=firmware_repo,
        timeout_seconds=120,
    )
    result["tool_path"] = str(tool_path)
    result["output_dir"] = str(tool_output)
    result["stdout"] = truncate(redact_text(result.get("stdout") or ""), 2000)
    result["stderr"] = truncate(redact_text(result.get("stderr") or ""), 2000)
    produced_files = [path for path in iter_files_bounded(tool_output) if path.is_file()]
    result["produced_files"] = [
        {
            "path": relative_to(path, output_dir),
            "size_bytes": path.stat().st_size,
            **sha256_file(path),
        }
        for path in produced_files[:50]
    ]
    return result


def firmware_integration(
    repo_root: Path,
    bundle_dir: Path,
    firmware_repo: Path | None,
    firmware_diag_log: Path | None,
    firmware_bundle: Path | None,
    no_copy: bool,
) -> dict[str, Any]:
    requested = any((firmware_repo, firmware_diag_log, firmware_bundle))
    result: dict[str, Any] = {
        "requested": requested,
        "decoder_status": "not_requested" if not requested else "raw_metadata_only",
    }
    if firmware_repo:
        result["firmware_repo"] = {
            "path": str(firmware_repo),
            "exists": firmware_repo.exists(),
        }
        if firmware_repo.exists():
            tool_path = detect_firmware_tool(firmware_repo)
            result["firmware_tool"] = {
                "available": bool(tool_path),
                "path": str(tool_path) if tool_path else None,
            }
            if tool_path and not firmware_bundle:
                tool_result = invoke_firmware_tool(tool_path, bundle_dir, firmware_repo)
                result["firmware_tool"]["invocation"] = tool_result
                result["decoder_status"] = "firmware_tool_invoked" if tool_result.get("exit_code") == 0 else "firmware_tool_failed_raw_metadata_only"
    if firmware_bundle:
        result["firmware_bundle"] = file_record(firmware_bundle, repo_root, bundle_dir, "firmware_bundle", no_copy)
        result["decoder_status"] = "ingested_firmware_bundle"
    if firmware_diag_log:
        result["firmware_diag_log"] = file_record(firmware_diag_log, repo_root, bundle_dir, "firmware_diag_log", no_copy)
        result["firmware_diag_log"]["jsonl_summary"] = summarize_jsonl(firmware_diag_log) if firmware_diag_log.exists() else None
        if result["decoder_status"] == "raw_metadata_only":
            result["decoder_status"] = "raw_log_metadata_only"
    return result


def manifest_for(bundle_dir: Path, diagnostic_path: Path, diagnostic_bundle: dict[str, Any]) -> dict[str, Any]:
    copied_files: list[dict[str, Any]] = []
    for path in sorted((bundle_dir / "source_files").rglob("*")) if (bundle_dir / "source_files").exists() else []:
        if path.is_file():
            copied_files.append(
                {
                    "path": relative_to(path, bundle_dir),
                    "size_bytes": path.stat().st_size,
                    **sha256_file(path),
                }
            )
    bundle_files = [
        {
            "path": relative_to(diagnostic_path, bundle_dir),
            "size_bytes": diagnostic_path.stat().st_size,
            **sha256_file(diagnostic_path),
        },
        *copied_files,
    ]
    source_records = diagnostic_bundle["source_files"]
    return {
        "schema_id": MANIFEST_SCHEMA_ID,
        "schema_version": 1,
        "bundle_schema_id": diagnostic_bundle["schema_id"],
        "bundle_schema_version": diagnostic_bundle["schema_version"],
        "generated_at_utc": diagnostic_bundle["generated_at_utc"],
        "bundle_dir": str(bundle_dir),
        "entry_point": "tools/collect_ai_diagnostics.ps1",
        "diagnostic_bundle_path": "diagnostic_bundle.json",
        "counts": {
            "source_files": len(source_records),
            "copied_source_files": sum(1 for item in source_records if item.get("bundle_copy_path")),
            "recent_warning_error_refs": len(diagnostic_bundle["recent_warning_error_refs"]),
            "ble_audio_artifacts": len(diagnostic_bundle["ble_audio_artifacts"]),
            "bundle_files": len(bundle_files),
        },
        "bundle_files": bundle_files,
    }


def build_bundle(args: argparse.Namespace) -> tuple[Path, Path, Path]:
    repo_root = resolve_input_path(args.repo_root, Path.cwd())
    if not repo_root:
        raise ValueError("repo root could not be resolved")
    output_dir = resolve_input_path(args.output_dir, repo_root)
    if not output_dir:
        raise ValueError("output dir could not be resolved")
    generated_at = utc_now()
    bundle_dir = make_bundle_dir(output_dir, generated_at)
    git_info = git_metadata(repo_root)
    records = discover_files(repo_root, bundle_dir, args.max_files, args.no_copy)
    warning_error_refs = collect_warning_error_refs(records, repo_root, args.max_error_refs)
    firmware = firmware_integration(
        repo_root=repo_root,
        bundle_dir=bundle_dir,
        firmware_repo=resolve_input_path(args.firmware_repo, repo_root),
        firmware_diag_log=resolve_input_path(args.firmware_diag_log, repo_root),
        firmware_bundle=resolve_input_path(args.firmware_bundle, repo_root),
        no_copy=args.no_copy,
    )
    ble_audio_artifacts = [
        record
        for record in records
        if record.get("category") == "ble_audio_artifact"
        or any(term in str(record.get("path", "")).lower() for term in ("ble", "embedded", "audio", "stream", "wav", "pcm", "asr"))
    ]
    diagnostic_bundle = {
        "schema_id": SCHEMA_ID,
        "schema_version": SCHEMA_VERSION,
        "generated_at_utc": utc_string(generated_at),
        "command": {
            "argv": sys.argv,
            "cwd": str(Path.cwd()),
            "output_dir": str(output_dir),
            "bundle_dir": str(bundle_dir),
            "max_files": args.max_files,
            "max_error_refs": args.max_error_refs,
            "no_copy": args.no_copy,
        },
        "source": {
            "collector": "tools.ai_diagnostics.collector",
            "entry_point": "tools/collect_ai_diagnostics.ps1",
            "repo_root": str(repo_root),
        },
        "git": git_info,
        "workflow": find_workflow_metadata(repo_root, git_info),
        "environment": environment_summary(),
        "source_files": records,
        "discovered_type_logs": [record for record in records if record.get("category") == "log"],
        "ble_audio_artifacts": ble_audio_artifacts,
        "recent_warning_error_refs": warning_error_refs,
        "firmware": firmware,
    }
    diagnostic_path = bundle_dir / "diagnostic_bundle.json"
    stable_json(diagnostic_path, diagnostic_bundle)
    manifest_path = bundle_dir / "manifest.json"
    stable_json(manifest_path, manifest_for(bundle_dir, diagnostic_path, diagnostic_bundle))
    return bundle_dir, manifest_path, diagnostic_path


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Collect a Listener-Type AI diagnostic bundle.")
    parser.add_argument("--repo-root", default=str(Path(__file__).resolve().parents[2]))
    parser.add_argument("--output-dir", default="tests/artifacts/ai_diagnostics")
    parser.add_argument("--firmware-repo")
    parser.add_argument("--firmware-diag-log")
    parser.add_argument("--firmware-bundle")
    parser.add_argument("--max-files", type=int, default=200)
    parser.add_argument("--max-error-refs", type=int, default=80)
    parser.add_argument("--no-copy", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv or sys.argv[1:])
    bundle_dir, manifest_path, diagnostic_path = build_bundle(args)
    print(f"ai_diagnostic_bundle_dir={bundle_dir}")
    print(f"ai_diagnostic_manifest={manifest_path}")
    print(f"ai_diagnostic_bundle={diagnostic_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
