import { useEffect, useState } from 'react'
import { WorkspacePage } from '../pages/WorkspacePage'
import { ProjectPage } from '../pages/ProjectPage'
import { HomePage } from '../pages/HomePage'
import { AddWikiPage } from '../pages/AddWikiPage'
import { navigate } from '../lib/router'

export function App() {
  const [path, setPath] = useState(window.location.pathname)
  useEffect(() => {
    const route = () => setPath(window.location.pathname)
    const click = (event: MouseEvent) => {
      if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return
      const target = event.target instanceof Element ? event.target.closest('a') : null
      if (!target || target.target || target.hasAttribute('download')) return
      const url = new URL(target.href, window.location.href)
      if (url.origin !== window.location.origin || url.pathname.startsWith('/oauth/') || url.pathname.startsWith('/api/')) return
      event.preventDefault()
      navigate(`${url.pathname}${url.search}${url.hash}`)
    }
    window.addEventListener('popstate', route)
    document.addEventListener('click', click)
    return () => { window.removeEventListener('popstate', route); document.removeEventListener('click', click) }
  }, [])
  const review = path.match(/^\/projects\/([^/]+)\/jobs\/([^/]+)$/)
  const project = path.match(/^\/projects\/([^/]+)$/)
  if (review) return <ProjectPage projectId={review[1]} jobId={review[2]} />
  if (project) return <ProjectPage projectId={project[1]} />
  if (path === '/workspace') return <WorkspacePage />
  if (path === '/add-wiki') return <AddWikiPage />
  return <HomePage />
}
