<script setup lang="ts">
import { ref } from 'vue'
import { ApiError, clearToken, loggedIn, login, setToken } from './api'
import Monitor from './components/Monitor.vue'
import ScriptEditor from './components/ScriptEditor.vue'
import SystemPanel from './components/SystemPanel.vue'

const tab = ref<'monitor' | 'system' | 'scripts'>('monitor')

const username = ref('')
const password = ref('')
const loginError = ref('')

async function doLogin() {
  loginError.value = ''
  try {
    const token = await login(username.value, password.value)
    setToken(token)
    password.value = ''
  } catch (err) {
    loginError.value = err instanceof ApiError ? err.message : '登录失败'
  }
}

function doLogout() {
  clearToken()
}
</script>

<template>
  <form v-if="!loggedIn" class="login-page" @submit.prevent="doLogin">
    <h1>后台</h1>
    <input v-model="username" type="text" placeholder="用户名" autocomplete="username" />
    <input v-model="password" type="password" placeholder="密码" autocomplete="current-password" />
    <button type="submit">登录</button>
    <p v-if="loginError" class="error">{{ loginError }}</p>
  </form>

  <template v-else>
    <div class="topbar">
      <h1>后台</h1>
      <div class="tabs">
        <button :class="{ active: tab === 'monitor' }" @click="tab = 'monitor'">点位监控</button>
        <button :class="{ active: tab === 'system' }" @click="tab = 'system'">系统管理</button>
        <button :class="{ active: tab === 'scripts' }" @click="tab = 'scripts'">脚本管理</button>
      </div>
      <button type="button" class="logout" @click="doLogout">退出登录</button>
    </div>
    <Monitor v-if="tab === 'monitor'" />
    <SystemPanel v-else-if="tab === 'system'" />
    <ScriptEditor v-else />
  </template>
</template>

<style scoped>
.login-page {
  max-width: 320px;
  margin: 120px auto 0;
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 28px 24px;
  background: var(--bg-alt);
  border: 1px solid var(--border);
  border-radius: 10px;
  box-shadow: 0 0 0 1px rgba(0, 212, 255, 0.08), 0 0 30px rgba(0, 212, 255, 0.12);
}

.login-page h1 {
  text-align: center;
  font-size: 20px;
  text-transform: uppercase;
  letter-spacing: 0.15em;
  margin: 0 0 8px;
  color: var(--accent);
  text-shadow: 0 0 12px var(--accent-glow);
}

.login-page input,
.login-page button {
  font: inherit;
  padding: 9px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--bg);
  color: var(--text);
}

.login-page button {
  cursor: pointer;
  border-color: var(--accent);
  color: var(--accent);
  background: var(--accent-dim);
  font-weight: 600;
  letter-spacing: 0.05em;
}

.error {
  color: var(--warn);
  font-size: 13px;
}

.topbar {
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 14px 20px;
  max-width: 960px;
  margin: 0 auto;
  border-bottom: 1px solid var(--border);
  box-shadow: 0 1px 0 0 rgba(0, 212, 255, 0.15);
}

.topbar h1 {
  font-size: 16px;
  margin: 0;
  white-space: nowrap;
  text-transform: uppercase;
  letter-spacing: 0.15em;
  color: var(--accent);
  text-shadow: 0 0 10px var(--accent-glow);
}

.tabs {
  display: flex;
  gap: 4px;
  flex: 1;
}

.tabs button {
  font: inherit;
  padding: 8px 16px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--bg-alt);
  color: var(--text-dim);
  cursor: pointer;
  letter-spacing: 0.02em;
}

.tabs button.active {
  background: var(--accent-dim);
  border-color: var(--accent);
  color: var(--accent);
  font-weight: 600;
  box-shadow: 0 0 12px var(--accent-glow);
}

.logout {
  font: inherit;
  padding: 6px 12px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--bg-alt);
  color: var(--text-dim);
  cursor: pointer;
  white-space: nowrap;
}

.logout:hover {
  color: var(--warn);
  border-color: var(--warn);
  box-shadow: 0 0 10px var(--warn-glow);
}
</style>
