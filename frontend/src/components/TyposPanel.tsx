import { ErrorBox } from './Shell'
import { useI18n } from '../lib/i18n'
import type { TypoPage } from '../types/api'

/** Typo fixes found on the current page, like AWB's Typos tab. */
export function TyposPanel({ typos }: { typos: TypoPage }) {
  const { t } = useI18n()
  return <div className="typos-panel">
    {typos.error && <ErrorBox message={typos.error} />}
    {typos.list_title && <p className="typo-list-source">{t('findsmith-project-typos-list', typos.rule_count, typos.list_title, typos.list_revision)} <a href={typos.list_url} target="_blank" rel="noreferrer">{t('findsmith-project-typos-open-list')}</a></p>}
    {typos.summary && <p className="typo-summary"><strong>{t('findsmith-project-typos-summary')}</strong> {typos.summary}</p>}
    {typos.stats.length > 0 ? <table className="typo-stats">
      <thead><tr><th>{t('findsmith-project-typos-rule')}</th><th>{t('findsmith-project-typos-find')}</th><th>{t('findsmith-project-typos-replace')}</th><th>{t('findsmith-project-typos-matches')}</th><th>{t('findsmith-project-typos-unchanged')}</th></tr></thead>
      <tbody>{typos.stats.map((stat, index) => <tr key={`${stat.find}-${index}`} className={stat.total > stat.self_matches ? 'changed' : ''}><td>{stat.word ?? '—'}</td><td><code>{stat.find}</code></td><td><code>{stat.replace}</code></td><td>{stat.total}</td><td>{stat.self_matches}</td></tr>)}</tbody>
    </table> : !typos.error && <div className="empty">{t('findsmith-project-typos-none')}</div>}
    {typos.rule_errors.length > 0 && <details className="typo-problems"><summary>{t('findsmith-project-typos-rule-errors', typos.rule_errors.length)}</summary><ul>{typos.rule_errors.map(error => <li key={error}><code>{error}</code></li>)}</ul></details>}
    {typos.problem_count > 0 && <details className="typo-problems"><summary>{t('findsmith-project-typos-problems', typos.problem_count)}</summary><ul>{typos.problems.map((problem, index) => <li key={`${problem.find}-${index}`}>{problem.word && <strong>{problem.word}: </strong>}<code>{problem.find}</code> — {problem.message}</li>)}</ul>{typos.problems.length < typos.problem_count && <p>{t('findsmith-project-typos-problems-truncated', typos.problems.length)}</p>}</details>}
  </div>
}
