// split.mjs <file.ts> <new-file.ts> <header-file> name1,name2,...   moves top-level statements (with their comments) by declared name
import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
const [file, out, headerFile, names, rt] = process.argv.slice(2);
const ts = createRequire(rt + "/package.json")("typescript");
const src = readFileSync(file, "utf8");
const sf = ts.createSourceFile(file, src, ts.ScriptTarget.Latest, true);
const want = new Set(names.split(","));
const moved = [];
const kept = [];
let seen = new Set();
for (const st of sf.statements) {
  let n;
  if (ts.isVariableStatement(st)) n = st.declarationList.declarations[0].name.getText(sf);
  else if (st.name) n = st.name.getText(sf);
  const text = src.slice(st.getFullStart(), st.end);
  if (n && want.has(n)) { moved.push(text); seen.add(n); } else kept.push(text);
}
for (const n of want) if (!seen.has(n)) console.error("not found:", n);
writeFileSync(file, kept.join("") + "\n");
writeFileSync(out, readFileSync(headerFile, "utf8") + moved.join("") + "\n");
