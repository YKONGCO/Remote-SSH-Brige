#!/usr/bin/env node
/**
 * Write one version into every file that records it.
 *
 * Usage:
 *   node scripts/set-version.mjs <major.minor.patch>
 *   node scripts/set-version.mjs <major.minor.patch> --dry-run
 *
 * The version is stored in package.json, package-lock.json, src-tauri/Cargo.toml,
 * src-tauri/tauri.conf.json and the local package entry in src-tauri/Cargo.lock.
 * CI builds pass --locked to Cargo, so the lock file must change together with
 * the manifest. Edits are verified before anything is written: every changed
 * line must differ only in its version string, and each file must still parse
 * with the requested version.
 */
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const versionPattern = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*)?$/;
const quotedVersion = /"\d+\.\d+\.\d+(?:-[^"]*)?"/;
const usage = [
  "usage: node scripts/set-version.mjs <major.minor.patch> [--dry-run]",
  "",
  "Writes the version to package.json, package-lock.json, src-tauri/Cargo.toml,",
  "src-tauri/tauri.conf.json and the local package entry in src-tauri/Cargo.lock.",
].join("\n");

function readProjectFile(file) {
  return readFileSync(join(projectRoot, file), "utf8");
}

function escapeRegExp(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function findIndex(lines, start, end, test, description) {
  for (let index = start; index < end; index += 1) {
    if (test(lines[index], index)) return index;
  }
  throw new Error(`${description} not found`);
}

function findUniqueIndex(lines, test, description) {
  let found = -1;
  for (let index = 0; index < lines.length; index += 1) {
    if (!test(lines[index], index)) continue;
    if (found !== -1) throw new Error(`${description} is not unique`);
    found = index;
  }
  if (found === -1) throw new Error(`${description} not found`);
  return found;
}

function createTargets(packageName) {
  const escapedName = escapeRegExp(packageName);
  return [
    {
      file: "package.json",
      locate(lines) {
        return [
          {
            index: findUniqueIndex(
              lines,
              (line) => line.startsWith('  "version": '),
              'package.json: top-level "version" line',
            ),
            label: '"version"',
          },
        ];
      },
      check(content, version) {
        const parsed = JSON.parse(content);
        if (parsed.version !== version) {
          throw new Error(`package.json: version is ${parsed.version}`);
        }
      },
    },
    {
      file: "package-lock.json",
      locate(lines) {
        const root = findUniqueIndex(
          lines,
          (line) => line.startsWith('  "version": '),
          'package-lock.json: top-level "version" line',
        );
        const entry = findUniqueIndex(
          lines,
          (line) => line === '    "": {',
          'package-lock.json: packages[""] entry',
        );
        let end = entry + 1;
        while (end < lines.length && !lines[end].startsWith("    }")) end += 1;
        const inner = findIndex(
          lines,
          entry + 1,
          end,
          (line) => line.startsWith('      "version": '),
          'package-lock.json: packages[""] version line',
        );
        return [
          { index: root, label: 'root "version"' },
          { index: inner, label: 'packages[""] "version"' },
        ];
      },
      check(content, version) {
        const parsed = JSON.parse(content);
        if (parsed.version !== version) {
          throw new Error(`package-lock.json: version is ${parsed.version}`);
        }
        const local = parsed.packages?.[""]?.version;
        if (local !== version) {
          throw new Error(`package-lock.json: packages[""].version is ${local}`);
        }
      },
    },
    {
      file: "src-tauri/Cargo.toml",
      locate(lines) {
        const start = findUniqueIndex(
          lines,
          (line) => line === "[package]",
          "src-tauri/Cargo.toml: [package] section",
        );
        for (let index = start + 1; index < lines.length; index += 1) {
          if (lines[index].startsWith("[")) break;
          if (lines[index].startsWith('version = "')) {
            return [{ index, label: "[package] version" }];
          }
        }
        throw new Error("src-tauri/Cargo.toml: [package] version not found");
      },
      check(content, version) {
        const match = content.match(/^\[package\][\s\S]*?^version = "([^"]*)"/m);
        if (!match || match[1] !== version) {
          throw new Error(`src-tauri/Cargo.toml: [package] version is ${match?.[1]}`);
        }
      },
    },
    {
      file: "src-tauri/tauri.conf.json",
      locate(lines) {
        return [
          {
            index: findUniqueIndex(
              lines,
              (line) => line.startsWith('  "version": '),
              'src-tauri/tauri.conf.json: top-level "version" line',
            ),
            label: '"version"',
          },
        ];
      },
      check(content, version) {
        const parsed = JSON.parse(content);
        if (parsed.version !== version) {
          throw new Error(`src-tauri/tauri.conf.json: version is ${parsed.version}`);
        }
      },
    },
    {
      file: "src-tauri/Cargo.lock",
      locate(lines) {
        const nameIndex = findUniqueIndex(
          lines,
          (line) => line === `name = "${packageName}"`,
          `src-tauri/Cargo.lock: package ${packageName}`,
        );
        for (let index = nameIndex + 1; index < lines.length; index += 1) {
          if (lines[index] === "[[package]]") break;
          if (lines[index].startsWith('version = "')) {
            return [{ index, label: `local package ${packageName}` }];
          }
        }
        throw new Error(
          `src-tauri/Cargo.lock: version for ${packageName} not found`,
        );
      },
      check(content, version) {
        const match = content.match(
          new RegExp(
            `\\[\\[package\\]\\]\\nname = "${escapedName}"\\nversion = "([^"]*)"`,
          ),
        );
        if (!match || match[1] !== version) {
          throw new Error(
            `src-tauri/Cargo.lock: ${packageName} version is ${match?.[1]}`,
          );
        }
      },
    },
  ];
}

