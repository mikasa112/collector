import EditorWorker from 'monaco-editor/editor/editor.worker.js?worker'
// monaco-editor 自带 `declare global { var MonacoEnvironment }` 的环境声明，
// 引入类型即可让下面的 self.MonacoEnvironment 通过类型检查。
import type {} from 'monaco-editor'

// Monaco 需要通过 Web Worker 跑通用编辑服务（查找/替换、diff 等）；
// Lua 只用到 Monarch 词法高亮，跑在主线程，不需要专门的 worker。
self.MonacoEnvironment = {
  getWorker() {
    return new EditorWorker()
  },
}
