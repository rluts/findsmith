import { useEffect, useRef, useState } from 'react'
import { Busy, ErrorBox, Shell } from '../components/Shell'
import { SideDiff, UnifiedDiff } from '../components/DiffView'
import { RenderedPreview } from '../components/RenderedPreview'
import { ApiError, getJson, postForm } from '../lib/api'
import { navigate } from '../lib/router'
import type { ActionResult, Project, RenderedPreview as RenderedPreviewData, Review, Rule, RuleMask, SavedRule, Session, Source } from '../types/api'

const sourceNames: Record<string, string> = { manual: 'Manual titles', category: 'Category members', backlinks: 'Backlinks', transclusions: 'Template transclusions', search: 'Wiki search', watchlist: 'My watchlist', contributions: 'User contributions', links_on_page: 'Links on a page', categories_on_page: 'Categories on a page', redirects_to_page: 'Redirects to a page' }
const blankMask = (): RuleMask => ({comments: false, tags: false, references: false, links: false, headings: false, templates: false, link_targets: false})
const maskOptions: Array<[keyof RuleMask, string]> = [['comments', 'Comments'], ['tags', 'Tags'], ['references', 'References'], ['links', 'Links'], ['headings', 'Headings'], ['templates', 'Templates'], ['link_targets', 'Link targets']]
const blankRule = (): Rule => ({ name: '', enabled: true, operation: 'replace', group_name: '', group_contains: '', group_invert: false, parameter: '', mask: blankMask(), find: '', replace: '', regex: false, case_sensitive: true, replace_all: true, multiline: false, dot_matches_new_line: false })
const blankSource = (kind = 'manual'): Source => ({ kind, label: sourceNames[kind], value: kind === 'watchlist' ? 'My watchlist' : '', summary: '' })

function projectForm(project: Project) {
  const values = new URLSearchParams({ name: project.project_name, wiki_id: project.wiki_id, queue_limit: String(project.queue_limit), page_title: project.page_title, state_version: String(project.state_version), csrf_token: project.csrf_token })
  project.sources.forEach(source => { values.append('source_kind', source.kind); values.append('source_value', source.value) })
  project.namespace_options.filter(option => option.selected).forEach(option => values.append('namespaces', String(option.id)))
  project.rules.forEach((rule, index) => {
    values.append('rule_name', rule.name)
    values.append('rule_operation', rule.operation ?? 'replace')
    values.append('rule_group', rule.group_name ?? '')
    values.append('rule_group_contains', rule.group_contains ?? '')
    values.append('rule_parameter', rule.parameter ?? '')
    maskOptions.forEach(([key]) => { if (rule.mask?.[key]) values.append(`rule_mask_${key}`, String(index)) })
    if (rule.group_invert) values.append('rule_group_invert', String(index))
    values.append('rule_find', rule.find); values.append('rule_replace', rule.replace)
    values.append('rule_type', rule.regex ? 'regex' : 'plain'); values.append('rule_case', rule.case_sensitive ? 'sensitive' : 'insensitive'); values.append('rule_scope', rule.replace_all ? 'all' : 'first')
    if (rule.enabled) values.append('rule_enabled', String(index))
    if (rule.multiline) values.append('rule_multiline', String(index))
    if (rule.dot_matches_new_line) values.append('rule_dot_newline', String(index))
  })
  return values
}

