import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'

let source = await readFile(new URL('../ux/game/plugin.js', import.meta.url), 'utf8')
for (const [from, to] of [
  ["import { Button, Input, NAV_AREA, PLUGIN_PAGE_AREA } from '@gents/ux-sdk'", 'const Button = null, Input = null, NAV_AREA = null, PLUGIN_PAGE_AREA = null'],
  ["import { useCallback, useEffect, useRef, useState } from 'react'", 'const useCallback = null, useEffect = null, useRef = null, useState = null'],
  ["import { jsx, jsxs } from 'react/jsx-runtime'", 'const jsx = null, jsxs = null'],
]) {
  assert.ok(source.includes(from))
  source = source.replace(from, to)
}
const { gameState, readRows, observeGameQueries } = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`)
const move = (id, parent, mark, cell) => ({game_id:'g', move_id:id, parent_move_id:parent, mark, cell})
const line = (...cells) => cells.map((cell, i) => move(`m${i}`, i ? `m${i-1}` : '', i % 2 ? 'O' : 'X', cell))

test('folds unordered moves into a legal turn, win and draw', () => {
  assert.deepEqual(gameState('g', []), {status:'playing',cells:Array(9).fill(null),next:'X',lastMove:'',winner:null})
  const won = gameState('g', line(0,3,1,4,2).reverse())
  assert.equal(won.status, 'won'); assert.equal(won.winner, 'X'); assert.equal(won.lastMove, 'm4')
  const draw = gameState('g', line(0,1,2,4,3,5,7,6,8))
  assert.equal(draw.status, 'draw'); assert.equal(draw.winner, null)
})

test('refuses duplicate IDs, invalid marks, occupied cells and turns after a win', () => {
  for (const rows of [
    [move('a','','X',0),move('a','a','O',1)],
    [move('a','','Z',0)],
    [move('a','','O',0)],
    [move('a','','X',0),move('b','a','O',0)],
    [...line(0,3,1,4,2),move('extra','m4','O',5)],
    [move('a','','X',9)],
    [{...move('a','','X',0),game_id:'another'}],
  ]) assert.equal(gameState('g', rows).status, 'invalid')
})

test('does not select a winner among concurrent children or ignore orphaned moves', () => {
  assert.equal(gameState('g', [move('a','','X',0),move('b','a','O',1),move('c','a','O',2)]).status, 'conflict')
  assert.equal(gameState('g', [move('a','missing','X',0)]).status, 'invalid')
  assert.equal(gameState('g', [move('a','b','X',0),move('b','a','O',1)]).status, 'invalid')
})

test('requires node binding and follows all pages without changing runtime', async () => {
  let selected = 'a', calls = []
  const ctx = {shell:{selectedDeployment:()=>({nodeDid:selected})},data:{supportsExpectedNode:true,call:async (surface,tool,args,node) => {
    calls.push({surface,tool,args,node})
    return {kind:'query',output:args.offset === 0 ? {results:[{move_id:'a'}],next_offset:1} : {results:[{move_id:'b'}]}}
  }}}
  assert.deepEqual(await readRows(ctx,'a','list_moves',{game_id:'g'}),[{move_id:'a'},{move_id:'b'}])
  assert.deepEqual(calls.map((call)=>call.args.offset),[0,1])
  assert.ok(calls.every((call)=>call.node === 'a'))
  ctx.data.supportsExpectedNode = false
  await assert.rejects(readRows(ctx,'a','list_moves'),/Update Gents/)
  ctx.data.supportsExpectedNode = true
  ctx.data.call = async () => {selected='b';return {kind:'query',output:{results:[]}}}
  await assert.rejects(readRows(ctx,'a','list_moves'),/runtime changed/)
})

test('rejects incomplete pages and stuck cursors', async () => {
  const ctx = {shell:{selectedDeployment:()=>({nodeDid:'a'})},data:{supportsExpectedNode:true,call:async()=>({kind:'query',output:{results:[],next_offset:0}})}}
  await assert.rejects(readRows(ctx,'a','list_moves'),/paged safely/)
  ctx.data.call = async()=>({kind:'query',output:{results:[],field_recovery:[{}]}})
  await assert.rejects(readRows(ctx,'a','list_moves'),/incomplete/)
})

test('reads a playable history in one bounded query and refuses larger histories', async () => {
  const rows = line(0,1,2,4,3,5,7,6,8)
  let calls = 0
  const ctx = {shell:{selectedDeployment:()=>({nodeDid:'a'})},data:{supportsExpectedNode:true,call:async (_surface,_tool,args)=>{
    calls++
    assert.equal(args.limit, 10)
    assert.equal(args.offset, 0)
    return {kind:'query',output:{results:rows}}
  }}}
  assert.equal((await readRows(ctx,'a','list_moves',{game_id:'g'},9)).length,9)
  assert.equal(calls,1)
  ctx.data.call = async()=>({kind:'query',output:{results:[...rows,move('extra','m8','X',0)],next_offset:10}})
  await assert.rejects(readRows(ctx,'a','list_moves',{game_id:'g'},9),/too many moves/)
  ctx.data.call = async()=>({kind:'query',output:{results:[rows[0]],next_offset:1}})
  await assert.rejects(readRows(ctx,'a','list_moves',{game_id:'g'},9),/too many moves/)
})

test('scopes live game watches to the selected node and stops both on switch', async () => {
  let node = 'a', refreshes = 0, ready = 0, failures = 0
  const watches = []
  const ctx = {shell:{selectedDeployment:()=>({nodeDid:node})},data:{watch:async (_surface,tool,args,listener)=>{
    const item = {tool,args,listener,stops:0}; watches.push(item); return () => {item.stops++}
  }}}
  const stop = observeGameQueries(ctx,'a','g',()=>{refreshes++},()=>{ready++},()=>{failures++})
  await new Promise((resolve)=>setTimeout(resolve,0))
  assert.equal(ready,1)
  assert.deepEqual(watches.map(({tool,args})=>({tool,args})),[{tool:'list_games',args:{}},{tool:'list_moves',args:{game_id:'g'}}])
  watches[1].listener({kind:'snapshot',value:{kind:'query',output:{}}})
  assert.equal(refreshes,1)
  node = 'b'
  watches[0].listener({kind:'snapshot',value:{kind:'query',output:{}}})
  assert.equal(refreshes,1)
  stop(); stop()
  assert.deepEqual(watches.map(({stops})=>stops),[1,1])
  assert.equal(failures,0)
})

test('falls back and disposes partial or late-starting watches', async () => {
  let resolveMove, stops = 0, failures = 0
  const ctx = {shell:{selectedDeployment:()=>({nodeDid:'a'})},data:{watch:async (_surface,tool)=>{
    if (tool === 'list_moves') return new Promise((resolve)=>{resolveMove=resolve})
    return () => {stops++}
  }}}
  const stop = observeGameQueries(ctx,'a','g',()=>{},()=>{},()=>{failures++})
  await new Promise((resolve)=>setTimeout(resolve,0))
  stop()
  resolveMove(()=>{stops++})
  await new Promise((resolve)=>setTimeout(resolve,0))
  assert.equal(stops,2)
  const older = {shell:ctx.shell,data:{}}
  observeGameQueries(older,'a','g',()=>{},()=>{},()=>{failures++})
  assert.equal(failures,1)
})

test('returns to fallback when either live query fails', async () => {
  let failure = 0, stops = 0
  const listeners = []
  const ctx = {shell:{selectedDeployment:()=>({nodeDid:'a'})},data:{watch:async (_surface,_tool,_args,listener)=>{
    listeners.push(listener); return () => {stops++}
  }}}
  observeGameQueries(ctx,'a','g',()=>{},()=>{},()=>{failure++})
  await new Promise((resolve)=>setTimeout(resolve,0))
  listeners[0]({kind:'error',message:'unavailable'})
  assert.equal(failure,1)
  assert.equal(stops,2)
  listeners[1]({kind:'error',message:'late'})
  assert.equal(failure,1)
})

test('ignores the old game after switching to another watch', async () => {
  let refreshes = 0
  const listeners = []
  const ctx = {shell:{selectedDeployment:()=>({nodeDid:'a'})},data:{watch:async (_surface,_tool,args,listener)=>{
    listeners.push({args,listener}); return () => {}
  }}}
  const stopOld = observeGameQueries(ctx,'a','old',()=>{refreshes++},()=>{},()=>{})
  await new Promise((resolve)=>setTimeout(resolve,0))
  stopOld()
  const stopNew = observeGameQueries(ctx,'a','new',()=>{refreshes++},()=>{},()=>{})
  await new Promise((resolve)=>setTimeout(resolve,0))
  listeners.find(({args})=>args.game_id==='old').listener({kind:'snapshot',value:{kind:'query',output:{}}})
  assert.equal(refreshes,0)
  listeners.find(({args})=>args.game_id==='new').listener({kind:'snapshot',value:{kind:'query',output:{}}})
  assert.equal(refreshes,1)
  stopNew()
})
