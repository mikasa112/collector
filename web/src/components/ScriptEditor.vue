<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import MonacoEditor from 'monaco-editor-vue3'
import {
  ApiError,
  createScript,
  deleteScript,
  getScript,
  listScripts,
  saveScript,
  type ScriptEntry,
} from '../api'

const scripts = ref<ScriptEntry[]>([])
const treeError = ref('')
const treeLoading = ref(false)

const selectedPath = ref('')
const content = ref('')
const originalContent = ref('')
const isNewFile = ref(false)
const fileError = ref('')
const fileLoading = ref(false)
const saving = ref(false)

const newPathInput = ref('')

const dirty = computed(() => selectedPath.value !== '' && content.value !== originalContent.value)

const editorOptions = computed(() => ({
  readOnly: fileLoading.value,
  automaticLayout: true,
  minimap: { enabled: false },
  fontSize: 13,
  tabSize: 2,
  scrollBeyondLastLine: false,
}))

async function loadTree() {
  treeError.value = ''
  treeLoading.value = true
  try {
    scripts.value = await listScripts()
  } catch (err) {
    treeError.value = err instanceof ApiError ? err.message : '加载脚本列表失败'
  } finally {
    treeLoading.value = false
  }
}

function depthOf(path: string): number {
  return path.split('/').length - 1
}

function nameOf(path: string): string {
  const parts = path.split('/')
  return parts[parts.length - 1]
}

async function openFile(entry: ScriptEntry) {
  if (entry.is_dir) return
  if (dirty.value && !confirm('当前脚本有未保存的修改，确认放弃并切换？')) return
  fileError.value = ''
  isNewFile.value = false
  fileLoading.value = true
  try {
    const text = await getScript(entry.path)
    selectedPath.value = entry.path
    content.value = text
    originalContent.value = text
  } catch (err) {
    fileError.value = err instanceof ApiError ? err.message : '加载脚本内容失败'
  } finally {
    fileLoading.value = false
  }
}

function startNewFile() {
  const raw = newPathInput.value.trim()
  if (!raw) return
  const path = raw.endsWith('.lua') ? raw : `${raw}.lua`
  if (scripts.value.some((e) => e.path === path)) {
    fileError.value = `脚本已存在: ${path}`
    return
  }
  fileError.value = ''
  isNewFile.value = true
  selectedPath.value = path
  content.value = ''
  originalContent.value = ''
  newPathInput.value = ''
}

async function saveCurrent() {
  if (!selectedPath.value) return
  fileError.value = ''
  saving.value = true
  try {
    if (isNewFile.value) {
      await createScript(selectedPath.value, content.value)
      isNewFile.value = false
    } else {
      await saveScript(selectedPath.value, content.value)
    }
    originalContent.value = content.value
    await loadTree()
  } catch (err) {
    fileError.value = err instanceof ApiError ? err.message : '保存脚本失败'
  } finally {
    saving.value = false
  }
}

async function deleteCurrent() {
  if (!selectedPath.value || isNewFile.value) return
  if (!confirm(`确认删除脚本 ${selectedPath.value}？此操作不可撤销。`)) return
  fileError.value = ''
  try {
    await deleteScript(selectedPath.value)
    selectedPath.value = ''
    content.value = ''
    originalContent.value = ''
    await loadTree()
  } catch (err) {
    fileError.value = err instanceof ApiError ? err.message : '删除脚本失败'
  }
}

onMounted(() => {
  loadTree()
})
</script>

