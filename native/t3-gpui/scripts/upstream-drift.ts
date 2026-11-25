// Lists upstream changes that may affect the Rust client: wire additions and removals in
// packages/contracts, and changed web/client-runtime files grouped by area.
// Usage: node native/t3-gpui/scripts/upstream-drift.ts [base] [head]
// Defaults: base = merge-base of HEAD and upstream/main, head = upstream/main.
import * as NodeChildProcess from "node:child_process";

const git = (...args: string[]) =>
  NodeChildProcess.execFileSync("git", args, { encoding: "utf8", maxBuffer: 64 << 20 });
const show = (rev: string, path: string) => {
  try {
    return git("show", `${rev}:${path}`);
  } catch {
    return "";
  }
};

const head = process.argv[3] ?? "upstream/main";
const base = process.argv[2] ?? git("merge-base", "HEAD", head).trim();

/** Values of one regex group across a file. */
const matches = (text: string, pattern: RegExp) =>
  new Set([...text.matchAll(pattern)].map((m) => m[1]!));

/** The block of an `export const <name> = Schema.Struct({` up to its closing `});`. */
const block = (text: string, name: string) => {
  const start = text.indexOf(`export const ${name} = `);
  if (start < 0) return "";
  const end = text.indexOf("\n});", start);
  return text.slice(start, end < 0 ? undefined : end);
};

const wire: Array<[string, string, (text: string) => Set<string>]> = [
  [
    "RPC methods",
    "packages/contracts/src/rpc.ts",
    (t) => matches(t, /^\s+\w+: "([a-zA-Z]+\.[a-zA-Z.]+)",$/gm),
  ],
  [
    "Orchestration RPC methods",
    "packages/contracts/src/orchestrationV2.ts",
    (t) => matches(t, /^\s+\w+: "(orchestration\.[a-zA-Z]+)",$/gm),
  ],
  [
    "Turn item, command and event types",
    "packages/contracts/src/orchestrationV2.ts",
    (t) => matches(t, /type: Schema\.Literal\("([a-z_.-]+)"\)/g),
  ],
  [
    "Server settings keys",
    "packages/contracts/src/settings.ts",
    (t) => matches(block(t, "ServerSettings"), /^\s{2}(\w+):/gm),
  ],
  [
    "Server config fields",
    "packages/contracts/src/server.ts",
    (t) => matches(block(t, "ServerConfig"), /^\s{2}(\w+):/gm),
  ],
  [
    "Environment capabilities",
    "packages/contracts/src/environment.ts",
    (t) => matches(block(t, "ExecutionEnvironmentCapabilities"), /^\s{2}(\w+):/gm),
  ],
];

console.log(`# Upstream drift ${base.slice(0, 10)}..${head}\n`);
for (const [title, path, extract] of wire) {
  const before = extract(show(base, path));
  const after = extract(show(head, path));
  const added = [...after].filter((x) => !before.has(x)).sort();
  const removed = [...before].filter((x) => !after.has(x)).sort();
  if (added.length === 0 && removed.length === 0) continue;
  console.log(`## ${title}`);
  for (const x of added) console.log(`+ ${x}`);
  for (const x of removed) console.log(`- ${x}`);
  console.log();
}

// Changed client files, grouped by their first folder under the surface they belong to.
const changed = git(
  "diff",
  "--name-only",
  base,
  head,
  "--",
  "apps/web/src",
  "packages/client-runtime/src",
  "packages/contracts/src",
)
  .split("\n")
  .filter((f) => f && !/\.test\.tsx?$|\.browser\.test\./.test(f));
const areas = new Map<string, string[]>();
for (const file of changed) {
  const parts = file.split("/");
  const area = file.startsWith("apps/web/src/components/")
    ? `web/${parts[4]?.includes(".") ? "components" : parts[4]}`
    : file.startsWith("apps/web/src/")
      ? "web"
      : file.startsWith("packages/client-runtime/")
        ? `client-runtime/${parts[3]?.includes(".") ? "" : parts[3]}`
        : "contracts";
  areas.set(area, [...(areas.get(area) ?? []), file]);
}
console.log("## Changed client files by area");
for (const [area, files] of [...areas].sort((a, b) => b[1].length - a[1].length)) {
  console.log(`- ${area}: ${files.length}`);
}
