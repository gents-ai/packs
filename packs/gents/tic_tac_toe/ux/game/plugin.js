import { Button, Input, NAV_AREA, PLUGIN_PAGE_AREA } from '@gents/ux-sdk'
import { useCallback, useEffect, useRef, useState } from 'react'
import { jsx, jsxs } from 'react/jsx-runtime'

const surface = 'tic-tac-toe-board'
const wins = [[0,1,2],[3,4,5],[6,7,8],[0,3,6],[1,4,7],[2,5,8],[0,4,8],[2,4,6]]

export function gameState(gameId, moves) {
  const ids = new Set(), children = new Map()
  for (const move of moves) {
    if (move.game_id !== gameId || typeof move.move_id !== 'string' || !move.move_id || ids.has(move.move_id) || !['X','O'].includes(move.mark) || !Number.isInteger(move.cell) || move.cell < 0 || move.cell > 8 || (move.parent_move_id != null && typeof move.parent_move_id !== 'string')) return { status: 'invalid' }
    ids.add(move.move_id)
    const parent = move.parent_move_id || ''
    if (!children.has(parent)) children.set(parent, [])
    children.get(parent).push(move)
  }
  const cells = Array(9).fill(null)
  let mark = 'X', parent = '', count = 0, winner = null
  while (children.has(parent)) {
    const next = children.get(parent)
    if (next.length !== 1) return { status: 'conflict' }
    const move = next[0]
    if (winner || mark !== move.mark || cells[move.cell] !== null || count >= 9) return { status: 'invalid' }
    cells[move.cell] = move.mark
    count += 1
    winner = wins.some((line) => line.every((cell) => cells[cell] === mark)) ? mark : null
    parent = move.move_id
    children.delete(move.parent_move_id || '')
    mark = mark === 'X' ? 'O' : 'X'
  }
  if (count !== moves.length) return { status: 'invalid' }
  return { status: winner ? 'won' : count === 9 ? 'draw' : 'playing', cells, next: mark, lastMove: parent, winner }
}

function mintId() {
  if (!globalThis.crypto?.getRandomValues) throw new Error('Secure game identifiers are unavailable.')
  return [...crypto.getRandomValues(new Uint8Array(16))].map((n) => n.toString(16).padStart(2, '0')).join('')
}

export async function readRows(ctx, node, tool, args = {}) {
  if (!ctx.data.supportsExpectedNode) throw new Error('Update Gents to use game data safely when switching runtimes.')
  const rows = []
  let offset = 0
  for (;;) {
    if (ctx.shell.selectedDeployment()?.nodeDid !== node) throw new Error('The selected runtime changed.')
    const result = await ctx.data.call(surface, tool, { ...args, limit: 1000, offset }, node)
    if (ctx.shell.selectedDeployment()?.nodeDid !== node) throw new Error('The selected runtime changed.')
    if (result.kind !== 'query' || !Array.isArray(result.output?.results) || result.output.field_recovery?.length) throw new Error('Game data is incomplete.')
    rows.push(...result.output.results)
    if (result.output.next_offset == null) {
      if (result.output.limit_note) throw new Error('Game data is incomplete.')
      return rows
    }
    if (!Number.isInteger(result.output.next_offset) || result.output.next_offset <= offset || result.output.next_offset > 100000) throw new Error('Game data could not be paged safely.')
    offset = result.output.next_offset
  }
}

