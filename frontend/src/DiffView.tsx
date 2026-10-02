import type { SideLine, UnifiedLine } from './types'

const Fragments = ({ items }: { items: {text: string; emphasized: boolean}[] }) => <>{items.map((item, index) => item.emphasized ? <mark key={index}>{item.text}</mark> : <span key={index}>{item.text}</span>)}</>

export function SideDiff({ lines }: { lines: SideLine[] }) {
  return <div className="side-diff">{lines.map((line, index) => <div className="diff-row" key={index}>
    <div className={`diff-cell ${line.left_kind}`}><i>{line.left_number}</i><b>{line.left_marker}</b><code><Fragments items={line.left_fragments} /></code></div>
    <div className={`diff-cell ${line.right_kind}`}><i>{line.right_number}</i><b>{line.right_marker}</b><code><Fragments items={line.right_fragments} /></code></div>
  </div>)}</div>
}

export function UnifiedDiff({ lines }: { lines: UnifiedLine[] }) {
  return <div className="unified-diff">{lines.map((line, index) => <div className={`unified-line ${line.kind}`} key={index}><b>{line.marker}</b><code><Fragments items={line.fragments} /></code></div>)}</div>
}
