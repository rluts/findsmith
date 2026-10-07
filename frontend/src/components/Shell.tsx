import { useState } from 'react'
import { useI18n } from '../lib/i18n'

function languageName(code: string): string {
  try {
    return new Intl.DisplayNames([code], { type: 'language' }).of(code) ?? code
  } catch {
    return code
  }
}

export function Shell({ children, title, subtitle, authenticated = true }: { children: React.ReactNode; title?: string; subtitle?: string; authenticated?: boolean }) {
  const { t, language, languages, setLanguage } = useI18n()
  const [languageError, setLanguageError] = useState('')
  return <div className="app-shell">
    <header className="titlebar"><span className="app-icon">◆</span><strong>FindSmith</strong>{title && <span> — {title}</span>}</header>
    <nav className="menubar" aria-label={t('findsmith-nav-main-menu')}>
      <a href="/">{t('findsmith-nav-home')}</a>{authenticated ? <><a href="/workspace">{t('findsmith-nav-projects')}</a><a href="/add-wiki">{t('findsmith-nav-add-wiki')}</a></> : <><span className="inactive" aria-disabled="true">{t('findsmith-nav-projects')}</span><span className="inactive" aria-disabled="true">{t('findsmith-nav-add-wiki')}</span></>}<span className="inactive" aria-disabled="true">{t('findsmith-nav-help')}</span>
    </nav>
    {subtitle && <div className="contextbar">{subtitle}</div>}
    {children}
    <footer className="statusbar"><span>{t('findsmith-footer-development')}</span><label className="language-choice">{t('findsmith-language-label')} <select value={language} onChange={event => { setLanguageError(''); void setLanguage(event.target.value).catch(error => setLanguageError(error instanceof Error ? error.message : String(error))) }}>{languages.map(code => <option key={code} value={code}>{languageName(code)}</option>)}</select></label>{languageError && <span role="alert">{languageError}</span>}<span>{t('findsmith-footer-license')} <a href="https://www.gnu.org/licenses/gpl-3.0.txt" target="_blank" rel="noreferrer">GPL-3.0-or-later</a></span></footer>
  </div>
}

export function Busy({ label = 'Loading…' }: { label?: string }) {
  const { t } = useI18n()
  return <div className="center panel">{label === 'Loading…' ? t('findsmith-loading') : label}</div>
}

export function ErrorBox({ message }: { message: string }) {
  return <div className="error-box" role="alert">{message}</div>
}
