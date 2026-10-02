import { useEffect, useState } from 'react'
import { getJson } from './api'
import type { Session } from './types'
import { WorkspacePage } from './WorkspacePage'
import { ProjectPage } from './ProjectPage'
import { ReviewPage } from './ReviewPage'

export function App() {
  const path = window.location.pathname
  const review = path.match(/^\/projects\/([^/]+)\/jobs\/([^/]+)$/)
  const project = path.match(/^\/projects\/([^/]+)$/)
  if (review) return <ReviewPage projectId={review[1]} jobId={review[2]} />
  if (project) return <ProjectPage projectId={project[1]} />
  if (path === '/workspace') return <WorkspacePage />
  return <HomePage />
}

export function Shell({ children, title, subtitle }: { children: React.ReactNode; title?: string; subtitle?: string }) {
  return <div className="app-shell">
    <header className="titlebar"><span className="app-icon">◆</span><strong>FindSmith</strong>{title && <span> — {title}</span>}</header>
    <nav className="menubar" aria-label="Main menu">
      <a href="/">File</a><a href="/workspace">Projects</a><span>Pages</span><span>Rules</span><span>Tools</span><span>Help</span>
    </nav>
    {subtitle && <div className="contextbar">{subtitle}</div>}
    {children}
    <footer className="statusbar"><span>FindSmith is under development</span><span>Read-only review mode</span></footer>
  </div>
}

function HomePage() {
  const [session, setSession] = useState<Session | null>(null)
  const [error, setError] = useState('')
  useEffect(() => { getJson<Session>('/api/session').then(setSession).catch(error => setError(error.message)) }, [])
  return <Shell title="MediaWiki find and replace">
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
          <p>Role: {session.role}</p>
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

export function Busy({ label = 'Loading…' }: { label?: string }) { return <div className="center panel">{label}</div> }
export function ErrorBox({ message }: { message: string }) { return <div className="error-box" role="alert">{message}</div> }
