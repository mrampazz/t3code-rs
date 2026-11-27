// Generates `assets/file-icons/*.svg` and `src/file_icons.rs`: the colored file icons the web draws
// with `PierreEntryIcon` (a code fence's language or title, `MarkdownCodeBlockTitleContent`), from
// the web's own Pierre resolver tables and sprites.
// Usage (Node 26): node native/t3-gpui/scripts/file-icons.ts
import * as NodeFS from "node:fs";
import * as NodePath from "node:path";

const root = NodePath.resolve(import.meta.dirname, "../../..");
const web = NodePath.join(root, "apps/web");
const pierre = await import(NodePath.join(web, "src/pierre-icons.ts"));
const treesDir = NodePath.dirname(
  NodeFS.realpathSync(NodePath.join(web, "node_modules/@pierre/trees/package.json")),
);
const trees = await import(NodePath.join(treesDir, "dist/index.js"));

// Module-private tables in the web and in @pierre/trees, read from their source.
const objectLiteral = (source: string, name: string): Record<string, string> => {
  const match = new RegExp(`const ${name}(?:: [^=]+)? = (\\{[\\s\\S]*?\\n\\});`).exec(source);
  if (!match) throw new Error(`${name} not found`);
  return Function(`return ${match[1]}`)();
};
const read = (path: string) => NodeFS.readFileSync(path, "utf8");
const builtIns = read(NodePath.join(treesDir, "dist/builtInIcons.js"));
const pierreSource = read(NodePath.join(web, "src/pierre-icons.ts"));
const entryIcon = read(NodePath.join(web, "src/components/chat/PierreEntryIcon.tsx"));
const colors = Object.fromEntries(
  [...entryIcon.matchAll(/^\s+"?([\w-]+)"?: \["(#\w+)", "(#\w+)"\],$/gm)].map((m) => [
    m[1],
    [m[2], m[3]],
  ]),
) as Record<string, [string, string]>;

const config = pierre.T3_PIERRE_ICONS;
if (config.byFileNameContains || config.remap) throw new Error("port byFileNameContains/remap");
const lower = (table: Record<string, string>) =>
  Object.fromEntries(Object.entries(table).map(([k, v]) => [k.toLowerCase(), v]));
const builtInExtensions = {
  ...lower(objectLiteral(builtIns, "BUILT_IN_FILE_EXTENSION_TOKENS")),
  ...lower(objectLiteral(builtIns, "COMPLETE_EXTENSION_OVERRIDES")),
};
const builtInFileNames = lower(objectLiteral(builtIns, "BUILT_IN_FILE_NAME_TOKENS"));

