import { useEffect, useState } from 'react'
import { getJson } from '../lib/api'
import type { Session } from '../types/api'
import { Shell } from '../components/Shell'
import { useI18n } from '../lib/i18n'

export function HomePage() {
  const { t } = useI18n()
  const [session, setSession] = useState<Session | null>(null)
  const [error, setError] = useState('')
  useEffect(() => { getJson<Session>('/api/session').then(setSession).catch(error => setError(error.message)) }, [])
  return <Shell title={t('findsmith-app-title')} authenticated={Boolean(session?.username)}>
    <main className="home">
      <section className="welcome panel">
        <h1>{t('findsmith-home-title')}</h1>
        <p className="lead">{t('findsmith-home-lead')}</p>
        <div className="notice"><strong>{t('findsmith-home-development-title')}</strong><br />{t('findsmith-home-development-message')}</div>
      </section>
      <section className="login panel">
        <h2>{t('findsmith-home-account')}</h2>
        {error && <p className="error">{error}</p>}
        {!session ? <p>{t('findsmith-home-checking-session')}</p> : session.username ? <>
          <p className="online">● {t('findsmith-signed-in-as', session.username)}</p>
          <a className="button primary" href="/workspace">{t('findsmith-home-open-workspace')}</a>
          {session.can_logout && <a className="button" href="/oauth/logout">{t('findsmith-home-logout')}</a>}
        </> : <>
          <p>{t('findsmith-home-login-description')}</p>
          <a className="button primary" href="/oauth/login">{t('findsmith-home-login')}</a>
        </>}
      </section>
    </main>
  </Shell>
}
