let guard: (() => boolean) | null = null

/** Lets the current page veto in-app navigation, for example to keep unsaved text. */
export function setNavigationGuard(next: (() => boolean) | null) {
  guard = next
}

export function navigate(path: string) {
  if (guard && !guard()) return
  const next = new URL(path, window.location.href)
  if (next.origin !== window.location.origin) {
    window.location.assign(next.href)
    return
  }
  window.history.pushState(null, '', `${next.pathname}${next.search}${next.hash}`)
  window.dispatchEvent(new PopStateEvent('popstate'))
}
