import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

const path = new URL('../ux/docs/plugin.js', import.meta.url)
const source = await readFile(path, 'utf8')
const stubs = {
  "import { Button, Input, NAV_AREA, PLUGIN_PAGE_AREA } from '@gents/ux-sdk'":
    'const Button = null, Input = null, NAV_AREA = null, PLUGIN_PAGE_AREA = null',
  "import { useCallback, useEffect, useRef, useState } from 'react'":
    'const useCallback = null, useEffect = null, useRef = null, useState = null',
  "import { jsx, jsxs } from 'react/jsx-runtime'": 'const jsx = null, jsxs = null',
}
let testSource = source
for (const [original, replacement] of Object.entries(stubs)) {
  assert.ok(testSource.includes(original), `Missing expected import: ${original}`)
  testSource = testSource.replace(original, replacement)
}
const { blocksToMarkdown, markdownToBlocks, domToMarkdown, revisionHeads, queryRows } =
  await import(`data:text/javascript;base64,${Buffer.from(testSource).toString('base64')}`)

const text = (value) => ({ nodeType: 3, nodeValue: value })
const element = (tag, ...children) => ({
  nodeType: 1,
  tagName: tag.toUpperCase(),
  childNodes: children,
  children: children.filter((child) => child.nodeType === 1),
})
const tree = (...children) => ({ childNodes: children })

function context(call) {
  let node = 'node-a'
  return {
    ctx: {
      shell: { selectedDeployment: () => ({ nodeDid: node }) },
      data: { call, supportsExpectedNode: true },
    },
    setNode(value) { node = value },
  }
}

test('rich content round-trips and unsupported Markdown keeps its source', () => {
  for (const markdown of [
    '',
    'Plain text',
    '# Proposal\n\nHello **team** and *friends*',
    'Heading\n\n- First\n- Second',
    '1. One\n2. Two',
    'Escape \\*literal\\*',
  ]) {
    const blocks = markdownToBlocks(markdown)
    assert.notEqual(blocks, null, markdown)
    assert.equal(blocksToMarkdown(blocks), markdown)
  }
  for (const markdown of [
    '[Link](https://example.com)',
    '<img src=x onerror=alert(1)>',
    '| Item | Price |\n| --- | --- |\n| A | 1 |',
    '```js\nconst x = 1\n```',
    'Paragraph\nwith a line break',
  ]) assert.equal(markdownToBlocks(markdown), null, markdown)
})

test('converts supported editor DOM and rejects pasted markup and unknown elements', () => {
  assert.equal(
    domToMarkdown(tree(
      element('h2', text('Title')),
      element('p', text('Hello '), element('strong', text('team'))),
      element('ul', element('li', text('First')), element('li', text('Second'))),
    )),
    '## Title\n\nHello **team**\n\n- First\n- Second',
  )
  assert.equal(domToMarkdown(tree(element('p', element('img')))), null)
  assert.equal(domToMarkdown(tree(element('p', element('a', text('link'))))), null)
  assert.equal(domToMarkdown(tree(element('p', element('script', text('unsafe'))))), null)
})

test('preserves sibling heads and refuses malformed or ambiguous revision histories', () => {
  const root = { revision_id: 'root' }
  const left = { revision_id: 'left', parent_revision_id: 'root' }
  const right = { revision_id: 'right', parent_revision_id: 'root' }
  assert.deepEqual(revisionHeads([root, left, right]), { heads: [left, right], invalid: false })
  assert.deepEqual(revisionHeads([root, { revision_id: 'root' }]), { heads: [], invalid: true })
  assert.deepEqual(revisionHeads([{ revision_id: 'a', parent_revision_id: 'missing' }]), { heads: [], invalid: true })
  assert.deepEqual(revisionHeads([{ revision_id: 'a', parent_revision_id: 'b' }, { revision_id: 'b', parent_revision_id: 'a' }]), { heads: [], invalid: true })
})

test('loads all revision pages and rejects a non-advancing cursor', async () => {
  const calls = []
  const { ctx } = context(async (surface, tool, args) => {
    calls.push({ surface, tool, args })
    if (args.offset === 0) return { kind: 'query', output: { results: [{ revision_id: 'a' }], next_offset: 1 } }
    return { kind: 'query', output: { results: [{ revision_id: 'b' }] } }
  })
  assert.deepEqual(await queryRows(ctx, { fields: ['revision_id'] }), [{ revision_id: 'a' }, { revision_id: 'b' }])
  assert.deepEqual(calls.map(({ args }) => args.offset), [0, 1])
  assert.ok(calls.every(({ surface, tool }) => surface === 'docs-editor' && tool === 'list_docs_revisions'))
  const stuck = context(async () => ({ kind: 'query', output: { results: [], next_offset: 0 } }))
  await assert.rejects(queryRows(stuck.ctx, { fields: ['revision_id'] }), /paged safely/)
})

test('recovers a complete UTF-8 document with the same filter and hash at each page', async () => {
  const calls = []
  const { ctx } = context(async (_surface, _tool, args) => {
    calls.push(args)
    if (!args.field_page) return { kind: 'query', output: {
      results: [{ revision_id: 'r', content: 'partial' }],
      field_recovery: [{ row: 0, doc_id: 'physical-id', field: 'content', total_bytes: 6 }],
    } }
    const offset = args.field_page.offset_bytes
    return { kind: 'query', output: { field_page: {
      doc_id: 'physical-id', field: 'content', offset_bytes: offset,
      text: offset === 0 ? 'é' : 'test', next_offset_bytes: offset === 0 ? 2 : 6,
      total_bytes: 6, value_hash: 'hash', complete: offset !== 0,
    } } }
  })
  const result = await queryRows(ctx, { document_id: 'document', fields: ['content', 'revision_id'] })
  assert.equal(result[0].content, 'étest')
  assert.equal(calls[1].document_id, 'document')
  assert.equal(calls[1].field_page.offset_bytes, 0)
  assert.equal(calls[2].field_page.offset_bytes, 2)
  assert.equal(calls[2].field_page.expected_hash, 'hash')
})

test('never returns truncated content after a changed hash or a runtime switch', async () => {
  const fixture = context(async (_surface, _tool, args) => {
    if (!args.field_page) return { kind: 'query', output: {
      results: [{ content: 'partial' }], field_recovery: [{ row: 0, doc_id: 'd', field: 'content', total_bytes: 2 }],
    } }
    const offset = args.field_page.offset_bytes
    return { kind: 'query', output: { field_page: {
      doc_id: 'd', field: 'content', offset_bytes: offset, text: offset ? 'b' : 'a',
      next_offset_bytes: offset + 1, total_bytes: 2, value_hash: offset ? 'new' : 'old', complete: offset === 1,
    } } }
  })
  await assert.rejects(queryRows(fixture.ctx, { fields: ['content'] }), /changed|recover/)
  const changed = context(async () => {
    changed.setNode('node-b')
    return { kind: 'query', output: { results: [] } }
  })
  await assert.rejects(queryRows(changed.ctx, { fields: ['content'] }), /runtime changed/)
})

test('refuses to read documents if the host cannot bind the runtime', async () => {
  let called = false
  const { ctx } = context(async () => { called = true })
  ctx.data.supportsExpectedNode = false
  await assert.rejects(queryRows(ctx, { fields: ['content'] }), /Update Gents/)
  assert.equal(called, false)
})
