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

export type Role = 'user' | 'admin' | 'super_admin'

// JWT 的 role 声明是后端登录时签发的 snake_case 字符串，这里只做展示层的按钮显隐，
// 后端路由仍会用 require_role 做最终校验，解码失败（token 损坏/过期）一律当无角色处理。
function decodeRole(token: string | null): Role | null {
  if (!token) return null
  try {
    const payload = token.split('.')[1]
    const base64 = payload.replace(/-/g, '+').replace(/_/g, '/')
    const json = decodeURIComponent(
      atob(base64)
        .split('')
        .map((c) => '%' + c.charCodeAt(0).toString(16).padStart(2, '0'))
        .join(''),
    )
    const claims = JSON.parse(json)
    return (claims.role as Role) ?? null
  } catch {
    return null
  }
}

// 全局登录态：整个前端只有一处登录入口（App.vue），登录成功或 token 失效时
// 这里统一更新，其它页面只需读取 loggedIn/currentRole，不用各自维护一份状态。
export const loggedIn = ref(!!getToken())
export const currentRole = ref<Role | null>(decodeRole(getToken()))

export function isAdmin(): boolean {
  return currentRole.value === 'admin' || currentRole.value === 'super_admin'
}

export function setToken(token: string) {
  localStorage.setItem(TOKEN_KEY, token)
  loggedIn.value = true
  currentRole.value = decodeRole(token)
}

export function clearToken() {
  localStorage.removeItem(TOKEN_KEY)
  loggedIn.value = false
  currentRole.value = null
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

interface ListResponseBody<T> {
  status: number
  err_msg?: string
  data?: T[]
  total?: number
}

// 列表接口（ListResponse）和其它接口（ObjResponse）响应体字段不同（data 是数组、err_msg 而非 msg），
// 但出错时后端统一走 Code::write 渲染 ObjResponse，所以这里兼容读取 msg/err_msg 两种错误字段。
async function listRequest<T>(path: string, init?: RequestInit): Promise<{ data: T[]; total: number }> {
  const headers = new Headers(init?.headers)
  const token = getToken()
  if (token) {
    headers.set('Authorization', `Bearer ${token}`)
  }
  const res = await fetch(path, { ...init, headers })
  if (res.status === 401) {
    clearToken()
  }
  const body: ListResponseBody<T> & { msg?: string } = await res.json()
  if (!res.ok || body.status >= 400) {
    throw new ApiError(body.status ?? res.status, body.msg ?? body.err_msg ?? '请求失败')
  }
  return { data: body.data ?? [], total: body.total ?? 0 }
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

export function listDevices(): Promise<string[]> {
  return request<string[]>('/v1/devices')
}

// 与后端 collector-api/src/models/field_binding.rs::PointKind 的变体名一一对应，
// 序列化为字符串 "Id"/"Key"/"Name"（该枚举没有 rename_all，走 serde 默认命名）。
export type PointKind = 'Id' | 'Key' | 'Name'

export interface FieldBindingView {
  field_key: string
  name: string
  direction: 'read' | 'write' | 'read_write'
  dev_id: string
  point_kind: PointKind
  point_value: string
  is_override: boolean
  updated_by?: string
}

export function listFieldBindings(): Promise<{ data: FieldBindingView[]; total: number }> {
  return listRequest<FieldBindingView>('/v1/field_binding/list')
}

export interface SetFieldBindingParams {
  field_key: string
  dev_id: string
  point_kind: PointKind
  point_value: string
}

export function setFieldBinding(params: SetFieldBindingParams): Promise<void> {
  return request<void>('/v1/field_binding', {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(params),
  })
}

export function resetFieldBinding(fieldKey: string): Promise<void> {
  return request<void>(`/v1/field_binding?field_key=${encodeURIComponent(fieldKey)}`, {
    method: 'DELETE',
  })
}
