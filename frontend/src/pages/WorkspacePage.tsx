import { useEffect, useState } from 'react'
import { Shell, Busy, ErrorBox } from '../components/Shell'
import { getJson, postForm } from '../lib/api'
import type { ActionResult, Workspace } from '../types/api'
import { navigate } from '../lib/router'
import { useI18n } from '../lib/i18n'

export function WorkspacePage() {
  const { t } = useI18n()
  const [data, setData] = useState<Workspace | null>(null)
  const [name, setName] = useState('')
  const [wikiId, setWikiId] = useState('')
  const [error, setError] = useState('')
  useEffect(() => { getJson<Workspace>('/api/workspace').then(result => { setData(result); setWikiId(result.default_wiki_id ?? result.wikis[0]?.id ?? '') }).catch(error => setError(error.message)) }, [])
  async function create(event: React.FormEvent) {
    event.preventDefault(); if (!data) return
    setError('')
    const values = new URLSearchParams({ name, wiki_id: wikiId, csrf_token: data.csrf_token })
    try { const result = await postForm<ActionResult>('/api/projects', values); navigate(result.redirect) }
    catch (error) { setError(error instanceof Error ? error.message : t('findsmith-request-failed')) }
  }
  return <Shell title={t('findsmith-workspace-title')} subtitle={data ? t('findsmith-signed-in-as', data.username) : t('findsmith-workspace-loading')}>
    <main className="workspace-grid">
      <section className="panel project-list">
        <h1>{t('findsmith-workspace-title')}</h1>
        {error && <ErrorBox message={error} />}
        {!data ? <Busy /> : data.projects.length ? <div className="listbox">
          {data.projects.map(project => <a className="project-row" href={`/projects/${project.id}`} key={project.id}>
            <strong>{project.name}</strong><span>{project.wiki_id}</span><time>{new Date(project.updated_at).toLocaleString()}</time>
          </a>)}
        </div> : <p className="empty">{t('findsmith-workspace-empty')}</p>}
      </section>
      <form className="panel compact-form" onSubmit={create}>
        <h2>{t('findsmith-workspace-new')}</h2>
        <label>{t('findsmith-workspace-name')}<input value={name} maxLength={255} required onChange={event => setName(event.target.value)} autoFocus /></label>
        <label>{t('findsmith-workspace-wiki')}<select value={wikiId} onChange={event => setWikiId(event.target.value)} required>{data?.wikis.map(wiki => <option value={wiki.id} key={wiki.id}>{wiki.id} ({wiki.name})</option>)}</select></label>
        {!data?.wikis.length && <p className="empty">{t('findsmith-workspace-no-wikis')} <a href="/add-wiki">{t('findsmith-workspace-add-wiki')}</a></p>}
        <button className="primary" type="submit" disabled={!wikiId}>{t('findsmith-workspace-create')}</button>
      </form>
    </main>
  </Shell>
}
