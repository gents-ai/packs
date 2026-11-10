import { Button, Input, NAV_AREA, PLUGIN_PAGE_AREA } from '@gents/ux-sdk'
import { useCallback, useEffect, useRef, useState } from 'react'
import { jsx, jsxs } from 'react/jsx-runtime'

const SURFACE = 'docs-editor'
const META = ['document_id', 'revision_id', 'parent_revision_id', 'title']
const ALL = [...META, 'content']
const escapeText = (text) => text.replace(/[\\*_`\[\]<>]/g, '\\$&')

function inlineMarkdown(nodes) {
  return nodes.map((node) => {
    if (node.type === 'text') return escapeText(node.text)
    const marker = node.type === 'strong' ? '**' : '*'
    return marker + inlineMarkdown(node.children) + marker
  }).join('')
}

function parseInline(text) {
  const nodes = []
  let plain = ''
  const flush = () => { if (plain) nodes.push({ type: 'text', text: plain }); plain = '' }
  for (let i = 0; i < text.length;) {
    const ch = text[i]
    if (ch === '\\') {
      if (i + 1 >= text.length || !'\\*_`[]<>'.includes(text[i + 1])) return null
      plain += text[i + 1]; i += 2; continue
    }
    if ('_`[]<>'.includes(ch)) return null
    if (ch === '*') {
      const marker = text[i + 1] === '*' ? '**' : '*'
      const end = text.indexOf(marker, i + marker.length)
      if (end < 0 || end === i + marker.length) return null
      const inner = parseInline(text.slice(i + marker.length, end))
      if (!inner) return null
      flush(); nodes.push({ type: marker === '**' ? 'strong' : 'em', children: inner }); i = end + marker.length
      continue
    }
    plain += ch; i++
  }
  flush()
  return nodes
}

export function blocksToMarkdown(blocks) {
  return blocks.map((block) => {
    if (block.type === 'heading') return '#'.repeat(block.level) + ' ' + inlineMarkdown(block.children)
    if (block.type === 'list') return block.items.map((item, i) => (block.ordered ? `${i + 1}. ` : '- ') + inlineMarkdown(item)).join('\n')
    return inlineMarkdown(block.children)
  }).join('\n\n')
}

