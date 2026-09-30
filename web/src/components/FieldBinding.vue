<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import {
  ApiError,
  isAdmin,
  listDevices,
  listFieldBindings,
  resetFieldBinding,
  setFieldBinding,
  type FieldBindingView,
  type PointKind,
} from '../api'

const bindings = ref<FieldBindingView[]>([])
const devices = ref<string[]>([])
const listError = ref('')
const listLoading = ref(false)

const editingKey = ref('')
const form = ref({ dev_id: '', point_kind: 'Id' as PointKind, point_value: '' })
const formError = ref('')
const saving = ref(false)
const resettingKey = ref('')

const DIRECTION_LABELS: Record<string, string> = {
  read: '只读',
  write: '只写',
  read_write: '读写',
}

const POINT_KINDS: { value: PointKind; label: string }[] = [
  { value: 'Id', label: '点位ID' },
  { value: 'Key', label: '点位Key' },
  { value: 'Name', label: '点位名称' },
]

const editingRow = computed(() => bindings.value.find((b) => b.field_key === editingKey.value))

// 可读字段（read/read_write）不支持绑定为 Name 引用，DataCenter 本身不支持按名称读取
function pointKindOptions(direction: string) {
  if (direction === 'write') return POINT_KINDS
  return POINT_KINDS.filter((k) => k.value !== 'Name')
}

async function loadBindings() {
  listError.value = ''
  listLoading.value = true
  try {
    const res = await listFieldBindings()
    bindings.value = res.data
  } catch (err) {
    listError.value = err instanceof ApiError ? err.message : '加载字段绑定列表失败'
  } finally {
    listLoading.value = false
  }
}

async function loadDevices() {
  try {
    devices.value = await listDevices()
  } catch {
    // 设备列表仅用于下拉辅助填写，加载失败不影响主流程，允许手动输入 dev_id
  }
}

function startEdit(row: FieldBindingView) {
  formError.value = ''
  editingKey.value = row.field_key
  form.value = { dev_id: row.dev_id, point_kind: row.point_kind, point_value: row.point_value }
}

function cancelEdit() {
  editingKey.value = ''
  formError.value = ''
}

async function saveEdit() {
  const row = editingRow.value
  if (!row) return
  formError.value = ''
  if (!form.value.dev_id.trim()) {
    formError.value = '设备标识不能为空'
    return
  }
  if (!form.value.point_value.trim()) {
    formError.value = '点位取值不能为空'
    return
  }
  if (form.value.point_kind === 'Id' && !/^\d+$/.test(form.value.point_value.trim())) {
    formError.value = 'point_kind 为点位ID时，取值必须是合法的数字'
    return
  }
  saving.value = true
  try {
    await setFieldBinding({
      field_key: row.field_key,
      dev_id: form.value.dev_id.trim(),
      point_kind: form.value.point_kind,
      point_value: form.value.point_value.trim(),
    })
    editingKey.value = ''
    await loadBindings()
  } catch (err) {
    formError.value = err instanceof ApiError ? err.message : '保存绑定失败'
  } finally {
    saving.value = false
  }
}

async function doReset(row: FieldBindingView) {
  if (!confirm(`确认将字段「${row.name}」重置为代码默认绑定？`)) return
  listError.value = ''
  resettingKey.value = row.field_key
  try {
    await resetFieldBinding(row.field_key)
    if (editingKey.value === row.field_key) editingKey.value = ''
    await loadBindings()
  } catch (err) {
    listError.value = err instanceof ApiError ? err.message : '重置绑定失败'
  } finally {
    resettingKey.value = ''
  }
}

onMounted(() => {
  loadBindings()
  loadDevices()
})
</script>

