import { useEffect, useState } from 'react'
import { Busy, ErrorBox, Shell } from '../components/Shell'
import { getJson, postForm } from '../lib/api'
import { navigate } from '../lib/router'
import type { ActionResult, Wiki } from '../types/api'

type WikiList = { wikis: Wiki[]; csrf_token: string }

export function AddWikiPage() {
  const [data, setData] = useState<WikiList | null>(null)
  const [domain, setDomain] = useState('')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  useEffect(() => { getJson<WikiList>('/api/wikis').then(setData).catch(error => setError(error.message)) }, [])
  async function add(event: React.FormEvent) {
    event.preventDefault()
    if (!data) return
    setError(''); setBusy(true)
    try {
      const result = await postForm<ActionResult>('/api/wikis', new URLSearchParams({ domain, csrf_token: data.csrf_token }))
      navigate(result.redirect)
    } catch (error) { setError(error instanceof Error ? error.message : 'Could not add wiki.') }
    finally { setBusy(false) }
  }
  return <Shell title="Add Wiki" authenticated={Boolean(data)}>
    <main className="workspace-grid">
      <form className="panel compact-form" onSubmit={add}>
        <h1>Add Wiki</h1>
        <p>Enter the domain of a Wikimedia project. FindSmith checks its protected AutoWikiBrowser CheckPageJSON and loads its namespaces.</p>
        {error && <ErrorBox message={error} />}
        {!data && !error ? <Busy /> : data ? <>
          <label>Wiki domain<input value={domain} onChange={event => setDomain(event.target.value)} placeholder="en.wikipedia.org" required /></label>
          <button className="primary" type="submit" disabled={busy}>{busy ? 'Checking wiki…' : 'Add Wiki'}</button>
        </> : null}
      </form>
      <section className="panel project-list"><h2>Added wikis</h2>
        {data?.wikis.length ? <div className="listbox">{data.wikis.map(wiki => <div className="project-row" key={wiki.id}><strong>{wiki.name}</strong><span>{wiki.id}</span><span>{wiki.domain}</span></div>)}</div> : <p className="empty">No wikis added yet.</p>}
      </section>
    </main>
  </Shell>
}
