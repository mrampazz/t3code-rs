// Generates `src/t3_tools.json`: T3 Code's own MCP tools as the web presents them, read from
// `packages/shared/src/t3McpToolPresentation.ts` (definitions), `client-runtime/src/work-log/
// t3ToolSentences.ts` (row sentences) and `client-runtime/src/t3ToolSummary.ts` (group labels).
// Usage (Node 26): node native/t3-gpui/scripts/t3-tools.ts
import * as NodeFS from "node:fs";
import * as NodePath from "node:path";
import * as NodeModule from "node:module";

const root = NodePath.resolve(import.meta.dirname, "../../..");
const read = (path: string) => NodeFS.readFileSync(NodePath.join(root, path), "utf8");
// TypeScript is only in pnpm's store; take the newest one there.
const store = NodePath.join(root, "node_modules/.pnpm");
const tsDir = NodeFS.readdirSync(store)
  .filter((dir) => dir.startsWith("typescript@"))
  .sort()
  .at(-1);
if (!tsDir) throw new Error("typescript is not installed; run `vp i`");
const require = NodeModule.createRequire(import.meta.filename);
const ts: typeof import("typescript") = require(
  NodePath.join(store, tsDir, "node_modules/typescript"),
);

const presentation = await import(
  NodePath.join(root, "packages/shared/src/t3McpToolPresentation.ts")
);
const tools: Record<string, unknown> = {};
for (const name of [...presentation.T3_MCP_TOOL_NAMES].sort()) {
  const { displayName, labels, icon, summaryAction } =
    presentation.resolveT3McpToolDefinition(name);
  tools[name] = { displayName, labels, icon, summaryAction };
}

const parse = (path: string) => ts.createSourceFile(path, read(path), ts.ScriptTarget.Latest, true);
const msgText = (node: ts.Node | undefined): string | undefined => {
  if (!node) return undefined;
  if (ts.isCallExpression(node) && node.expression.getText() === "msg") {
    const [first] = node.arguments;
    return first && ts.isStringLiteralLike(first) ? first.text : undefined;
  }
  if (ts.isCallExpression(node) && node.expression.getText() === "resolve") {
    return msgText(node.arguments[0]);
  }
  if (ts.isParenthesizedExpression(node)) return msgText(node.expression);
  return undefined;
};

// `[labels, { detail: msg(...), changeRequest: msg(...) }]` entries.
const sentences: Record<string, { detail: string; changeRequest?: string }> = {};
const visitSentences = (node: ts.Node) => {
  if (ts.isArrayLiteralExpression(node) && node.elements.length === 2) {
    const [key, value] = node.elements;
    if (key && ts.isStringLiteralLike(key) && value && ts.isObjectLiteralExpression(value)) {
      const field = (name: string) =>
        value.properties.find(
          (p): p is ts.PropertyAssignment =>
            ts.isPropertyAssignment(p) && p.name.getText() === name,
        )?.initializer;
      const detail = msgText(field("detail"));
      if (detail) {
        const changeRequest = msgText(field("changeRequest"));
        sentences[key.text] = { detail, ...(changeRequest ? { changeRequest } : {}) };
      }
    }
  }
  ts.forEachChild(node, visitSentences);
};
visitSentences(parse("packages/client-runtime/src/work-log/t3ToolSentences.ts"));

// Cases of `summarizeT3ToolCalls` shaped `const count = <counter>; label = done ? A : B;`.
// Others are ported by hand (`t3_tools.rs`) and listed as `special`.
const summaries: Record<string, { count: string; done: string; tried: string }> = {};
const special: string[] = [];
const counter = (expr: string): string | undefined => {
  if (expr === "selected.length") return "calls";
  if (expr === "countEntities(threadIds)") return "threads";
  if (expr === "countEntities(projectIds)") return "projects";
  return /^countEntities\(entityIds\("(\w+)"\)\)$/.exec(expr)?.[1];
};
const visitSummaries = (node: ts.Node) => {
  if (ts.isCaseClause(node) && ts.isStringLiteralLike(node.expression)) {
    const action = node.expression.text;
    const block = node.statements[0];
    const statements = block && ts.isBlock(block) ? block.statements : [];
    const [declare, assign, end] = statements;
    let summary: (typeof summaries)[string] | undefined;
    if (
      statements.length === 3 &&
      declare &&
      ts.isVariableStatement(declare) &&
      assign &&
      ts.isExpressionStatement(assign) &&
      ts.isBinaryExpression(assign.expression) &&
      ts.isConditionalExpression(assign.expression.right) &&
      assign.expression.right.condition.getText() === "done" &&
      end &&
      ts.isBreakStatement(end)
    ) {
      const declaration = declare.declarationList.declarations[0];
      const count = declaration?.initializer && counter(declaration.initializer.getText());
      const done = msgText(assign.expression.right.whenTrue);
      const tried = msgText(assign.expression.right.whenFalse);
      if (count && done && tried) summary = { count, done, tried };
    }
    if (summary) summaries[action] = summary;
    else special.push(action);
    return;
  }
  ts.forEachChild(node, visitSummaries);
};
visitSummaries(parse("packages/client-runtime/src/t3ToolSummary.ts"));

const out = NodePath.join(import.meta.dirname, "../src/t3_tools.json");
NodeFS.writeFileSync(
  out,
  `${JSON.stringify({ tools, sentences, summaries, special: special.sort() }, null, 2)}\n`,
);
console.log(
  `${Object.keys(tools).length} tools, ${Object.keys(sentences).length} sentences, ` +
    `${Object.keys(summaries).length} summaries, special: ${special.join(", ")}`,
);
