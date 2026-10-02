export function navigate(path: string) {
  const next = new URL(path, window.location.href)
  if (next.origin !== window.location.origin) {
    window.location.assign(next.href)
    return
  }
  window.history.pushState(null, '', `${next.pathname}${next.search}${next.hash}`)
  window.dispatchEvent(new PopStateEvent('popstate'))
}