function assertOnlyVersionLines(file, beforeLines, afterLines, changedIndices) {
  if (beforeLines.length !== afterLines.length) {
    throw new Error(`${file}: line count changed`);
  }
  const changed = new Set(changedIndices);
  for (let index = 0; index < beforeLines.length; index += 1) {
    if (beforeLines[index] === afterLines[index]) continue;
    if (!changed.has(index)) {
      throw new Error(`${file}:${index + 1}: unexpected change`);
    }
    const mask = (line) => line.replace(quotedVersion, '"*"');
    if (mask(beforeLines[index]) !== mask(afterLines[index])) {
      throw new Error(`${file}:${index + 1}: change is not limited to the version`);
    }
  }
}

function planUpdates(targets, version) {
  const updates = [];
  for (const target of targets) {
    const before = readProjectFile(target.file);
    const beforeLines = before.split("\n");
    const afterLines = beforeLines.slice();
    const changes = [];
    for (const { index, label } of target.locate(beforeLines)) {
      const token = beforeLines[index].match(quotedVersion);
      if (!token) {
        throw new Error(`${target.file}:${index + 1}: no version string in ${label}`);
      }
      if (token[0] === `"${version}"`) continue;
      afterLines[index] = beforeLines[index].replace(token[0], `"${version}"`);
      changes.push({ index, label, from: token[0].slice(1, -1) });
    }
    const content = afterLines.join("\n");
    assertOnlyVersionLines(
      target.file,
      beforeLines,
      afterLines,
      changes.map((change) => change.index),
    );
    target.check(content, version);
    if (changes.length > 0) {
      updates.push({ file: target.file, content, changes });
    }
  }
  return updates;
}

function printChanges(updates, version) {
  for (const update of updates) {
    for (const change of update.changes) {
      console.log(
        `  ${update.file.padEnd(26)} ${change.label.padEnd(24)} ${change.from} -> ${version}`,
      );
    }
  }
}

function main() {
  const args = process.argv.slice(2);
  if (args.includes("--help") || args.includes("-h")) {
    console.log(usage);
    return;
  }
  const dryRun = args.includes("--dry-run");
  const unexpected = args.filter((arg) => arg.startsWith("-") && arg !== "--dry-run");
  if (unexpected.length > 0) {
    throw new Error(`unknown option ${unexpected[0]}\n${usage}`);
  }
  const values = args.filter((arg) => !arg.startsWith("-"));
  if (values.length !== 1) {
    throw new Error(`expected one version argument\n${usage}`);
  }
  const version = values[0].replace(/^v/, "");
  if (!versionPattern.test(version)) {
    throw new Error(`"${values[0]}" is not a major.minor.patch version`);
  }

  const packageJson = JSON.parse(readProjectFile("package.json"));
  const current = packageJson.version;
  const updates = planUpdates(createTargets(packageJson.name), version);

  if (updates.length === 0) {
    console.log(`Remote SSH Brige is already at ${version}; nothing to write.`);
    return;
  }

  console.log(`Remote SSH Brige ${current} -> ${version}`);
  printChanges(updates, version);
  if (dryRun) {
    console.log("Dry run: no files were written.");
    return;
  }
  for (const update of updates) {
    writeFileSync(join(projectRoot, update.file), update.content);
  }
  console.log(`Updated ${updates.length} file(s).`);
}

try {
  main();
} catch (error) {
  console.error(`set-version: ${error.message}`);
  process.exit(1);
}
