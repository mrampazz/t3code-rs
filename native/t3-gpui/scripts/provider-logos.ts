// Generates `assets/providers/*.svg`, each driver's logo as `ProviderInstanceIcon` draws it:
// Icons.tsx marks rendered with React, and each provider package's `icon`. Theme-colored
// logos get `-light` and `-dark` files. Usage (Node 26): node native/t3-gpui/scripts/provider-logos.ts
import * as NodeFS from "node:fs";
import * as NodeModule from "node:module";
import * as NodeOS from "node:os";
import * as NodePath from "node:path";
import * as NodeURL from "node:url";

const root = NodePath.resolve(import.meta.dirname, "../../..");
const store = NodePath.join(root, "node_modules/.pnpm");
const inStore = (name: string, module = name) => {
  const dir = NodeFS.readdirSync(store)
    .filter((d) => d.startsWith(`${name}@`))
    .sort()
    .at(-1);
  if (!dir) throw new Error(`${name} is not installed; run \`vp i\``);
  return NodePath.join(store, dir, "node_modules", module);
};
const require = NodeModule.createRequire(import.meta.filename);
const ts: typeof import("typescript") = require(inStore("typescript"));
const React = require(inStore("react"));
const ReactDOMServer = require(NodePath.join(inStore("react-dom"), "server"));

const out = NodePath.join(import.meta.dirname, "../assets/providers");
NodeFS.rmSync(out, { recursive: true, force: true });
NodeFS.mkdirSync(out, { recursive: true });
const write = (name: string, svg: string) =>
  NodeFS.writeFileSync(NodePath.join(out, `${name}.svg`), `${svg}\n`);

// Bundled marks: compile Icons.tsx to CommonJS with a stub `cn`, then render each one.
const source = NodeFS.readFileSync(
  NodePath.join(root, "apps/web/src/components/Icons.tsx"),
  "utf8",
).replace(`import { cn } from "~/lib/utils";`, "const cn = (...c) => c.filter(Boolean).join(' ');");
const compiled = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.CommonJS,
    target: ts.ScriptTarget.ES2022,
    jsx: ts.JsxEmit.React,
  },
}).outputText;
const tmp = NodeFS.mkdtempSync(NodePath.join(NodeOS.tmpdir(), "provider-logos-"));
const iconsPath = NodePath.join(tmp, "Icons.cjs");
NodeFS.writeFileSync(
  iconsPath,
  compiled.replace(/require\("react"\)/g, `require(${JSON.stringify(inStore("react"))})`),
);
const icons = require(iconsPath);
NodeFS.rmSync(tmp, { recursive: true, force: true });

const render = (component: unknown, color: string) =>
  ReactDOMServer.renderToStaticMarkup(
    React.createElement(component, { xmlns: "http://www.w3.org/2000/svg", width: 20, height: 20 }),
  )
    // `fill-current` and `currentColor` take the logo's color; gpui has no CSS.
    .replace(/class="[^"]*"/g, "")
    .replaceAll("currentColor", color)
    .replace("<svg ", `<svg fill="${color}" `);

// `PROVIDER_ICON_BY_PROVIDER` and `BUNDLED_LOGO_COLOR_BY_PROVIDER` in ProviderInstanceIcon.tsx.
const bundled: Array<[string, string, string | null]> = [
  ["codex", "OpenAI", null],
  ["claudeAgent", "ClaudeAI", "#d97757"],
  ["antigravity", "AntigravityIcon", "#5b87bf"],
  ["openai", "OpenAI", null],
  ["anthropic", "AnthropicIcon", null],
  ["openrouter", "OpenRouterIcon", null],
  ["ollama", "OllamaIcon", null],
];
for (const [driver, name, color] of bundled) {
  const component = icons[name];
  if (!component) throw new Error(`Icons.tsx has no ${name}`);
  if (color) {
    write(driver, render(component, color));
  } else {
    write(`${driver}-light`, render(component, "#000000"));
    write(`${driver}-dark`, render(component, "#ffffff"));
  }
}

