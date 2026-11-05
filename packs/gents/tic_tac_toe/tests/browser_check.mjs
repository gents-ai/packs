import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'
import path from 'node:path'
import http from 'node:http'

const dependencies = process.env.GENTS_DESKTOP_ROOT ?? process.env.TIC_TAC_TOE_TEST_DEPS
if (!dependencies) throw new Error('Set GENTS_DESKTOP_ROOT or TIC_TAC_TOE_TEST_DEPS to a directory with the browser test dependencies.')
const require = createRequire(path.join(dependencies, 'package.json'))
const esbuild = require('esbuild')
const { chromium } = require('playwright')
const pluginPath = fileURLToPath(new URL('../ux/game/plugin.js', import.meta.url))
const entry = `import React from 'react'; import {createRoot} from 'react-dom/client'; import plugin from ${JSON.stringify(pluginPath)};
const games = [], moves = [], sends = []; window.testState = {games, moves, sends, failNext:false};
const ctx = {shell:{selectedDeployment:()=>({nodeDid:'node-a'})}, onEvent:()=>()=>{}, setInterval:(f,n)=>{const id=setInterval(f,n);return ()=>clearInterval(id)}, send:async text=>sends.push(text), data:{supportsExpectedNode:true,call:async (surface,tool,args,node)=>{
if(node!=='node-a')throw Error('node mismatch');
if(tool==='list_games')return {kind:'query',output:{results:games.filter(r=>!args.game_id||r.game_id===args.game_id)}};
if(tool==='list_moves'){if(window.testState.blockGame===args.game_id)await new Promise(resolve=>window.testState.releaseGame=resolve); return {kind:'query',output:{results:moves.filter(r=>r.game_id===args.game_id)}};}
if(tool==='create_game')games.push({...args}); else if(tool==='create_move')moves.push({...args}); else throw Error('unknown tool');
if(window.testState.failNext){window.testState.failNext=false;throw Error('receipt unavailable')}
return {kind:'create',output:{document_id:'physical'}};
}},registerMany(items){createRoot(document.getElementById('root')).render(items.find(item=>item.area==='plugin.pages').render())}};
window.testCtx=ctx; plugin.register(ctx);`
const result = await esbuild.build({
  stdin:{contents:entry,resolveDir:dependencies,loader:'js'}, bundle:true,write:false,format:'iife',nodePaths:[path.join(dependencies,'node_modules')],
  plugins:[{name:'sdk',setup(build){
    build.onResolve({filter:/^@gents\/ux-sdk$/},()=>({path:'sdk',namespace:'stub'}))
    build.onLoad({filter:/.*/,namespace:'stub'},()=>({contents:`import React from 'react'; export const NAV_AREA='nav', PLUGIN_PAGE_AREA='plugin.pages'; export function Button(p){return React.createElement('button',p)} export function Input(p){return React.createElement('input',p)}`,resolveDir:dependencies,loader:'js'}))
  }}],
})
const html = `<!doctype html><html><body><div id="root"></div><script>${result.outputFiles[0].text}</script></body></html>`
const server = http.createServer((_req,res)=>{res.setHeader('Content-Type','text/html');res.end(html)})
await new Promise((resolve)=>server.listen(0,'127.0.0.1',resolve))
const browser = await chromium.launch({headless:true,...(process.env.GENTS_CHROMIUM_PATH?{executablePath:process.env.GENTS_CHROMIUM_PATH}:{})})
try {
  const page = await browser.newPage()
  await page.goto(`http://127.0.0.1:${server.address().port}`)
  await page.getByLabel('Game name').fill('Friendly match')
  await page.getByRole('button',{name:'New game'}).click()
  await page.getByRole('status').getByText('X to play').waitFor()
  await page.getByRole('button',{name:'Cell 1: empty'}).click()
  await page.getByRole('status').getByText('O to play').waitFor()
  assert.equal(await page.getByRole('button',{name:'Cell 1: X'}).isDisabled(),true)
  await page.getByLabel('Play as').selectOption('O')
  await page.evaluate(()=>{window.testState.failNext=true})
  await page.getByRole('button',{name:'Cell 2: empty'}).click()
  await page.getByRole('alert').getByText('receipt unavailable').waitFor()
  assert.equal(await page.evaluate(()=>window.testState.moves.length),2)
  await page.getByRole('button',{name:'Check pending write'}).click()
  await page.getByRole('status').getByText('X to play').waitFor()
  assert.equal(await page.evaluate(()=>window.testState.moves.length),2)
  await page.getByRole('button',{name:'Ask selected agent to play'}).click()
  assert.match((await page.evaluate(()=>window.testState.sends))[0],/Play one legal X move/)
  await page.evaluate(()=>{const s=window.testState; const parent=s.moves[0].move_id; s.moves.push({...s.moves[1],move_id:'conflicting',parent_move_id:parent,cell:2})})
  await page.getByRole('status').getByText(/Conflicting moves/).waitFor({timeout:5000})
  assert.equal(await page.getByRole('button',{name:'Ask selected agent to play'}).isDisabled(),true)
  await page.evaluate(()=>{const s=window.testState; s.games.push({game_id:'other',title:'Other match'}); s.moves.push({game_id:'other',move_id:'first',mark:'X',cell:4}); s.blockGame='other'})
  await page.getByRole('button',{name:'Other match'}).click({timeout:5000})
  await page.getByRole('status').getByText('Loading game…').waitFor()
  assert.equal(await page.getByRole('button',{name:'Cell 1: empty'}).isDisabled(),true)
  assert.equal(await page.getByRole('button',{name:'Ask selected agent to play'}).isDisabled(),true)
  await page.waitForFunction(()=>typeof window.testState.releaseGame==='function')
  await page.evaluate(()=>{window.testState.blockGame=null; window.testState.releaseGame()})
  await page.getByRole('status').getByText('O to play').waitFor()
  console.log('Create, play, uncertain receipt, agent request and conflict checks passed.')
} finally { await browser.close(); server.close() }
