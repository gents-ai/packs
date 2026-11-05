import { createRequire } from "node:module";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import path from "node:path";
import http from "node:http";
const desktopRoot = process.env.GENTS_DESKTOP_ROOT;
if (!desktopRoot)
  throw new Error(
    "Set GENTS_DESKTOP_ROOT to a Gents checkout with desktop test dependencies installed.",
  );
const req = createRequire(path.join(desktopRoot, "package.json"));
const esbuild = req("esbuild");
const { chromium } = req("playwright");
const pluginPath = fileURLToPath(
  new URL("../ux/docs/plugin.js", import.meta.url),
);
const entry = `import React from 'react'; import {createRoot} from 'react-dom/client'; import plugin from ${JSON.stringify(pluginPath)};
const records=[{document_id:'doc',revision_id:'r1',title:'Agreement',content:'Hello **world**'}]; window.testRecords=records;
const ctx={ shell:{ selectedDeployment:()=>({nodeDid:'node-a'}) }, onEvent:()=>()=>{}, setInterval:()=>()=>{}, data:{ supportsExpectedNode:true, call:async (surface,tool,args,expected)=>{
if(expected && expected!=='node-a') throw Error('runtime changed');
if(tool==='list_docs_revisions') return {kind:'query',output:{results:records.filter(r=>!args.document_id||r.document_id===args.document_id).map(r=>Object.fromEntries(args.fields.map(k=>[k,r[k]??null])))}};
if(tool==='create_docs_revision'){ records.push({...args}); return {kind:'create',output:{document_id:'physical'}}; }
throw Error('Unknown tool'); } }, registerMany(items){ const page=items.find(x=>x.area==='plugin.pages'); createRoot(document.getElementById('root')).render(page.render()); } }; window.testCtx=ctx; plugin.register(ctx);`;
const result = await esbuild.build({
  stdin: { contents: entry, resolveDir: desktopRoot, loader: "js" },
  bundle: true,
  write: false,
  format: "iife",
  nodePaths: [path.join(desktopRoot, "node_modules")],
  plugins: [
    {
      name: "sdk",
      setup(b) {
        b.onResolve({ filter: /^@gents\/ux-sdk$/ }, () => ({
          path: "sdk",
          namespace: "stub",
        }));
        b.onLoad({ filter: /.*/, namespace: "stub" }, () => ({
          contents: `import React from 'react'; export const NAV_AREA='nav', PLUGIN_PAGE_AREA='plugin.pages'; export function Button(p){return React.createElement('button',p)} export function Input(p){return React.createElement('input',p)}`,
          resolveDir: desktopRoot,
          loader: "js",
        }));
      },
    },
  ],
});
const html = `<!doctype html><html><body><div id="root"></div><script>${result.outputFiles[0].text}</script></body></html>`;
const server = http.createServer((_req, res) => {
  res.setHeader("Content-Type", "text/html");
  res.end(html);
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const browser = await chromium.launch({
  headless: true,
  ...(process.env.GENTS_CHROMIUM_PATH
    ? { executablePath: process.env.GENTS_CHROMIUM_PATH }
    : {}),
});
try {
  const page = await browser.newPage();
  page.on("pageerror", (e) => console.error("PAGE ERROR", e.message));
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  await page.getByRole("button", { name: "Open", exact: true }).click();
  let editor = page.getByRole("textbox", { name: "Document content" });
  await editor.waitFor();
  assert.equal(await editor.innerText(), "Hello world");
  await editor.click();
  await page.keyboard.press("End");
  await page.keyboard.type(" revised");
  await page.getByRole("button", { name: "Save revision" }).click();
  await page.waitForFunction(() => window.testRecords.length === 2);
  assert.equal(
    (await page.evaluate(() => window.testRecords[1])).content,
    "Hello **world revised**",
  );
  await page.getByRole("button", { name: "Markdown source" }).click();
  assert.equal(
    await page.getByRole("textbox", { name: "Markdown source" }).inputValue(),
    "Hello **world revised**",
  );
  await page
    .getByRole("textbox", { name: "Markdown source" })
    .fill("| Heading |\n| --- |\n| Cell |");
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  assert.equal(
    await page.getByRole("textbox", { name: "Markdown source" }).inputValue(),
    "| Heading |\n| --- |\n| Cell |",
  );
  assert.equal(await page.getByRole("button", { name: "Rich text" }).isDisabled(), true);
  await page.evaluate(() =>
    window.testRecords.push({
      document_id: "doc",
      revision_id: "other",
      parent_revision_id: window.testRecords[1].revision_id,
      title: "Agreement",
      content: "Other edit",
    }),
  );
  await page.getByRole("button", { name: "Save revision" }).click();
  await page
    .getByRole("alert")
    .getByText(/changed or has conflicting revisions/)
    .waitFor();
  assert.equal(
    await page.getByRole("textbox", { name: "Markdown source" }).inputValue(),
    "| Heading |\n| --- |\n| Cell |",
  );
  assert.equal(await page.evaluate(() => window.testRecords.length), 3);
  const formatPage = await browser.newPage();
  await formatPage.goto(`http://127.0.0.1:${server.address().port}`);
  await formatPage.getByRole("button", { name: "Open", exact: true }).click();
  const formatEditor = formatPage.getByRole("textbox", { name: "Document content" });
  await formatEditor.waitFor();
  await formatEditor.focus();
  await formatPage.evaluate(() => {
    const editor = document.querySelector('[aria-label="Document content"]');
    const text = editor.querySelector('p').firstChild;
    const range = document.createRange();
    range.setStart(text, 0);
    range.setEnd(text, 5);
    const selection = window.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
  });
  await formatPage.getByRole("button", { name: "Bold", exact: true }).click();
  await formatPage.getByRole("button", { name: "Save revision" }).click();
  await formatPage.waitForFunction(() => window.testRecords.length === 2);
  assert.equal((await formatPage.evaluate(() => window.testRecords[1])).content, "**Hello** **world**");
  await formatPage.close();
  await page.evaluate(() => {
    window.testCtx.data.supportsExpectedNode = false;
  });
  await page.getByRole("button", { name: "Save revision" }).click();
  await page
    .getByRole("alert")
    .getByText(/Update Gents to save documents safely/)
    .waitFor();
  assert.equal(await page.evaluate(() => window.testRecords.length), 3);
  console.log(
    "Rich editor save, source, conflict and runtime-binding checks passed.",
  );
} finally {
  await browser.close();
  server.close();
}