export function ProjectPage({ projectId, jobId }: { projectId: string; jobId?: string }) {
  const [project, setProject] = useState<Project | null>(null)
  const [savedRules, setSavedRules] = useState<SavedRule[]>([])
  const [libraryError, setLibraryError] = useState('')
  const [review, setReview] = useState<Review | null>(null)
  const [session, setSession] = useState<Session | null>(null)
  const [text, setText] = useState('')
  const [textDirty, setTextDirty] = useState(false)
  const [configurationDirty, setConfigurationDirty] = useState(false)
  const [groupsError, setGroupsError] = useState('')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState('')
  const [previewMode, setPreviewMode] = useState<'diff' | 'preview'>('diff')
  const [rulesTab, setRulesTab] = useState<'rules' | 'status'>('rules')
  const [rendered, setRendered] = useState<{ projectId: string; requestedTitle: string; requestedText: string; data: RenderedPreviewData } | null>(null)
  const [renderBusy, setRenderBusy] = useState<string | null>(null)
  const [renderError, setRenderError] = useState<{ title: string; message: string } | null>(null)
  const [queueFilter, setQueueFilter] = useState('')
  const [queueStatus, setQueueStatus] = useState('all')
  const [comparison, setComparison] = useState('')
  const [preparseError, setPreparseError] = useState('')
  const [renderEquivalence, setRenderEquivalence] = useState(false)
  const renderRequest = useRef(0)
  const activeJobId = jobId ?? project?.latest_job_id ?? ''
  const reviewPath = activeJobId ? `/api/projects/${projectId}/jobs/${activeJobId}` : ''

  async function loadProject() {
    const data = await getJson<Project>(`/api/projects/${projectId}`)
    setProject(data); setConfigurationDirty(false)
    return data
  }
  async function loadReview(preserveText = false) {
    if (!reviewPath) { setReview(null); return null }
    const data = await getJson<Review>(reviewPath)
    setReview(data)
    if (data.page && !preserveText) { setText(data.page.proposed_text); setTextDirty(false) }
    return data
  }

  useEffect(() => { void loadProject().catch(error => setError(error.message)) }, [projectId])
  useEffect(() => {
    let cancelled = false
    const timer = window.setInterval(() => {
      void getJson<Project>(`/api/projects/${projectId}`)
        .then(data => {
          if (!cancelled) setProject(current => current?.project_id === data.project_id ? { ...current, awb_access: data.awb_access } : current)
        })
        .catch(error => {
          if (!cancelled) setProject(current => current?.project_id === projectId && current.awb_access
            ? { ...current, awb_access: { ...current.awb_access, allowed: null, error: error instanceof Error ? error.message : 'Could not check AutoWikiBrowser access.' } }
            : current)
        })
    }, 5 * 60 * 1000)
    return () => { cancelled = true; window.clearInterval(timer) }
  }, [projectId])
  useEffect(() => { void getJson<SavedRule[]>('/api/rules').then(setSavedRules).catch(error => setLibraryError(error instanceof Error ? error.message : 'Could not load saved rules.')) }, [projectId])
  useEffect(() => {
    if (!project?.wiki_id) return
    setSession(null); setGroupsError('')
    void getJson<Session>(`/api/session?include_groups=true&wiki_id=${encodeURIComponent(project.wiki_id)}`).then(setSession).catch(error => setGroupsError(error instanceof Error ? error.message : 'Could not load Wikimedia groups.'))
  }, [project?.wiki_id])
  useEffect(() => {
    if (!reviewPath) { setReview(null); return }
    void loadReview().catch(error => {
      if (error instanceof ApiError && error.status === 404 && jobId === projectId) {
        setError('')
        navigate(`/projects/${projectId}`)
      } else setError(error.message)
    })
  }, [reviewPath])
  useEffect(() => {
    if (review?.status !== 'building') return
    const timer = window.setInterval(() => { void Promise.all([loadProject(), loadReview()]).catch(error => setError(error.message)) }, 2000)
    return () => window.clearInterval(timer)
  }, [review?.status, reviewPath])
  useEffect(() => {
    if (!review?.preparse_active || !reviewPath || preparseError) return
    const timer = window.setTimeout(() => {
      void postForm<ActionResult>(`${reviewPath}/action`, new URLSearchParams({csrf_token: review.csrf_token, state_version: String(review.state_version), action: 'preparse_step'}))
        .then(() => loadReview())
        .catch(error => setPreparseError(error instanceof Error ? error.message : 'Pre-parse failed.'))
    }, 500)
    return () => window.clearTimeout(timer)
  }, [review?.preparse_active, review?.state_version, reviewPath, preparseError])
  useEffect(() => {
    if (review?.status !== 'active' || review.preparse_active || busy) return
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null
      if (event.altKey || event.ctrlKey || event.metaKey || target?.isContentEditable || target?.closest('input, textarea, select, button, a, summary, [role="button"]')) return
      const action = ({n: 'next', p: 'previous', s: 'skip'} as Record<string, string>)[event.key.toLowerCase()]
      if (action) { event.preventDefault(); void reviewAction(action) }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [review?.status, review?.preparse_active, review?.state_version, busy, textDirty, reviewPath])

  function update(patch: Partial<Project>) { setProject(current => current ? { ...current, ...patch, ...('rules' in patch ? { page_preview: null, page_preview_error: null } : {}) } : current); setConfigurationDirty(true) }
  function move<T>(items: T[], index: number, direction: number) { const copy = [...items]; const target = index + direction; if (target < 0 || target >= copy.length) return copy; [copy[index], copy[target]] = [copy[target], copy[index]]; return copy }
  function allowReplacingManualText() { return !textDirty || window.confirm('Discard the manual changes in the proposed wikitext and apply the current project rules?') }

  async function loadRendered(title: string, proposedText: string) {
    if (!project) return
    const request = ++renderRequest.current
    setRenderBusy(title); setRenderError(null)
    try {
      const data = await postForm<RenderedPreviewData>(`/api/projects/${projectId}/render-preview`, new URLSearchParams({ csrf_token: project.csrf_token, title, proposed_text: proposedText }))
      if (request === renderRequest.current) setRendered({ projectId, requestedTitle: title, requestedText: proposedText, data })
    } catch (error) {
      if (request === renderRequest.current) setRenderError({ title, message: error instanceof Error ? error.message : 'Could not render preview.' })
    } finally { if (request === renderRequest.current) setRenderBusy(null) }
  }

  function showRendered(title: string, proposedText: string) {
    setPreviewMode('preview')
    if (!rendered || rendered.projectId !== projectId || rendered.requestedTitle !== title) void loadRendered(title, proposedText)
  }

  async function saveRule(rule: Rule) {
    if (!project) return
    setBusy('Saving rule…'); setLibraryError('')
    try {
      await postForm<SavedRule>('/api/rules', new URLSearchParams({ csrf_token: project.csrf_token, rule_json: JSON.stringify(rule) }))
      setSavedRules(await getJson<SavedRule[]>('/api/rules'))
    } catch (error) { setLibraryError(error instanceof Error ? error.message : 'Could not save rule.') }
    finally { setBusy('') }
  }
  async function deleteSavedRule(item: SavedRule) {
    if (!project || !window.confirm(`Delete saved rule “${item.name}” from your library?`)) return
    setBusy('Deleting saved rule…'); setLibraryError('')
    try {
      await postForm(`/api/rules/${item.id}/delete`, new URLSearchParams({ csrf_token: project.csrf_token }))
      setSavedRules(current => current.filter(rule => rule.id !== item.id))
    } catch (error) { setLibraryError(error instanceof Error ? error.message : 'Could not delete saved rule.') }
    finally { setBusy('') }
  }

  async function submitProject(path: string, label: string, refreshReview = false, createsJob = false) {
    if (!project || (refreshReview && !allowReplacingManualText())) return
    setBusy(label); setError('')
    try {
      const result = await postForm<ActionResult | Project>(path, projectForm(project))
      if ('page_preview' in result) setProject(current => current ? { ...current, page_preview: result.page_preview, page_preview_error: result.page_preview_error } : current)
      else {
        const saved = await loadProject()
        if (refreshReview) {
          const nextJobId = createsJob && 'id' in result && result.id ? result.id : saved.latest_job_id
          if (nextJobId && nextJobId !== activeJobId) navigate(`/projects/${projectId}/jobs/${nextJobId}`)
          else await loadReview()
        }
      }
    } catch (error) { setError(error instanceof Error ? error.message : 'Request failed.') }
    finally { setBusy('') }
  }
  async function removeProject() {
    if (!project || !window.confirm(`Delete “${project.project_name}” permanently?`)) return
    setBusy('Deleting…')
    try { const result = await postForm<ActionResult>(`/api/projects/${projectId}/delete`, new URLSearchParams({ csrf_token: project.csrf_token, confirmation: 'delete' })); navigate(result.redirect) }
    catch (error) { setError(error instanceof Error ? error.message : 'Could not delete project.'); setBusy('') }
  }
  async function reviewAction(name: string) {
    if (!review || !reviewPath || ((name === 'next' || name === 'skip' || name === 'previous' || name === 'preparse_start') && !allowReplacingManualText())) return
    if (name === 'preparse_start') setPreparseError('')
    setBusy(name); setError('')
    try { await postForm<ActionResult>(`${reviewPath}/action`, new URLSearchParams({ csrf_token: review.csrf_token, state_version: String(review.state_version), action: name, render_equivalent: name === 'preparse_start' && renderEquivalence ? 'true' : 'false' })); await Promise.all([loadProject(), loadReview()]) }
    catch (error) { setError(error instanceof Error ? error.message : 'Review action failed.') }
    finally { setBusy('') }
  }
  async function recompute() {
    if (!review || !reviewPath) return
    setBusy('Recomputing…'); setError('')
    try { const data = await postForm<Review>(`${reviewPath}/recompute`, new URLSearchParams({ csrf_token: review.csrf_token, proposed_text: text })); setReview(data); if (data.page) setText(data.page.proposed_text); setTextDirty(false) }
    catch (error) { setError(error instanceof Error ? error.message : 'Could not recompute diff.') }
    finally { setBusy('') }
  }
  async function queueAction(actionName: string, index = 0, titles = '') {
    if (!review || !reviewPath || !allowReplacingManualText()) return
    setBusy(actionName); setError('')
    const values = new URLSearchParams({ csrf_token: review.csrf_token, state_version: String(review.state_version), action: actionName, index: String(index), titles })
    try { await postForm<ActionResult>(`${reviewPath}/queue`, values); await Promise.all([loadProject(), loadReview()]) }
    catch (error) { setError(error instanceof Error ? error.message : 'Queue action failed.') }
    finally { setBusy('') }
  }

  function exportQueue() {
    if (!review) return
    const quote = (value: unknown) => { const raw = String(value ?? ''); const safe = /^[=+\-@\t\r]/.test(raw) ? `'${raw}` : raw; return `"${safe.replaceAll('"', '""')}"` }
    const rows = [['position', 'title', 'status', 'reason'], ...review.queue.map((entry, index) => [String(index + 1), entry.title, entry.status, entry.reason ?? ''])]
    const csv = rows.map(row => row.map(quote).join(',')).join('\r\n') + '\r\n'
    const url = URL.createObjectURL(new Blob(['\ufeff', csv], {type: 'text/csv;charset=utf-8'}))
    const link = document.createElement('a'); link.href = url; link.download = `findsmith-queue-${review.job_id}.csv`; link.click()
    window.setTimeout(() => URL.revokeObjectURL(url), 1000)
  }

  if (!project) return <Shell title="Project"><main><Busy label={error || 'Loading project…'} /></main></Shell>
  const building = review?.status === 'building'
  const active = review?.status === 'active'
  const previewTitle = review ? review.current?.title : project.page_preview?.title
  const previewText = review ? (review.page ? text : undefined) : project.page_preview?.proposed_text
  const matchingRendered = rendered && rendered.projectId === projectId && rendered.requestedTitle === previewTitle ? rendered : null
  const renderStale = matchingRendered && matchingRendered.requestedText !== previewText
  const shownQueue = review?.queue.map((entry, index) => ({entry, index})).filter(({entry}) => (queueStatus === 'all' || entry.status === queueStatus) && entry.title.toLocaleLowerCase().includes(queueFilter.toLocaleLowerCase())) ?? []
  const comparisonTitles = [...new Set(comparison.split(/\r?\n/).map(title => title.trim()).filter(Boolean))]
  const queueTitles = new Set(review?.queue.map(entry => entry.title.toLocaleLowerCase()) ?? [])
  const comparisonOnly = comparisonTitles.filter(title => !queueTitles.has(title.toLocaleLowerCase()))
  const comparisonSet = new Set(comparisonTitles.map(title => title.toLocaleLowerCase()))
  const queueOnly = review?.queue.filter(entry => !comparisonSet.has(entry.title.toLocaleLowerCase())) ?? []
  function renderPanel() {
    if (!previewTitle || previewText === undefined) return <div className="empty preview-empty">Select a page to render.</div>
    const isRendering = renderBusy === previewTitle
    return <div className="rendered-preview-panel">
      <div className="rendered-preview-toolbar"><span>{renderStale ? 'Preview is out of date. Refresh to see the current proposed text.' : 'Rendered by the selected wiki.'}</span><button disabled={isRendering} onClick={() => loadRendered(previewTitle, previewText)}>{isRendering ? 'Rendering…' : 'Refresh preview'}</button></div>
      {renderError?.title === previewTitle && <ErrorBox message={renderError.message} />}
      {matchingRendered ? <RenderedPreview preview={matchingRendered.data} /> : <div className="empty preview-empty">{isRendering ? 'Rendering preview…' : 'Click Refresh preview to render this page.'}</div>}
    </div>
  }
  return <Shell title={project.project_name} subtitle={review ? `${review.position} / ${review.total} · ${review.pending} pending · ${review.reviewed} reviewed · ${review.skipped} skipped` : `Project · ${project.wiki_id} · configuration version ${project.state_version}`}>
    {error && <ErrorBox message={error} />}
    {project.awb_access?.allowed === false && <div className="notice" role="status">Your Wikimedia account is not in the <a href={project.awb_access.check_page} target="_blank" rel="noreferrer">AutoWikiBrowser CheckPageJSON</a> enabledusers or enabledbots list for this wiki. FindSmith reviews are still available; editing will require approval when implemented.</div>}
    {project.awb_access?.allowed === null && <div className="notice" role="status">Could not verify AutoWikiBrowser access for this wiki: {project.awb_access.error}</div>}
    <main className="awb-workspace">
      <section className="pane sources-pane">
        <div className="pane-title">Make list {review && <span>{review.total} pages</span>}</div>
        <details open={!review} className="source-settings"><summary>Page sources and filters</summary>
          <label>Source<select id="new-source">{Object.entries(sourceNames).map(([kind, label]) => <option value={kind} key={kind}>{label}</option>)}</select></label>
          <button onClick={() => { const element = document.getElementById('new-source') as HTMLSelectElement; update({ sources: [...project.sources, blankSource(element.value)] }) }}>Add source</button>
          <div className="source-list listbox">{project.sources.map((source, index) => <div className="source-card" key={index}>
            <div><strong>{sourceNames[source.kind]}</strong><span className="row-actions"><button disabled={index === 0} onClick={() => update({sources: move(project.sources, index, -1)})}>↑</button><button disabled={index === project.sources.length - 1} onClick={() => update({sources: move(project.sources, index, 1)})}>↓</button><button onClick={() => update({ sources: project.sources.filter((_, item) => item !== index) })}>×</button></span></div>
            <select value={source.kind} onChange={event => { const sources = [...project.sources]; sources[index] = blankSource(event.target.value); update({sources}) }}>{Object.entries(sourceNames).map(([kind, label]) => <option value={kind} key={kind}>{label}</option>)}</select>
            {source.kind !== 'watchlist' && <textarea value={source.value} placeholder={source.kind === 'contributions' ? 'One Wikimedia username per line' : 'One value per line'} onChange={event => { const sources = [...project.sources]; sources[index] = {...source, value: event.target.value}; update({sources}) }} />}
          </div>)}{!project.sources.length && <p className="empty">Add a source to build a page list.</p>}</div>
          <fieldset><legend>Namespaces</legend><div className="namespace-list">{project.namespace_options.map((option, index) => <label key={option.id}><input type="checkbox" checked={option.selected} onChange={event => { const options = [...project.namespace_options]; options[index] = {...option, selected: event.target.checked}; update({namespace_options: options}) }} />{option.label}</label>)}</div></fieldset>
          <label>Maximum pages<input type="number" min="1" max="500" value={project.queue_limit} onChange={event => update({queue_limit: Number(event.target.value)})} /></label>
        </details>
        <button className="primary build-list" disabled={!!busy} onClick={() => submitProject(`/api/projects/${projectId}/build`, 'Building…', true, true)}>Build page list</button>
        {review && <>{building && <div className="generation"><progress /><p>{review.generation_message}</p><button className="danger" onClick={() => reviewAction('stop')}>Cancel generation</button></div>}
          <details className="queue-tools"><summary>List tools</summary><label>Find title<input value={queueFilter} onChange={event => setQueueFilter(event.target.value)} placeholder="Filter the queue" /></label><label>Status<select value={queueStatus} onChange={event => setQueueStatus(event.target.value)}><option value="all">All statuses</option><option value="pending">Pending</option><option value="reviewed">Reviewed</option><option value="skipped">Skipped</option><option value="missing">Missing</option><option value="invalid">Invalid</option></select></label><p>{shownQueue.length} of {review.queue.length} shown</p><div className="row-actions"><button disabled={building || !!busy} onClick={() => queueAction('sort_remaining')}>Sort remaining by title</button><button onClick={exportQueue}>Export CSV</button></div><label>Compare with titles<textarea value={comparison} onChange={event => setComparison(event.target.value)} placeholder="Paste titles, one per line" /></label>{comparison.trim() && <><p>{comparisonOnly.length} only in pasted list · {queueOnly.length} only in queue · {comparisonTitles.length - comparisonOnly.length} in both</p><details><summary>Show differences</summary><strong>Only in pasted list</strong><ul>{comparisonOnly.map(title => <li key={title}>{title}</li>)}</ul><strong>Only in queue</strong><ul>{queueOnly.map(entry => <li key={entry.title}>{entry.title}</li>)}</ul></details></>}</details>
          <div className="queue-list listbox">{shownQueue.map(({entry, index}) => <div className={`queue-row ${index + 1 === review.position ? 'selected' : ''}`} key={`${entry.title}-${index}`}><span>{index + 1}</span><strong title={entry.reason ?? ''}>{entry.title}</strong><i>{entry.status}</i>{!building && <span className="row-actions"><button disabled={index === 0} onClick={() => queueAction('move_up', index)}>↑</button><button disabled={index === review.queue.length - 1} onClick={() => queueAction('move_down', index)}>↓</button><button onClick={() => queueAction('remove', index)}>×</button></span>}</div>)}</div>
          {!building && <><AppendPages onAppend={titles => queueAction('append', 0, titles)} /><button className="danger clear-queue" disabled={!review.queue.length} onClick={() => { if (window.confirm('Clear the entire queue?')) void queueAction('clear') }}>Clear queue</button></>}</>}
      </section>

      <section className="pane rules-pane">
        <div className="tabs"><button type="button" className={rulesTab === 'rules' || !review ? 'active' : ''} onClick={() => setRulesTab('rules')}>Rules</button><span className="inactive" aria-disabled="true">Options</span>{review && <button type="button" className={rulesTab === 'status' ? 'active' : ''} onClick={() => setRulesTab('status')}>Status</button>}</div>
        {(!review || rulesTab === 'rules') && <>
        <label>Project name<input value={project.project_name} maxLength={255} onChange={event => update({project_name: event.target.value})} /></label>
        <fieldset><legend>Wikimedia account</legend><p>User Groups: {session ? (session.groups.length ? session.groups.join(', ') : 'none reported') : (groupsError ? 'unavailable' : 'loading…')}</p>{session?.groups_error && <p className="error">{session.groups_error}</p>}{groupsError && <p className="error">{groupsError}</p>}</fieldset>
        {review && <>{active && (review.preparse_active ? <div className="generation"><progress value={review.preparse_cursor} max={review.total || 1} /><p>Pre-parsing {review.preparse_cursor} of {review.total} pages. Pages without changes are skipped; pages with changes remain for review.</p>{preparseError && <ErrorBox message={preparseError} />}<button className="danger" disabled={!!busy} onClick={() => reviewAction('preparse_stop')}>Stop pre-parse</button></div> : <><div className="review-buttons"><button disabled={!!busy} onClick={() => reviewAction('previous')}>Previous</button><button disabled={!!busy} onClick={() => reviewAction('skip')}>Skip</button><button className="danger" disabled={!!busy} onClick={() => reviewAction('stop')}>Stop review</button><button className="primary" disabled={!!busy} onClick={() => reviewAction('next')}>Next</button></div><p className="keyboard-hint">Shortcuts when no field is focused: N next · P previous · S skip</p><label className="preparse-option"><input type="checkbox" checked={renderEquivalence} onChange={event => setRenderEquivalence(event.target.checked)} />Also skip pages whose rendered output is unchanged (slower)</label><button disabled={!!busy} onClick={() => reviewAction('preparse_start')}>Pre-parse queue</button></>)}{review.status === 'stopped' && <button className="primary resume-review" disabled={!!busy} onClick={() => reviewAction('resume')}>Resume review</button>}<p className="read-only">Review only: “Next” records the result but does not edit Wikimedia.</p></>}
        <div className="rule-list">{project.rules.map((rule, index) => <article className={`rule-card ${rule.operation?.startsWith('category_') || rule.operation?.startsWith('template_') ? 'structural-rule' : ''}`} key={index}>
          <div className="rule-head"><label><input type="checkbox" checked={rule.enabled} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, enabled: event.target.checked}; update({rules}) }} /> {rule.name || `Rule ${index + 1}`} <small>· {rule.operation?.startsWith('category_') ? 'category' : rule.operation?.startsWith('template_') ? 'template' : 'text'}</small></label><span className="row-actions"><button disabled={index === 0} onClick={() => update({rules: move(project.rules, index, -1)})}>↑</button><button disabled={index === project.rules.length - 1} onClick={() => update({rules: move(project.rules, index, 1)})}>↓</button><button onClick={() => update({rules: project.rules.filter((_, item) => item !== index)})}>×</button></span></div>
          <label>Name<input value={rule.name} maxLength={255} placeholder="Optional in this project" onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, name: event.target.value}; update({rules}) }} /></label>
          <label>Operation<select value={rule.operation ?? 'replace'} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, operation: event.target.value as Rule['operation']}; update({rules}) }}><option value="replace">Find and replace</option><option value="append">Append wikitext</option><option value="prepend">Prepend wikitext</option><option value="category_add">Add category</option><option value="category_remove">Remove category</option><option value="category_replace">Replace category</option><option value="template_rename">Rename template</option><option value="template_parameter_set">Set template parameter</option><option value="template_parameter_remove">Remove template parameter</option></select></label>
          <details className="rule-condition"><summary>Conditional group</summary><p>Rules with the same group name share one condition, checked against the original page.</p><label>Group name<input value={rule.group_name ?? ''} maxLength={100} placeholder="Optional" onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, group_name: event.target.value}; update({rules}) }} /></label><label>Page must contain<input value={rule.group_contains ?? ''} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, group_contains: event.target.value}; update({rules}) }} /></label><label><input type="checkbox" checked={rule.group_invert ?? false} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, group_invert: event.target.checked}; update({rules}) }} /> Invert condition</label></details>
          {['replace', 'category_remove', 'category_replace', 'template_rename', 'template_parameter_set', 'template_parameter_remove'].includes(rule.operation ?? 'replace') && <label>{rule.operation === 'replace' ? 'Find' : rule.operation?.startsWith('template_') ? 'Template name' : 'Category to find'}<input value={rule.find} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, find: event.target.value}; update({rules}) }} /></label>}
          {(rule.operation === 'template_parameter_set' || rule.operation === 'template_parameter_remove') && <label>Parameter name<input value={rule.parameter ?? ''} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, parameter: event.target.value}; update({rules}) }} /></label>}
          {rule.operation !== 'category_remove' && rule.operation !== 'template_parameter_remove' && <label>{rule.operation === 'replace' || !rule.operation ? 'Replace with' : rule.operation === 'append' || rule.operation === 'prepend' ? 'Wikitext' : rule.operation === 'template_rename' ? 'New template name' : rule.operation === 'template_parameter_set' ? 'Parameter value' : 'Category name'} {rule.operation === 'replace' && <small>(leave empty to delete matches)</small>}<input value={rule.replace} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, replace: event.target.value}; update({rules}) }} /></label>}
          {(rule.operation ?? 'replace') === 'replace' && <><div className="rule-options"><label><input type="checkbox" checked={rule.regex} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, regex: event.target.checked}; update({rules}) }} />Regex</label><label><input type="checkbox" checked={rule.case_sensitive} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, case_sensitive: event.target.checked}; update({rules}) }} />Case sensitive</label><label><input type="checkbox" checked={rule.replace_all} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, replace_all: event.target.checked}; update({rules}) }} />Replace all</label></div>
          {rule.regex && <div className="rule-options"><label><input type="checkbox" checked={rule.multiline} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, multiline: event.target.checked}; update({rules}) }} />Multiline</label><label><input type="checkbox" checked={rule.dot_matches_new_line} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, dot_matches_new_line: event.target.checked}; update({rules}) }} />Dot matches newline</label></div>}</>}
          {(rule.operation ?? 'replace') === 'replace' && <details className="rule-condition"><summary>Protect wikitext regions</summary><p>Skip a match if it overlaps a selected region.</p><div className="rule-options">{maskOptions.map(([key, label]) => <label key={key}><input type="checkbox" checked={rule.mask?.[key] ?? false} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, mask: {...(rule.mask ?? blankMask()), [key]: event.target.checked}}; update({rules}) }} />{label}</label>)}</div></details>}
          {rule.operation?.startsWith('template_') && <label><input type="checkbox" className="inline-checkbox" checked={rule.replace_all} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, replace_all: event.target.checked}; update({rules}) }} /> Apply to all matching templates</label>}
          <button disabled={!!busy || !rule.name.trim() || (['replace', 'category_remove', 'category_replace', 'template_rename', 'template_parameter_set', 'template_parameter_remove'].includes(rule.operation ?? 'replace') && !rule.find) || (['append', 'prepend', 'category_add', 'category_replace', 'template_rename'].includes(rule.operation ?? 'replace') && !rule.replace) || (rule.operation?.startsWith('template_parameter_') && !rule.parameter)} onClick={() => saveRule(rule)}>Save to my rules</button>
        </article>)}</div>
        <button onClick={() => update({rules: [...project.rules, blankRule()]})}>Add rule</button>
        <fieldset className="saved-rules"><legend>My saved rules</legend>
          {libraryError && <ErrorBox message={libraryError} />}
          {savedRules.length ? savedRules.map(item => <div className="saved-rule-row" key={item.id}>
            <span title={`${item.rule.operation ?? 'replace'}: ${item.rule.find} → ${item.rule.replace}`}><strong>{item.name}</strong><small> · {item.rule.operation ?? 'replace'}</small></span>
            <button disabled={!!busy || project.rules.length >= 100} onClick={() => update({rules: [...project.rules, {...item.rule}]})}>Add to project</button>
            <button className="danger" disabled={!!busy} onClick={() => deleteSavedRule(item)}>Delete</button>
          </div>) : <p className="empty">Name a rule and save it here to use it in other projects.</p>}
        </fieldset>
        {configurationDirty && <p className="unsaved-note">Configuration changed. Apply rules to refresh the current review.</p>}
        <div className="command-stack"><button className="primary" disabled={!!busy} onClick={() => submitProject(`/api/projects/${projectId}`, review ? 'Applying rules…' : 'Saving…', Boolean(review))}>{review ? 'Apply rules' : 'Save configuration'}</button><button className="danger" onClick={removeProject}>Delete project</button></div>
        </>}
        {review && rulesTab === 'status' && <>
          <fieldset><legend>Review progress</legend><dl><dt>Status</dt><dd>{review.status}</dd><dt>Pending</dt><dd>{review.pending}</dd><dt>Reviewed</dt><dd>{review.reviewed}</dd><dt>Skipped</dt><dd>{review.skipped}</dd><dt>Unavailable</dt><dd>{review.unavailable}</dd></dl></fieldset>
          <fieldset className="decision-history"><legend>Decision history</legend>{review.queue.filter(entry => entry.status !== 'pending').length ? <ol>{review.queue.filter(entry => entry.status !== 'pending').map((entry, index) => <li key={`${entry.title}-${index}`}><strong>{entry.title}</strong> — {entry.status}{entry.reason ? `: ${entry.reason}` : ''}{entry.replacement_count !== null ? ` · ${entry.replacement_count} replacements` : ''}</li>)}</ol> : <p className="empty">No decisions recorded yet.</p>}</fieldset>
        </>}
        {busy && <p className="busy">{busy}</p>}
      </section>

      <section className="pane editor-pane">
        <div className="tabs"><span className="active">{review ? 'Review' : 'Preview'}</span>{review && <><span className="inactive" aria-disabled="true">History</span><span className="inactive" aria-disabled="true">What links here</span><span className="inactive" aria-disabled="true">Logs</span><span className="inactive" aria-disabled="true">Typos</span></>}</div>
        {review ? <>
          {review.page_error && <ErrorBox message={review.page_error} />}
          {review.page ? <>
            <div className="page-heading"><a href={review.page.full_url} target="_blank" rel="noreferrer">{review.current?.title ?? 'Current revision'}</a><strong title="Comparison with the current wiki revision; no wiki edit has been made.">{review.page.changed ? 'Proposed changes' : 'No proposed changes'}</strong></div>
            <div className="meta-strip"><span>Revision {review.page.revision_id}</span><span>NS {review.current?.namespace ?? '—'}</span><span>{review.page.content_model}</span><span>{review.page.editable ? 'Editable' : 'Not editable'}</span><span>Protection: {review.page.protection}</span></div>
            <details className="wikitext-accordion"><summary>Proposed wikitext{textDirty ? ' · modified' : ''}</summary><textarea className="editor" value={text} maxLength={4194304} onChange={event => { setText(event.target.value); setTextDirty(true) }} /><button onClick={recompute} disabled={!!busy}>Recompute diff</button></details>
            <div className="view-switch"><button className={previewMode === 'diff' ? 'active' : ''} onClick={() => setPreviewMode('diff')}>Diff</button><button className={previewMode === 'preview' ? 'active' : ''} onClick={() => showRendered(review.current?.title ?? '', text)}>Preview</button></div>
            {previewMode === 'preview' ? renderPanel() : <><div className="rule-counts">{review.page.rules.map((rule, index) => <span className={rule.changed ? 'changed' : ''} key={index}>Rule {index + 1}: {rule.count}</span>)}</div>{review.page.changed ? <><SideDiff lines={review.page.side_by_side_diff} /><details><summary>Unified diff</summary><UnifiedDiff lines={review.page.unified_diff} /></details></> : <div className="no-diff">No diff — the proposed wikitext matches the current revision.</div>}</>}
          </> : <div className="empty preview-empty">{building ? review.generation_message : 'No reviewable page is selected.'}</div>}
        </> : <>
          <div className="preview-tools"><label>Page title<input value={project.page_title} onChange={event => update({page_title: event.target.value, page_preview: null})} placeholder="Example page" /></label><button disabled={!!busy || !project.page_title.trim()} onClick={() => submitProject(`/api/projects/${projectId}/page-preview`, 'Loading preview…')}>Preview</button></div>
          {project.page_preview_error && <ErrorBox message={project.page_preview_error} />}
          {project.page_preview ? <>
            <div className="page-heading"><a href={project.page_preview.full_url} target="_blank" rel="noreferrer">{project.page_preview.title}</a><strong>{project.page_preview.total_replacements} operation(s)</strong></div>
            <div className="meta-strip"><span>Revision {project.page_preview.revision_id}</span><span>NS {project.page_preview.namespace}</span><span>{project.page_preview.content_model}</span><span>{project.page_preview.redirect ? 'Redirect' : 'Page'}</span><span>{project.page_preview.editable ? 'Editable' : 'Not editable'}</span><span>Protection: {project.page_preview.protection}</span></div>
            <div className="view-switch"><button className={previewMode === 'diff' ? 'active' : ''} onClick={() => setPreviewMode('diff')}>Diff</button><button className={previewMode === 'preview' ? 'active' : ''} onClick={() => showRendered(project.page_preview!.title, project.page_preview!.proposed_text)}>Preview</button></div>
            {previewMode === 'preview' ? renderPanel() : <><div className="rule-counts">{project.page_preview.rules.map((rule, index) => <span className={rule.changed ? 'changed' : ''} key={index}>Rule {index + 1}: {rule.count}</span>)}</div>{project.page_preview.changed ? <><SideDiff lines={project.page_preview.side_by_side_diff} /><details><summary>Unified diff</summary><UnifiedDiff lines={project.page_preview.unified_diff} /></details></> : <div className="no-diff">No diff — the rules did not change this page.</div>}</>}
          </> : <div className="empty preview-empty">Build a page list to begin review, or enter a page title to test the current rules.</div>}
        </>}
      </section>
    </main>
  </Shell>
}

function AppendPages({ onAppend }: { onAppend: (titles: string) => void }) {
  const [titles, setTitles] = useState('')
  return <div className="append-pages"><textarea value={titles} onChange={event => setTitles(event.target.value)} placeholder="Append page titles, one per line" /><button onClick={() => { if (titles.trim()) { onAppend(titles); setTitles('') } }}>Append</button></div>
}
