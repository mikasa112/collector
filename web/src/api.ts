const TOKEN_KEY = 'collector_token'

export interface ObjResponse<T> {
  status: number
  msg?: string
  data?: T
}

export function getToken(): string | null {
  return localStorage.getItem(TOKEN_KEY)
}

export function setToken(token: string) {
  localStorage.setItem(TOKEN_KEY, token)
}

export function clearToken() {
  localStorage.removeItem(TOKEN_KEY)
}

export class ApiError extends Error {
  status: number
  constructor(status: number, message: string) {
    super(message)
    this.status = status
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const headers = new Headers(init?.headers)
  const token = getToken()
  if (token) {
    headers.set('Authorization', `Bearer ${token}`)
  }
  const res = await fetch(path, { ...init, headers })
  if (res.status === 401) {
    clearToken()
  }
  const body: ObjResponse<T> = await res.json()
  if (!res.ok || body.status >= 400) {
    throw new ApiError(body.status ?? res.status, body.msg ?? '请求失败')
  }
  return body.data as T
}

export function login(username: string, password: string): Promise<string> {
  return request<string>('/v1/login', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username, password }),
  })
}

export function getConfig(): Promise<string> {
  return request<string>('/v1/system/config')
}

export function putConfig(content: string): Promise<void> {
  return request<void>('/v1/system/config', {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ content }),
  })
}

export interface BackupInfo {
  name: string
  created_at: string
  size: number
}

export function listBackups(): Promise<BackupInfo[]> {
  return request<BackupInfo[]>('/v1/system/backups')
}

export function restoreBackup(name: string): Promise<void> {
  return request<void>(`/v1/system/backups/${encodeURIComponent(name)}/restore`, {
    method: 'POST',
  })
}

export function restart(): Promise<void> {
  return request<void>('/v1/system/restart', { method: 'POST' })
}

export interface UnitStatus {
  active_state: string
}

export function getStatus(): Promise<UnitStatus> {
  return request<UnitStatus>('/v1/system/status')
}
