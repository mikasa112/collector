<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref } from 'vue'
import {
  ApiError,
  getConfig,
  getStatus,
  listBackups,
  putConfig,
  restart,
  restoreBackup,
  type BackupInfo,
} from '../api'

const configText = ref('')
const configError = ref('')
const configSaving = ref(false)
const configLoaded = ref(false)
const viewMode = ref<'form' | 'json'>('form')

interface DeviceForm {
  id: string
  desc: string
  config: {
    type: string
    com_type: string
    register_file: string
    interval: number | string
    timeout: number | string
    request_interval: number | string
    max_gap: number | string
    ip: string
    port: number | string
    slave: number | string
    serial_tty: string
    baud_rate: number | string
    data_bits: number | string
    parity: string
    stop_bits: number | string
    interface: string
    desc: string
  }
}

const COM_TYPES = [
  { value: 'ModbusTCP', label: 'Modbus TCP' },
  { value: 'ModbusRTU', label: 'Modbus RTU（串口）' },
  { value: 'CAN', label: 'CAN' },
  { value: 'IEC104', label: 'IEC 60870-5-104' },
  { value: 'IEC61850', label: 'IEC 61850' },
  { value: 'GPIO', label: 'GPIO' },
]

// configObj 保留了原始 JSON 解析结果，未在表单中出现的字段（如 mqtt_routes）
// 只会被原样透传，不会在保存时丢失。
const configObj = ref<any>(null)
const deviceList = ref<DeviceForm[]>([])
const parseError = ref('')

function applyDefaults(parsed: any) {
  parsed.product_type = parsed.product_type ?? ''
  parsed.project = parsed.project ?? ''
  parsed.program = parsed.program ?? {}
  const p = parsed.program
  p.emu = p.emu ?? {}
  p.emu.enable = p.emu.enable ?? false
  p.http = p.http ?? {}
  p.http.enable = p.http.enable ?? true
  p.http.ip = p.http.ip ?? '0.0.0.0'
  p.http.port = p.http.port ?? 9091
  p.mod = p.mod ?? {}
  p.mod.enable = p.mod.enable ?? true
  p.mod.path = p.mod.path ?? 'lua_scripts'
  p.north_modbus = p.north_modbus ?? {}
  p.north_modbus.enable = p.north_modbus.enable ?? false
  p.north_modbus.north_modbus_host = p.north_modbus.north_modbus_host ?? '0.0.0.0'
  p.north_modbus.north_modbus_port = p.north_modbus.north_modbus_port ?? 9092
  p.north_modbus.north_modbus_conf = p.north_modbus.north_modbus_conf ?? ''
  p.eg25_gl = p.eg25_gl ?? {}
  p.eg25_gl.enable = p.eg25_gl.enable ?? true
  p.mqtt = p.mqtt ?? {}
  p.mqtt.enable = p.mqtt.enable ?? false
  p.mqtt.mqtt_host = p.mqtt.mqtt_host ?? '127.0.0.1'
  p.mqtt.mqtt_port = p.mqtt.mqtt_port ?? 1883
  p.mqtt.mqtt_username = p.mqtt.mqtt_username ?? ''
  p.mqtt.mqtt_password = p.mqtt.mqtt_password ?? ''
  p.mqtt.mqtt_yt = p.mqtt.mqtt_yt ?? ''
  p.mqtt.mqtt_yk = p.mqtt.mqtt_yk ?? ''
  parsed.devices = parsed.devices ?? {}
  return parsed
}

function toDeviceForm(key: string, dev: any): DeviceForm {
  const c = dev?.config ?? {}
  return {
    id: dev?.id ?? key,
    desc: dev?.desc ?? '',
    config: {
      type: c.type ?? '',
      com_type: c.com_type ?? 'ModbusTCP',
      register_file: c.register_file ?? '',
      interval: c.interval ?? 2000,
      timeout: c.timeout ?? 2000,
      request_interval: c.request_interval ?? '',
      max_gap: c.max_gap ?? '',
      ip: c.ip ?? '',
      port: c.port ?? '',
      slave: c.slave ?? '',
      serial_tty: c.serial_tty ?? '',
      baud_rate: c.baud_rate ?? '',
      data_bits: c.data_bits ?? 8,
      parity: c.parity ?? 'N',
      stop_bits: c.stop_bits ?? 1,
      interface: c.interface ?? '',
      desc: c.desc ?? '',
    },
  }
}

