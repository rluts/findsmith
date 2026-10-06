export function Shell({ children, title, subtitle, authenticated = true }: { children: React.ReactNode; title?: string; subtitle?: string; authenticated?: boolean }) {
  return <div className="app-shell">
    <header className="titlebar"><span className="app-icon">◆</span><strong>FindSmith</strong>{title && <span> — {title}</span>}</header>
    <nav className="menubar" aria-label="Main menu">
      <a href="/">Home</a>{authenticated ? <><a href="/workspace">Your Projects</a><a href="/add-wiki">Add Wiki</a></> : <><span className="inactive" aria-disabled="true">Your Projects</span><span className="inactive" aria-disabled="true">Add Wiki</span></>}<span className="inactive" aria-disabled="true">Help</span>
    </nav>
    {subtitle && <div className="contextbar">{subtitle}</div>}
    {children}
    <footer className="statusbar"><span>FindSmith is under development</span><span>License: <a href="https://www.gnu.org/licenses/gpl-3.0.txt" target="_blank" rel="noreferrer">GPL-3.0-or-later</a></span></footer>
  </div>
}

export function Busy({ label = 'Loading…' }: { label?: string }) {
  return <div className="center panel">{label}</div>
}

export function ErrorBox({ message }: { message: string }) {
  return <div className="error-box" role="alert">{message}</div>
}
