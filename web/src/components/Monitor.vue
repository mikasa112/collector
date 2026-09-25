<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'

interface Word {
  zh: string
  en: string
}

interface Point {
  id: number
  key: string
  name: string
  value: number | number[]
  words?: Record<string, Word>
  unit?: string
}

interface ObjResponse<T> {
  status: number
  msg?: string
  data?: T
}

const devices = ref<string[]>([])
const selectedDev = ref('')
const points = ref<Point[]>([])
const filter = ref('')
const connected = ref(false)

let socket: WebSocket | null = null
let reconnectTimer: ReturnType<typeof setTimeout> | null = null

function wsUrl(dev: string): string {
  const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${protocol}//${location.host}/v1/ws/data?dev=${encodeURIComponent(dev)}&lang=zh`
}

function closeSocket() {
  if (reconnectTimer) {
    clearTimeout(reconnectTimer)
    reconnectTimer = null
  }
  if (socket) {
    socket.onopen = null
    socket.onmessage = null
    socket.onclose = null
    socket.onerror = null
    socket.close()
    socket = null
  }
  connected.value = false
}

function connect(dev: string) {
  closeSocket()
  if (!dev) {
    points.value = []
    return
  }

  const ws = new WebSocket(wsUrl(dev))
  socket = ws

  ws.onopen = () => {
    connected.value = true
  }
  ws.onmessage = (evt) => {
    try {
      points.value = JSON.parse(evt.data)
    } catch (err) {
      console.error('解析点位数据失败', err)
    }
  }
  ws.onclose = () => {
    connected.value = false
    if (socket === ws) {
      reconnectTimer = setTimeout(() => connect(dev), 2000)
    }
  }
  ws.onerror = () => {
    ws.close()
  }
}

async function loadDevices() {
  const res = await fetch('/v1/devices')
  const body: ObjResponse<string[]> = await res.json()
  devices.value = body.data ?? []
  if (!selectedDev.value && devices.value.length > 0) {
    selectedDev.value = devices.value[0]
  }
}

watch(selectedDev, (dev) => connect(dev))

onMounted(loadDevices)
onBeforeUnmount(closeSocket)

const filteredPoints = computed(() => {
  const keyword = filter.value.trim().toLowerCase()
  if (!keyword) return points.value
  return points.value.filter(
    (p) => p.key.toLowerCase().includes(keyword) || p.name.toLowerCase().includes(keyword),
  )
})

function displayValue(point: Point): string {
  if (Array.isArray(point.value)) {
    return `[${point.value.join(', ')}]`
  }
  const word = point.words?.[String(point.value)]
  if (word) {
    return `${word.zh}(${point.value})`
  }
  const unit = point.unit ? ` ${point.unit}` : ''
  return `${point.value}${unit}`
}
</script>

<template>
  <div class="page">
    <header>
      <h1>点位监控</h1>
      <div class="toolbar">
        <select v-model="selectedDev">
          <option v-if="devices.length === 0" value="" disabled>暂无设备</option>
          <option v-for="dev in devices" :key="dev" :value="dev">{{ dev }}</option>
        </select>
        <input v-model="filter" type="text" placeholder="按 key / name 过滤" />
        <span class="status" :class="{ ok: connected }">{{ connected ? '已连接' : '未连接' }}</span>
        <span class="count">{{ filteredPoints.length }} / {{ points.length }} 点</span>
      </div>
    </header>

    <table>
      <thead>
        <tr>
          <th>ID</th>
          <th>Key</th>
          <th>名称</th>
          <th>值</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="p in filteredPoints" :key="p.id">
          <td class="mono">{{ p.id }}</td>
          <td class="mono">{{ p.key }}</td>
          <td>{{ p.name }}</td>
          <td>{{ displayValue(p) }}</td>
        </tr>
      </tbody>
    </table>
  </div>
</template>

<style scoped>
.page {
  max-width: 960px;
  margin: 0 auto;
  padding: 16px 20px 40px;
}

h1 {
  font-size: 20px;
  margin: 8px 0 16px;
  text-transform: uppercase;
  letter-spacing: 0.1em;
  color: var(--accent);
  text-shadow: 0 0 10px var(--accent-glow);
}

.toolbar {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 12px;
}

select,
input {
  font: inherit;
  padding: 6px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--bg-alt);
  color: var(--text);
}

input {
  flex: 1;
}

.status {
  position: relative;
  padding-left: 14px;
  color: var(--warn);
  font-size: 12px;
}

.status::before {
  content: '';
  position: absolute;
  left: 0;
  top: 50%;
  width: 7px;
  height: 7px;
  margin-top: -3.5px;
  border-radius: 50%;
  background: var(--warn);
  box-shadow: 0 0 8px var(--warn-glow);
}

.status.ok {
  color: var(--ok);
}

.status.ok::before {
  background: var(--ok);
  box-shadow: 0 0 8px var(--ok-glow);
}

.count {
  color: var(--text-dim);
  font-size: 12px;
  white-space: nowrap;
}

table {
  width: 100%;
  border-collapse: collapse;
  font-size: 13px;
  border: 1px solid var(--border);
  border-radius: 8px;
  overflow: hidden;
}

th,
td {
  text-align: left;
  padding: 8px 10px;
  border-bottom: 1px solid var(--border);
}

thead th {
  position: sticky;
  top: 0;
  background: var(--bg-alt);
  border-bottom: 1px solid var(--accent);
}

tbody tr:hover {
  background: var(--accent-dim);
}

.mono {
  font-family: ui-monospace, Consolas, monospace;
  color: var(--accent);
}
</style>
