import { ref } from 'vue'

const TOKEN_KEY = 'collector_token'

export interface ObjResponse<T> {
  status: number
  msg?: string
  data?: T
}

export function getToken(): string | null {
  return localStorage.getItem(TOKEN_KEY)
}

// 全局登录态：整个前端只有一处登录入口（App.vue），登录成功或 token 失效时
// 这里统一更新，其它页面只需读取 loggedIn，不用各自维护一份状态。
export const loggedIn = ref(!!getToken())

export function setToken(token: string) {
  localStorage.setItem(TOKEN_KEY, token)
  loggedIn.value = true
}

export function clearToken() {
  localStorage.removeItem(TOKEN_KEY)
  loggedIn.value = false
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

export interface ScriptEntry {
  path: string
  is_dir: boolean
  size: number
  modified: string
}

export function listScripts(): Promise<ScriptEntry[]> {
  return request<ScriptEntry[]>('/v1/system/scripts')
}

export function getScript(path: string): Promise<string> {
  return request<string>(`/v1/system/scripts/file?path=${encodeURIComponent(path)}`)
}

export function saveScript(path: string, content: string): Promise<void> {
  return request<void>('/v1/system/scripts/file', {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ path, content }),
  })
}

export function createScript(path: string, content: string): Promise<void> {
  return request<void>('/v1/system/scripts/file', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ path, content }),
  })
}

export function deleteScript(path: string): Promise<void> {
  return request<void>(`/v1/system/scripts/file?path=${encodeURIComponent(path)}`, {
    method: 'DELETE',
  })
}
