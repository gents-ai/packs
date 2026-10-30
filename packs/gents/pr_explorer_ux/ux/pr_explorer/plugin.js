// PR Explorer: a page that shows one analysis of a branch range as logical
// units (left) and the unified diff of the selected file (right). The
// documents are the pr_explorer pack's (`pr_explorer` surface); this page
// only reads them, through the three query tools its manifest declares. An
// analysis is produced by the pr-explorer agent: the page's Analyze button
// asks it, on a click, as the user's next turn in the selected session.
//
// Everything drawn from a document (paths, titles, summaries, diff lines) is
// untrusted and rendered as React text, never as HTML.
import {
  Badge,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Group,
  Input,
  NAV_AREA,
  PLUGIN_PAGE_AREA,
  Row,
  TRANSCRIPT_DIRECTIVE_AREA,
  useQuery,
} from '@gents/ux-sdk'
import { useEffect, useMemo, useState } from 'react'
import { jsx, jsxs } from 'react/jsx-runtime'

const PLUGIN_ID = 'pr_explorer'
const SURFACE = 'pr_explorer'
const DIRECTIVE = 'pr-explorer'
/* what a ::pr-explorer{analysis} chip may name: `<head12>-<UTC stamp>` */
const ANALYSIS_ID = /^[0-9a-f]{12}-\d{14}$/
/* the user's own form input, before it becomes a prompt */
const REPO = /^(?!.*(^|\/)\.\.(\/|$))[A-Za-z0-9._/-]{1,200}$/
const REF = /^(?!-)[A-Za-z0-9._/@^~-]{1,200}$/
/* a GitHub pull request, as a link (anything after the number ignored) or
   owner/repo#n; github.com only for now */