function loadDeviceList(devicesMap: Record<string, any>) {
  deviceList.value = Object.entries(devicesMap ?? {}).map(([key, dev]) => toDeviceForm(key, dev))
}

function addDevice() {
  let n = deviceList.value.length + 1
  let id = `device_${n}`
  while (deviceList.value.some((d) => d.id === id)) {
    n += 1
    id = `device_${n}`
  }
  deviceList.value.push(toDeviceForm(id, { id }))
}

function removeDevice(index: number) {
  const dev = deviceList.value[index]
  if (!confirm(`确认删除设备 ${dev.id}？`)) return
  deviceList.value.splice(index, 1)
}

function showsNetwork(com: string) {
  return com === 'ModbusTCP' || com === 'IEC104' || com === 'IEC61850'
}
function showsSlave(com: string) {
  return com === 'ModbusTCP' || com === 'ModbusRTU'
}
function showsInterface(com: string) {
  return com === 'CAN'
}
function showsBaudRate(com: string) {
  return com === 'ModbusRTU' || com === 'CAN'
}
function baudRateLabel(com: string) {
  return com === 'CAN' ? '比特率（可选）' : '波特率'
}
function showsRegisterFile(com: string) {
  return com === 'ModbusTCP' || com === 'ModbusRTU' || com === 'CAN' || com === 'GPIO'
}
function showsRequestGap(com: string) {
  return com === 'ModbusTCP' || com === 'ModbusRTU'
}

function toNumOrNull(v: any): number | null {
  if (v === '' || v === null || v === undefined) return null
  const n = Number(v)
  return Number.isFinite(n) ? n : null
}
function toStrOrNull(v: any): string | null {
  if (v === '' || v === null || v === undefined) return null
  return v
}

function validateDeviceIds(): string {
  const seen = new Set<string>()
  for (const dev of deviceList.value) {
    const id = dev.id.trim()
    if (!id) return '设备标识不能为空'
    if (seen.has(id)) return `设备标识重复: ${id}`
    seen.add(id)
  }
  return ''
}

function buildDevicesMap(): Record<string, any> {
  const map: Record<string, any> = {}
  for (const dev of deviceList.value) {
    const id = dev.id.trim()
    const c = dev.config
    map[id] = {
      id,
      desc: toStrOrNull(dev.desc),
      config: {
        type: toStrOrNull(c.type),
        com_type: c.com_type || null,
        register_file: toStrOrNull(c.register_file),
        interval: toNumOrNull(c.interval),
        timeout: toNumOrNull(c.timeout),
        request_interval: toNumOrNull(c.request_interval),
        max_gap: toNumOrNull(c.max_gap),
        ip: toStrOrNull(c.ip),
        port: toNumOrNull(c.port),
        slave: toNumOrNull(c.slave),
        serial_tty: toStrOrNull(c.serial_tty),
        baud_rate: toNumOrNull(c.baud_rate),
        data_bits: toNumOrNull(c.data_bits),
        parity: toStrOrNull(c.parity),
        stop_bits: toNumOrNull(c.stop_bits),
        interface: toStrOrNull(c.interface),
        desc: toStrOrNull(c.desc),
      },
    }
  }
  return map
}