// Package icons: the `icon` literal of each provider package's `defineProviderClient`.
type PackageIcon = {
  viewBox: string;
  fill: { light: string; dark: string };
  paths: Array<{
    d: string;
    fillRule?: string;
    clipRule?: string;
    fill?: { light: string; dark: string };
    gradient?: {
      x1: number;
      y1: number;
      x2: number;
      y2: number;
      stops: Array<{ offset: number | string; color: string }>;
    };
  }>;
};
const packageSvg = (icon: PackageIcon, fill: string, dark: boolean) => {
  const defs = icon.paths
    .map((path, index) =>
      path.gradient
        ? `<linearGradient id="g${index}" x1="${path.gradient.x1}" y1="${path.gradient.y1}" x2="${path.gradient.x2}" y2="${path.gradient.y2}" gradientUnits="userSpaceOnUse">${path.gradient.stops
            .map((stop) => `<stop offset="${stop.offset}" stop-color="${stop.color}"/>`)
            .join("")}</linearGradient>`
        : "",
    )
    .join("");
  const paths = icon.paths
    .map(
      (path, index) =>
        `<path d="${path.d}"${path.fillRule ? ` fill-rule="${path.fillRule}"` : ""}${
          path.clipRule ? ` clip-rule="${path.clipRule}"` : ""
        }${
          path.gradient
            ? ` fill="url(#g${index})"`
            : path.fill
              ? ` fill="${dark ? path.fill.dark : path.fill.light}"`
              : ""
        }/>`,
    )
    .join("");
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${icon.viewBox}" fill="${fill}" width="20" height="20">${
    defs ? `<defs>${defs}</defs>` : ""
  }${paths}</svg>`;
};
const packages = NodeFS.readdirSync(NodePath.join(root, "packages")).filter((d) =>
  d.startsWith("provider-"),
);
const written: string[] = [];
for (const dir of packages) {
  const path = NodePath.join(root, "packages", dir, "src/client.ts");
  if (!NodeFS.existsSync(path)) continue;
  const file = ts.createSourceFile(
    path,
    NodeFS.readFileSync(path, "utf8"),
    ts.ScriptTarget.Latest,
    true,
  );
  let driver: string | undefined;
  let icon: PackageIcon | undefined;
  // Module-level constants an icon literal may name (Kiro's path pieces and colors).
  const constants = file.statements
    .filter(ts.isVariableStatement)
    .filter((statement) => !statement.getText().startsWith("export"))
    .map((statement) => statement.getText())
    .join("\n");
  const visit = (node: ts.Node) => {
    if (ts.isPropertyAssignment(node) && node.name.getText() === "driverKind") {
      driver = /make\("([^"]+)"\)/.exec(node.initializer.getText())?.[1];
    }
    if (
      ts.isPropertyAssignment(node) &&
      node.name.getText() === "icon" &&
      ts.isObjectLiteralExpression(node.initializer)
    ) {
      // A plain data literal in our own source.
      icon = new Function(`${constants}\nreturn (${node.initializer.getText()});`)() as PackageIcon;
    }
    ts.forEachChild(node, visit);
  };
  visit(file);
  if (!driver || !icon) continue;
  const themed =
    icon.fill.light !== icon.fill.dark ||
    icon.paths.some((path) => path.fill && path.fill.light !== path.fill.dark);
  if (themed) {
    write(`${driver}-light`, packageSvg(icon, icon.fill.light, false));
    write(`${driver}-dark`, packageSvg(icon, icon.fill.dark, true));
  } else {
    write(driver, packageSvg(icon, icon.fill.light, false));
  }
  written.push(driver);
}
console.log(
  `bundled: ${bundled.map(([d]) => d).join(", ")}; packages: ${written.join(", ")} -> ${NodeURL.pathToFileURL(out).pathname}`,
);