function GamePage({ ctx }) {
  const [node, setNode] = useState(() => ctx.shell.selectedDeployment()?.nodeDid ?? '')
  const [games, setGames] = useState([]), [moves, setMoves] = useState([]), [game, setGame] = useState(''), [loadedGame, setLoadedGame] = useState('')
  const [title, setTitle] = useState(''), [player, setPlayer] = useState('X')
  const [error, setError] = useState(''), [busy, setBusy] = useState(false), [pending, setPending] = useState(null)
  const generation = useRef(0), active = useRef(true), refreshing = useRef(false)
  const selected = () => ctx.shell.selectedDeployment()?.nodeDid ?? ''
  const state = game && loadedGame === game ? gameState(game, moves) : { status: 'loading' }
  const refresh = useCallback(async () => {
    if (refreshing.current || !node) return
    refreshing.current = true
    const token = ++generation.current
    try {
      const list = await readRows(ctx, node, 'list_games')
      const current = game ? await readRows(ctx, node, 'list_moves', { game_id: game }) : []
      if (!active.current || token !== generation.current || selected() !== node) return
      setGames(list); setMoves(current); setLoadedGame(game); setError('')
    } catch (e) { if (active.current && token === generation.current) setError(String(e?.message ?? e)) }
    finally { refreshing.current = false }
  }, [ctx, node, game])
  useEffect(() => {
    active.current = true
    void refresh()
    const tick = () => {
      const next = selected()
      if (next !== node) { generation.current++; setNode(next); setGame(''); setLoadedGame(''); setGames([]); setMoves([]); setError('Runtime changed. Select a game on this runtime.') }
      else void refresh()
    }
    const stop = ctx.setInterval(tick, 1000)
    const off = ctx.onEvent('client-updated', tick)
    return () => { active.current = false; generation.current++; stop(); off() }
  }, [ctx, node, refresh])
  const write = async (tool, payload) => {
    if (!ctx.data.supportsExpectedNode) throw new Error('Update Gents before writing game data.')
    if (!node || selected() !== node) throw new Error('The selected runtime changed.')
    const result = await ctx.data.call(surface, tool, payload, node)
    if (selected() !== node) throw new Error('The runtime changed while saving. Return to the original runtime to check the result.')
    if (result.kind !== 'create' || typeof result.output?.document_id !== 'string' || !result.output.document_id) throw new Error('The write could not be confirmed.')
  }
  const newGame = async (event) => {
    event.preventDefault()
    if (!title.trim() || busy || pending) return
    setBusy(true); setError('')
    try {
      const payload = { game_id: mintId(), title: title.trim() }
      setPending({ node, tool: 'create_game', payload })
      await write('create_game', payload)
      setPending(null); setGame(payload.game_id); setTitle(''); await refresh()
    } catch (e) { setError(String(e?.message ?? e)) } finally { if (active.current) setBusy(false) }
  }
  const play = async (cell) => {
    if (!game || busy || pending || state.status !== 'playing' || state.next !== player || state.cells[cell] != null) return
    setBusy(true); setError('')
    try {
      const observed = await readRows(ctx, node, 'list_moves', { game_id: game })
      const current = gameState(game, observed)
      if (current.status !== 'playing' || current.lastMove !== state.lastMove || current.next !== player || current.cells[cell] != null) throw new Error('The game changed. Refresh and choose your move again.')
      const payload = { game_id: game, move_id: mintId(), ...(current.lastMove ? { parent_move_id: current.lastMove } : {}), mark: player, cell }
      setPending({ node, tool: 'create_move', payload })
      await write('create_move', payload)
      setPending(null); await refresh()
    } catch (e) { setError(String(e?.message ?? e)) } finally { if (active.current) setBusy(false) }
  }
  const checkPending = async () => {
    if (!pending || busy) return
    if (node !== pending.node) { setError('Return to the original runtime to check this write.'); return }
    setBusy(true)
    try {
      const { tool, payload } = pending
      const rows = await readRows(ctx, node, tool === 'create_game' ? 'list_games' : 'list_moves', { game_id: payload.game_id })
      const matches = rows.filter((row) => tool === 'create_game' ? row.game_id === payload.game_id : row.move_id === payload.move_id)
      if (matches.length !== 1 || !Object.entries(payload).every(([key, value]) => matches[0][key] === value)) throw new Error('The write is not yet confirmed. Check again later.')
      setPending(null); if (tool === 'create_game') setGame(payload.game_id); await refresh()
    } catch (e) { setError(String(e?.message ?? e)) } finally { if (active.current) setBusy(false) }
  }
  const status = state.status === 'loading' ? 'Loading game…' : state.status === 'won' ? `${state.winner} won` : state.status === 'draw' ? 'Draw' : state.status === 'playing' ? `${state.next} to play` : state.status === 'conflict' ? 'Conflicting moves. Resolve the game before continuing.' : 'Invalid or incomplete game history.'
  return jsxs('div', { className: 'ttt', children: [
    jsxs('aside', { className: 'ttt-sidebar', children: [
      jsx('h2', { children: 'Games' }),
      jsxs('form', { onSubmit: (event) => void newGame(event), className: 'ttt-create', children: [
        jsx(Input, { value: title, onChange: (e) => setTitle(e.target.value), placeholder: 'Game name', 'aria-label': 'Game name' }),
        jsx(Button, { type: 'submit', disabled: !node || busy || Boolean(pending), children: 'New game' }),
      ] }),
      ...games.map((row) => jsx('button', { type:'button', className:'ttt-game', disabled:busy || Boolean(pending), onClick:() => {if (row.game_id !== game) {setGame(row.game_id); setLoadedGame(''); setMoves([])}}, children: row.title || 'Untitled game' }, row.game_id)),
    ] }),
    jsxs('main', { className: 'ttt-main', children: [
      error ? jsx('p', { role:'alert', className:'ttt-error', children:error }) : null,
      pending ? jsx(Button, { disabled:busy, onClick:() => void checkPending(), children:'Check pending write' }) : null,
      !game ? jsx('p', { children: node ? 'Create or choose a game.' : 'Select a runtime to play.' }) : jsxs('section', { className:'ttt-sheet', children: [
        jsx('h1', { children: games.find((row) => row.game_id === game)?.title || 'Tic-tac-toe' }),
        jsx('p', { role:'status', children:status }),
        jsxs('label', { className:'ttt-player', children:['Play as ', jsxs('select', { value:player, onChange:(e) => setPlayer(e.target.value), children:[jsx('option',{value:'X',children:'X'}),jsx('option',{value:'O',children:'O'})] })] }),
        jsx('div', { className:'ttt-board', role:'group', 'aria-label':'Game board', children:Array.from({length:9},(_,i) => jsx('button', { type:'button', className:'ttt-cell', 'aria-label':`Cell ${i+1}: ${state.cells?.[i] || 'empty'}`, disabled:busy || Boolean(pending) || state.status !== 'playing' || state.next !== player || state.cells[i] != null, onClick:() => void play(i), children:state.cells?.[i] ?? '' }, i)) }),
        jsx('p', { className:'ttt-hint', children:'Marks are not verified player identities. Select a chat with an authorized game agent; game and agent access are granted separately.' }),
        jsx(Button, { disabled:busy || Boolean(pending) || state.status !== 'playing', onClick:() => void ctx.send(`Play one legal ${state.next} move in tic-tac-toe game ${game}. Read the complete move history and do not play if it is conflicting or invalid.`).catch((e) => setError(String(e?.message ?? e))), children:'Ask selected agent to play' }),
      ] }),
    ] }),
  ] })
}

export default {
  id:'tic_tac_toe', name:'Tic-tac-toe', description:'Play a shared game.', defaultEnabled:false,
  register(ctx) { ctx.registerMany([
    { id:'nav', area:NAV_AREA, order:60, data:{id:'tic_tac_toe:nav',label:'Tic-tac-toe',icon:'gamepad',to:{name:'plugin',pluginId:'tic_tac_toe'},placement:'footer',order:60} },
    { id:'page', area:PLUGIN_PAGE_AREA, render:() => jsx(GamePage,{ctx}) },
  ]) },
}