const PR_URL = /^https:\/\/github\.com\/([A-Za-z0-9](?:[A-Za-z0-9-]{0,38}))\/([A-Za-z0-9._-]{1,100})\/pull\/([1-9]\d{0,6})(?:[/?#].*)?$/
const PR_SHORT = /^([A-Za-z0-9](?:[A-Za-z0-9-]{0,38}))\/([A-Za-z0-9._-]{1,100})#([1-9]\d{0,6})$/
/* bounds on what one render draws */
const LINE_PAGE = 5000
const MAX_UNIT_FILES = 2000
const FILE_FIELDS = ['analysis_id', 'ordinal', 'path', 'old_path', 'change', 'binary', 'additions', 'deletions', 'diff_bytes', 'truncated']

/* the analysis a ::pr-explorer chip deep-linked to: set by the chip's click,
   taken once by the page it opens */
let pendingAnalysis = null

const str = (v) => (typeof v === 'string' ? v : '')
const num = (v) => (typeof v === 'number' && Number.isFinite(v) ? v : 0)
const rowsOf = (q) => (Array.isArray(q.data?.results) ? q.data.results : [])

export function analyzePrompt(repo, base, head) {
  return `Analyze ${base}...${head || 'HEAD'} in ${repo} with PR Explorer.`
}

/* a PR link or owner/repo#n as its parts and canonical URL; null otherwise.
   The URL is rebuilt from validated parts, never passed through. */
export function parsePrLink(text) {
  const t = str(text).trim()
  const m = PR_URL.exec(t) ?? PR_SHORT.exec(t)
  if (!m || m[2] === '.' || m[2] === '..') return null
  const [, owner, repo, number] = m
  return { owner, repo, number: Number(number), url: `https://github.com/${owner}/${repo}/pull/${number}` }
}

export function prPrompt(pr) {
  return `Review ${pr.url} with PR Explorer.`
}

/* how an analysis names its change: owner/repo #n for a PR, the range otherwise */
export function refLabel(prRef) {
  const pr = parsePrLink(prRef)
  return pr ? `${pr.owner}/${pr.repo} #${pr.number}` : str(prRef)
}

/* a unit's `files` field: a JSON array of paths as a string, model-written */
export function unitFiles(text) {
  try {
    const parsed = JSON.parse(str(text))
    return Array.isArray(parsed) ? parsed.filter((p) => typeof p === 'string').slice(0, MAX_UNIT_FILES) : []
  } catch {
    return []
  }
}

/* the units as the page draws them: the model's units in order, each file
   resolved against the file list; files no unit claims gathered last; a
   file two units claim drawn in the first and flagged */
export function layout(units, files) {
  const byPath = new Map(files.map((f) => [str(f.path), f]))
  const owner = new Map()
  const sorted = [...units].sort((a, b) => num(a.ordinal) - num(b.ordinal))
  const out = sorted.map((u) => {
    const entries = []
    for (const path of unitFiles(u.files)) {
      const file = byPath.get(path)
      if (owner.has(path)) {
        entries.push({ path, file, duplicate: true })
        continue
      }
      owner.set(path, u.unit_id)
      entries.push({ path, file, missing: !file })
    }
    return { id: str(u.unit_id), title: str(u.title), summary: str(u.summary), entries }
  })
  const left = files
    .filter((f) => !owner.has(str(f.path)))
    .sort((a, b) => num(a.ordinal) - num(b.ordinal))
  if (left.length > 0) {
    out.push({
      id: '__unassigned',
      title: 'Unassigned',
      summary: 'Changed files no unit claims.',
      unassigned: true,
      entries: left.map((f) => ({ path: str(f.path), file: f })),
    })
  }
  return out
}

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/

/* a unified diff as rows: kind, old/new line numbers, text */
export function diffRows(diff) {
  const rows = []
  let oldNo = 0
  let newNo = 0
  let inHunk = false
  const lines = str(diff).split('\n')
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop()
  for (const line of lines) {
    const hunk = HUNK.exec(line)
    if (hunk) {
      oldNo = Number(hunk[1])
      newNo = Number(hunk[2])
      inHunk = true
      rows.push({ kind: 'hunk', text: line })
    } else if (!inHunk || line.startsWith('diff --git ')) {
      inHunk = false
      rows.push({ kind: 'meta', text: line })
    } else if (line.startsWith('+')) {
      rows.push({ kind: 'add', newNo: newNo++, text: line })
    } else if (line.startsWith('-')) {
      rows.push({ kind: 'del', oldNo: oldNo++, text: line })
    } else if (line.startsWith('\\')) {
      rows.push({ kind: 'meta', text: line })
    } else {
      rows.push({ kind: 'ctx', oldNo: oldNo++, newNo: newNo++, text: line })
    }
  }
  return rows
}

const kib = (n) => (n >= 1024 * 1024 ? `${(n / 1024 / 1024).toFixed(1)} MiB` : `${Math.ceil(n / 1024)} KiB`)

function Change({ file }) {
  if (!file) return jsx(Badge, { variant: 'outline', children: '?' })
  return jsx(Badge, { variant: 'outline', className: `gents-prx-change gents-prx-change-${str(file.change)}`, children: str(file.change) || 'M' })
}

function Counts({ file }) {
  if (!file) return null
  return jsxs('span', {
    className: 'gents-prx-counts',
    children: [
      jsx('span', { className: 'gents-prx-plus', children: `+${num(file.additions)}` }),
      ' ',
      jsx('span', { className: 'gents-prx-minus', children: `−${num(file.deletions)}` }),
    ],
  })
}

function UnitNav({ units, open, toggle, selected, select }) {
  return jsx('nav', {
    className: 'gents-prx-nav',
    'aria-label': 'Units',
    children: units.map((u) =>
      jsxs('section', {
        className: u.unassigned ? 'gents-prx-unit gents-prx-unit-warn' : 'gents-prx-unit',
        'data-unit': u.id,
        children: [
          jsxs('button', {
            type: 'button',
            className: 'gents-prx-unit-head',
            'aria-expanded': open.has(u.id),
            onClick: () => toggle(u.id),
            children: [
              jsxs('span', {
                className: 'gents-prx-unit-title',
                children: [
                  jsx('span', { className: 'gents-prx-caret', children: open.has(u.id) ? '▾' : '▸' }),
                  u.title || u.id,
                  jsx('span', { className: 'gents-prx-muted', children: `${u.entries.length} file${u.entries.length === 1 ? '' : 's'}` }),
                ],
              }),
              u.summary ? jsx('span', { className: 'gents-prx-unit-summary', children: u.summary }) : null,
            ],
          }),
          open.has(u.id)
            ? jsx('ul', {
                className: 'gents-prx-files',
                children: u.entries.map((e, i) =>
                  jsx('li', {
                    children: jsxs('button', {
                      type: 'button',
                      'data-path': e.path,
                      disabled: e.duplicate,
                      'aria-current': !e.duplicate && e.path === selected ? 'true' : undefined,
                      className: !e.duplicate && e.path === selected ? 'gents-prx-file gents-prx-file-on' : 'gents-prx-file',
                      title: e.missing ? 'no stored diff for this path' : e.duplicate ? 'already in an earlier unit' : e.path,
                      onClick: () => select(e.path),
                      children: [
                        jsx(Change, { file: e.file }),
                        jsx('span', { className: 'gents-prx-path', children: e.path }),
                        e.duplicate ? jsx(Badge, { variant: 'destructive', children: 'dup' }) : null,
                        e.missing ? jsx(Badge, { variant: 'secondary', children: 'missing' }) : null,
                        jsx(Counts, { file: e.file }),
                      ],
                    }),
                  }, `${e.path}:${i}`),
                ),
              })
            : null,
        ],
      }, u.id),
    ),
  })
}

function DiffPane({ ctx, analysisId, path, file }) {
  const q = useQuery(ctx, SURFACE, 'pr_list_files', {
    args: { analysis_id: analysisId, path },
    enabled: Boolean(analysisId && path),
    live: false,
    intervalMs: 0,
  })
  const [shown, setShown] = useState(LINE_PAGE)
  useEffect(() => setShown(LINE_PAGE), [analysisId, path])
  const doc = rowsOf(q)[0]
  const rows = useMemo(() => diffRows(doc?.diff), [doc])
  if (!path) {
    return jsx('div', { className: 'gents-prx-pane gents-prx-hint', children: 'Pick a file on the left to see its diff.' })
  }
  if (!file) {
    return jsxs('div', {
      className: 'gents-prx-pane',
      children: [
        jsx('header', { className: 'gents-prx-pane-head', children: jsx('span', { className: 'gents-prx-path', children: path }) }),
        jsx('div', {
          className: 'gents-prx-notice',
          role: 'status',
          children: 'No diff is stored for this file in this analysis. Run the analysis again to store it.',
        }),
      ],
    })
  }
  const renamed = str(file.old_path) && str(file.old_path) !== path
  return jsxs('div', {
    className: 'gents-prx-pane',
    children: [
      jsxs('header', {
        className: 'gents-prx-pane-head',
        children: [
          jsx(Change, { file }),
          jsx('span', { className: 'gents-prx-path', children: renamed ? `${str(file.old_path)} → ${path}` : path }),
          jsx(Counts, { file }),
        ],
      }),
      file.truncated
        ? jsx('div', {
            className: 'gents-prx-notice',
            role: 'status',
            children: `Truncated: this file's diff is ${kib(num(file.diff_bytes))}; only the first ${kib(str(doc?.diff).length || 262144)} is stored and shown.`,
          })
        : null,
      file.binary ? jsx('div', { className: 'gents-prx-notice', children: 'Binary file: no text diff.' }) : null,
      q.error ? jsx('div', { className: 'gents-prx-error', children: q.error }) : null,
      q.loading && !doc ? jsx('div', { className: 'gents-prx-hint', children: 'Loading diff…' }) : null,
      doc
        ? jsx('div', {
            className: 'gents-prx-diff-scroll',
            children: jsx('table', {
              className: 'gents-prx-diff',
              children: jsx('tbody', {
                children: rows.slice(0, shown).map((r, i) =>
                  jsxs('tr', {
                    className: `gents-prx-${r.kind}`,
                    children: [
                      jsx('td', { className: 'gents-prx-ln', children: r.oldNo ?? '' }),
                      jsx('td', { className: 'gents-prx-ln', children: r.newNo ?? '' }),
                      jsx('td', { className: 'gents-prx-code', children: r.text }),
                    ],
                  }, i),
                ),
              }),
            }),
          })
        : null,
      rows.length > shown
        ? jsxs('div', {
            className: 'gents-prx-more',
            children: [
              jsx('span', { className: 'gents-prx-muted', children: `Showing ${shown} of ${rows.length} lines.` }),
              jsx(Button, { size: 'sm', variant: 'outline', onClick: () => setShown((n) => n + LINE_PAGE), children: 'Show more' }),
            ],
          })
        : null,
    ],
  })
}

function AnalyzeForm({ ctx, initialRepo, compact }) {
  const [link, setLink] = useState('')
  const [repo, setRepo] = useState(initialRepo || '')
  const [base, setBase] = useState('')
  const [head, setHead] = useState('HEAD')
  const [sent, setSent] = useState(false)
  /* a PR link, when given, is the request; the range fields wait */
  const usingLink = link.trim() !== ''
  const pr = usingLink ? parsePrLink(link) : null
  const rangeValid = REPO.test(repo) && REF.test(base) && (head === '' || REF.test(head))
  const valid = usingLink ? pr !== null : rangeValid
  const submit = (e) => {
    e.preventDefault()
    if (!valid) return
    /* a user gesture: the request goes to the selected session's agent */
    void ctx.send(pr ? prPrompt(pr) : analyzePrompt(repo, base, head)).then(() => setSent(true))
  }
  return jsxs('form', {
    className: compact ? 'gents-prx-form gents-prx-form-compact' : 'gents-prx-form',
    onSubmit: submit,
    children: [
      jsx(Input, {
        className: 'gents-prx-form-link',
        value: link,
        onChange: (e) => setLink(e.target.value),
        placeholder: 'GitHub PR link, e.g. https://github.com/owner/repo/pull/123',
        'aria-label': 'PR link',
        'aria-invalid': usingLink && !pr ? 'true' : undefined,
      }),
      jsx(Button, { type: 'submit', size: 'sm', disabled: !valid, children: 'Analyze' }),
      jsx('span', { className: 'gents-prx-muted gents-prx-form-or', children: usingLink ? (pr ? `${pr.owner}/${pr.repo} #${pr.number}` : 'Not a github.com pull request link.') : 'or a local range:' }),
      jsx(Input, { value: repo, disabled: usingLink, onChange: (e) => setRepo(e.target.value), placeholder: 'repo (relative to the PR Explorer root)', 'aria-label': 'Repository' }),
      jsx(Input, { value: base, disabled: usingLink, onChange: (e) => setBase(e.target.value), placeholder: 'base (e.g. main)', 'aria-label': 'Base' }),
      jsx(Input, { value: head, disabled: usingLink, onChange: (e) => setHead(e.target.value), placeholder: 'head', 'aria-label': 'Head' }),
      sent ? jsx('span', { className: 'gents-prx-muted', role: 'status', children: 'Sent to the selected session.' }) : null,
    ],
  })
}

function EmptyState({ ctx, error }) {
  return jsxs(Card, {
    className: 'gents-prx-empty',
    children: [
      jsxs(CardHeader, {
        children: [
          jsx(CardTitle, { children: 'No analysis yet' }),
          jsx(CardDescription, {
            children: 'Ask the PR Explorer agent to review a GitHub pull request or a local branch range and group its files into units. The request goes to the selected session, so start a session with the PR Explorer agent first, or paste the request into one.',
          }),
        ],
      }),
      jsxs(CardContent, {
        children: [
          error
            ? jsx('div', { className: 'gents-prx-error', children: `${error} (is the pr_explorer pack installed on this node?)` })
            : null,
          jsx(AnalyzeForm, { ctx }),
        ],
      }),
    ],
  })
}

function PrExplorerPage({ ctx }) {
  const analysesQ = useQuery(ctx, SURFACE, 'pr_list_analyses', { pages: 'all' })
  const analyses = useMemo(
    () => [...rowsOf(analysesQ)].sort((a, b) => (str(a.created_at) < str(b.created_at) ? 1 : -1)),
    [analysesQ.data],
  )
  const [wanted] = useState(() => {
    const id = pendingAnalysis
    pendingAnalysis = null
    return id
  })
  const [analysisId, setAnalysisId] = useState(wanted)
  const [repo, setRepo] = useState(null)
  const current = analyses.find((a) => a.analysis_id === analysisId) ?? null
  const repos = useMemo(() => [...new Set(analyses.map((a) => str(a.repository)))].sort(), [analyses])
  const activeRepo = current ? str(current.repository) : repo ?? repos[0] ?? null
  const inRepo = analyses.filter((a) => str(a.repository) === activeRepo)
  /* no explicit choice (or a chip naming one not here yet): the newest */
  const active = current ?? inRepo[0] ?? null
  const id = active ? str(active.analysis_id) : ''
  const activePr = active ? parsePrLink(active.pr_ref) : null

  const unitsQ = useQuery(ctx, SURFACE, 'pr_list_units', { args: { analysis_id: id }, enabled: Boolean(id), pages: 'all' })
  const filesQ = useQuery(ctx, SURFACE, 'pr_list_files', { args: { analysis_id: id, fields: FILE_FIELDS }, enabled: Boolean(id), pages: 'all' })
  const units = useMemo(() => layout(rowsOf(unitsQ), rowsOf(filesQ)), [unitsQ.data, filesQ.data])
  const fileByPath = useMemo(() => new Map(rowsOf(filesQ).map((f) => [str(f.path), f])), [filesQ.data])

  const [open, setOpen] = useState(() => new Set())
  const [selected, setSelected] = useState(null)
  const [showForm, setShowForm] = useState(false)
  useEffect(() => {
    /* a new analysis opens its first unit on its first file */
    setSelected(null)
    setOpen(new Set())
  }, [id])
  useEffect(() => {
    if (selected || units.length === 0) return
    const first = units[0]
    /* the first unit, and any files no unit claims: a gap is never hidden */
    setOpen(new Set([first.id, ...units.filter((u) => u.unassigned).map((u) => u.id)]))
    const file = first.entries.find((e) => e.file && !e.duplicate) ?? first.entries.find((e) => !e.duplicate)
    if (file) setSelected(file.path)
  }, [units, selected])
  const toggle = (unitId) =>
    setOpen((s) => {
      const next = new Set(s)
      if (next.has(unitId)) next.delete(unitId)
      else next.add(unitId)
      return next
    })
  const refresh = () => {
    analysesQ.refresh()
    unitsQ.refresh()
    filesQ.refresh()
  }

  if (analyses.length === 0) {
    return jsxs('div', {
      className: 'gents-prx gents-prx-empty-page',
      'data-context': 'app',
      children: [
        jsx(Group, { title: 'PR Explorer', action: jsx(Button, { size: 'sm', variant: 'ghost', onClick: refresh, children: analysesQ.loading ? 'Loading…' : 'Refresh' }), children: null }),
        analysesQ.loading && !analysesQ.data ? jsx('div', { className: 'gents-prx-hint', children: 'Loading…' }) : jsx(EmptyState, { ctx, error: analysesQ.error }),
      ],
    })
  }

  return jsxs('div', {
    className: 'gents-prx',
    'data-context': 'app',
    children: [
      jsxs(Group, {
        title: 'PR Explorer',
        action: jsxs('div', {
          className: 'gents-prx-actions',
          children: [
            activePr
              ? jsx(Button, { size: 'sm', variant: 'ghost', onClick: () => void ctx.os.openExternal(activePr.url), children: 'Open on GitHub' })
              : null,
            jsx(Button, { size: 'sm', variant: 'ghost', onClick: refresh, children: analysesQ.loading ? 'Loading…' : 'Refresh' }),
            jsx(Button, { size: 'sm', variant: 'outline', onClick: () => setShowForm((v) => !v), children: showForm ? 'Close' : 'New analysis' }),
          ],
        }),
        children: [
          jsx(Row, {
            label: 'Analysis',
            description: active
              ? `${refLabel(active.pr_ref)} · ${str(active.base_sha).slice(0, 8)}..${str(active.head_sha).slice(0, 8)} · ${num(active.file_count)} files · ${str(active.created_at)}`
              : undefined,
            error: analysesQ.error ?? unitsQ.error ?? filesQ.error,
            children: jsxs('div', {
              className: 'gents-prx-pickers',
              children: [
                jsx('select', {
                  className: 'gents-prx-select',
                  'aria-label': 'Repository',
                  value: activeRepo ?? '',
                  onChange: (e) => {
                    setRepo(e.target.value)
                    setAnalysisId(null)
                  },
                  children: repos.map((r) => jsx('option', { value: r, children: r }, r)),
                }),
                jsx('select', {
                  className: 'gents-prx-select',
                  'aria-label': 'Analysis',
                  value: id,
                  onChange: (e) => setAnalysisId(e.target.value),
                  children: inRepo.map((a) =>
                    jsx('option', { value: str(a.analysis_id), children: `${refLabel(a.pr_ref)} · ${str(a.created_at)}` }, str(a.analysis_id)),
                  ),
                }),
              ],
            }),
          }),
          showForm
            ? jsx(Row, {
                label: 'New analysis',
                description: 'sent to the selected session; open one with the PR Explorer agent',
                children: jsx(AnalyzeForm, { ctx, initialRepo: activeRepo ?? '', compact: true }),
              })
            : null,
        ],
      }),
      active && filesQ.data && rowsOf(filesQ).length < num(active.file_count)
        ? jsx('div', {
            className: 'gents-prx-notice gents-prx-notice-warn',
            role: 'status',
            children: `Only ${rowsOf(filesQ).length} of ${num(active.file_count)} changed files have a stored diff: this analysis was recorded without them. Run it again (New analysis) to fix it.`,
          })
        : null,
      wanted && !current && analysesQ.data
        ? jsx('div', { className: 'gents-prx-notice', children: 'The analysis that chip names is not on this node; showing the newest instead.' })
        : null,
      jsxs('div', {
        className: 'gents-prx-split',
        children: [
          units.length === 0 && !unitsQ.loading
            ? jsx('div', { className: 'gents-prx-nav gents-prx-hint', children: 'This analysis has no units.' })
            : jsx(UnitNav, { units, open, toggle, selected, select: setSelected }),
          jsx(DiffPane, { ctx, analysisId: id, path: selected, file: selected ? fileByPath.get(selected) ?? null : null }),
        ],
      }),
    ],
  })
}

export default {
  id: PLUGIN_ID,
  name: 'PR Explorer',
  description: 'Browse a branch range as logical units with a per-file unified diff; the pr_explorer pack writes the analyses this page reads.',
  register(ctx) {
    ctx.registerMany([
      {
        id: 'nav',
        area: NAV_AREA,
        order: 61,
        data: {
          id: 'pr_explorer:nav',
          label: 'PR Explorer',
          icon: 'file',
          to: { name: 'plugin', pluginId: PLUGIN_ID },
          placement: 'footer',
          order: 61,
        },
      },
      {
        id: 'page',
        area: PLUGIN_PAGE_AREA,
        render: () => jsx(PrExplorerPage, { ctx }),
      },
      {
        /* ::pr-explorer{analysis="…"}: the id is model output, validated
           before it can select anything */
        id: 'directive',
        area: TRANSCRIPT_DIRECTIVE_AREA,
        data: {
          name: DIRECTIVE,
          label: 'PR Explorer · {analysis}',
          icon: 'file',
          onClick: (attrs) => {
            pendingAnalysis = ANALYSIS_ID.test(attrs.analysis ?? '') ? attrs.analysis : null
            ctx.shell.navigate({ name: 'plugin', pluginId: PLUGIN_ID })
          },
        },
      },
    ])
  },
}