<template>
  <div class="panel">
    <div class="editor-layout">
        <section class="tree-pane">
          <div class="tree-header">
            <h2>lua_scripts</h2>
            <button type="button" :disabled="treeLoading" @click="loadTree">刷新</button>
          </div>
          <p v-if="treeError" class="error">{{ treeError }}</p>
          <ul class="tree">
            <li
              v-for="entry in scripts"
              :key="entry.path"
              :class="{ dir: entry.is_dir, active: entry.path === selectedPath }"
              :style="{ paddingLeft: `${depthOf(entry.path) * 16 + 8}px` }"
              @click="openFile(entry)"
            >
              <span class="icon">{{ entry.is_dir ? '📁' : '📄' }}</span>
              <span class="name">{{ nameOf(entry.path) }}</span>
            </li>
            <li v-if="scripts.length === 0" class="empty">暂无脚本文件</li>
          </ul>
          <div class="new-file">
            <input
              v-model="newPathInput"
              type="text"
              class="mono"
              placeholder="新脚本路径，如 my_script.lua"
              @keyup.enter="startNewFile"
            />
            <button type="button" @click="startNewFile">新建</button>
          </div>
        </section>

        <section class="editor-pane">
          <template v-if="selectedPath">
            <div class="editor-header">
              <span class="mono path">{{ selectedPath }}</span>
              <span v-if="isNewFile" class="badge">未保存的新文件</span>
              <span v-else-if="dirty" class="badge">已修改</span>
            </div>
            <p v-if="fileError" class="error">{{ fileError }}</p>
            <MonacoEditor
              v-model:value="content"
              class="code-editor"
              language="lua"
              theme="vs-dark"
              height="480px"
              :options="editorOptions"
            />
            <div class="actions">
              <button :disabled="saving || (!dirty && !isNewFile)" @click="saveCurrent">保存</button>
              <button :disabled="isNewFile" @click="deleteCurrent">删除</button>
            </div>
          </template>
          <p v-else class="empty">从左侧选择一个脚本文件，或新建一个</p>
        </section>
    </div>
  </div>
</template>

<style scoped>
.panel {
  max-width: 1100px;
  margin: 0 auto;
  padding: 16px 20px 40px;
}

h2 {
  font-size: 16px;
  margin: 0;
  color: var(--accent);
  text-transform: uppercase;
  letter-spacing: 0.1em;
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

.error {
  color: var(--warn);
  font-size: 13px;
}

.empty {
  color: var(--text-dim);
  text-align: center;
  padding: 12px 0;
}

.editor-layout {
  display: grid;
  grid-template-columns: 280px minmax(0, 1fr);
  gap: 16px;
  align-items: start;
}

.tree-pane {
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 12px 14px;
  background: var(--bg-alt);
  box-shadow: 0 0 0 1px rgba(0, 212, 255, 0.05);
}

.tree-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 8px;
}

.tree-header button {
  padding: 4px 10px;
  font-size: 12px;
}

.tree {
  list-style: none;
  margin: 0;
  padding: 0;
  max-height: 480px;
  overflow-y: auto;
}

.tree li {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 8px;
  border-radius: 4px;
  cursor: pointer;
  font-size: 13px;
  white-space: nowrap;
}

.tree li:hover {
  background: var(--bg);
}

.tree li.active {
  background: var(--accent-dim);
  color: var(--accent);
  box-shadow: inset 0 0 0 1px var(--accent);
}

.tree li.dir {
  cursor: default;
  color: var(--text-dim);
}

.new-file {
  display: flex;
  gap: 6px;
  margin-top: 10px;
}

.new-file input {
  flex: 1;
  min-width: 0;
  padding: 6px 8px;
  font-size: 12px;
}

.new-file button {
  padding: 6px 10px;
  font-size: 12px;
}

.editor-pane {
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 14px 16px;
  background: var(--bg-alt);
  min-width: 0;
  overflow: hidden;
  box-shadow: 0 0 0 1px rgba(0, 212, 255, 0.05);
}

.editor-header {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 8px;
}

.path {
  font-weight: 600;
}

.badge {
  font-size: 12px;
  color: var(--warn);
}

.code-editor {
  width: 100%;
  min-width: 0;
  border: 1px solid var(--border);
  border-radius: 6px;
  overflow: hidden;
}

.actions {
  display: flex;
  gap: 10px;
  margin-top: 10px;
}
</style>
