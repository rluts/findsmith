export class ApiError extends Error {
  constructor(public status: number, message: string) { super(message) }
}

let requestFailedMessage = 'Request failed.'
export function setRequestFailedMessage(message: string) { requestFailedMessage = message }

export async function getJson<T>(path: string, cache: RequestCache = 'no-store'): Promise<T> {
  const response = await fetch(path, { cache, headers: { Accept: 'application/json' } })
    .catch(() => { throw new ApiError(0, requestFailedMessage) })
  return parse<T>(response)
}

export async function postForm<T>(path: string, values: URLSearchParams): Promise<T> {
  const response = await fetch(path, {
    method: 'POST',
    headers: { Accept: 'application/json', 'Content-Type': 'application/x-www-form-urlencoded;charset=UTF-8' },
    body: values,
  }).catch(() => { throw new ApiError(0, requestFailedMessage) })
  return parse<T>(response)
}

async function parse<T>(response: Response): Promise<T> {
  const data = await response.json().catch(() => ({ message: requestFailedMessage }))
  if (!response.ok) throw new ApiError(response.status, data.message ?? data.title ?? requestFailedMessage)
  return data as T
}
