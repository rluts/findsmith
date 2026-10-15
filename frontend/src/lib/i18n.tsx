import { createContext, useContext, useEffect, useMemo, useState } from 'react'
import Banana from 'banana-i18n'
import { getJson, postForm, setRequestFailedMessage } from './api'
import type { Session } from '../types/api'

type Catalog = { language: string; locale: string; messages: Record<string, string> }
export type LanguageOption = { code: string; locale: string }
type I18n = { language: string; locale: string; languages: LanguageOption[]; t: (key: string, ...args: Array<string | number>) => string; setLanguage: (language: string) => Promise<void> }
const Context = createContext<I18n | null>(null)
const build = document.querySelector<HTMLMetaElement>('meta[name="findsmith-build"]')?.content ?? ''
const catalogPath = (language: string) => `/api/translations/${encodeURIComponent(language)}?v=${encodeURIComponent(build)}`
const rtlScripts = new Set(['Adlm', 'Arab', 'Hebr', 'Mand', 'Nkoo', 'Rohg', 'Samr', 'Syrc', 'Thaa'])

// `locale` is the BCP 47 tag the server derives from the MediaWiki language code.
function applyDocumentLanguage(locale: string) {
  document.documentElement.lang = locale
  try {
    document.documentElement.dir = rtlScripts.has(new Intl.Locale(locale).maximize().script ?? '') ? 'rtl' : 'ltr'
  } catch {
    document.documentElement.dir = 'ltr'
  }
}

// A message banana-i18n cannot parse falls back to plain placeholder
// substitution rather than breaking the page.
function formatMessage(banana: Banana, key: string, args: Array<string | number>) {
  try {
    return banana.i18n(key, ...args)
  } catch {
    return banana.getMessage(key).replace(/\$(\d+)/g, (match, index: string) => args[Number(index) - 1]?.toString() ?? match)
  }
}

export function I18nProvider({ children }: { children: React.ReactNode }) {
  const [catalog, setCatalog] = useState<Catalog | null>(null)
  const [session, setSession] = useState<Session | null>(null)
  const [languages, setLanguages] = useState<LanguageOption[]>([{ code: 'en', locale: 'en' }])
  const [loadError, setLoadError] = useState('')

  useEffect(() => {
    let active = true
    const available = getJson<LanguageOption[]>('/api/translations', 'default').then(codes => {
      if (active) setLanguages(codes)
    })
    getJson<Session>('/api/session').then(async current => {
      const messages = await getJson<Catalog>(catalogPath(current.language), 'default').catch(() => getJson<Catalog>(catalogPath('en'), 'default'))
      if (active) { setRequestFailedMessage(messages.messages['findsmith-request-failed'] ?? 'Request failed.'); setSession(current); setCatalog(messages); applyDocumentLanguage(messages.locale) }
    }).catch(async () => {
      const messages = await getJson<Catalog>(catalogPath('en'), 'default').catch(() => null)
      if (active && messages) { setRequestFailedMessage(messages.messages['findsmith-request-failed'] ?? 'Request failed.'); setCatalog(messages); applyDocumentLanguage(messages.locale) }
      else if (active) setLoadError('Could not load interface messages. Please reload the page.')
    })
    void available.catch(() => { /* Keep English available if the language list cannot load. */ })
    return () => { active = false }
  }, [])

  async function setLanguage(language: string) {
    const current = await getJson<Session>('/api/session')
    const result = await postForm<{language: string}>('/api/language', new URLSearchParams({ language, csrf_token: current.csrf_token }))
    const messages = await getJson<Catalog>(catalogPath(result.language), 'default').catch(() => getJson<Catalog>(catalogPath('en'), 'default'))
    setRequestFailedMessage(messages.messages['findsmith-request-failed'] ?? 'Request failed.'); setSession({ ...current, language: result.language }); setCatalog(messages); applyDocumentLanguage(messages.locale)
  }

  // The server already merged the fallback chain, so banana needs only this catalog.
  const banana = useMemo(() => catalog && new Banana(catalog.locale, { messages: catalog.messages }), [catalog])

  if (loadError) return <main className="center panel" role="alert">{loadError}</main>
  if (!catalog || !banana) return <main className="center panel">Loading…</main>
  const activeBanana = banana
  function t(key: string, ...args: Array<string | number>) {
    return formatMessage(activeBanana, key, args)
  }
  return <Context.Provider value={{ language: catalog.language, locale: catalog.locale, languages, t, setLanguage }}>{children}</Context.Provider>
}

export function useI18n(): I18n {
  const context = useContext(Context)
  if (!context) throw new Error('I18nProvider is missing')
  return context
}
