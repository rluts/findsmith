import { useEffect, useState } from 'react'
import { getJson } from '../lib/api'
import type { Session } from '../types/api'
import { Shell } from '../components/Shell'

export function HomePage() {
  const [session, setSession] = useState<Session | null>(null)
  const [error, setError] = useState('')
  useEffect(() => { getJson<Session>('/api/session').then(setSession).catch(error => setError(error.message)) }, [])
  return <Shell title="MediaWiki find and replace" authenticated={Boolean(session?.username)}>
    <main className="home">
      <section className="welcome panel">
        <h1>FindSmith</h1>
        <p className="lead">A browser-based MediaWiki find-and-replace workspace inspired by <strong>AutoWikiBrowser</strong>.</p>
        <div className="notice"><strong>Under development</strong><br />This tool is not ready for production use yet. Reviews are read-only and do not edit wiki pages.</div>
      </section>
      <section className="login panel">
        <h2>Wikimedia account</h2>
        {error && <p className="error">{error}</p>}
        {!session ? <p>Checking session…</p> : session.username ? <>
          <p className="online">● Signed in as <strong>{session.username}</strong></p>
          <p>Ukrainian Wikipedia groups: {session.groups.length ? session.groups.join(', ') : 'none reported'}</p>
          {session.groups_error && <p className="error">{session.groups_error}</p>}
          <a className="button primary" href="/workspace">Open workspace</a>
          {session.can_logout && <a className="button" href="/oauth/logout">Log out</a>}
        </> : <>
          <p>Sign in with Wikimedia OAuth. Your password and OAuth tokens stay outside the frontend.</p>
          <a className="button primary" href="/oauth/login">Log in with Wikimedia</a>
        </>}
      </section>
    </main>
  </Shell>
}
