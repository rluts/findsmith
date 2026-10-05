import { useMemo } from 'react'
import type { RenderedPreview as RenderedPreviewData } from '../types/api'

function escapeAttribute(value: string) {
  return value.replaceAll('&', '&amp;').replaceAll('"', '&quot;').replaceAll('<', '&lt;')
}

export function RenderedPreview({ preview }: { preview: RenderedPreviewData }) {
  const srcDoc = useMemo(() => buildDocument(preview), [preview])
  return <iframe className="rendered-preview-frame" title={`Rendered preview of ${preview.title}`} srcDoc={srcDoc} sandbox="allow-popups allow-popups-to-escape-sandbox" referrerPolicy="no-referrer" />
}

function buildDocument(preview: RenderedPreviewData) {
  const origin = new URL(preview.wiki_origin)
  const article = new URL(`/wiki/${encodeURIComponent(preview.title.replaceAll(' ', '_'))}`, origin)
  const stylesheet = new URL('/w/load.php', origin)
  stylesheet.search = new URLSearchParams({
    modules: ['mediawiki.legacy.commonPrint,shared', 'mediawiki.skinning.content', 'mediawiki.skinning.interface', 'skins.vector.styles', 'site', ...preview.module_styles].join('|'),
    only: 'styles', skin: 'vector', lang: 'uk',
  }).toString()
  return `<!doctype html><html lang="uk"><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src https: data:; style-src https: 'unsafe-inline'; font-src https: data:; frame-src 'none'; form-action 'none'"><base href="${escapeAttribute(article.href)}" target="_blank"><link rel="stylesheet" href="${escapeAttribute(stylesheet.href)}"><style>body{margin:0;padding:16px;color:#202122;background:#fff;font-family:Arial,sans-serif;line-height:1.6}.mw-body-content{max-width:960px;margin:auto}img{max-width:100%;height:auto}</style></head><body><main class="mw-body-content">${preview.html}</main></body></html>`
}
