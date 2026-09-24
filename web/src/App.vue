<script setup lang="ts">
import { ref } from 'vue'
import Monitor from './components/Monitor.vue'
import SystemPanel from './components/SystemPanel.vue'

const tab = ref<'monitor' | 'system'>('monitor')
</script>

<template>
  <div class="tabs">
    <button :class="{ active: tab === 'monitor' }" @click="tab = 'monitor'">点位监控</button>
    <button :class="{ active: tab === 'system' }" @click="tab = 'system'">系统管理</button>
  </div>
  <Monitor v-if="tab === 'monitor'" />
  <SystemPanel v-else />
</template>

<style scoped>
.tabs {
  display: flex;
  gap: 4px;
  padding: 12px 20px 0;
  max-width: 960px;
  margin: 0 auto;
}

.tabs button {
  font: inherit;
  padding: 8px 16px;
  border: 1px solid var(--border);
  border-bottom: none;
  border-radius: 6px 6px 0 0;
  background: var(--bg-alt);
  color: var(--text-dim);
  cursor: pointer;
}

.tabs button.active {
  background: var(--bg);
  color: var(--text);
  font-weight: 600;
}
</style>
