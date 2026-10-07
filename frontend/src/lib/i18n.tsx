import { createContext, useContext, useEffect, useState } from 'react'
import { getJson, postForm, setRequestFailedMessage } from './api'
import type { Session } from '../types/api'

type Catalog = { language: string; messages: Record<string, string> }
type I18n = { language: string; languages: string[]; t: (key: string, ...args: Array<string | number>) => string; setLanguage: (language: string) => Promise<void> }
const Context = createContext<I18n | null>(null)
const build = document.querySelector<HTMLMetaElement>('meta[name="findsmith-build"]')?.content ?? ''
const catalogPath = (language: string) => `/api/translations/${encodeURIComponent(language)}?v=${encodeURIComponent(build)}`
const rtlScripts = new Set(['Adlm', 'Arab', 'Hebr', 'Mand', 'Nkoo', 'Rohg', 'Samr', 'Syrc', 'Thaa'])

function applyDocumentLanguage(language: string) {
  document.documentElement.lang = language
  try {
    document.documentElement.dir = rtlScripts.has(new Intl.Locale(language).maximize().script ?? '') ? 'rtl' : 'ltr'
  } catch {
    document.documentElement.dir = 'ltr'
  }
}

function formatMessage(message: string, language: string, args: Array<string | number>) {
  const withPlurals = message.replace(/\{\{PLURAL:\$(\d+)\|([^{}]*)\}\}/g, (match, index: string, forms: string) => {
    const count = Number(args[Number(index) - 1])
    if (!Number.isFinite(count)) return match
    const choices = Object.fromEntries(forms.split('|').map(form => form.split(/=(.*)/s, 2)))
    const category = new Intl.PluralRules(language).select(count)
    return choices[category] ?? choices.other ?? match
  })
  return withPlurals.replace(/\$(\d+)/g, (match, index: string) => args[Number(index) - 1]?.toString() ?? match)
}

export function I18nProvider({ children }: { children: React.ReactNode }) {
  const [catalog, setCatalog] = useState<Catalog | null>(null)
  const [session, setSession] = useState<Session | null>(null)
  const [languages, setLanguages] = useState<string[]>(['en'])
  const [loadError, setLoadError] = useState('')

  useEffect(() => {
    let active = true
    const available = getJson<string[]>('/api/translations', 'default').then(codes => {
      if (active) setLanguages(codes)
    })
    getJson<Session>('/api/session').then(async current => {
      const messages = await getJson<Catalog>(catalogPath(current.language), 'default').catch(() => getJson<Catalog>(catalogPath('en'), 'default'))
      if (active) { setRequestFailedMessage(messages.messages['findsmith-request-failed'] ?? 'Request failed.'); setSession(current); setCatalog(messages); applyDocumentLanguage(messages.language) }
    }).catch(async () => {
      const messages = await getJson<Catalog>(catalogPath('en'), 'default').catch(() => null)
      if (active && messages) { setRequestFailedMessage(messages.messages['findsmith-request-failed'] ?? 'Request failed.'); setCatalog(messages); applyDocumentLanguage(messages.language) }
      else if (active) setLoadError('Could not load interface messages. Please reload the page.')
    })
    void available.catch(() => { /* Keep English available if the language list cannot load. */ })
    return () => { active = false }
  }, [])

  async function setLanguage(language: string) {
    const current = await getJson<Session>('/api/session')
    const result = await postForm<{language: string}>('/api/language', new URLSearchParams({ language, csrf_token: current.csrf_token }))
    const messages = await getJson<Catalog>(catalogPath(result.language), 'default').catch(() => getJson<Catalog>(catalogPath('en'), 'default'))
    setRequestFailedMessage(messages.messages['findsmith-request-failed'] ?? 'Request failed.'); setSession({ ...current, language: result.language }); setCatalog(messages); applyDocumentLanguage(messages.language)
  }

  if (loadError) return <main className="center panel" role="alert">{loadError}</main>
  if (!catalog) return <main className="center panel">Loading…</main>
  const activeCatalog = catalog
  function t(key: string, ...args: Array<string | number>) {
    const message = activeCatalog.messages[key] ?? key
    return formatMessage(message, activeCatalog.language, args)
  }
  return <Context.Provider value={{ language: catalog.language, languages, t, setLanguage }}>{children}</Context.Provider>
}

export function useI18n(): I18n {
  const context = useContext(Context)
  if (!context) throw new Error('I18nProvider is missing')
  return context
}
