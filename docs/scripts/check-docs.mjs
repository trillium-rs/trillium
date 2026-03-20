#!/usr/bin/env node
// Extracts rust fenced blocks from guide docs, compiles each one against the
// workspace crates, and bails with attribution on the first failure.
//
// Lines prefixed with "# " (rustdoc-style hidden lines) are stripped before
// compilation so authors can add hidden context without it appearing in the
// rendered docs.
//
// Usage: node docs/scripts/check-docs.mjs

import { readFileSync, writeFileSync, readdirSync, cpSync, rmSync } from 'fs';
import { join, relative, dirname, basename } from 'path';
import { fileURLToPath } from 'url';
import { spawnSync } from 'child_process';

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(__dirname, '../..');
const guideDir = join(__dirname, '../guide');
const docsCheckDir = join(repoRoot, 'docs-check');
const cargoTomlPath = join(docsCheckDir, 'Cargo.toml');
const mainRsPath = join(docsCheckDir, 'src', 'main.rs');

const STUB_CARGO_TOML = `[package]
name = "docs-check"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
`;

const STUB_MAIN_RS = `fn main() {}\n`;

function findMarkdownFiles(dir) {
  const results = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) results.push(...findMarkdownFiles(full));
    else if (entry.name.endsWith('.md')) results.push(full);
  }
  return results.sort();
}

// Returns [{code, lineNum}] for every ```rust block in content.
// lineNum is the 1-based line of the opening fence.
function extractRustBlocks(content) {
  const blocks = [];
  const regex = /^```rust[ \t]*\n([\s\S]*?)^```[ \t]*$/gm;
  let match;
  while ((match = regex.exec(content)) !== null) {
    const lineNum = content.slice(0, match.index).split('\n').length;
    blocks.push({ code: match[1], lineNum });
  }
  return blocks;
}

// Strip rustdoc-style hidden lines ("# " prefix or bare "#").
function stripHiddenLines(code) {
  return code
    .split('\n')
    .map(line => (line === '#' ? '' : line.startsWith('# ') ? line.slice(2) : line))
    .join('\n');
}

// Parse the preamble: the contiguous run of `# `-prefixed lines at the top of
// a block, terminated by the first bare `#` line. Returns deps TOML and a list
// of file paths to stage. Recognized directives:
//   # [dependencies]      — subsequent `# key = val` lines are cargo deps
//   # file: path/to/file  — path is workspace-relative; file is copied into docs-check/ by basename
function parsePreamble(rawCode) {
  const lines = rawCode.split('\n');
  const depLines = [];
  const files = [];
  let inDeps = false;
  let i = 0;

  for (; i < lines.length; i++) {
    const line = lines[i];
    if (line === '#') { i++; break; } // bare # terminates preamble, consume it
    if (!line.startsWith('# ')) break; // visible code, preamble done
    const content = line.slice(2);
    if (content === '[dependencies]') {
      inDeps = true;
    } else if (content.startsWith('file: ')) {
      files.push(content.slice(6).trim());
    } else if (inDeps) {
      depLines.push(content);
    } else {
      break; // unrecognized hidden line before any directive, end preamble
    }
  }

  return { depsToml: depLines.join('\n'), files, code: lines.slice(i).join('\n') };
}

function generateCargoToml(depsToml) {
  return `${STUB_CARGO_TOML}${depsToml}\n`;
}

function main() {
  const mdFiles = findMarkdownFiles(guideDir);
  console.log(`Found ${mdFiles.length} markdown files`);

  let checked = 0;
  let failure = null;
  const stagedFiles = [];

  try {
    outer: for (const mdFile of mdFiles) {
      const content = readFileSync(mdFile, 'utf8');
      const blocks = extractRustBlocks(content);
      if (blocks.length === 0) continue;

      const relFile = relative(repoRoot, mdFile);

      for (let i = 0; i < blocks.length; i++) {
        const { code, lineNum } = blocks[i];
        const { depsToml, files, code: bodyCode } = parsePreamble(code);

        for (const file of files) {
          const dest = join(docsCheckDir, basename(file));
          cpSync(join(repoRoot, file), dest, { recursive: true });
          stagedFiles.push(dest);
        }

        writeFileSync(cargoTomlPath, generateCargoToml(depsToml));
        writeFileSync(mainRsPath, stripHiddenLines(bodyCode));

        const result = spawnSync('cargo', ['check', '--tests', '-q', '-p', 'docs-check'], {
          cwd: repoRoot,
          encoding: 'utf8',
          stdio: ['ignore', 'ignore', 'pipe'],
        });

        checked++;

        if (result.status !== 0) {
          failure = `FAIL: ${relFile} line ${lineNum} (block ${i + 1})\n${result.stderr}`;
          break outer;
        }

        console.log(`ok  ${relFile}:${lineNum}`);
      }
    }
  } finally {
    writeFileSync(cargoTomlPath, STUB_CARGO_TOML);
    writeFileSync(mainRsPath, STUB_MAIN_RS);
    for (const f of stagedFiles) rmSync(f, { recursive: true, force: true });
  }

  if (failure) {
    console.error(failure);
    process.exit(1);
  }

  console.log(`\n${checked} blocks passed.`);
}

main();
