import { useEffect, useRef, useState } from 'react'
import { Busy, ErrorBox, Shell } from '../components/Shell'
import { SideDiff } from '../components/DiffView'
import { RenderedPreview } from '../components/RenderedPreview'
import { ApiError, getJson, postForm } from '../lib/api'
import { navigate } from '../lib/router'
import { useI18n } from '../lib/i18n'
import type { ActionResult, Project, RenderedPreview as RenderedPreviewData, Review, Rule, RuleMask, SavedRule, Session, SkipOptions, Source } from '../types/api'

const sourceKinds = ['manual', 'category', 'backlinks', 'transclusions', 'search', 'watchlist', 'contributions', 'links_on_page', 'categories_on_page', 'redirects_to_page']
const blankMask = (): RuleMask => ({comments: false, tags: false, references: false, links: false, headings: false, templates: false, link_targets: false})
const maskOptions: Array<keyof RuleMask> = ['comments', 'tags', 'references', 'links', 'headings', 'templates', 'link_targets']
const blankRule = (): Rule => ({ name: '', enabled: true, operation: 'replace', parameter: '', mask: blankMask(), find: '', replace: '', regex: false, case_sensitive: true, replace_all: true, multiline: false, dot_matches_new_line: false })
const blankSource = (kind = 'manual'): Source => ({ kind, label: '', value: kind === 'watchlist' ? 'My watchlist' : '', summary: '' })
const skipToggles: Array<[keyof SkipOptions, string]> = [['no_change', 'findsmith-project-skip-no-change'], ['redirects', 'findsmith-project-skip-redirects'], ['protected', 'findsmith-project-skip-protected'], ['outside_namespaces', 'findsmith-project-skip-outside-namespaces']]

function projectForm(project: Project, stateVersion = project.state_version) {
  const values = new URLSearchParams({ name: project.project_name, wiki_id: project.wiki_id, queue_limit: String(project.queue_limit), page_title: project.page_title, state_version: String(stateVersion), csrf_token: project.csrf_token })
  const skip = project.skip
  values.append('skip_no_change', String(skip.no_change)); values.append('skip_redirects', String(skip.redirects)); values.append('skip_protected', String(skip.protected)); values.append('skip_outside_namespaces', String(skip.outside_namespaces))
  values.append('skip_contains', skip.contains); values.append('skip_not_contains', skip.not_contains); values.append('skip_regex', String(skip.regex)); values.append('skip_case_sensitive', String(skip.case_sensitive))
  values.append('skip_min_replacements', String(skip.min_replacements)); values.append('skip_max_replacements', String(skip.max_replacements))
  project.sources.forEach(source => { values.append('source_kind', source.kind); values.append('source_value', source.value) })
  project.namespace_options.filter(option => option.selected).forEach(option => values.append('namespaces', String(option.id)))
  project.rules.forEach((rule, index) => {
    values.append('rule_name', rule.name)
    values.append('rule_operation', rule.operation ?? 'replace')
    values.append('rule_parameter', rule.parameter ?? '')
    maskOptions.forEach(key => { if (rule.mask?.[key]) values.append(`rule_mask_${key}`, String(index)) })
    values.append('rule_find', rule.find); values.append('rule_replace', rule.replace)
    values.append('rule_type', rule.regex ? 'regex' : 'plain'); values.append('rule_case', rule.case_sensitive ? 'sensitive' : 'insensitive'); values.append('rule_scope', rule.replace_all ? 'all' : 'first')
    if (rule.enabled) values.append('rule_enabled', String(index))
    if (rule.multiline) values.append('rule_multiline', String(index))
    if (rule.dot_matches_new_line) values.append('rule_dot_newline', String(index))
  })
  return values
}

