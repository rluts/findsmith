import { useEffect, useState } from 'react'
import { Busy, ErrorBox, Shell } from './App'
import { getJson, postForm } from './api'
import { SideDiff, UnifiedDiff } from './DiffView'
import type { ActionResult, Project, Rule, Source } from './types'

const sourceNames: Record<string, string> = { manual: 'Manual titles', category: 'Category members', backlinks: 'Backlinks', transclusions: 'Template transclusions', search: 'Wiki search' }
const blankRule = (): Rule => ({ enabled: true, find: '', replace: '', regex: false, case_sensitive: true, replace_all: true, multiline: false, dot_matches_new_line: false })
const blankSource = (kind = 'manual'): Source => ({ kind, label: sourceNames[kind], value: '', summary: '' })

function projectForm(project: Project) {
  const values = new URLSearchParams({
    name: project.project_name, wiki_id: project.wiki_id, queue_limit: String(project.queue_limit),
    page_title: project.page_title, state_version: String(project.state_version), csrf_token: project.csrf_token,
  })
  project.sources.forEach(source => { values.append('source_kind', source.kind); values.append('source_value', source.value) })
  project.namespace_options.filter(option => option.selected).forEach(option => values.append('namespaces', String(option.id)))
  project.rules.forEach((rule, index) => {
    values.append('rule_find', rule.find); values.append('rule_replace', rule.replace)
    values.append('rule_type', rule.regex ? 'regex' : 'plain'); values.append('rule_case', rule.case_sensitive ? 'sensitive' : 'insensitive')
    values.append('rule_scope', rule.replace_all ? 'all' : 'first')
    if (rule.enabled) values.append('rule_enabled', String(index))
    if (rule.multiline) values.append('rule_multiline', String(index))
    if (rule.dot_matches_new_line) values.append('rule_dot_newline', String(index))
  })
  return values
}