<template>
  <div class="panel">
    <section class="card">
      <div class="card-header">
        <h2>字段元模型映射</h2>
        <button type="button" :disabled="listLoading" @click="loadBindings">刷新</button>
      </div>
      <p class="hint">策略代码中的逻辑字段（如 SOC）默认绑定到内置的设备点位，此处可覆盖为其它点位，保存后立即生效、无需重启。</p>
      <p v-if="listError" class="error">{{ listError }}</p>

      <table>
        <thead>
          <tr>
            <th>字段</th>
            <th>方向</th>
            <th>当前绑定</th>
            <th>状态</th>
            <th>更新人</th>
            <th v-if="isAdmin()"></th>
          </tr>
        </thead>
        <tbody>
          <template v-for="row in bindings" :key="row.field_key">
            <tr>
              <td>
                <div>{{ row.name }}</div>
                <div class="mono dim">{{ row.field_key }}</div>
              </td>
              <td>{{ DIRECTION_LABELS[row.direction] ?? row.direction }}</td>
              <td class="mono">{{ row.dev_id }} / {{ row.point_kind }}={{ row.point_value }}</td>
              <td>
                <span :class="['badge', row.is_override ? 'override' : 'default']">
                  {{ row.is_override ? '已覆盖' : '默认' }}
                </span>
              </td>
              <td>{{ row.updated_by ?? '-' }}</td>
              <td v-if="isAdmin()" class="ops">
                <button type="button" @click="startEdit(row)">编辑</button>
                <button
                  type="button"
                  :disabled="!row.is_override || resettingKey === row.field_key"
                  @click="doReset(row)"
                >
                  重置
                </button>
              </td>
            </tr>
            <tr v-if="editingKey === row.field_key" class="edit-row">
              <td colspan="6">
                <div class="edit-form">
                  <label class="field">
                    <span>设备标识</span>
                    <input v-model="form.dev_id" type="text" class="mono" list="field-binding-devices" />
                  </label>
                  <label class="field">
                    <span>点位引用方式</span>
                    <select v-model="form.point_kind">
                      <option v-for="k in pointKindOptions(row.direction)" :key="k.value" :value="k.value">
                        {{ k.label }}
                      </option>
                    </select>
                  </label>
                  <label class="field">
                    <span>点位取值</span>
                    <input v-model="form.point_value" type="text" class="mono" />
                  </label>
                  <div class="edit-actions">
                    <button type="button" :disabled="saving" @click="saveEdit">保存</button>
                    <button type="button" :disabled="saving" @click="cancelEdit">取消</button>
                  </div>
                  <p v-if="formError" class="error">{{ formError }}</p>
                </div>
              </td>
            </tr>
          </template>
          <tr v-if="bindings.length === 0 && !listLoading">
            <td :colspan="isAdmin() ? 6 : 5" class="empty">暂无已注册的字段</td>
          </tr>
        </tbody>
      </table>
      <datalist id="field-binding-devices">
        <option v-for="dev in devices" :key="dev" :value="dev" />
      </datalist>
    </section>
  </div>
</template>

<style scoped>
.panel {
  max-width: 960px;
  margin: 0 auto;
  padding: 16px 20px 40px;
}

.card {
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 14px 16px;
  background: var(--bg-alt);
  box-shadow: 0 0 0 1px rgba(0, 212, 255, 0.05);
}

.card-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
}

h2 {
  font-size: 16px;
  margin: 0;
  color: var(--accent);
  text-transform: uppercase;
  letter-spacing: 0.1em;
}

.hint {
  color: var(--text-dim);
  font-size: 13px;
  margin: 8px 0 12px;
}

input,
select,
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

.dim {
  color: var(--text-dim);
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
  vertical-align: top;
}

thead th {
  background: var(--bg-alt);
  border-bottom: 1px solid var(--accent);
}

tbody tr:not(.edit-row):hover {
  background: var(--accent-dim);
}

.ops {
  display: flex;
  gap: 6px;
  white-space: nowrap;
}

.ops button {
  padding: 5px 10px;
  font-size: 12px;
}

.badge {
  display: inline-block;
  padding: 2px 8px;
  border-radius: 10px;
  font-size: 12px;
}

.badge.default {
  color: var(--text-dim);
  border: 1px solid var(--border);
}

.badge.override {
  color: var(--ok);
  border: 1px solid var(--ok);
  box-shadow: 0 0 8px var(--ok-glow);
}

.edit-row td {
  background: var(--bg);
}

.edit-form {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 10px 16px;
  align-items: end;
}

.field {
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 13px;
}

.field > span {
  color: var(--text-dim);
}

.edit-actions {
  display: flex;
  gap: 10px;
}

.edit-form .error {
  grid-column: 1 / -1;
  margin: 0;
}
</style>