function buildSaveObject(): any {
  const base = JSON.parse(JSON.stringify(configObj.value))
  const p = configObj.value.program
  base.product_type = configObj.value.product_type || null
  base.project = configObj.value.project || null
  base.program = {
    emu: { enable: !!p.emu.enable },
    http: {
      enable: !!p.http.enable,
      ip: p.http.ip || '0.0.0.0',
      port: toNumOrNull(p.http.port) ?? 9091,
    },
    mod: {
      enable: !!p.mod.enable,
      path: p.mod.path || 'lua_scripts',
    },
    north_modbus: {
      enable: !!p.north_modbus.enable,
      north_modbus_host: p.north_modbus.north_modbus_host || '0.0.0.0',
      north_modbus_port: toNumOrNull(p.north_modbus.north_modbus_port) ?? 9092,
      north_modbus_conf: p.north_modbus.north_modbus_conf || '',
    },
    eg25_gl: { enable: !!p.eg25_gl.enable },
    mqtt: {
      enable: !!p.mqtt.enable,
      mqtt_host: p.mqtt.mqtt_host || '127.0.0.1',
      mqtt_port: toNumOrNull(p.mqtt.mqtt_port) ?? 1883,
      mqtt_username: p.mqtt.mqtt_username || '',
      mqtt_password: p.mqtt.mqtt_password || '',
      mqtt_yt: p.mqtt.mqtt_yt || '',
      mqtt_yk: p.mqtt.mqtt_yk || '',
    },
  }
  base.devices = buildDevicesMap()
  return base
}

function switchToJson() {
  if (viewMode.value === 'json') return
  configText.value = JSON.stringify(buildSaveObject(), null, 2)
  viewMode.value = 'json'
}

function switchToForm() {
  if (viewMode.value === 'form') return
  try {
    configObj.value = applyDefaults(JSON.parse(configText.value))
    loadDeviceList(configObj.value.devices)
    parseError.value = ''
    viewMode.value = 'form'
  } catch (err) {
    alert('当前 JSON 内容不合法，无法切换到表单模式，请先修正格式')
  }
}

async function loadConfig() {
  configError.value = ''
  try {
    configText.value = await getConfig()
    configLoaded.value = true
    try {
      configObj.value = applyDefaults(JSON.parse(configText.value))
      loadDeviceList(configObj.value.devices)
      parseError.value = ''
      viewMode.value = 'form'
    } catch {
      parseError.value = '配置文件不是合法 JSON，已切换为原始 JSON 模式'
      viewMode.value = 'json'
    }
  } catch (err) {
    configError.value = err instanceof ApiError ? err.message : '加载配置失败'
  }
}

async function loadBackups() {
  backupError.value = ''
  try {
    backups.value = await listBackups()
  } catch (err) {
    backupError.value = err instanceof ApiError ? err.message : '加载备份列表失败'
  }
}

async function saveConfig() {
  configError.value = ''
  if (viewMode.value === 'form') {
    const idError = validateDeviceIds()
    if (idError) {
      configError.value = idError
      return
    }
    configText.value = JSON.stringify(buildSaveObject(), null, 2)
  }
  configSaving.value = true
  try {
    await putConfig(configText.value)
    await loadBackups()
  } catch (err) {
    configError.value = err instanceof ApiError ? err.message : '保存失败'
  } finally {
    configSaving.value = false
  }
}

const backups = ref<BackupInfo[]>([])
const backupError = ref('')

const restarting = ref(false)
const restartMessage = ref('')
let pollTimer: ReturnType<typeof setInterval> | null = null
let pollElapsed = 0

function stopPolling() {
  if (pollTimer) {
    clearInterval(pollTimer)
    pollTimer = null
  }
}

async function doRestart() {
  if (!confirm('确认重启采集服务？重启期间设备会短暂断线。')) return
  try {
    await restart()
  } catch (err) {
    restartMessage.value = err instanceof ApiError ? `触发重启失败: ${err.message}` : '触发重启失败'
    return
  }
  restarting.value = true
  restartMessage.value = '重启中，等待服务恢复…'
  pollElapsed = 0
  stopPolling()
  pollTimer = setInterval(async () => {
    pollElapsed += 2
    try {
      const status = await getStatus()
      if (status.active_state === 'active') {
        stopPolling()
        restartMessage.value = '服务已恢复，正在刷新…'
        location.reload()
        return
      }
    } catch {
      // 服务重启期间连接不上是预期情况，继续轮询
    }
    if (pollElapsed >= 60) {
      stopPolling()
      restarting.value = false
      restartMessage.value = '等待超时，请手动确认服务状态'
    }
  }, 2000)
}

async function doRestore(name: string) {
  if (!confirm(`确认回滚到备份 ${name}？当前配置会被覆盖（但会先自动备份）。`)) return
  backupError.value = ''
  try {
    await restoreBackup(name)
    await Promise.all([loadConfig(), loadBackups()])
  } catch (err) {
    backupError.value = err instanceof ApiError ? err.message : '回滚失败'
  }
}

