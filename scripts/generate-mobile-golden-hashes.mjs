import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { writeFileSync } from "node:fs";

const commit = process.argv[2];
const output = process.argv[3] ?? "scripts/MOBILE_GOLDEN_HASHES.txt";
const protectedPath = "android-app/UnoOneAgent/";

if (!commit) {
  console.error(
    "Usage: node scripts/generate-mobile-golden-hashes.mjs <commit> [output]",
  );
  process.exit(2);
}

const files = execFileSync(
  "git",
  ["ls-tree", "-r", "--name-only", commit, "--", protectedPath],
  { encoding: "utf8" },
)
  .split(/\r?\n/)
  .filter(Boolean)
  .sort();

if (files.length === 0) {
  throw new Error(`No protected files found at ${commit}:${protectedPath}`);
}

const lines = files.map((path) => {
  const blob = execFileSync("git", ["show", `${commit}:${path}`], {
    encoding: "buffer",
    maxBuffer: 64 * 1024 * 1024,
  });
  const digest = createHash("sha256").update(blob).digest("hex");
  return `${digest}  ${path}`;
});

writeFileSync(output, `${lines.join("\n")}\n`, { encoding: "utf8" });
console.log(`Wrote ${lines.length} canonical Git-blob hashes to ${output}`);
