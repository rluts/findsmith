export class ApiError extends Error {
  constructor(public status: number, message: string) { super(message) }
}

export async function getJson<T>(path: string): Promise<T> {
  const response = await fetch(path, { headers: { Accept: 'application/json' } })
  return parse<T>(response)
}

export async function postForm<T>(path: string, values: URLSearchParams): Promise<T> {
  const response = await fetch(path, {
    method: 'POST',
    headers: { Accept: 'application/json', 'Content-Type': 'application/x-www-form-urlencoded;charset=UTF-8' },
    body: values,
  })
  return parse<T>(response)
}

async function parse<T>(response: Response): Promise<T> {
  const data = await response.json().catch(() => ({ message: `Request failed (${response.status}).` }))
  if (!response.ok) throw new ApiError(response.status, data.message ?? data.title ?? 'Request failed.')
  return data as T
}