onMounted(() => {
  loadConfig()
  loadBackups()
})

onBeforeUnmount(stopPolling)
</script>

<template>
  <div class="panel">
    <section class="config">
        <div class="config-header">
          <h2>配置编辑</h2>
          <div class="mode-toggle">
            <button type="button" :class="{ active: viewMode === 'form' }" @click="switchToForm">表单模式</button>
            <button type="button" :class="{ active: viewMode === 'json' }" @click="switchToJson">原始 JSON</button>
          </div>
        </div>
        <p v-if="configError" class="error">{{ configError }}</p>
        <p v-if="parseError" class="error">{{ parseError }}</p>

        <div v-if="viewMode === 'form' && configObj" class="form-view">
          <div class="card">
            <h3>基本信息</h3>
            <div class="grid-2">
              <label class="field">
                <span>产品类型</span>
                <input v-model="configObj.product_type" type="text" />
              </label>
              <label class="field">
                <span>项目名称</span>
                <input v-model="configObj.project" type="text" />
              </label>
            </div>
          </div>

          <div class="card">
            <h3>HTTP 服务</h3>
            <label class="checkbox-row">
              <input v-model="configObj.program.http.enable" type="checkbox" />
              <span>启用</span>
            </label>
            <div class="grid-2">
              <label class="field">
                <span>监听 IP</span>
                <input v-model="configObj.program.http.ip" type="text" />
              </label>
              <label class="field">
                <span>端口</span>
                <input v-model.number="configObj.program.http.port" type="number" min="0" max="65535" />
              </label>
            </div>
          </div>

          <div class="card">
            <h3>Lua 脚本模块</h3>
            <label class="checkbox-row">
              <input v-model="configObj.program.mod.enable" type="checkbox" />
              <span>启用</span>
            </label>
            <label class="field">
              <span>脚本目录</span>
              <input v-model="configObj.program.mod.path" type="text" />
            </label>
          </div>

          <div class="card">
            <h3>北向 Modbus</h3>
            <label class="checkbox-row">
              <input v-model="configObj.program.north_modbus.enable" type="checkbox" />
              <span>启用</span>
            </label>
            <div class="grid-2">
              <label class="field">
                <span>监听 IP</span>
                <input v-model="configObj.program.north_modbus.north_modbus_host" type="text" />
              </label>
              <label class="field">
                <span>端口</span>
                <input
                  v-model.number="configObj.program.north_modbus.north_modbus_port"
                  type="number"
                  min="0"
                  max="65535"
                />
              </label>
            </div>
            <label class="field">
              <span>点表文件</span>
              <input v-model="configObj.program.north_modbus.north_modbus_conf" type="text" class="mono" />
            </label>
          </div>

          <div class="card">
            <h3>MQTT</h3>
            <label class="checkbox-row">
              <input v-model="configObj.program.mqtt.enable" type="checkbox" />
              <span>启用</span>
            </label>
            <div class="grid-2">
              <label class="field">
                <span>服务器地址</span>
                <input v-model="configObj.program.mqtt.mqtt_host" type="text" />
              </label>
              <label class="field">
                <span>端口</span>
                <input v-model.number="configObj.program.mqtt.mqtt_port" type="number" min="0" max="65535" />
              </label>
              <label class="field">
                <span>用户名</span>
                <input v-model="configObj.program.mqtt.mqtt_username" type="text" />
              </label>
              <label class="field">
                <span>密码</span>
                <input v-model="configObj.program.mqtt.mqtt_password" type="password" />
              </label>
              <label class="field">
                <span>遥测主题</span>
                <input v-model="configObj.program.mqtt.mqtt_yt" type="text" class="mono" />
              </label>
              <label class="field">
                <span>遥控主题</span>
                <input v-model="configObj.program.mqtt.mqtt_yk" type="text" class="mono" />
              </label>
            </div>
          </div>

          <div class="card">
            <h3>其它功能</h3>
            <div class="grid-2">
              <label class="checkbox-row">
                <input v-model="configObj.program.emu.enable" type="checkbox" />
                <span>虚拟机（EMU）</span>
              </label>
              <label class="checkbox-row">
                <input v-model="configObj.program.eg25_gl.enable" type="checkbox" />
                <span>EG25-GL 4G 模块</span>
              </label>
            </div>
          </div>

          <div class="card">
            <div class="config-header">
              <h3>设备列表</h3>
              <button type="button" @click="addDevice">+ 添加设备</button>
            </div>

            <div v-for="(dev, index) in deviceList" :key="index" class="device-card">
              <div class="config-header">
                <span class="device-title">设备 {{ index + 1 }}</span>
                <button type="button" @click="removeDevice(index)">删除</button>
              </div>
              <div class="grid-2">
                <label class="field">
                  <span>设备标识</span>
                  <input v-model="dev.id" type="text" class="mono" />
                </label>
                <label class="field">
                  <span>设备类型</span>
                  <input v-model="dev.config.type" type="text" placeholder="如 PCS / BMS" />
                </label>
                <label class="field">
                  <span>设备描述</span>
                  <input v-model="dev.desc" type="text" />
                </label>
                <label class="field">
                  <span>通信协议</span>
                  <select v-model="dev.config.com_type">
                    <option v-for="t in COM_TYPES" :key="t.value" :value="t.value">{{ t.label }}</option>
                  </select>
                </label>
              </div>

              <div v-if="showsRegisterFile(dev.config.com_type)" class="field">
                <span>点表文件</span>
                <input v-model="dev.config.register_file" type="text" class="mono" />
              </div>

              <div class="grid-2">
                <label class="field">
                  <span>采集间隔 (ms)</span>
                  <input v-model.number="dev.config.interval" type="number" min="0" />
                </label>
                <label class="field">
                  <span>超时时间 (ms)</span>
                  <input v-model.number="dev.config.timeout" type="number" min="0" />
                </label>
                <template v-if="showsRequestGap(dev.config.com_type)">
                  <label class="field">
                    <span>请求间隔 (ms，可选)</span>
                    <input v-model.number="dev.config.request_interval" type="number" min="0" />
                  </label>
                  <label class="field">
                    <span>寄存器合并最大间隙（可选）</span>
                    <input v-model.number="dev.config.max_gap" type="number" min="0" />
                  </label>
                </template>
              </div>

              <div v-if="showsNetwork(dev.config.com_type)" class="grid-2">
                <label class="field">
                  <span>IP 地址</span>
                  <input v-model="dev.config.ip" type="text" />
                </label>
                <label class="field">
                  <span>端口</span>
                  <input v-model.number="dev.config.port" type="number" min="0" max="65535" />
                </label>
              </div>

              <div v-if="showsSlave(dev.config.com_type)" class="field">
                <span>从站地址</span>
                <input v-model.number="dev.config.slave" type="number" min="0" max="255" />
              </div>

              <div v-if="showsInterface(dev.config.com_type)" class="field">
                <span>CAN 接口</span>
                <input v-model="dev.config.interface" type="text" placeholder="如 can0" />
              </div>

              <template v-if="dev.config.com_type === 'ModbusRTU'">
                <div class="grid-2">
                  <label class="field">
                    <span>串口设备</span>
                    <input v-model="dev.config.serial_tty" type="text" placeholder="如 /dev/ttyUSB0" />
                  </label>
                  <label class="field">
                    <span>{{ baudRateLabel(dev.config.com_type) }}</span>
                    <input v-model.number="dev.config.baud_rate" type="number" min="0" />
                  </label>
                  <label class="field">
                    <span>数据位</span>
                    <select v-model.number="dev.config.data_bits">
                      <option :value="5">5</option>
                      <option :value="6">6</option>
                      <option :value="7">7</option>
                      <option :value="8">8</option>
                    </select>
                  </label>
                  <label class="field">
                    <span>校验位</span>
                    <select v-model="dev.config.parity">
                      <option value="N">无 (N)</option>
                      <option value="E">偶校验 (E)</option>
                      <option value="O">奇校验 (O)</option>
                    </select>
                  </label>
                  <label class="field">
                    <span>停止位</span>
                    <select v-model.number="dev.config.stop_bits">
                      <option :value="1">1</option>
                      <option :value="2">2</option>
                    </select>
                  </label>
                </div>
              </template>

              <template v-else-if="showsBaudRate(dev.config.com_type)">
                <label class="field">
                  <span>{{ baudRateLabel(dev.config.com_type) }}</span>
                  <input v-model.number="dev.config.baud_rate" type="number" min="0" />
                </label>
              </template>

              <label class="field">
                <span>备注（可选）</span>
                <input v-model="dev.config.desc" type="text" />
              </label>
            </div>

            <p v-if="deviceList.length === 0" class="empty">暂无设备，点击“添加设备”创建一个</p>
          </div>
        </div>

        <textarea
          v-else
          v-model="configText"
          class="mono"
          rows="20"
          :disabled="!configLoaded"
        ></textarea>

        <div class="actions">
          <button :disabled="configSaving || !configLoaded" @click="saveConfig">保存配置</button>
          <button :disabled="restarting" @click="doRestart">重启采集服务</button>
        </div>
        <p v-if="restartMessage">{{ restartMessage }}</p>
      </section>

      <section class="backups">
        <h2>配置备份</h2>
        <p v-if="backupError" class="error">{{ backupError }}</p>
        <table>
          <thead>
            <tr>
              <th>文件名</th>
              <th>时间</th>
              <th>大小</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="b in backups" :key="b.name">
              <td class="mono">{{ b.name }}</td>
              <td>{{ b.created_at }}</td>
              <td>{{ b.size }} B</td>
              <td><button @click="doRestore(b.name)">回滚</button></td>
            </tr>
            <tr v-if="backups.length === 0">
              <td colspan="4" class="empty">暂无备份</td>
            </tr>
          </tbody>
        </table>
      </section>
  </div>
