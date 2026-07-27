#!/usr/bin/env node
/**
 * Machine gate for AI maintainability budgets.
 * Goals:
 *  - docs/goals/20260727-type-maintainability-excellent.md
 *  - docs/goals/20260727-type-maintainability-phase2.md
 */
import { readFileSync, existsSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const repoRoot = process.cwd();
const srcRoot = join(repoRoot, "src-tauri", "src");

/** @type {{ path: string, maxLines: number }[]} */
const FILE_BUDGETS = [
  { path: "coordinator.rs", maxLines: 9500 },
  { path: "commands/mod.rs", maxLines: 9000 },
  { path: "commands/device/mod.rs", maxLines: 80 },
  { path: "commands/device/settings.rs", maxLines: 2800 },
  { path: "commands/device/ble.rs", maxLines: 1200 },
  { path: "commands/device/firmware.rs", maxLines: 2800 },
  { path: "coordinator/dictation.rs", maxLines: 7500 },
  { path: "coordinator/support.rs", maxLines: 1500 },
  { path: "embedded_ble/mod.rs", maxLines: 2000 },
  { path: "embedded_ble/windows_ble/mod.rs", maxLines: 11000 },
  { path: "embedded_ble/windows_ble/ota_transfer.rs", maxLines: 1200 },
  { path: "embedded_ble/windows_ble/pairing.rs", maxLines: 3500 },
  { path: "types.rs", maxLines: 4000 },
  { path: "polish.rs", maxLines: 3500 },
  { path: "lib.rs", maxLines: 3500 },
];

const SEPARATED_TESTS = [
  {
    production: "coordinator.rs",
    testsFile: "coordinator_tests.rs",
    forbidden: /^\s*mod tests\s*\{/m,
  },
  {
    production: "coordinator/dictation.rs",
    testsFile: "coordinator/dictation_tests.rs",
    forbidden: /^\s*mod tests\s*\{/m,
  },
  {
    production: "commands/mod.rs",
    testsFile: "commands_tests.rs",
    forbidden: /^\s*mod tests\s*\{/m,
  },
  {
    production: "embedded_ble/mod.rs",
    testsFile: "embedded_ble/mod_tests.rs",
    forbidden: /^\s*mod tests\s*\{/m,
  },
  {
    production: "embedded_ble/windows_ble/mod.rs",
    testsFile: "embedded_ble/windows_ble_tests.rs",
    forbidden: /^\s*mod tests\s*\{/m,
  },
];

function countLines(absPath) {
  const text = readFileSync(absPath, "utf8");
  if (text.length === 0) return 0;
  return text.replace(/\r\n/g, "\n").split("\n").length;
}

function walkRs(dir, out = []) {
  if (!existsSync(dir)) return out;
  for (const name of readdirSync(dir)) {
    if (name === "target") continue;
    const p = join(dir, name);
    const st = statSync(p);
    if (st.isDirectory()) walkRs(p, out);
    else if (name.endsWith(".rs") && !name.endsWith("_tests.rs") && name !== "tests.rs") {
      out.push(p);
    }
  }
  return out;
}

const failures = [];
const report = { budgets: [], separatedTests: [], largest: [] };

for (const budget of FILE_BUDGETS) {
  const abs = join(srcRoot, budget.path);
  if (!existsSync(abs)) {
    failures.push(`missing budgeted file: ${budget.path}`);
    continue;
  }
  const lines = countLines(abs);
  const ok = lines <= budget.maxLines;
  report.budgets.push({ path: budget.path, lines, maxLines: budget.maxLines, ok });
  if (!ok) failures.push(`${budget.path}: ${lines} lines > budget ${budget.maxLines}`);
}

for (const rule of SEPARATED_TESTS) {
  const prod = join(srcRoot, rule.production);
  const tests = join(srcRoot, rule.testsFile);
  if (!existsSync(prod)) {
    failures.push(`missing production file for test separation: ${rule.production}`);
    continue;
  }
  if (!existsSync(tests)) {
    failures.push(`missing separated tests file: ${rule.testsFile}`);
    report.separatedTests.push({ production: rule.production, ok: false, reason: "missing tests file" });
    continue;
  }
  const prodText = readFileSync(prod, "utf8");
  const hasInlineBody = rule.forbidden.test(prodText);
  const hasPathMod =
    /#\[path\s*=\s*"[^"]+"\]\s*\n\s*mod tests\s*;/.test(prodText) ||
    /mod tests\s*;/.test(prodText);
  const ok = !hasInlineBody && hasPathMod;
  report.separatedTests.push({
    production: rule.production,
    testsFile: rule.testsFile,
    hasInlineBody,
    hasPathMod,
    ok,
  });
  if (hasInlineBody) failures.push(`${rule.production}: still contains inline mod tests { ... }`);
  if (!hasPathMod) failures.push(`${rule.production}: expected path-based or external mod tests;`);
}

const all = walkRs(srcRoot)
  .map((p) => ({ path: relative(srcRoot, p).replace(/\\/g, "/"), lines: countLines(p) }))
  .sort((a, b) => b.lines - a.lines);
report.largest = all.slice(0, 15);
report.softOver4500 = all.filter((f) => f.lines > 4500);

if (failures.length) {
  console.error("MODULE BUDGET FAIL");
  for (const f of failures) console.error(" -", f);
  console.error(JSON.stringify(report, null, 2));
  process.exit(1);
}

console.log("MODULE BUDGET PASS");
console.log(JSON.stringify(report, null, 2));
