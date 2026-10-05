import { useEffect, useState } from 'react'
import { Shell, Busy, ErrorBox } from '../components/Shell'
import { getJson, postForm } from '../lib/api'
import type { ActionResult, Workspace } from '../types/api'
import { navigate } from '../lib/router'

export function WorkspacePage() {
  const [data, setData] = useState<Workspace | null>(null)
  const [name, setName] = useState('')
  const [wikiId, setWikiId] = useState('')
  const [error, setError] = useState('')
  useEffect(() => { getJson<Workspace>('/api/workspace').then(result => { setData(result); setWikiId(result.wikis[0]?.id ?? '') }).catch(error => setError(error.message)) }, [])
  async function create(event: React.FormEvent) {
    event.preventDefault(); if (!data) return
    setError('')
    const values = new URLSearchParams({ name, wiki_id: wikiId, csrf_token: data.csrf_token })
    try { const result = await postForm<ActionResult>('/api/projects', values); navigate(result.redirect) }
    catch (error) { setError(error instanceof Error ? error.message : 'Could not create project.') }
  }
  return <Shell title="Projects" subtitle={data ? `Signed in as ${data.username}` : 'Loading projects'}>
    <main className="workspace-grid">
      <section className="panel project-list">
        <h1>Projects</h1>
        {error && <ErrorBox message={error} />}
        {!data ? <Busy /> : data.projects.length ? <div className="listbox">
          {data.projects.map(project => <a className="project-row" href={`/projects/${project.id}`} key={project.id}>
            <strong>{project.name}</strong><span>{project.wiki_id}</span><time>{new Date(project.updated_at).toLocaleString()}</time>
          </a>)}
        </div> : <p className="empty">No projects yet.</p>}
      </section>
      <form className="panel compact-form" onSubmit={create}>
        <h2>New project</h2>
        <label>Project name<input value={name} maxLength={255} required onChange={event => setName(event.target.value)} autoFocus /></label>
        <label>Wiki<select value={wikiId} onChange={event => setWikiId(event.target.value)} required>{data?.wikis.map(wiki => <option value={wiki.id} key={wiki.id}>{wiki.id} ({wiki.name})</option>)}</select></label>
        {!data?.wikis.length && <p className="empty">No wikis added yet. <a href="/add-wiki">Add a wiki</a> first.</p>}
        <button className="primary" type="submit" disabled={!wikiId}>Create project</button>
      </form>
    </main>
  </Shell>
}