</template>

<style scoped>
.panel {
  max-width: 960px;
  margin: 0 auto;
  padding: 16px 20px 40px;
}

h2 {
  font-size: 16px;
  margin: 16px 0 8px;
  color: var(--accent);
  text-transform: uppercase;
  letter-spacing: 0.1em;
}

h3 {
  font-size: 14px;
  margin: 0 0 10px;
  color: var(--text-dim);
  text-transform: uppercase;
  letter-spacing: 0.08em;
}

input,
textarea,
select,
button {
  font: inherit;
  padding: 8px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--bg);
  color: var(--text);
}

textarea {
  width: 100%;
  box-sizing: border-box;
  resize: vertical;
}

.mono {
  font-family: ui-monospace, Consolas, monospace;
  font-size: 13px;
}

button {
  cursor: pointer;
  border-color: var(--accent);
  color: var(--accent);
}

button:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}

.actions {
  display: flex;
  gap: 10px;
  margin-top: 10px;
}

.error {
  color: var(--warn);
  font-size: 13px;
}

.empty {
  color: var(--text-dim);
  text-align: center;
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
  background: var(--bg-alt);
  border-bottom: 1px solid var(--accent);
}

tbody tr:hover {
  background: var(--accent-dim);
}

.config-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
}

.mode-toggle {
  display: flex;
  gap: 6px;
}

.mode-toggle button {
  padding: 6px 12px;
  font-size: 13px;
}

.mode-toggle button.active {
  background: var(--accent-dim);
  color: var(--accent);
  box-shadow: 0 0 12px var(--accent-glow);
}

.form-view {
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.card {
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 14px 16px;
  background: var(--bg-alt);
  box-shadow: 0 0 0 1px rgba(0, 212, 255, 0.05);
}

.grid-2 {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 10px 16px;
}

.field {
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 13px;
  margin-top: 10px;
}

.field > span {
  color: var(--text-dim);
}

.checkbox-row {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
}

.checkbox-row input {
  width: auto;
  padding: 0;
}

.device-card {
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 12px 14px;
  margin-top: 10px;
  background: var(--bg);
}

.device-card .field:first-child,
.device-card .grid-2:first-of-type .field {
  margin-top: 0;
}

.device-title {
  font-weight: 600;
  font-size: 13px;
  color: var(--accent);
}
</style>
