import { useEffect, useState } from 'react'
import { Busy, ErrorBox, Shell } from './App'
import { getJson, postForm } from './api'
import { SideDiff, UnifiedDiff } from './DiffView'
import type { ActionResult, Review } from './types'

export function ReviewPage({ projectId, jobId }: { projectId: string; jobId: string }) {
  const [review, setReview] = useState<Review | null>(null)
  const [text, setText] = useState('')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState('')
  const path = `/api/projects/${projectId}/jobs/${jobId}`
  const load = () => getJson<Review>(path).then(data => { setReview(data); if (data.page) setText(data.page.proposed_text) }).catch(error => setError(error.message))
  useEffect(() => { void load() }, [path])
  useEffect(() => { if (review?.status !== 'building') return; const timer = window.setInterval(load, 2000); return () => window.clearInterval(timer) }, [review?.status, path])

  async function action(name: string) {
    if (!review) return
    setBusy(name); setError('')
    try { await postForm<ActionResult>(`${path}/action`, new URLSearchParams({ csrf_token: review.csrf_token, state_version: String(review.state_version), action: name })); await load() }
    catch (error) { setError(error instanceof Error ? error.message : 'Review action failed.') }
    finally { setBusy('') }
  }
  async function recompute() {
    if (!review) return
    setBusy('recompute'); setError('')
    try { const data = await postForm<Review>(`${path}/recompute`, new URLSearchParams({ csrf_token: review.csrf_token, proposed_text: text })); setReview(data); if (data.page) setText(data.page.proposed_text) }
    catch (error) { setError(error instanceof Error ? error.message : 'Could not recompute diff.') }
    finally { setBusy('') }
  }
  async function queue(actionName: string, index = 0, titles = '') {
    if (!review) return
    setBusy(actionName); setError('')
    const values = new URLSearchParams({ csrf_token: review.csrf_token, state_version: String(review.state_version), action: actionName, index: String(index), titles })
    try { await postForm<ActionResult>(`${path}/queue`, values); await load() }
    catch (error) { setError(error instanceof Error ? error.message : 'Queue action failed.') }
    finally { setBusy('') }
  }
  if (!review) return <Shell title="Review"><main><Busy label={error || 'Loading review…'} /></main></Shell>
  const building = review.status === 'building'
  const active = review.status === 'active'
  return <Shell title={`${review.project_name} · Review`} subtitle={`${review.position} / ${review.total} · ${review.pending} pending · ${review.reviewed} reviewed · ${review.skipped} skipped`}>
    {error && <ErrorBox message={error} />}
    <main className="awb-review">
      <section className="pane queue-pane">
        <div className="pane-title">Make list <span>{review.total} pages</span></div>
        {building && <div className="generation"><progress /><p>{review.generation_message}</p><button className="danger" onClick={() => action('stop')}>Cancel generation</button></div>}
        <div className="queue-list listbox">{review.queue.map((entry, index) => <div className={`queue-row ${index + 1 === review.position ? 'selected' : ''}`} key={`${entry.title}-${index}`}>
          <span>{index + 1}</span><strong title={entry.reason ?? ''}>{entry.title}</strong><i>{entry.status}</i>
          {!building && <span className="row-actions"><button disabled={index === 0} onClick={() => queue('move_up', index)}>↑</button><button disabled={index === review.queue.length - 1} onClick={() => queue('move_down', index)}>↓</button><button onClick={() => queue('remove', index)}>×</button></span>}
        </div>)}</div>
        {!building && <><AppendPages onAppend={titles => queue('append', 0, titles)} /><button className="danger clear-queue" disabled={!review.queue.length} onClick={() => { if (window.confirm('Clear the entire queue?')) void queue('clear') }}>Clear queue</button></>}
      </section>

      <section className="pane control-pane">
        <div className="tabs"><span className="active">Options</span><span>More…</span><span>Skip</span><span>Start</span></div>
        <fieldset><legend>Page statistics</legend><dl><dt>Status</dt><dd>{review.status}</dd><dt>Pending</dt><dd>{review.pending}</dd><dt>Reviewed</dt><dd>{review.reviewed}</dd><dt>Unavailable</dt><dd>{review.unavailable}</dd></dl></fieldset>
        {review.current && <fieldset><legend>Current page</legend><strong>{review.current.title}</strong><p>Namespace: {review.current.namespace ?? '—'}</p><p>{review.current.reason}</p></fieldset>}
        <div className="review-buttons"><button disabled={!active || !!busy} onClick={() => action('previous')}>Previous</button><button disabled={!active || !!busy} onClick={() => action('skip')}>Skip</button><button className="danger" disabled={!active || !!busy} onClick={() => action('stop')}>Stop</button><button className="primary" disabled={!active || !!busy} onClick={() => action('next')}>Next</button></div>
        <p className="read-only">Review only: “Next” records the result but does not edit Wikimedia.</p>
        <a className="button" href={`/projects/${projectId}`}>Project settings</a>
      </section>

      <section className="pane editor-pane">
        <div className="tabs"><span className="active">Edit box</span><span>History</span><span>What links here</span><span>Logs</span><span>Typos</span></div>
        {review.page_error && <ErrorBox message={review.page_error} />}
        {review.page ? <>
          <div className="page-heading"><a href={review.page.full_url} target="_blank" rel="noreferrer">Current revision</a><strong>{review.page.changed ? 'Changed' : 'No changes'}</strong></div>
          <div className="meta-strip"><span>Revision {review.page.revision_id}</span><span>{review.page.content_model}</span><span>{review.page.editable ? 'Editable' : 'Not editable'}</span><span>Protection: {review.page.protection}</span></div>
          <textarea className="editor" value={text} maxLength={4194304} onChange={event => setText(event.target.value)} />
          <button onClick={recompute} disabled={!!busy}>Recompute diff</button>
          <div className="rule-counts">{review.page.rules.map((rule, index) => <span className={rule.changed ? 'changed' : ''} key={index}>Rule {index + 1}: {rule.count}</span>)}</div>
          <SideDiff lines={review.page.side_by_side_diff} />
          <details><summary>Unified diff</summary><UnifiedDiff lines={review.page.unified_diff} /></details>
        </> : <div className="empty preview-empty">{building ? review.generation_message : 'No reviewable page is selected.'}</div>}
      </section>
    </main>
  </Shell>
}

function AppendPages({ onAppend }: { onAppend: (titles: string) => void }) {
  const [titles, setTitles] = useState('')
  return <div className="append-pages"><textarea value={titles} onChange={event => setTitles(event.target.value)} placeholder="Append page titles, one per line" /><button onClick={() => { if (titles.trim()) { onAppend(titles); setTitles('') } }}>Append</button></div>
}
