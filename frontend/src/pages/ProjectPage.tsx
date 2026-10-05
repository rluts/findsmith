import { useEffect, useRef, useState } from 'react'
import { Busy, ErrorBox, Shell } from '../components/Shell'
import { SideDiff, UnifiedDiff } from '../components/DiffView'
import { RenderedPreview } from '../components/RenderedPreview'
import { ApiError, getJson, postForm } from '../lib/api'
import { navigate } from '../lib/router'
import type { ActionResult, Project, RenderedPreview as RenderedPreviewData, Review, Rule, SavedRule, Session, Source } from '../types/api'

const sourceNames: Record<string, string> = { manual: 'Manual titles', category: 'Category members', backlinks: 'Backlinks', transclusions: 'Template transclusions', search: 'Wiki search' }
const blankRule = (): Rule => ({ name: '', enabled: true, find: '', replace: '', regex: false, case_sensitive: true, replace_all: true, multiline: false, dot_matches_new_line: false })
const blankSource = (kind = 'manual'): Source => ({ kind, label: sourceNames[kind], value: '', summary: '' })

function projectForm(project: Project) {
  const values = new URLSearchParams({ name: project.project_name, wiki_id: project.wiki_id, queue_limit: String(project.queue_limit), page_title: project.page_title, state_version: String(project.state_version), csrf_token: project.csrf_token })
  project.sources.forEach(source => { values.append('source_kind', source.kind); values.append('source_value', source.value) })
  project.namespace_options.filter(option => option.selected).forEach(option => values.append('namespaces', String(option.id)))
  project.rules.forEach((rule, index) => {
    values.append('rule_name', rule.name)
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
  const [rendered, setRendered] = useState<{ projectId: string; requestedTitle: string; requestedText: string; data: RenderedPreviewData } | null>(null)
  const [renderBusy, setRenderBusy] = useState<string | null>(null)
  const [renderError, setRenderError] = useState<{ title: string; message: string } | null>(null)
  const renderRequest = useRef(0)
  const activeJobId = jobId ?? project?.latest_job_id ?? ''
  const reviewPath = activeJobId ? `/api/projects/${projectId}/jobs/${activeJobId}` : ''

  async function loadProject(includeAccess = false) {
    const data = await getJson<Project>(`/api/projects/${projectId}${includeAccess ? '?include_awb_access=true' : ''}`)
    setProject(current => ({ ...data, awb_access: data.awb_access ?? (current?.project_id === data.project_id ? current.awb_access : null) })); setConfigurationDirty(false)
    return data
  }
  async function loadReview(preserveText = false) {
    if (!reviewPath) { setReview(null); return null }
    const data = await getJson<Review>(reviewPath)
    setReview(data)
    if (data.page && !preserveText) { setText(data.page.proposed_text); setTextDirty(false) }
    return data
  }

  useEffect(() => { void loadProject(true).catch(error => setError(error.message)) }, [projectId])
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
    if (!review || !reviewPath || ((name === 'next' || name === 'skip' || name === 'previous') && !allowReplacingManualText())) return
    setBusy(name); setError('')
    try { await postForm<ActionResult>(`${reviewPath}/action`, new URLSearchParams({ csrf_token: review.csrf_token, state_version: String(review.state_version), action: name })); await Promise.all([loadProject(), loadReview()]) }
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

  if (!project) return <Shell title="Project"><main><Busy label={error || 'Loading project…'} /></main></Shell>
  const building = review?.status === 'building'
  const active = review?.status === 'active'
  const previewTitle = review ? review.current?.title : project.page_preview?.title
  const previewText = review ? (review.page ? text : undefined) : project.page_preview?.proposed_text
  const matchingRendered = rendered && rendered.projectId === projectId && rendered.requestedTitle === previewTitle ? rendered : null
  const renderStale = matchingRendered && matchingRendered.requestedText !== previewText
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
          <label>Source<select id="new-source"><option value="manual">Manual titles</option><option value="category">Category members</option><option value="backlinks">Backlinks</option><option value="transclusions">Template transclusions</option><option value="search">Wiki search</option></select></label>
          <button onClick={() => { const element = document.getElementById('new-source') as HTMLSelectElement; update({ sources: [...project.sources, blankSource(element.value)] }) }}>Add source</button>
          <div className="source-list listbox">{project.sources.map((source, index) => <div className="source-card" key={index}>
            <div><strong>{sourceNames[source.kind]}</strong><span className="row-actions"><button disabled={index === 0} onClick={() => update({sources: move(project.sources, index, -1)})}>↑</button><button disabled={index === project.sources.length - 1} onClick={() => update({sources: move(project.sources, index, 1)})}>↓</button><button onClick={() => update({ sources: project.sources.filter((_, item) => item !== index) })}>×</button></span></div>
            <select value={source.kind} onChange={event => { const sources = [...project.sources]; sources[index] = {...source, kind: event.target.value, label: sourceNames[event.target.value]}; update({sources}) }}><option value="manual">Manual titles</option><option value="category">Category</option><option value="backlinks">Backlinks</option><option value="transclusions">Transclusions</option><option value="search">Search</option></select>
            <textarea value={source.value} placeholder="One value per line" onChange={event => { const sources = [...project.sources]; sources[index] = {...source, value: event.target.value}; update({sources}) }} />
          </div>)}{!project.sources.length && <p className="empty">Add a source to build a page list.</p>}</div>
          <fieldset><legend>Namespaces</legend><div className="namespace-list">{project.namespace_options.map((option, index) => <label key={option.id}><input type="checkbox" checked={option.selected} onChange={event => { const options = [...project.namespace_options]; options[index] = {...option, selected: event.target.checked}; update({namespace_options: options}) }} />{option.label}</label>)}</div></fieldset>
          <label>Maximum pages<input type="number" min="1" max="500" value={project.queue_limit} onChange={event => update({queue_limit: Number(event.target.value)})} /></label>
        </details>
        <button className="primary build-list" disabled={!!busy} onClick={() => submitProject(`/api/projects/${projectId}/build`, 'Building…', true, true)}>Build page list</button>
        {review && <>{building && <div className="generation"><progress /><p>{review.generation_message}</p><button className="danger" onClick={() => reviewAction('stop')}>Cancel generation</button></div>}
          <div className="queue-list listbox">{review.queue.map((entry, index) => <div className={`queue-row ${index + 1 === review.position ? 'selected' : ''}`} key={`${entry.title}-${index}`}><span>{index + 1}</span><strong title={entry.reason ?? ''}>{entry.title}</strong><i>{entry.status}</i>{!building && <span className="row-actions"><button disabled={index === 0} onClick={() => queueAction('move_up', index)}>↑</button><button disabled={index === review.queue.length - 1} onClick={() => queueAction('move_down', index)}>↓</button><button onClick={() => queueAction('remove', index)}>×</button></span>}</div>)}</div>
          {!building && <><AppendPages onAppend={titles => queueAction('append', 0, titles)} /><button className="danger clear-queue" disabled={!review.queue.length} onClick={() => { if (window.confirm('Clear the entire queue?')) void queueAction('clear') }}>Clear queue</button></>}</>}
      </section>

      <section className="pane rules-pane">
        <div className="tabs"><span className="active">Rules</span><span className="inactive" aria-disabled="true">Options</span></div>
        <label>Project name<input value={project.project_name} maxLength={255} onChange={event => update({project_name: event.target.value})} /></label>
        <fieldset><legend>Wikimedia account</legend><p>User Groups: {session ? (session.groups.length ? session.groups.join(', ') : 'none reported') : (groupsError ? 'unavailable' : 'loading…')}</p>{session?.groups_error && <p className="error">{session.groups_error}</p>}{groupsError && <p className="error">{groupsError}</p>}</fieldset>
        {review && <>{active && <div className="review-buttons"><button disabled={!!busy} onClick={() => reviewAction('previous')}>Previous</button><button disabled={!!busy} onClick={() => reviewAction('skip')}>Skip</button><button className="danger" disabled={!!busy} onClick={() => reviewAction('stop')}>Stop review</button><button className="primary" disabled={!!busy} onClick={() => reviewAction('next')}>Next</button></div>}{review.status === 'stopped' && <button className="primary resume-review" disabled={!!busy} onClick={() => reviewAction('resume')}>Resume review</button>}<p className="read-only">Review only: “Next” records the result but does not edit Wikimedia.</p></>}
        <div className="rule-list">{project.rules.map((rule, index) => <article className="rule-card" key={index}>
          <div className="rule-head"><label><input type="checkbox" checked={rule.enabled} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, enabled: event.target.checked}; update({rules}) }} /> {rule.name || `Rule ${index + 1}`}</label><span className="row-actions"><button disabled={index === 0} onClick={() => update({rules: move(project.rules, index, -1)})}>↑</button><button disabled={index === project.rules.length - 1} onClick={() => update({rules: move(project.rules, index, 1)})}>↓</button><button onClick={() => update({rules: project.rules.filter((_, item) => item !== index)})}>×</button></span></div>
          <label>Name<input value={rule.name} maxLength={255} placeholder="Optional in this project" onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, name: event.target.value}; update({rules}) }} /></label>
          <label>Find<input value={rule.find} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, find: event.target.value}; update({rules}) }} /></label><label>Replace with <small>(leave empty to delete matches)</small><input value={rule.replace} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, replace: event.target.value}; update({rules}) }} /></label>
          <div className="rule-options"><label><input type="checkbox" checked={rule.regex} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, regex: event.target.checked}; update({rules}) }} />Regex</label><label><input type="checkbox" checked={rule.case_sensitive} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, case_sensitive: event.target.checked}; update({rules}) }} />Case sensitive</label><label><input type="checkbox" checked={rule.replace_all} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, replace_all: event.target.checked}; update({rules}) }} />Replace all</label></div>
          {rule.regex && <div className="rule-options"><label><input type="checkbox" checked={rule.multiline} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, multiline: event.target.checked}; update({rules}) }} />Multiline</label><label><input type="checkbox" checked={rule.dot_matches_new_line} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, dot_matches_new_line: event.target.checked}; update({rules}) }} />Dot matches newline</label></div>}
          <button disabled={!!busy || !rule.name.trim() || !rule.find} onClick={() => saveRule(rule)}>Save to my rules</button>
        </article>)}</div>
        <button onClick={() => update({rules: [...project.rules, blankRule()]})}>Add replacement rule</button>
        <fieldset className="saved-rules"><legend>My saved rules</legend>
          {libraryError && <ErrorBox message={libraryError} />}
          {savedRules.length ? savedRules.map(item => <div className="saved-rule-row" key={item.id}>
            <span title={`${item.rule.find} → ${item.rule.replace}`}><strong>{item.name}</strong><small> · {item.rule.find} → {item.rule.replace || '∅'}</small></span>
            <button disabled={!!busy || project.rules.length >= 100} onClick={() => update({rules: [...project.rules, {...item.rule}]})}>Add to project</button>
            <button className="danger" disabled={!!busy} onClick={() => deleteSavedRule(item)}>Delete</button>
          </div>) : <p className="empty">Name a rule and save it here to use it in other projects.</p>}
        </fieldset>
        {configurationDirty && <p className="unsaved-note">Configuration changed. Apply rules to refresh the current review.</p>}
        <div className="command-stack"><button className="primary" disabled={!!busy} onClick={() => submitProject(`/api/projects/${projectId}`, review ? 'Applying rules…' : 'Saving…', Boolean(review))}>{review ? 'Apply rules' : 'Save configuration'}</button><button className="danger" onClick={removeProject}>Delete project</button></div>
        {review && <><fieldset><legend>Review progress</legend><dl><dt>Status</dt><dd>{review.status}</dd><dt>Pending</dt><dd>{review.pending}</dd><dt>Reviewed</dt><dd>{review.reviewed}</dd><dt>Skipped</dt><dd>{review.skipped}</dd><dt>Unavailable</dt><dd>{review.unavailable}</dd></dl></fieldset>
          {review.current && <fieldset><legend>Current page</legend><strong>{review.current.title}</strong><p>Namespace: {review.current.namespace ?? '—'}</p><p>{review.current.reason}</p></fieldset>}</>}
        {busy && <p className="busy">{busy}</p>}
      </section>

      <section className="pane editor-pane">
        <div className="tabs"><span className="active">{review ? 'Review' : 'Preview'}</span>{review && <><span className="inactive" aria-disabled="true">History</span><span className="inactive" aria-disabled="true">What links here</span><span className="inactive" aria-disabled="true">Logs</span><span className="inactive" aria-disabled="true">Typos</span></>}</div>
        {review ? <>
          {review.page_error && <ErrorBox message={review.page_error} />}
          {review.page ? <>
            <div className="page-heading"><a href={review.page.full_url} target="_blank" rel="noreferrer">{review.current?.title ?? 'Current revision'}</a><strong title="Comparison with the current wiki revision; no wiki edit has been made.">{review.page.changed ? 'Proposed changes' : 'No proposed changes'}</strong></div>
            <div className="meta-strip"><span>Revision {review.page.revision_id}</span><span>{review.page.content_model}</span><span>{review.page.editable ? 'Editable' : 'Not editable'}</span><span>Protection: {review.page.protection}</span></div>
            <details className="wikitext-accordion"><summary>Proposed wikitext{textDirty ? ' · modified' : ''}</summary><textarea className="editor" value={text} maxLength={4194304} onChange={event => { setText(event.target.value); setTextDirty(true) }} /><button onClick={recompute} disabled={!!busy}>Recompute diff</button></details>
            <div className="view-switch"><button className={previewMode === 'diff' ? 'active' : ''} onClick={() => setPreviewMode('diff')}>Diff</button><button className={previewMode === 'preview' ? 'active' : ''} onClick={() => showRendered(review.current?.title ?? '', text)}>Preview</button></div>
            {previewMode === 'preview' ? renderPanel() : <><div className="rule-counts">{review.page.rules.map((rule, index) => <span className={rule.changed ? 'changed' : ''} key={index}>Rule {index + 1}: {rule.count}</span>)}</div>{review.page.changed ? <><SideDiff lines={review.page.side_by_side_diff} /><details><summary>Unified diff</summary><UnifiedDiff lines={review.page.unified_diff} /></details></> : <div className="no-diff">No diff — the proposed wikitext matches the current revision.</div>}</>}
          </> : <div className="empty preview-empty">{building ? review.generation_message : 'No reviewable page is selected.'}</div>}
        </> : <>
          <div className="preview-tools"><label>Page title<input value={project.page_title} onChange={event => update({page_title: event.target.value, page_preview: null})} placeholder="Example page" /></label><button disabled={!!busy || !project.page_title.trim()} onClick={() => submitProject(`/api/projects/${projectId}/page-preview`, 'Loading preview…')}>Preview</button></div>
          {project.page_preview_error && <ErrorBox message={project.page_preview_error} />}
          {project.page_preview ? <>
            <div className="page-heading"><a href={project.page_preview.full_url} target="_blank" rel="noreferrer">{project.page_preview.title}</a><strong>{project.page_preview.total_replacements} replacement(s)</strong></div>
            <div className="meta-strip"><span>Revision {project.page_preview.revision_id}</span><span>NS {project.page_preview.namespace}</span><span>{project.page_preview.content_model}</span><span>{project.page_preview.redirect ? 'Redirect' : 'Page'}</span><span>{project.page_preview.editable ? 'Editable' : 'Not editable'}</span><span>Protection: {project.page_preview.protection}</span></div>
            <div className="view-switch"><button className={previewMode === 'diff' ? 'active' : ''} onClick={() => setPreviewMode('diff')}>Diff</button><button className={previewMode === 'preview' ? 'active' : ''} onClick={() => showRendered(project.page_preview!.title, project.page_preview!.proposed_text)}>Preview</button></div>
            {previewMode === 'preview' ? renderPanel() : <><div className="rule-counts">{project.page_preview.rules.map((rule, index) => <span className={rule.changed ? 'changed' : ''} key={index}>Rule {index + 1}: {rule.count}</span>)}</div>{project.page_preview.changed ? <><SideDiff lines={project.page_preview.side_by_side_diff} /><details><summary>Unified diff</summary><UnifiedDiff lines={project.page_preview.unified_diff} /></details></> : <div className="no-diff">No diff — the replacement rules did not change this page.</div>}</>}
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