export function ProjectPage({ projectId, jobId }: { projectId: string; jobId?: string }) {
  const { t } = useI18n()
  const queueStatusLabel = (status: string) => t(`findsmith-project-${status}`)
  const reviewStatusLabel = (status: string) => t(`findsmith-api-${status === 'building' ? 'building-queue' : status}`)
  const editResultLabel = (result: string) => result === 'saved' ? t('findsmith-project-saved') : t(`findsmith-project-result-${result.replaceAll(/[_ ]/g, '-')}`)
  const ruleOperationLabel = (operation: Rule['operation']) => t(({
    replace: 'findsmith-project-find-and-replace',
    append: 'findsmith-project-append-wikitext',
    prepend: 'findsmith-project-prepend-wikitext',
    category_add: 'findsmith-project-add-category',
    category_remove: 'findsmith-project-remove-category',
    category_replace: 'findsmith-project-replace-category',
    template_rename: 'findsmith-project-rename-template',
    template_parameter_set: 'findsmith-project-set-template-parameter',
    template_parameter_remove: 'findsmith-project-remove-template-parameter'
  })[operation ?? 'replace'])
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
  const [rulesTab, setRulesTab] = useState<'rules' | 'options' | 'status'>('rules')
  const [autosave, setAutosave] = useState<{ state: 'idle' | 'saving' | 'saved' | 'failed'; message?: string }>({ state: 'idle' })
  const [rendered, setRendered] = useState<{ projectId: string; requestedTitle: string; requestedText: string; data: RenderedPreviewData } | null>(null)
  const [renderBusy, setRenderBusy] = useState<string | null>(null)
  const [renderError, setRenderError] = useState<{ title: string; message: string } | null>(null)
  const [queueFilter, setQueueFilter] = useState('')
  const [queueStatus, setQueueStatus] = useState('all')
  const [comparison, setComparison] = useState('')
  const [preparseError, setPreparseError] = useState('')
  const [policyAcknowledged, setPolicyAcknowledged] = useState(false)
  const [minorEdit, setMinorEdit] = useState(false)
  const [botEdit, setBotEdit] = useState<boolean | null>(null)
  const [editSummary, setEditSummary] = useState('')
  const renderRequest = useRef(0)
  // Autosave bookkeeping: every persisted edit bumps editRevision; the server
  // state version is tracked separately so explicit saves never conflict with it.
  const projectRef = useRef<Project | null>(null)
  const editRevision = useRef(0)
  const savedRevision = useRef(0)
  const stateVersion = useRef(0)
  const pendingAutosave = useRef<Promise<void> | null>(null)
  projectRef.current = project
  const activeJobId = jobId ?? project?.latest_job_id ?? ''
  const reviewPath = activeJobId ? `/api/projects/${projectId}/jobs/${activeJobId}` : ''

  useEffect(() => { setPolicyAcknowledged(false) }, [activeJobId])
  useEffect(() => {
    if (review) setEditSummary(review.suggested_edit_summary)
  }, [review?.job_id, review?.position, review?.current?.title])

  async function loadProject() {
    const data = await getJson<Project>(`/api/projects/${projectId}`)
    stateVersion.current = data.state_version; savedRevision.current = editRevision.current
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
            ? { ...current, awb_access: { ...current.awb_access, allowed: null, error: error instanceof Error ? error.message : t('findsmith-project-error-awb-check') } }
            : current)
        })
    }, 5 * 60 * 1000)
    return () => { cancelled = true; window.clearInterval(timer) }
  }, [projectId])
  useEffect(() => { void getJson<SavedRule[]>('/api/rules').then(setSavedRules).catch(error => setLibraryError(error instanceof Error ? error.message : t('findsmith-project-error-saved-rules-load'))) }, [projectId])
  useEffect(() => {
    if (!project?.wiki_id) return
    setBotEdit(null)
    setSession(null); setGroupsError('')
    void getJson<Session>(`/api/session?include_groups=true&wiki_id=${encodeURIComponent(project.wiki_id)}`).then(setSession).catch(error => setGroupsError(error instanceof Error ? error.message : t('findsmith-project-error-groups-load')))
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
        .catch(error => setPreparseError(error instanceof Error ? error.message : t('findsmith-project-error-preparse')))
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

  useEffect(() => {
    if (!project || editRevision.current === savedRevision.current) return
    const timer = window.setTimeout(() => { void runAutosave() }, 1500)
    return () => window.clearTimeout(timer)
  }, [project])
  useEffect(() => {
    if (error || busy || textDirty || !review?.page?.skip_reason || review.status !== 'active' || review.preparse_active || review.current?.status !== 'pending') return
    const timer = window.setTimeout(() => { void reviewAction('next') }, 600)
    return () => window.clearTimeout(timer)
  }, [review?.state_version, review?.page?.skip_reason, review?.current?.title, busy, textDirty, error])

  async function runAutosave() {
    const current = projectRef.current
    if (!current || editRevision.current === savedRevision.current || pendingAutosave.current) return
    const revision = editRevision.current
    setAutosave({ state: 'saving' })
    const request = postForm<{ state_version: number }>(`/api/projects/${projectId}/autosave`, projectForm(current, stateVersion.current))
      .then(result => {
        stateVersion.current = result.state_version; savedRevision.current = Math.max(savedRevision.current, revision)
        setProject(latest => latest ? { ...latest, state_version: result.state_version } : latest)
        setAutosave({ state: 'saved' })
      })
      .catch(error => setAutosave({ state: 'failed', message: error instanceof Error ? error.message : t('findsmith-request-failed') }))
      .finally(() => { pendingAutosave.current = null })
    pendingAutosave.current = request
    await request
  }
  async function flushAutosave() {
    await pendingAutosave.current
    if (editRevision.current !== savedRevision.current) await runAutosave()
  }

  function update(patch: Partial<Project>, persist = true) {
    if (persist) editRevision.current += 1
    setProject(current => current ? { ...current, ...patch, ...('rules' in patch ? { page_preview: null, page_preview_error: null } : {}) } : current)
    if (persist) setConfigurationDirty(true)
  }
  function updateSkip(patch: Partial<SkipOptions>) { if (project) update({ skip: { ...project.skip, ...patch } }) }
  function move<T>(items: T[], index: number, direction: number) { const copy = [...items]; const target = index + direction; if (target < 0 || target >= copy.length) return copy; [copy[index], copy[target]] = [copy[target], copy[index]]; return copy }
  function allowReplacingManualText() { return !textDirty || window.confirm(t('findsmith-project-confirm-discard-manual')) }

  async function loadRendered(title: string, proposedText: string) {
    if (!project) return
    const request = ++renderRequest.current
    setRenderBusy(title); setRenderError(null)
    try {
      const data = await postForm<RenderedPreviewData>(`/api/projects/${projectId}/render-preview`, new URLSearchParams({ csrf_token: project.csrf_token, title, proposed_text: proposedText }))
      if (request === renderRequest.current) setRendered({ projectId, requestedTitle: title, requestedText: proposedText, data })
    } catch (error) {
      if (request === renderRequest.current) setRenderError({ title, message: error instanceof Error ? error.message : t('findsmith-api-could-not-render-preview') })
    } finally { if (request === renderRequest.current) setRenderBusy(null) }
  }

  function showRendered(title: string, proposedText: string) {
    setPreviewMode('preview')
    if (!rendered || rendered.projectId !== projectId || rendered.requestedTitle !== title) void loadRendered(title, proposedText)
  }

  async function saveRule(rule: Rule) {
    if (!project) return
    setBusy(t('findsmith-project-busy-saving-rule')); setLibraryError('')
    try {
      await postForm<SavedRule>('/api/rules', new URLSearchParams({ csrf_token: project.csrf_token, rule_json: JSON.stringify(rule) }))
      setSavedRules(await getJson<SavedRule[]>('/api/rules'))
    } catch (error) { setLibraryError(error instanceof Error ? error.message : t('findsmith-api-could-not-save-rule')) }
    finally { setBusy('') }
  }
  async function deleteSavedRule(item: SavedRule) {
    if (!project || !window.confirm(t('findsmith-project-confirm-delete-saved-rule', item.name))) return
    setBusy(t('findsmith-project-busy-deleting-rule')); setLibraryError('')
    try {
      await postForm(`/api/rules/${item.id}/delete`, new URLSearchParams({ csrf_token: project.csrf_token }))
      setSavedRules(current => current.filter(rule => rule.id !== item.id))
    } catch (error) { setLibraryError(error instanceof Error ? error.message : t('findsmith-project-error-deleting-rule')) }
    finally { setBusy('') }
  }

  async function submitProject(path: string, label: string, refreshReview = false, createsJob = false) {
    if (!project || (refreshReview && !allowReplacingManualText())) return
    setBusy(label); setError('')
    try {
      await pendingAutosave.current
      const result = await postForm<ActionResult | Project>(path, projectForm(project, stateVersion.current))
      if ('page_preview' in result) setProject(current => current ? { ...current, page_preview: result.page_preview, page_preview_error: result.page_preview_error } : current)
      else {
        const saved = await loadProject()
        if (refreshReview) {
          const nextJobId = 'id' in result && result.id && (createsJob || result.id !== projectId) ? result.id : saved.latest_job_id
          if (nextJobId && nextJobId !== activeJobId) navigate(`/projects/${projectId}/jobs/${nextJobId}`)
          else await loadReview()
        }
      }
    } catch (error) { setError(error instanceof Error ? error.message : t('findsmith-request-failed')) }
    finally { setBusy('') }
  }
  async function duplicateProject() {
    if (!project) return
    setBusy(t('findsmith-project-busy-duplicating')); setError('')
    try {
      await flushAutosave()
      const result = await postForm<ActionResult>(`/api/projects/${projectId}/duplicate`, new URLSearchParams({ csrf_token: project.csrf_token }))
      navigate(result.redirect)
    } catch (error) { setError(error instanceof Error ? error.message : t('findsmith-project-error-duplicating-project')) }
    finally { setBusy('') }
  }
  async function removeProject() {
    if (!project || !window.confirm(t('findsmith-project-confirm-delete-named-project', project.project_name))) return
    setBusy(t('findsmith-project-busy-deleting'))
    try { const result = await postForm<ActionResult>(`/api/projects/${projectId}/delete`, new URLSearchParams({ csrf_token: project.csrf_token, confirmation: 'delete' })); navigate(result.redirect) }
    catch (error) { setError(error instanceof Error ? error.message : t('findsmith-project-error-deleting-project')); setBusy('') }
  }
  async function reviewAction(name: string) {
    if (!review || !reviewPath || ((name === 'next' || name === 'skip' || name === 'previous' || name === 'preparse_start') && !allowReplacingManualText())) return
    if (name === 'preparse_start') setPreparseError('')
    setBusy(name); setError('')
    try { await postForm<ActionResult>(`${reviewPath}/action`, new URLSearchParams({ csrf_token: review.csrf_token, state_version: String(review.state_version), action: name })); await Promise.all([loadProject(), loadReview()]) }
    catch (error) { setError(error instanceof Error ? error.message : t('findsmith-project-error-review-action')) }
    finally { setBusy('') }
  }
  async function savePage() {
    if (!review || !reviewPath || !review.page || !['active', 'complete'].includes(review.status) || !['pending', 'reviewed'].includes(review.current?.status ?? '') || textDirty || !review.page.changed || !(policyAcknowledged || review.policy_acknowledged)) return
    setBusy(t('findsmith-project-busy-saving-edit')); setError('')
    try {
      await postForm<ActionResult>(`${reviewPath}/action`, new URLSearchParams({
        csrf_token: review.csrf_token, state_version: String(review.state_version), action: 'save',
        proposed_text: text, base_revid: String(review.page.revision_id), start_timestamp: review.page.server_timestamp,
        policy_acknowledged: String(policyAcknowledged || review.policy_acknowledged),
        minor: String(minorEdit && review.page.can_minor), bot: String((botEdit ?? review.page.can_bot) && review.page.can_bot),
        edit_summary: editSummary,
      }))
      await Promise.all([loadProject(), loadReview()])
    } catch (error) { setError(error instanceof Error ? error.message : t('findsmith-project-error-saving-edit')) }
    finally { setBusy('') }
  }
  async function recompute() {
    if (!review || !reviewPath) return
    setBusy(t('findsmith-project-busy-recomputing')); setError('')
    try { const data = await postForm<Review>(`${reviewPath}/recompute`, new URLSearchParams({ csrf_token: review.csrf_token, proposed_text: text })); setReview(data); if (data.page) setText(data.page.proposed_text); setTextDirty(false) }
    catch (error) { setError(error instanceof Error ? error.message : t('findsmith-project-error-recomputing')) }
    finally { setBusy('') }
  }
  async function queueAction(actionName: string, index = 0, titles = '') {
    if (!review || !reviewPath || !allowReplacingManualText()) return
    setBusy(actionName); setError('')
    const values = new URLSearchParams({ csrf_token: review.csrf_token, state_version: String(review.state_version), action: actionName, index: String(index), titles })
    try { await postForm<ActionResult>(`${reviewPath}/queue`, values); await Promise.all([loadProject(), loadReview()]) }
    catch (error) { setError(error instanceof Error ? error.message : t('findsmith-project-error-queue-action')) }
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

  if (!project) return <Shell title={t('findsmith-workspace-title')}><main><Busy label={error || t('findsmith-project-loading-project')} /></main></Shell>
  const building = review?.status === 'building'
  const tab = !review && rulesTab === 'status' ? 'rules' : rulesTab
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
  const fullEditSummary = review ? t('findsmith-edit-summary-with-attribution', editSummary.trim(), review.edit_summary_attribution_link) : ''
  const summaryLength = Array.from(fullEditSummary).length
  const showSaveControls = review?.page && !review.preparse_active && ['active', 'complete'].includes(review.status) && ['pending', 'reviewed'].includes(review.current?.status ?? '')
  const saveBlockers = review?.page ? [
    !['pending', 'reviewed'].includes(review.current?.status ?? '') && t('findsmith-project-save-blocker-not-reviewable'),
    !review.page.changed && t('findsmith-project-save-blocker-no-changes'),
    !editSummary.trim() && t('findsmith-project-save-blocker-summary-required'),
    summaryLength > 500 && t('findsmith-project-save-blocker-summary-length'),
    !!review.edit_summary_attribution_link && editSummary.includes(review.edit_summary_attribution_link) && t('findsmith-project-save-blocker-attribution'),
    textDirty && t('findsmith-project-save-blocker-recompute'),
    !(policyAcknowledged || review.policy_acknowledged) && t('findsmith-project-save-blocker-policies'),
    !review.page.editable && t('findsmith-project-save-blocker-page-rights'),
    !review.page.can_edit && (review.page.edit_access_message || t('findsmith-project-save-blocker-rights-unverified')),
    project.awb_access?.allowed !== true && (project.awb_access?.allowed === false ? t('findsmith-project-save-blocker-awb-list') : t('findsmith-project-save-blocker-awb-check')),
  ].filter((reason): reason is string => typeof reason === 'string') : []
  const commandStack = <>
    {review && (configurationDirty || project.latest_job_outdated) && <p className="unsaved-note">{t('findsmith-project-configuration-changed-apply-rules-to-refresh-the-current-review')}</p>}
    {autosave.state !== 'idle' && <p className="autosave-status" role="status" aria-live="polite">{autosave.state === 'saving' ? t('findsmith-project-busy-saving') : autosave.state === 'saved' ? t('findsmith-project-autosave-saved') : t('findsmith-project-autosave-failed', autosave.message ?? '')}</p>}
    <div className="command-stack"><button className="primary" disabled={!!busy} onClick={() => submitProject(`/api/projects/${projectId}`, review ? t('findsmith-project-busy-applying-rules') : t('findsmith-project-busy-saving'), Boolean(review))}>{review ? t('findsmith-project-apply-rules') : t('findsmith-project-save-configuration')}</button><button disabled={!!busy} onClick={duplicateProject}>{t('findsmith-project-duplicate-project')}</button><button className="danger" onClick={removeProject}>{t('findsmith-project-delete-project')}</button></div>
  </>
  function renderPanel() {
    if (!previewTitle || previewText === undefined) return <div className="empty preview-empty">{t('findsmith-project-select-a-page-to-render')}</div>
    const isRendering = renderBusy === previewTitle
    return <div className="rendered-preview-panel">
      <div className="rendered-preview-toolbar"><span>{renderStale ? t('findsmith-project-preparse-stale') : t('findsmith-project-rendered-by-wiki')}</span><button disabled={isRendering} onClick={() => loadRendered(previewTitle, previewText)}>{isRendering ? t('findsmith-project-rendering') : t('findsmith-project-refresh-preview')}</button></div>
      {renderError?.title === previewTitle && <ErrorBox message={renderError.message} />}
      {matchingRendered ? <RenderedPreview preview={matchingRendered.data} /> : <div className="empty preview-empty">{isRendering ? t('findsmith-project-rendering-preview') : t('findsmith-project-click-refresh-preview')}</div>}
    </div>
  }
  return <Shell title={project.project_name} subtitle={review ? t('findsmith-project-review-subtitle', review.position, review.total, review.pending, review.reviewed, review.skipped) : t('findsmith-project-config-subtitle', project.wiki_id, project.state_version)}>
    {error && <ErrorBox message={error} />}
    {project.awb_access?.allowed === false && <div className="notice" role="status"><p>{t('findsmith-project-awb-access-warning')}</p><a href={project.awb_access.check_page} target="_blank" rel="noreferrer">{t('findsmith-project-view-checkpagejson')}</a></div>}
    {project.awb_access?.allowed === null && <div className="notice" role="status">{t('findsmith-project-access-error', project.awb_access.error ?? '')}</div>}
    <main className="awb-workspace">
      <section className="pane sources-pane">
        <div className="pane-title">{t('findsmith-project-make-list')} {review && <span>{t('findsmith-project-pages-count', review.total)}</span>}</div>
        <details open={!review} className="source-settings"><summary>{t('findsmith-project-page-sources-and-filters')}</summary>
          <label>{t('findsmith-project-source')}<select id="new-source">{sourceKinds.map(kind => <option value={kind} key={kind}>{t(`findsmith-project-source-${kind.replaceAll('_', '-')}`)}</option>)}</select></label>
          <button onClick={() => { const element = document.getElementById('new-source') as HTMLSelectElement; update({ sources: [...project.sources, blankSource(element.value)] }) }}>{t('findsmith-project-add-source')}</button>
          <div className="source-list listbox">{project.sources.map((source, index) => <div className="source-card" key={index}>
            <div><strong>{t(`findsmith-project-source-${source.kind.replaceAll('_', '-')}`)}</strong><span className="row-actions"><button aria-label={t('findsmith-project-move-item-up', t(`findsmith-project-source-${source.kind.replaceAll('_', '-')}`))} disabled={index === 0} onClick={() => update({sources: move(project.sources, index, -1)})}>↑</button><button aria-label={t('findsmith-project-move-item-down', t(`findsmith-project-source-${source.kind.replaceAll('_', '-')}`))} disabled={index === project.sources.length - 1} onClick={() => update({sources: move(project.sources, index, 1)})}>↓</button><button aria-label={t('findsmith-project-remove-item', t(`findsmith-project-source-${source.kind.replaceAll('_', '-')}`))} onClick={() => update({ sources: project.sources.filter((_, item) => item !== index) })}>×</button></span></div>
            <select value={source.kind} onChange={event => { const sources = [...project.sources]; sources[index] = blankSource(event.target.value); update({sources}) }}>{sourceKinds.map(kind => <option value={kind} key={kind}>{t(`findsmith-project-source-${kind.replaceAll('_', '-')}`)}</option>)}</select>
            {source.kind !== 'watchlist' && <textarea value={source.value} placeholder={source.kind === 'contributions' ? t('findsmith-project-source-username-placeholder') : t('findsmith-project-source-value-placeholder')} onChange={event => { const sources = [...project.sources]; sources[index] = {...source, value: event.target.value}; update({sources}) }} />}
          </div>)}{!project.sources.length && <p className="empty">{t('findsmith-project-add-a-source-to-build-a-page-list')}</p>}</div>
          <fieldset><legend>{t('findsmith-project-namespaces')}</legend><div className="namespace-list">{project.namespace_options.map((option, index) => <label key={option.id}><input type="checkbox" checked={option.selected} onChange={event => { const options = [...project.namespace_options]; options[index] = {...option, selected: event.target.checked}; update({namespace_options: options}) }} />{option.label}</label>)}</div></fieldset>
          <label>{t('findsmith-project-maximum-pages')}<input type="number" min="1" max="500" value={project.queue_limit} onChange={event => update({queue_limit: Number(event.target.value)})} /></label>
        </details>
        <button className="primary build-list" disabled={!!busy} onClick={() => submitProject(`/api/projects/${projectId}/build`, t('findsmith-project-busy-building'), true, true)}>{t('findsmith-project-build-page-list')}</button>
        {review && <>{building && <div className="generation"><progress /><p>{review.generation_message}</p><button className="danger" onClick={() => reviewAction('stop')}>{t('findsmith-project-cancel-generation')}</button></div>}
          <details className="queue-tools"><summary>{t('findsmith-project-list-tools')}</summary><label>{t('findsmith-project-find-title')}<input value={queueFilter} onChange={event => setQueueFilter(event.target.value)} placeholder={t('findsmith-project-filter-placeholder')} /></label><label>{t('findsmith-project-status')}<select value={queueStatus} onChange={event => setQueueStatus(event.target.value)}><option value="all">{t('findsmith-project-all-statuses')}</option><option value="pending">{t('findsmith-project-pending')}</option><option value="reviewed">{t('findsmith-project-reviewed')}</option><option value="saved">{t('findsmith-project-saved')}</option><option value="skipped">{t('findsmith-project-skipped')}</option><option value="missing">{t('findsmith-project-missing')}</option><option value="invalid">{t('findsmith-project-invalid')}</option></select></label><p>{t('findsmith-project-shown-count', shownQueue.length, review.queue.length)}</p><div className="row-actions"><button disabled={building || !!busy} onClick={() => queueAction('sort_by_title')}>{t('findsmith-project-sort-by-title')}</button><button onClick={exportQueue}>{t('findsmith-project-export-csv')}</button></div><label>{t('findsmith-project-compare-with-titles')}<textarea value={comparison} onChange={event => setComparison(event.target.value)} placeholder={t('findsmith-project-compare-placeholder')} /></label>{comparison.trim() && <><p>{t('findsmith-project-comparison-count', comparisonOnly.length, queueOnly.length, comparisonTitles.length - comparisonOnly.length)}</p><details><summary>{t('findsmith-project-show-differences')}</summary><strong>{t('findsmith-project-only-in-pasted-list')}</strong><ul>{comparisonOnly.map(title => <li key={title}>{title}</li>)}</ul><strong>{t('findsmith-project-only-in-queue')}</strong><ul>{queueOnly.map(entry => <li key={entry.title}>{entry.title}</li>)}</ul></details></>}</details>
          <div className="queue-list listbox">{shownQueue.map(({entry, index}) => <div className={`queue-row ${index + 1 === review.position ? 'selected' : ''}`} key={`${entry.title}-${index}`}><button type="button" className="queue-select" disabled={building || !!busy} aria-current={index + 1 === review.position ? 'page' : undefined} onClick={() => queueAction('select', index)}><span>{index + 1}</span><strong title={entry.reason ? `${entry.title}\n${entry.reason}` : entry.title}>{entry.title}</strong></button><i>{queueStatusLabel(entry.status)}</i>{!building && <span className="row-actions"><button aria-label={t('findsmith-project-move-item-up', entry.title)} disabled={index === 0} onClick={() => queueAction('move_up', index)}>↑</button><button aria-label={t('findsmith-project-move-item-down', entry.title)} disabled={index === review.queue.length - 1} onClick={() => queueAction('move_down', index)}>↓</button><button aria-label={t('findsmith-project-remove-item', entry.title)} onClick={() => queueAction('remove', index)}>×</button></span>}</div>)}</div>
          {!building && <><AppendPages onAppend={titles => queueAction('append', 0, titles)} /><button className="danger clear-queue" disabled={!review.queue.length} onClick={() => { if (window.confirm(t('findsmith-project-confirm-clear-queue'))) void queueAction('clear') }}>{t('findsmith-project-clear-queue')}</button></>}</>}
      </section>

      <section className="pane rules-pane">
        <div className="tabs"><button type="button" className={tab === 'rules' ? 'active' : ''} onClick={() => setRulesTab('rules')}>{t('findsmith-project-rules')}</button><button type="button" className={tab === 'options' ? 'active' : ''} onClick={() => setRulesTab('options')}>{t('findsmith-project-options')}</button>{review && <button type="button" className={tab === 'status' ? 'active' : ''} onClick={() => setRulesTab('status')}>{t('findsmith-project-status')}</button>}</div>
        {tab === 'rules' && <>
        <label>{t('findsmith-workspace-name')}<input value={project.project_name} maxLength={255} onChange={event => update({project_name: event.target.value})} /></label>
        <fieldset><legend>{t('findsmith-home-account')}</legend><p>{t('findsmith-project-groups', session ? (session.groups.length ? session.groups.join(', ') : t('findsmith-project-none-reported')) : (groupsError ? t('findsmith-project-unavailable-lower') : t('findsmith-project-loading-lower')))}</p>{session?.groups_error && <p className="error">{session.groups_error}</p>}{groupsError && <p className="error">{groupsError}</p>}</fieldset>
          {review && <>{active && (review.preparse_active ? <div className="generation"><progress value={review.preparse_cursor} max={review.total || 1} /><p>{t('findsmith-project-preparse-progress', review.preparse_cursor, review.total)}</p>{preparseError && <ErrorBox message={preparseError} />}<button className="danger" disabled={!!busy} onClick={() => reviewAction('preparse_stop')}>{t('findsmith-project-stop-pre-parse')}</button></div> : <><div className="review-buttons"><button disabled={!!busy} onClick={() => reviewAction('previous')}>{t('findsmith-project-previous')}</button><button disabled={!!busy} onClick={() => reviewAction('skip')}>{t('findsmith-project-skip')}</button><button className="danger" disabled={!!busy} onClick={() => reviewAction('stop')}>{t('findsmith-project-stop-review')}</button><button className="primary" disabled={!!busy} onClick={() => reviewAction('next')}>{t('findsmith-project-next')}</button></div><p className="keyboard-hint">{t('findsmith-project-shortcuts-when-no-field-is-focused-n-next-p-previous-s-skip')}</p><button disabled={!!busy} onClick={() => reviewAction('preparse_start')}>{t('findsmith-project-pre-parse-queue')}</button></>)}{review.status === 'stopped' && !review.edit_in_flight && !review.edit_policy_blocked && <button className="primary resume-review" disabled={!!busy} onClick={() => reviewAction('resume')}>{t('findsmith-project-resume-review')}</button>}{review.current?.reason && <p className="notice">{review.current.reason}</p>}{review.page?.skip_reason && review.current?.status === 'pending' && active && <p className="notice" role="status">{t('findsmith-project-skip-auto-notice', review.page.skip_reason)}</p>}{review.edit_in_flight && <div className="notice" role="status"><p>{t('findsmith-project-edit-in-flight-notice')}</p><button disabled={!!busy} onClick={() => reviewAction('verify_edit')}>{t('findsmith-project-verify-edit')}</button></div>}</>}
          {showSaveControls && review && review.page && <fieldset className="edit-controls"><legend>{t('findsmith-project-save-to-wikimedia')}</legend><div className="edit-summary-control"><label htmlFor="edit-summary">{t('findsmith-project-edit-summary')}</label><input id="edit-summary" value={editSummary} onChange={event => setEditSummary(event.target.value)} /><span className="edit-summary-preview">{fullEditSummary}</span><small>{t('findsmith-project-summary-length', summaryLength)}</small></div><label><input type="checkbox" checked={policyAcknowledged || review.policy_acknowledged} disabled={review.policy_acknowledged} onChange={event => setPolicyAcknowledged(event.target.checked)} /> {t('findsmith-project-i-will-follow-this-wiki-s-editing-policies')}</label><label><input type="checkbox" checked={minorEdit && review.page.can_minor} disabled={!review.page.can_minor} onChange={event => setMinorEdit(event.target.checked)} /> {t('findsmith-project-minor-edit')}</label><label><input type="checkbox" checked={(botEdit ?? review.page.can_bot) && review.page.can_bot} disabled={!review.page.can_bot} onChange={event => setBotEdit(event.target.checked)} /> {t('findsmith-project-bot-edit')}</label><button className="primary" disabled={!!busy || saveBlockers.length > 0} onClick={savePage}>{t('findsmith-project-save')}</button>{saveBlockers.length > 0 && <ul className="save-blockers" aria-live="polite">{saveBlockers.map(reason => <li key={reason}>{reason}</li>)}</ul>}{active && <p>{t('findsmith-project-next-records-the-review-without-editing-wikimedia')}</p>}</fieldset>}
        <div className="rule-list">{project.rules.map((rule, index) => <article className={`rule-card ${rule.operation?.startsWith('category_') || rule.operation?.startsWith('template_') ? 'structural-rule' : ''}`} key={index}>
          <div className="rule-head"><label><input type="checkbox" checked={rule.enabled} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, enabled: event.target.checked}; update({rules}) }} /> {rule.name || t('findsmith-project-rule-number', index + 1)} <small>· {t(rule.operation?.startsWith('category_') ? 'findsmith-project-rule-kind-category' : rule.operation?.startsWith('template_') ? 'findsmith-project-rule-kind-template' : 'findsmith-project-rule-kind-text')}</small></label><span className="row-actions"><button aria-label={t('findsmith-project-move-item-up', rule.name || t('findsmith-project-rule-number', index + 1))} disabled={index === 0} onClick={() => update({rules: move(project.rules, index, -1)})}>↑</button><button aria-label={t('findsmith-project-move-item-down', rule.name || t('findsmith-project-rule-number', index + 1))} disabled={index === project.rules.length - 1} onClick={() => update({rules: move(project.rules, index, 1)})}>↓</button><button aria-label={t('findsmith-project-remove-item', rule.name || t('findsmith-project-rule-number', index + 1))} onClick={() => update({rules: project.rules.filter((_, item) => item !== index)})}>×</button></span></div>
          <label>{t('findsmith-project-name')}<input value={rule.name} maxLength={255} placeholder={t('findsmith-project-rule-name-placeholder')} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, name: event.target.value}; update({rules}) }} /></label>
          <label>{t('findsmith-project-operation')}<select value={rule.operation ?? 'replace'} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, operation: event.target.value as Rule['operation']}; update({rules}) }}><option value="replace">{t('findsmith-project-find-and-replace')}</option><option value="append">{t('findsmith-project-append-wikitext')}</option><option value="prepend">{t('findsmith-project-prepend-wikitext')}</option><option value="category_add">{t('findsmith-project-add-category')}</option><option value="category_remove">{t('findsmith-project-remove-category')}</option><option value="category_replace">{t('findsmith-project-replace-category')}</option><option value="template_rename">{t('findsmith-project-rename-template')}</option><option value="template_parameter_set">{t('findsmith-project-set-template-parameter')}</option><option value="template_parameter_remove">{t('findsmith-project-remove-template-parameter')}</option></select></label>
          {['replace', 'category_remove', 'category_replace', 'template_rename', 'template_parameter_set', 'template_parameter_remove'].includes(rule.operation ?? 'replace') && <label>{rule.operation === 'replace' ? t('findsmith-project-find') : rule.operation?.startsWith('template_') ? t('findsmith-project-template-name') : t('findsmith-project-category-to-find')}<input value={rule.find} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, find: event.target.value}; update({rules}) }} /></label>}
          {(rule.operation === 'template_parameter_set' || rule.operation === 'template_parameter_remove') && <label>{t('findsmith-project-parameter-name')}<input value={rule.parameter ?? ''} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, parameter: event.target.value}; update({rules}) }} /></label>}
          {rule.operation !== 'category_remove' && rule.operation !== 'template_parameter_remove' && <label>{rule.operation === 'replace' || !rule.operation ? t('findsmith-project-replace-with') : rule.operation === 'append' || rule.operation === 'prepend' ? t('findsmith-project-wikitext') : rule.operation === 'template_rename' ? t('findsmith-project-new-template-name') : rule.operation === 'template_parameter_set' ? t('findsmith-project-parameter-value') : t('findsmith-project-category-name')} {rule.operation === 'replace' && <small>{t('findsmith-project-leave-empty-to-delete-matches')}</small>}{rule.operation === 'append' || rule.operation === 'prepend' ? <textarea rows={5} value={rule.replace} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, replace: event.target.value}; update({rules}) }} /> : <input value={rule.replace} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, replace: event.target.value}; update({rules}) }} />}</label>}
          {(rule.operation ?? 'replace') === 'replace' && <><div className="rule-options"><label><input type="checkbox" checked={rule.regex} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, regex: event.target.checked}; update({rules}) }} />{t('findsmith-project-regex')}</label><label><input type="checkbox" checked={rule.case_sensitive} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, case_sensitive: event.target.checked}; update({rules}) }} />{t('findsmith-project-case-sensitive')}</label><label><input type="checkbox" checked={rule.replace_all} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, replace_all: event.target.checked}; update({rules}) }} />{t('findsmith-project-replace-all')}</label></div>
          {rule.regex && <div className="rule-options"><label><input type="checkbox" checked={rule.multiline} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, multiline: event.target.checked}; update({rules}) }} />{t('findsmith-project-multiline')}</label><label><input type="checkbox" checked={rule.dot_matches_new_line} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, dot_matches_new_line: event.target.checked}; update({rules}) }} />{t('findsmith-project-dot-matches-newline')}</label></div>}</>}
          {(rule.operation ?? 'replace') === 'replace' && <details className="rule-condition"><summary>{t('findsmith-project-protect-wikitext-regions')}</summary><p>{t('findsmith-project-skip-a-match-if-it-overlaps-a-selected-region')}</p><div className="rule-options">{maskOptions.map(key => <label key={key}><input type="checkbox" checked={rule.mask?.[key] ?? false} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, mask: {...(rule.mask ?? blankMask()), [key]: event.target.checked}}; update({rules}) }} />{t(`findsmith-project-mask-${key.replaceAll('_', '-')}`)}</label>)}</div></details>}
          {rule.operation?.startsWith('template_') && <label><input type="checkbox" className="inline-checkbox" checked={rule.replace_all} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, replace_all: event.target.checked}; update({rules}) }} /> {t('findsmith-project-apply-to-all-matching-templates')}</label>}
          <button disabled={!!busy || !rule.name.trim() || (['replace', 'category_remove', 'category_replace', 'template_rename', 'template_parameter_set', 'template_parameter_remove'].includes(rule.operation ?? 'replace') && !rule.find) || (['append', 'prepend', 'category_add', 'category_replace', 'template_rename'].includes(rule.operation ?? 'replace') && !rule.replace) || (rule.operation?.startsWith('template_parameter_') && !rule.parameter)} onClick={() => saveRule(rule)}>{t('findsmith-project-save-to-my-rules')}</button>
        </article>)}</div>
        <button onClick={() => update({rules: [...project.rules, blankRule()]})}>{t('findsmith-project-add-rule')}</button>
        <fieldset className="saved-rules"><legend>{t('findsmith-project-my-saved-rules')}</legend>
          {libraryError && <ErrorBox message={libraryError} />}
          {savedRules.length ? savedRules.map(item => <div className="saved-rule-row" key={item.id}>
            <span title={`${ruleOperationLabel(item.rule.operation)}: ${item.rule.find} → ${item.rule.replace}`}><strong>{item.name}</strong><small> · {ruleOperationLabel(item.rule.operation)}</small></span>
            <button disabled={!!busy || project.rules.length >= 100} onClick={() => update({rules: [...project.rules, {...item.rule}]})}>{t('findsmith-project-add-to-project')}</button>
            <button className="danger" disabled={!!busy} onClick={() => deleteSavedRule(item)}>{t('findsmith-project-delete')}</button>
          </div>) : <p className="empty">{t('findsmith-project-name-a-rule-and-save-it-here-to-use-it-in-other-projects')}</p>}
        </fieldset>
        {commandStack}
        </>}
        {tab === 'options' && <>
          <fieldset className="skip-options"><legend>{t('findsmith-project-skip-options')}</legend>
            {skipToggles.map(([key, label]) => <label key={key}><input type="checkbox" checked={Boolean(project.skip[key])} onChange={event => updateSkip({[key]: event.target.checked})} />{t(label)}</label>)}
            <label>{t('findsmith-project-skip-contains')}<input value={project.skip.contains} onChange={event => updateSkip({contains: event.target.value})} /></label>
            <label>{t('findsmith-project-skip-not-contains')}<input value={project.skip.not_contains} onChange={event => updateSkip({not_contains: event.target.value})} /></label>
            <div className="rule-options"><label><input type="checkbox" checked={project.skip.regex} onChange={event => updateSkip({regex: event.target.checked})} />{t('findsmith-project-skip-regex')}</label><label><input type="checkbox" checked={project.skip.case_sensitive} onChange={event => updateSkip({case_sensitive: event.target.checked})} />{t('findsmith-project-skip-case-sensitive')}</label></div>
            <label>{t('findsmith-project-skip-min-replacements')}<input type="number" min="0" value={project.skip.min_replacements} onChange={event => updateSkip({min_replacements: Math.max(0, Number(event.target.value) || 0)})} /></label>
            <label>{t('findsmith-project-skip-max-replacements')}<input type="number" min="0" value={project.skip.max_replacements} onChange={event => updateSkip({max_replacements: Math.max(0, Number(event.target.value) || 0)})} /></label>
            <small>{t('findsmith-project-skip-limit-help')}</small>
            <p className="keyboard-hint">{t('findsmith-project-skip-bot-exclusion-note')}</p>
          </fieldset>
          {commandStack}
        </>}
        {review && tab === 'status' && <>
          <fieldset><legend>{t('findsmith-project-review-progress')}</legend><dl><dt>{t('findsmith-project-status')}</dt><dd>{reviewStatusLabel(review.status)}</dd><dt>{t('findsmith-project-pending')}</dt><dd>{review.pending}</dd><dt>{t('findsmith-project-reviewed')}</dt><dd>{review.reviewed}</dd><dt>{t('findsmith-project-saved-to-wiki')}</dt><dd>{review.saved}</dd><dt>{t('findsmith-project-skipped')}</dt><dd>{review.skipped}</dd><dt>{t('findsmith-project-unavailable')}</dt><dd>{review.unavailable}</dd></dl></fieldset>
          <fieldset className="decision-history"><legend>{t('findsmith-project-decision-history')}</legend>{review.queue.filter(entry => entry.status !== 'pending' || entry.edit_result).length ? <ol>{review.queue.filter(entry => entry.status !== 'pending' || entry.edit_result).map((entry, index) => <li key={`${entry.title}-${index}`}><strong>{entry.title}</strong> — {queueStatusLabel(entry.status)}{entry.edit_result ? ` · ${editResultLabel(entry.edit_result)}` : ''}{entry.edit_revision_id ? ` · ${t('findsmith-project-revision', entry.edit_revision_id)}` : ''}{entry.reason ? `: ${entry.reason}` : ''}{entry.replacement_count !== null ? ` · ${t('findsmith-project-history-replacements', entry.replacement_count)}` : ''}</li>)}</ol> : <p className="empty">{t('findsmith-project-no-decisions-recorded-yet')}</p>}</fieldset>
        </>}
        {busy && <p className="busy">{busy}</p>}
      </section>

      <section className="pane editor-pane">
        <div className="tabs"><span className="active">{review ? t('findsmith-project-review') : t('findsmith-project-preview')}</span>{review && <><span className="inactive" aria-disabled="true">{t('findsmith-project-history')}</span><span className="inactive" aria-disabled="true">{t('findsmith-project-what-links-here')}</span><span className="inactive" aria-disabled="true">{t('findsmith-project-logs')}</span><span className="inactive" aria-disabled="true">{t('findsmith-project-typos')}</span></>}</div>
        {review ? <>
          {review.page_error && <ErrorBox message={review.page_error} />}
          {review.page ? <>
            <div className="page-heading"><a href={review.page.full_url} target="_blank" rel="noreferrer">{review.current?.title ?? t('findsmith-project-current-revision')}</a><strong title={t('findsmith-project-comparison-tooltip')}>{review.page.changed ? t('findsmith-project-proposed-changes') : t('findsmith-project-no-proposed-changes')}</strong></div>
            <div className="meta-strip"><span>{t('findsmith-project-revision', review.page.revision_id)}</span><span>{t('findsmith-project-namespace-short', review.current?.namespace ?? '—')}</span><span>{review.page.content_model}</span><span>{review.page.editable ? t('findsmith-project-editable') : t('findsmith-project-not-editable')}</span><span>{t('findsmith-project-protection', review.page.protection)}</span></div>
            <details className="wikitext-accordion"><summary>{t(textDirty ? 'findsmith-project-proposed-wikitext-modified' : 'findsmith-project-proposed-wikitext')}</summary><textarea className="editor" value={text} maxLength={4194304} onChange={event => { setText(event.target.value); setTextDirty(true) }} /><button onClick={recompute} disabled={!!busy}>{t('findsmith-project-recompute-diff')}</button></details>
            <div className="view-switch"><button className={previewMode === 'diff' ? 'active' : ''} onClick={() => setPreviewMode('diff')}>{t('findsmith-project-diff')}</button><button className={previewMode === 'preview' ? 'active' : ''} onClick={() => showRendered(review.current?.title ?? '', text)}>{t('findsmith-project-preview')}</button></div>
            {previewMode === 'preview' ? renderPanel() : <><div className="rule-counts">{review.page.rules.map((rule, index) => <span className={rule.changed ? 'changed' : ''} key={index}>{t('findsmith-project-rule-count', index + 1, rule.count)}</span>)}</div>{review.page.changed ? <SideDiff lines={review.page.side_by_side_diff} /> : <div className="no-diff">{t('findsmith-project-no-diff-the-proposed-wikitext-matches-the-current-revision')}</div>}</>}
          </> : <div className="empty preview-empty">{building ? review.generation_message : t('findsmith-project-no-reviewable-page')}</div>}
        </> : <>
          <div className="preview-tools"><label>{t('findsmith-project-page-title')}<input value={project.page_title} onChange={event => update({page_title: event.target.value, page_preview: null}, false)} placeholder={t('findsmith-project-example-page-placeholder')} /></label><button disabled={!!busy || !project.page_title.trim()} onClick={() => submitProject(`/api/projects/${projectId}/page-preview`, t('findsmith-project-busy-loading-preview'))}>{t('findsmith-project-preview')}</button></div>
          {project.page_preview_error && <ErrorBox message={project.page_preview_error} />}
          {project.page_preview ? <>
            <div className="page-heading"><a href={project.page_preview.full_url} target="_blank" rel="noreferrer">{project.page_preview.title}</a><strong>{t('findsmith-project-operations-count', project.page_preview.total_replacements)}</strong></div>
            <div className="meta-strip"><span>{t('findsmith-project-revision', project.page_preview.revision_id)}</span><span>{t('findsmith-project-namespace-short', project.page_preview.namespace)}</span><span>{project.page_preview.content_model}</span><span>{project.page_preview.redirect ? t('findsmith-project-redirect') : t('findsmith-project-page')}</span><span>{project.page_preview.editable ? t('findsmith-project-editable') : t('findsmith-project-not-editable')}</span><span>{t('findsmith-project-protection', project.page_preview.protection)}</span></div>
            <div className="view-switch"><button className={previewMode === 'diff' ? 'active' : ''} onClick={() => setPreviewMode('diff')}>{t('findsmith-project-diff')}</button><button className={previewMode === 'preview' ? 'active' : ''} onClick={() => showRendered(project.page_preview!.title, project.page_preview!.proposed_text)}>{t('findsmith-project-preview')}</button></div>
            {previewMode === 'preview' ? renderPanel() : <><div className="rule-counts">{project.page_preview.rules.map((rule, index) => <span className={rule.changed ? 'changed' : ''} key={index}>{t('findsmith-project-rule-count', index + 1, rule.count)}</span>)}</div>{project.page_preview.changed ? <SideDiff lines={project.page_preview.side_by_side_diff} /> : <div className="no-diff">{t('findsmith-project-no-diff-the-rules-did-not-change-this-page')}</div>}</>}
          </> : <div className="empty preview-empty">{t('findsmith-project-build-a-page-list-to-begin-review-or-enter-a-page-title-to-test-t')}</div>}
        </>}
      </section>
    </main>
  </Shell>
}

function AppendPages({ onAppend }: { onAppend: (titles: string) => void }) {
  const { t } = useI18n()
  const [titles, setTitles] = useState('')
  return <div className="append-pages"><textarea value={titles} onChange={event => setTitles(event.target.value)} placeholder={t('findsmith-project-append-titles-placeholder')} /><button onClick={() => { if (titles.trim()) { onAppend(titles); setTitles('') } }}>{t('findsmith-project-append')}</button></div>
}
