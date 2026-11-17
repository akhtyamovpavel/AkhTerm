import {closeSync, mkdirSync, mkdtempSync, openSync, rmdirSync, unlinkSync, writeSync} from "node:fs"
import {homedir} from "node:os"
import {join} from "node:path"

function usage(api, id) {
  const messages = api.state.session.messages(id).filter(m => m.role === "assistant" && m.tokens &&
    (m.tokens.input || m.tokens.cache?.read || m.tokens.cache?.write))
  const last = messages.at(-1)
  if (!last) return {}
  const sums = messages.reduce((sum, m) => {
    sum.input += m.tokens.input || 0
    sum.read += m.tokens.cache?.read || 0
    sum.write += m.tokens.cache?.write || 0
    sum.output += m.tokens.output || 0
    return sum
  }, {input: 0, read: 0, write: 0, output: 0})
  const t = last.tokens
  return {...sums, request: last.id, last_read: t.cache?.read,
    context: t.input + (t.cache?.read || 0) + (t.cache?.write || 0)}
}

export default {
  id: "akhterm-usage",
  async tui(api) {
    const state = process.env.XDG_STATE_HOME || join(homedir(), ".local/state")
    const root = join(state, "akhterm", "opencode")
    mkdirSync(root, {recursive: true, mode: 0o700})
    const directory = mkdtempSync(join(root, "client-"))
    let file, fd, previous
    function close() {
      if (fd !== undefined) closeSync(fd)
      fd = undefined
      if (file) unlinkSync(file)
      file = previous = undefined
    }
    function update() {
      const route = api.route.current
      const id = route.name === "session" && route.params?.sessionID
      if (!id || !/^ses_[a-zA-Z0-9_-]+$/.test(id)) return close()
      const target = join(directory, id + ".jsonl")
      if (file !== target) {
        close()
        fd = openSync(target, "wx", 0o600)
        file = target
      }
      const line = JSON.stringify(usage(api, id))
      if (line !== previous) writeSync(fd, line + "\n")
      previous = line
    }
    const timer = setInterval(() => {
      try { update() } catch { /* Retry when the session or filesystem is ready. */ }
    }, 750)
    timer.unref()
    api.lifecycle.onDispose(() => {
      clearInterval(timer)
      close()
      rmdirSync(directory)
    })
  }
}
