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

export function Busy({ label = 'Loading…' }: { label?: string }) {
  return <div className="center panel">{label}</div>
}

export function ErrorBox({ message }: { message: string }) {
  return <div className="error-box" role="alert">{message}</div>
}