// Every icon a lookup can return: a sprite symbol in its token's colors.
const sprite = `${trees.getBuiltInSpriteSheet("complete")}${config.spriteSheet}`;
const symbol = (name: string) => {
  const match = new RegExp(`<symbol id="${name}" viewBox="([^"]+)">([\\s\\S]*?)</symbol>`).exec(
    sprite,
  );
  if (!match) throw new Error(`symbol ${name} not found`);
  return { viewBox: match[1], body: match[2].trim() };
};
const paint = (body: string, color: string, dark: boolean) =>
  body
    .replace(/light-dark\(\s*(#\w+),\s*(#\w+)\s*\)/g, (_, light, darkColor) =>
      dark ? darkColor : light,
    )
    .replace(/var\(--[\w-]+,\s*(#\w+)\)/g, "$1")
    .replace(/currentcolor/gi, color);

const out = NodePath.join(import.meta.dirname, "../assets/file-icons");
NodeFS.rmSync(out, { recursive: true, force: true });
NodeFS.mkdirSync(out, { recursive: true });
const icons: string[] = [];
const iconIndex = (name: string, token: string | undefined) => {
  const key = token ? `${name}--${token}` : name;
  let index = icons.indexOf(key);
  if (index >= 0) return index;
  const [light, dark] = colors[token ?? "default"] ?? colors.default;
  const { viewBox, body } = symbol(name);
  for (const [suffix, color, isDark] of [
    ["light", light, false],
    ["dark", dark, true],
  ] as const) {
    NodeFS.writeFileSync(
      NodePath.join(out, `${key}-${suffix}.svg`),
      `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${viewBox}" width="16" height="16">${paint(body, color, isDark)}</svg>\n`,
    );
  }
  icons.push(key);
  return icons.length - 1;
};
const builtIn = (token: string) => iconIndex(`file-tree-builtin-${token}`, token);
const DEFAULT = builtIn("default");

const arms = (table: Record<string, number>) =>
  Object.entries(table)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([key, index]) => `        ${JSON.stringify(key)} => ${index},`)
    .join("\n");
const lookup = (name: string, doc: string, table: Record<string, number>) => `/// ${doc}
fn ${name}(key: &str) -> Option<usize> {
    Some(match key {
${arms(table)}
        _ => return None,
    })
}
`;
const map = (table: Record<string, string>, toIndex: (value: string) => number) =>
  Object.fromEntries(Object.entries(table).map(([k, v]) => [k, toIndex(v)]));

const sections = [
  lookup(
    "t3_file_name",
    "`T3_PIERRE_ICONS.byFileName`.",
    map(lower(config.byFileName), (name) => iconIndex(name, undefined)),
  ),
  lookup(
    "t3_extension",
    "`T3_PIERRE_ICONS.byFileExtension`.",
    map(lower(config.byFileExtension), (name) => iconIndex(name, undefined)),
  ),
  lookup("built_in_file_name", "`BUILT_IN_FILE_NAME_TOKENS`.", map(builtInFileNames, builtIn)),
  lookup(
    "built_in_extension",
    "`BUILT_IN_FILE_EXTENSION_TOKENS` with the complete set's overrides.",
    map(builtInExtensions, builtIn),
  ),
];
const languageExtensions = objectLiteral(pierreSource, "LANGUAGE_EXTENSION_ALIASES");
const languageFileNames = objectLiteral(pierreSource, "LANGUAGE_FILE_NAMES");
const strings = (name: string, doc: string, table: Record<string, string>) => `/// ${doc}
fn ${name}(key: &str) -> Option<&'static str> {
    Some(match key {
${Object.entries(table)
  .map(([k, v]) => `        ${JSON.stringify(k)} => ${JSON.stringify(v)},`)
  .join("\n")}
        _ => return None,
    })
}
`;

const rust = `//! Generated by \`scripts/file-icons.ts\`; do not edit. The colored file icons the web draws
//! with \`PierreEntryIcon\`, resolved like \`resolvePierreIconForEntry\`.

/// The icon for \`file_name\`: its SVG in the theme's colors, and whether it is the generic file.
pub fn for_file(file_name: &str, dark: bool) -> (&'static str, bool) {
    let index = resolve(&file_name.rsplit('/').next().unwrap_or(file_name).to_lowercase());
    (svg(index, dark), index == ${DEFAULT})
}

/// \`syntheticFileNameForLanguageId\`: the file name whose icon stands for a fence language.
pub fn file_name_for_language(language: &str) -> String {
    let language = language.to_lowercase();
    if let Some(name) = language_file_name(&language) {
        return name.to_owned();
    }
    format!("file.{}", language_extension(&language).unwrap_or(&language))
}

fn resolve(name: &str) -> usize {
    let extensions: Vec<&str> = name.match_indices('.').map(|(i, _)| &name[i + 1..]).collect();
    t3_file_name(name)
        .or_else(|| extensions.iter().find_map(|e| t3_extension(e)))
        .or_else(|| built_in_file_name(name))
        .or_else(|| extensions.iter().find_map(|e| built_in_extension(e)))
        .unwrap_or(${DEFAULT})
}

fn svg(index: usize, dark: bool) -> &'static str {
    match (index, dark) {
${icons
  .flatMap((key, index) =>
    ["light", "dark"].map(
      (suffix) =>
        `        (${index}, ${suffix === "dark"}) => include_str!("../assets/file-icons/${key}-${suffix}.svg"),`,
    ),
  )
  .join("\n")}
        _ => unreachable!(),
    }
}

${strings("language_extension", "`LANGUAGE_EXTENSION_ALIASES`.", languageExtensions)}
${strings("language_file_name", "`LANGUAGE_FILE_NAMES`.", languageFileNames)}
${sections.join("\n")}`;
NodeFS.writeFileSync(NodePath.join(import.meta.dirname, "../src/file_icons.rs"), rust);
console.log(`${icons.length} icons`);
