import { useEffect, useState } from 'react'
import { getJson } from '../lib/api'
import type { Session } from '../types/api'
import { Shell } from '../components/Shell'

export function HomePage() {
  const [session, setSession] = useState<Session | null>(null)
  const [error, setError] = useState('')
  useEffect(() => { getJson<Session>('/api/session').then(setSession).catch(error => setError(error.message)) }, [])
  return <Shell title="Find and replace tool for Wikimedia projects" authenticated={Boolean(session?.username)}>
    <main className="home">
      <section className="welcome panel">
        <h1>FindSmith</h1>
        <p className="lead">A browser-based Wikimedia find-and-replace workspace inspired by <strong>AutoWikiBrowser</strong>.</p>
        <div className="notice"><strong>Under development</strong><br />This tool is not ready for production use yet. Review each proposed change carefully before saving it to Wikimedia.</div>
      </section>
      <section className="login panel">
        <h2>Wikimedia account</h2>
        {error && <p className="error">{error}</p>}
        {!session ? <p>Checking session…</p> : session.username ? <>
          <p className="online">● Signed in as <strong>{session.username}</strong></p>
          <a className="button primary" href="/workspace">Open workspace</a>
          {session.can_logout && <a className="button" href="/oauth/logout">Log out</a>}
        </> : <>
          <p>Sign in through Wikimedia. FindSmith never sees or stores your password. OAuth tokens are kept in an encrypted sign-in cookie, not in our database.</p>
          <a className="button primary" href="/oauth/login">Log in with Wikimedia</a>
        </>}
      </section>
    </main>
  </Shell>
}