export function ProjectPage({ projectId }: { projectId: string }) {
  const [project, setProject] = useState<Project | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState('')
  const load = () => getJson<Project>(`/api/projects/${projectId}`).then(setProject).catch(error => setError(error.message))
  useEffect(() => { void load() }, [projectId])
  useEffect(() => {
    if (!project?.latest_job_building) return
    const timer = window.setInterval(load, 2000); return () => window.clearInterval(timer)
  }, [project?.latest_job_building, projectId])

  function update(patch: Partial<Project>) { setProject(current => current ? { ...current, ...patch } : current) }
  function move<T>(items: T[], index: number, direction: number) { const copy = [...items]; const target = index + direction; if (target < 0 || target >= copy.length) return copy; [copy[index], copy[target]] = [copy[target], copy[index]]; return copy }
  async function submit(path: string, label: string, navigate = false) {
    if (!project) return
    setBusy(label); setError('')
    try {
      const result = await postForm<ActionResult | Project>(path, projectForm(project))
      if (navigate && 'redirect' in result) window.location.assign(result.redirect)
      else if ('page_preview' in result) update({ page_preview: result.page_preview, page_preview_error: result.page_preview_error })
      else await load()
    } catch (error) { setError(error instanceof Error ? error.message : 'Request failed.') }
    finally { setBusy('') }
  }
  async function removeProject() {
    if (!project || !window.confirm(`Delete “${project.project_name}” permanently?`)) return
    setBusy('Deleting…')
    try { const result = await postForm<ActionResult>(`/api/projects/${projectId}/delete`, new URLSearchParams({ csrf_token: project.csrf_token, confirmation: 'delete' })); window.location.assign(result.redirect) }
    catch (error) { setError(error instanceof Error ? error.message : 'Could not delete project.'); setBusy('') }
  }
  if (!project) return <Shell title="Project"><main><Busy label={error || 'Loading project…'} /></main></Shell>
  return <Shell title={project.project_name} subtitle={`Project · ${project.wiki_id} · configuration version ${project.state_version}`}>
    {error && <ErrorBox message={error} />}
    <main className="awb-workspace">
      <section className="pane sources-pane">
        <div className="pane-title">Make list</div>
        <label>Source
          <select id="new-source"><option value="manual">Manual titles</option><option value="category">Category members</option><option value="backlinks">Backlinks</option><option value="transclusions">Template transclusions</option><option value="search">Wiki search</option></select>
        </label>
        <button onClick={() => { const element = document.getElementById('new-source') as HTMLSelectElement; update({ sources: [...project.sources, blankSource(element.value)] }) }}>Add source</button>
        <div className="source-list listbox">
          {project.sources.map((source, index) => <div className="source-card" key={index}>
            <div><strong>{sourceNames[source.kind]}</strong><span className="row-actions"><button disabled={index === 0} onClick={() => update({sources: move(project.sources, index, -1)})}>↑</button><button disabled={index === project.sources.length - 1} onClick={() => update({sources: move(project.sources, index, 1)})}>↓</button><button onClick={() => update({ sources: project.sources.filter((_, item) => item !== index) })}>×</button></span></div>
            <select value={source.kind} onChange={event => { const sources = [...project.sources]; sources[index] = {...source, kind: event.target.value, label: sourceNames[event.target.value]}; update({sources}) }}><option value="manual">Manual titles</option><option value="category">Category</option><option value="backlinks">Backlinks</option><option value="transclusions">Transclusions</option><option value="search">Search</option></select>
            <textarea value={source.value} placeholder="One value per line" onChange={event => { const sources = [...project.sources]; sources[index] = {...source, value: event.target.value}; update({sources}) }} />
          </div>)}
          {!project.sources.length && <p className="empty">Add a source to build a page list.</p>}
        </div>
        <fieldset><legend>Namespaces</legend><div className="namespace-list">{project.namespace_options.map((option, index) => <label key={option.id}><input type="checkbox" checked={option.selected} onChange={event => { const options = [...project.namespace_options]; options[index] = {...option, selected: event.target.checked}; update({namespace_options: options}) }} />{option.label}</label>)}</div></fieldset>
        <label>Maximum pages<input type="number" min="1" max="500" value={project.queue_limit} onChange={event => update({queue_limit: Number(event.target.value)})} /></label>
      </section>

      <section className="pane rules-pane">
        <div className="tabs"><span className="active">Rules</span><span>Options</span><span>Skip</span><span>Logs</span></div>
        <label>Project name<input value={project.project_name} maxLength={255} onChange={event => update({project_name: event.target.value})} /></label>
        <div className="rule-list">
          {project.rules.map((rule, index) => <article className="rule-card" key={index}>
            <div className="rule-head"><label><input type="checkbox" checked={rule.enabled} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, enabled: event.target.checked}; update({rules}) }} /> Rule {index + 1}</label><span className="row-actions"><button disabled={index === 0} onClick={() => update({rules: move(project.rules, index, -1)})}>↑</button><button disabled={index === project.rules.length - 1} onClick={() => update({rules: move(project.rules, index, 1)})}>↓</button><button onClick={() => update({rules: project.rules.filter((_, item) => item !== index)})}>×</button></span></div>
            <label>Find<input value={rule.find} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, find: event.target.value}; update({rules}) }} /></label>
            <label>Replace with <small>(leave empty to delete matches)</small><input value={rule.replace} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, replace: event.target.value}; update({rules}) }} /></label>
            <div className="rule-options"><label><input type="checkbox" checked={rule.regex} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, regex: event.target.checked}; update({rules}) }} />Regex</label><label><input type="checkbox" checked={rule.case_sensitive} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, case_sensitive: event.target.checked}; update({rules}) }} />Case sensitive</label><label><input type="checkbox" checked={rule.replace_all} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, replace_all: event.target.checked}; update({rules}) }} />Replace all</label></div>
            {rule.regex && <div className="rule-options"><label><input type="checkbox" checked={rule.multiline} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, multiline: event.target.checked}; update({rules}) }} />Multiline</label><label><input type="checkbox" checked={rule.dot_matches_new_line} onChange={event => { const rules = [...project.rules]; rules[index] = {...rule, dot_matches_new_line: event.target.checked}; update({rules}) }} />Dot matches newline</label></div>}
          </article>)}
        </div>
        <button onClick={() => update({rules: [...project.rules, blankRule()]})}>Add replacement rule</button>
        <div className="command-stack"><button className="primary" disabled={!!busy} onClick={() => submit(`/api/projects/${projectId}/build`, 'Building…', true)}>Build page list</button><button disabled={!!busy} onClick={() => submit(`/api/projects/${projectId}`, 'Saving…')}>Save configuration</button><button className="danger" onClick={removeProject}>Delete project</button></div>
        {busy && <p className="busy">{busy}</p>}
        {project.latest_job_id && <div className="job-summary"><strong>{project.latest_job_status}</strong><p>{project.latest_job_message}</p><p>{project.latest_job_total} queued · {project.latest_job_duplicates} duplicates{project.latest_job_limit_reached ? ' · limit reached' : ''}</p>{project.latest_queue_preview.length > 0 && <ol>{project.latest_queue_preview.map((entry, index) => <li key={`${entry.title}-${index}`}>{entry.title}</li>)}</ol>}<a className="button primary" href={`/projects/${projectId}/jobs/${project.latest_job_id}`}>Open review</a></div>}
      </section>

      <section className="pane preview-pane">
        <div className="tabs"><span className="active">Preview</span><span>History</span><span>Links</span><span>Page logs</span></div>
        <div className="preview-tools"><label>Page title<input value={project.page_title} onChange={event => update({page_title: event.target.value})} placeholder="Example page" /></label><button disabled={!!busy || !project.page_title.trim()} onClick={() => submit(`/api/projects/${projectId}/page-preview`, 'Loading preview…')}>Preview</button></div>
        {project.page_preview_error && <ErrorBox message={project.page_preview_error} />}
        {project.page_preview ? <>
          <div className="page-heading"><a href={project.page_preview.full_url} target="_blank" rel="noreferrer">{project.page_preview.title}</a><strong>{project.page_preview.total_replacements} replacement(s)</strong></div>
          <div className="meta-strip"><span>Revision {project.page_preview.revision_id}</span><span>NS {project.page_preview.namespace}</span><span>{project.page_preview.content_model}</span><span>{project.page_preview.redirect ? 'Redirect' : 'Page'}</span><span>{project.page_preview.editable ? 'Editable' : 'Not editable'}</span><span>Protection: {project.page_preview.protection}</span></div>
          <div className="rule-counts">{project.page_preview.rules.map((rule, index) => <span className={rule.changed ? 'changed' : ''} key={index}>Rule {index + 1}: {rule.count}</span>)}</div>
          <SideDiff lines={project.page_preview.side_by_side_diff} />
          <details><summary>Unified diff</summary><UnifiedDiff lines={project.page_preview.unified_diff} /></details>
        </> : <div className="empty preview-empty">Enter a page title to test the current rules.</div>}
      </section>
    </main>
  </Shell>
}
