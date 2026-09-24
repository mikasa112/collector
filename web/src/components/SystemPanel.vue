<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref } from 'vue'
import {
  ApiError,
  getConfig,
  getStatus,
  getToken,
  listBackups,
  login,
  putConfig,
  restart,
  restoreBackup,
  setToken,
  type BackupInfo,
} from '../api'

const loggedIn = ref(!!getToken())
const username = ref('')
const password = ref('')
const loginError = ref('')

const configText = ref('')
const configError = ref('')
const configSaving = ref(false)
const configLoaded = ref(false)

const backups = ref<BackupInfo[]>([])
const backupError = ref('')

const restarting = ref(false)
const restartMessage = ref('')
let pollTimer: ReturnType<typeof setInterval> | null = null
let pollElapsed = 0

async function doLogin() {
  loginError.value = ''
  try {
    const token = await login(username.value, password.value)
    setToken(token)
    loggedIn.value = true
    await Promise.all([loadConfig(), loadBackups()])
  } catch (err) {
    loginError.value = err instanceof ApiError ? err.message : '登录失败'
  }
}

async function loadConfig() {
  configError.value = ''
  try {
    configText.value = await getConfig()
    configLoaded.value = true
  } catch (err) {
    configError.value = err instanceof ApiError ? err.message : '加载配置失败'
    if (err instanceof ApiError && err.status === 401) {
      loggedIn.value = false
    }
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
  configSaving.value = true
  configError.value = ''
  try {
    await putConfig(configText.value)
    await loadBackups()
  } catch (err) {
    configError.value = err instanceof ApiError ? err.message : '保存失败'
  } finally {
    configSaving.value = false
  }
}

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
  if (loggedIn.value) {
    loadConfig()
    loadBackups()
  }
})

onBeforeUnmount(stopPolling)
</script>

<template>
  <div class="panel">
    <form v-if="!loggedIn" class="login" @submit.prevent="doLogin">
      <h2>登录</h2>
      <input v-model="username" type="text" placeholder="用户名" autocomplete="username" />
      <input v-model="password" type="password" placeholder="密码" autocomplete="current-password" />
      <button type="submit">登录</button>
      <p v-if="loginError" class="error">{{ loginError }}</p>
    </form>

    <template v-else>
      <section class="config">
        <h2>配置编辑</h2>
        <p v-if="configError" class="error">{{ configError }}</p>
        <textarea v-model="configText" class="mono" rows="20" :disabled="!configLoaded"></textarea>
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
    </template>
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
}

.login {
  display: flex;
  flex-direction: column;
  gap: 10px;
  max-width: 320px;
}

input,
textarea,
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
}

th,
td {
  text-align: left;
  padding: 6px 10px;
  border-bottom: 1px solid var(--border);
}
</style>