export function markdownToBlocks(markdown) {
  if (typeof markdown !== 'string' || markdown.includes('\r')) return null
  if (markdown === '') return []
  const blocks = []
  for (const part of markdown.split('\n\n')) {
    if (!part || part.includes('\n\n')) return null
    const lines = part.split('\n')
    const heading = lines.length === 1 && /^(#{1,3}) (.+)$/.exec(part)
    if (heading) {
      const children = parseInline(heading[2]); if (!children) return null
      blocks.push({ type: 'heading', level: heading[1].length, children }); continue
    }
    if (lines.every((line) => /^- .+/.test(line))) {
      const items = lines.map((line) => parseInline(line.slice(2)))
      if (items.some((item) => !item)) return null
      blocks.push({ type: 'list', ordered: false, items }); continue
    }
    if (lines.every((line, i) => line.startsWith(`${i + 1}. `))) {
      const items = lines.map((line, i) => parseInline(line.slice(`${i + 1}. `.length)))
      if (items.some((item) => !item)) return null
      blocks.push({ type: 'list', ordered: true, items }); continue
    }
    if (lines.length !== 1 || /^\s|\s$/.test(part) || /^(#|>|-|\d+\.) /.test(part) || /^([-*_])(?:\s*\1){2,}\s*$/.test(part) || /^~~~/.test(part)) return null
    const children = parseInline(part); if (!children) return null
    blocks.push({ type: 'paragraph', children })
  }
  return blocksToMarkdown(blocks) === markdown ? blocks : null
}

function inlineNodes(container) {
  const nodes = []
  for (const child of container.childNodes) {
    if (child.nodeType === 3) { nodes.push({ type: 'text', text: child.nodeValue ?? '' }); continue }
    if (child.nodeType !== 1) return null
    const tag = child.tagName.toLowerCase()
    if (tag === 'br' && container.childNodes.length === 1) continue
    if (!['strong', 'b', 'em', 'i'].includes(tag)) return null
    const children = inlineNodes(child); if (!children || children.length === 0) return null
    nodes.push({ type: tag === 'strong' || tag === 'b' ? 'strong' : 'em', children })
  }
  return nodes
}

export function domToMarkdown(root) {
  const blocks = []
  for (const element of root.childNodes) {
    if (element.nodeType === 3 && element.nodeValue?.trim()) {
      blocks.push({ type: 'paragraph', children: [{ type: 'text', text: element.nodeValue }] }); continue
    }
    if (element.nodeType !== 1) return null
    const tag = element.tagName.toLowerCase()
    if (tag === 'ul' || tag === 'ol') {
      const items = []
      for (const item of element.children) {
        if (item.tagName.toLowerCase() !== 'li') return null
        const children = inlineNodes(item); if (!children?.length) return null
        items.push(children)
      }
      if (!items.length) return null
      blocks.push({ type: 'list', ordered: tag === 'ol', items }); continue
    }
    if (!['p', 'div', 'h1', 'h2', 'h3'].includes(tag)) return null
    const children = inlineNodes(element); if (!children) return null
    blocks.push(tag[0] === 'h' ? { type: 'heading', level: Number(tag[1]), children } : { type: 'paragraph', children })
  }
  if (blocks.length === 1 && blocks[0].type === 'paragraph' && blocks[0].children.length === 0) return ''
  const markdown = blocksToMarkdown(blocks)
  const parsed = markdownToBlocks(markdown)
  const normalize = (nodes) => nodes.reduce((result, node) => {
    if (node.type === 'text' && result.at(-1)?.type === 'text') result.at(-1).text += node.text
    else if (node.type === 'text') result.push({ ...node })
    else result.push({ ...node, children: normalize(node.children) })
    return result
  }, [])
  const canonical = (items) => items.map((block) => block.type === 'list' ? { ...block, items: block.items.map(normalize) } : { ...block, children: normalize(block.children) })
  return parsed && JSON.stringify(canonical(parsed)) === JSON.stringify(canonical(blocks)) ? markdown : null
}

function makeInline(parent, children) {
  for (const child of children) {
    if (child.type === 'text') parent.appendChild(document.createTextNode(child.text))
    else {
      const element = document.createElement(child.type)
      makeInline(element, child.children)
      parent.appendChild(element)
    }
  }
}

function paint(editor, blocks) {
  editor.replaceChildren()
  for (const block of blocks) {
    const element = document.createElement(block.type === 'heading' ? `h${block.level}` : block.type === 'list' ? (block.ordered ? 'ol' : 'ul') : 'p')
    if (block.type === 'list') for (const item of block.items) { const li = document.createElement('li'); makeInline(li, item); element.appendChild(li) }
    else makeInline(element, block.children)
    editor.appendChild(element)
  }
}

export function revisionHeads(rows) {
  const ids = new Map()
  for (const row of rows) {
    if (!row.revision_id || ids.has(row.revision_id)) return { heads: [], invalid: true }
    ids.set(row.revision_id, row)
  }
  const parents = new Set()
  for (const row of rows) {
    const parent = row.parent_revision_id
    if (!parent) continue
    if (!ids.has(parent) || parent === row.revision_id) return { heads: [], invalid: true }
    parents.add(parent)
    const seen = new Set([row.revision_id])
    let next = parent
    while (next) {
      if (seen.has(next)) return { heads: [], invalid: true }
      seen.add(next)
      next = ids.get(next)?.parent_revision_id
    }
  }
  return { heads: rows.filter((row) => !parents.has(row.revision_id)), invalid: false }
}

export async function queryRows(ctx, args) {
  const rows = []
  let offset = 0
  if (!ctx.data.supportsExpectedNode) throw new Error('Update Gents to open documents safely when switching runtimes.')
  const node = ctx.shell.selectedDeployment()?.nodeDid
  if (!node) throw new Error('Select a runtime to open documents.')
  for (;;) {
    const result = await ctx.data.call(SURFACE, 'list_docs_revisions', { ...args, offset, limit: 1000 }, node)
    if (ctx.shell.selectedDeployment()?.nodeDid !== node) throw new Error('The selected runtime changed while loading documents.')
    if (result.kind !== 'query' || !Array.isArray(result.output?.results)) throw new Error('Document results are unavailable.')
    const out = result.output
    for (const recovery of out.field_recovery ?? []) {
      if (!Number.isInteger(recovery.row) || !out.results[recovery.row] || !args.fields.includes(recovery.field) || !recovery.doc_id || !Number.isInteger(recovery.total_bytes) || recovery.total_bytes > 8 * 1024 * 1024) throw new Error('A document field cannot be recovered safely.')
      let position = 0
      let hash
      let value = ''
      for (;;) {
        const field_page = { doc_id: recovery.doc_id, field: recovery.field, offset_bytes: position, ...(hash ? { expected_hash: hash } : {}) }
        const response = await ctx.data.call(SURFACE, 'list_docs_revisions', { ...args, field_page }, node)
        if (ctx.shell.selectedDeployment()?.nodeDid !== node) throw new Error('The selected runtime changed while loading documents.')
        const page = response.output?.field_page
        if (response.kind !== 'query' || !page || page.doc_id !== recovery.doc_id || page.field !== recovery.field || page.offset_bytes !== position || typeof page.text !== 'string' || typeof page.value_hash !== 'string' || (hash && hash !== page.value_hash) || page.total_bytes !== recovery.total_bytes || !Number.isInteger(page.next_offset_bytes) || page.next_offset_bytes < position || (page.next_offset_bytes === position && !page.complete)) throw new Error('A document field changed or could not be recovered safely.')
        value += page.text
        hash = page.value_hash
        position = page.next_offset_bytes
        if (page.complete) {
          if (position !== recovery.total_bytes || new TextEncoder().encode(value).length !== position) throw new Error('A document field was incomplete.')
          out.results[recovery.row][recovery.field] = value
          break
        }
        if (position > recovery.total_bytes) throw new Error('A document field could not be recovered safely.')
      }
    }
    rows.push(...out.results)
    if (out.next_offset == null) {
      if (out.limit_note) throw new Error('Document results are incomplete. Narrow the query.')
      return rows
    }
    if (!Number.isInteger(out.next_offset) || out.next_offset <= offset || out.next_offset > 100000) throw new Error('Document results could not be paged safely.')
    offset = out.next_offset
  }
}

function mintId() {
  if (!globalThis.crypto?.randomUUID) throw new Error('Secure document identifiers are unavailable.')
  return globalThis.crypto.randomUUID()
}

function RichEditor({ value, onChange, onInvalid }) {
  const editor = useRef(null)
  const initial = useRef(value)
  useEffect(() => { const blocks = markdownToBlocks(initial.current); if (editor.current && blocks) paint(editor.current, blocks) }, [])
  const command = (name, value) => {
    editor.current?.focus()
    document.execCommand(name, false, value)
    const next = domToMarkdown(editor.current)
    if (next === null) onInvalid(true)
    else { onInvalid(false); onChange(next) }
  }
  const tool = (label, name, argument) => jsx('button', {
    type: 'button', className: 'docs-tool', title: label, 'aria-label': label,
    onMouseDown: (event) => event.preventDefault(), onClick: () => command(name, argument), children: label,
  }, label)
  return jsxs('div', { className: 'docs-rich', children: [
    jsxs('div', { role: 'toolbar', 'aria-label': 'Text formatting', className: 'docs-toolbar', children: [
      tool('Bold', 'bold'), tool('Italic', 'italic'), tool('Heading', 'formatBlock', 'h2'),
      tool('Bullet list', 'insertUnorderedList'), tool('Numbered list', 'insertOrderedList'),
    ] }),
    jsx('div', {
      ref: editor, className: 'docs-editor', contentEditable: true, suppressContentEditableWarning: true,
      role: 'textbox', 'aria-label': 'Document content', 'aria-multiline': true,
      onPaste: (event) => { event.preventDefault(); document.execCommand('insertText', false, event.clipboardData.getData('text/plain')) },
      onInput: () => { const next = domToMarkdown(editor.current); if (next === null) onInvalid(true); else { onInvalid(false); onChange(next) } },
    }),
  ] })
}

function DocsPage({ ctx }) {
  const [node, setNode] = useState(() => ctx.shell.selectedDeployment()?.nodeDid ?? '')
  const [records, setRecords] = useState([])
  const [drafts, setDrafts] = useState({})
  const [active, setActive] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [pendingSaves, setPendingSaves] = useState({})
  const [invalid, setInvalid] = useState(false)
  const alive = useRef(true)
  const generation = useRef(0)
  const selectedNode = () => ctx.shell.selectedDeployment()?.nodeDid ?? ''
  const draft = drafts[active]
  const pending = pendingSaves[active]
  const setPending = (value) => setPendingSaves((prior) => {
    const next = { ...prior }
    if (value) next[active] = value
    else delete next[active]
    return next
  })
  const update = (patch) => setDrafts((prior) => ({ ...prior, [active]: { ...prior[active], ...patch } }))
  const refresh = useCallback(async () => {
    const mine = ++generation.current
    try {
      const next = await queryRows(ctx, { fields: META })
      if (!alive.current || mine !== generation.current || selectedNode() !== node) return
      setRecords(next); setError('')
    } catch (e) { if (alive.current && mine === generation.current) setError(String(e?.message ?? e)) }
  }, [ctx, node])
  useEffect(() => {
    alive.current = true
    if (node) void refresh()
    const observeNode = () => {
      const current = selectedNode()
      if (current !== node) { generation.current++; setNode(current); setActive(''); setRecords([]); setError('The selected runtime changed. Drafts remain in the previous runtime until you return to it.') }
      return current === node
    }
    const stop = ctx.onEvent('client-updated', () => { if (observeNode()) void refresh() })
    const stopChecking = ctx.setInterval(observeNode, 1000)
    return () => { alive.current = false; generation.current++; stop(); stopChecking() }
  }, [ctx, node, refresh])
  const groups = new Map()
  for (const record of records) {
    if (!record.document_id) continue
    if (!groups.has(record.document_id)) groups.set(record.document_id, [])
    groups.get(record.document_id).push(record)
  }
  const start = () => {
    if (invalid || busy || pending) { setError('Finish the current edit or confirm its save first.'); return }
    try {
      const id = mintId()
      setDrafts((prior) => ({ ...prior, [node + ':' + id]: { node, documentId: id, revisionId: '', title: 'Untitled', content: '', mode: 'rich', loading: false } }))
      setActive(node + ':' + id); setInvalid(false); setError('')
    } catch (e) { setError(String(e?.message ?? e)) }
  }
  const open = async (documentId, revisionId) => {
    if (invalid || busy || pending) { setError('Finish the current edit or confirm its save first.'); return }
    const key = node + ':' + documentId
    if (drafts[key]?.revisionId === revisionId) { setActive(key); setInvalid(false); return }
    if (drafts[key]?.dirty) { setError('Save or keep your current draft before opening another revision.'); return }
    setBusy(true); setError('')
    try {
      const rows = await queryRows(ctx, { document_id: documentId, fields: ALL })
      if (!alive.current || selectedNode() !== node) return
      const matches = rows.filter((row) => row.revision_id === revisionId)
      if (matches.length !== 1 || typeof matches[0].content !== 'string') throw new Error('The selected revision is missing, incomplete, or ambiguous.')
      const row = matches[0]
      setDrafts((prior) => ({ ...prior, [key]: { node, documentId, revisionId, title: row.title ?? '', content: row.content, mode: markdownToBlocks(row.content) ? 'rich' : 'source', dirty: false } }))
      setActive(key); setInvalid(false)
    } catch (e) { setError(String(e?.message ?? e)) } finally { if (alive.current) setBusy(false) }
  }
  const save = async () => {
    if (!draft || invalid || busy) return
    setBusy(true); setError('')
    try {
      if (!ctx.data.supportsExpectedNode) throw new Error('Update Gents to save documents safely when switching runtimes.')
      if (selectedNode() !== draft.node) throw new Error('The selected runtime changed. Reopen the draft before saving.')
      if (pending) {
        const observed = await queryRows(ctx, { document_id: pending.document_id, fields: ALL })
        const matches = observed.filter((row) => row.revision_id === pending.revision_id)
        if (matches.length !== 1 || matches[0].content !== pending.content || matches[0].title !== pending.title || (matches[0].parent_revision_id ?? '') !== (pending.parent_revision_id ?? '')) throw new Error('The save has not been confirmed. Your draft is retained; check again later.')
        setPending(null)
        setDrafts((prior) => ({ ...prior, [active]: { ...prior[active], revisionId: pending.revision_id, dirty: prior[active].content !== pending.content || prior[active].title !== pending.title } }))
        await refresh(); return
      }
      const current = await queryRows(ctx, { document_id: draft.documentId, fields: META })
      if (selectedNode() !== draft.node) throw new Error('The selected runtime changed. Reopen the draft before saving.')
      const state = revisionHeads(current)
      if (state.invalid || state.heads.length > 1 || (draft.revisionId && (state.heads.length !== 1 || state.heads[0].revision_id !== draft.revisionId)) || (!draft.revisionId && current.length)) throw new Error('This document has changed or has conflicting revisions. Review its history before saving.')
      if (!draft.title.trim()) throw new Error('Enter a document title before saving.')
      const payload = { document_id: draft.documentId, revision_id: mintId(), ...(draft.revisionId ? { parent_revision_id: draft.revisionId } : {}), title: draft.title, content: draft.content }
      setPending(payload)
      const receipt = await ctx.data.call(SURFACE, 'create_docs_revision', payload, draft.node)
      if (selectedNode() !== draft.node) throw new Error('The runtime changed while saving. Check the original runtime for the result.')
      if (receipt.kind !== 'create' || typeof receipt.output?.document_id !== 'string' || !receipt.output.document_id) throw new Error('The save has not been confirmed. Check its status before trying again.')
      setPending(null)
      setDrafts((prior) => ({ ...prior, [active]: { ...prior[active], revisionId: payload.revision_id, dirty: prior[active].content !== payload.content || prior[active].title !== payload.title } }))
      await refresh()
    } catch (e) { if (alive.current) setError(String(e?.message ?? e)) } finally { if (alive.current) setBusy(false) }
  }
  return jsxs('div', { className: 'gents-docs', children: [
    jsxs('aside', { className: 'docs-sidebar', children: [
      jsxs('div', { className: 'docs-sidebar-header', children: [jsx('h2', { children: 'Documents' }), jsx(Button, { onClick: start, disabled: !node || busy || invalid || Boolean(pending), children: 'New' })] }),
      jsx('button', { className: 'docs-refresh', type: 'button', onClick: () => void refresh(), disabled: !node, children: 'Refresh' }),
      ...Object.entries(drafts).filter(([, item]) => item.node === node && (item.dirty || !item.revisionId)).map(([key, item]) => jsx('button', { className: 'docs-refresh', type: 'button', disabled: busy || invalid || Boolean(pending), onClick: () => { setActive(key); setInvalid(false) }, children: `Draft: ${item.title || 'Untitled'}` }, key)),
      ...Array.from(groups.entries()).map(([id, rows]) => {
        const state = revisionHeads(rows)
        const title = rows[0]?.title || 'Untitled'
        return jsxs('div', { className: 'docs-document', children: [
          jsx('strong', { children: title }),
          state.invalid ? jsx('small', { children: 'History needs review' }) : state.heads.length > 1 ? jsx('small', { children: 'Conflicting revisions' }) : null,
          ...state.heads.map((row) => jsx('button', { type: 'button', onClick: () => void open(id, row.revision_id), disabled: busy || invalid || Boolean(pending), children: state.heads.length > 1 ? `Open revision ${row.revision_id.slice(0, 8)}` : 'Open' }, row.revision_id)),
        ] }, id)
      }),
      !node ? jsx('p', { children: 'Select a runtime to open documents.' }) : null,
    ] }),
    jsxs('main', { className: 'docs-main', children: [
      error ? jsx('div', { role: 'alert', className: 'docs-error', children: error }) : null,
      draft ? jsxs('div', { className: 'docs-sheet', children: [
        jsxs('div', { className: 'docs-heading', children: [
          jsx(Input, { value: draft.title, 'aria-label': 'Document title', onChange: (event) => update({ title: event.target.value, dirty: true }) }),
          jsx(Button, { onClick: () => void save(), disabled: busy || invalid, children: pending ? 'Check save' : busy ? 'Saving…' : 'Save revision' }),
        ] }),
        jsxs('div', { className: 'docs-mode', children: [
          jsx('span', { children: draft.revisionId ? 'Editing a saved revision' : 'New document' }),
          jsx('button', { type: 'button', onClick: () => { if (invalid) { setError('Undo unsupported formatting before changing views.'); return } update({ mode: draft.mode === 'rich' ? 'source' : 'rich' }) }, disabled: draft.mode === 'source' && !markdownToBlocks(draft.content), children: draft.mode === 'rich' ? 'Markdown source' : 'Rich text' }),
        ] }),
        draft.mode === 'rich' ? jsx(RichEditor, { key: active + ':' + draft.revisionId, value: draft.content, onChange: (content) => update({ content, dirty: true }), onInvalid: setInvalid }) : jsx('textarea', { className: 'docs-source', 'aria-label': 'Markdown source', value: draft.content, onChange: (event) => update({ content: event.target.value, dirty: true }) }),
        invalid ? jsx('p', { className: 'docs-error', role: 'alert', children: 'This formatting cannot be saved safely. Undo it or use plain text.' }) : null,
        draft.mode === 'source' && !markdownToBlocks(draft.content) ? jsx('p', { className: 'docs-hint', children: 'This document uses Markdown formatting that the rich editor cannot yet preserve. Edit it in source.' }) : null,
      ] }) : jsx('div', { className: 'docs-empty', children: node ? 'Select a document or create a new one.' : 'Connect a runtime to work with documents.' }),
    ] }),
  ] })
}

export default {
  id: 'docs', name: 'Docs', description: 'Edit and review document revisions.', defaultEnabled: false,
  register(ctx) {
    ctx.registerMany([
      { id: 'nav', area: NAV_AREA, order: 60, data: { id: 'docs:nav', label: 'Docs', icon: 'file-text', to: { name: 'plugin', pluginId: 'docs' }, placement: 'footer', order: 60 } },
      { id: 'page', area: PLUGIN_PAGE_AREA, render: () => jsx(DocsPage, { ctx }) },
    ])
  },
}
