#!/usr/bin/env node
/** Host-only Harness API Key bridge; stdout carries NDJSON only. */
import { createRequire } from 'node:module'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import process from 'node:process'
import readline from 'node:readline'

const [servicePath, dshHome] = process.argv.slice(2)
if (!servicePath || !dshHome) {
  console.error('usage: credential-service.mjs <dsh-acp-path> <dsh-home>')
  process.exit(2)
}

const localPackage = join(servicePath, 'packages/credentials/credentials-local/package.json')
const requireLocal = createRequire(localPackage)
const load = async name => import(pathToFileURL(requireLocal.resolve(name)).href)
const [{ Context }, { default: LocalCredentialProvider }, { credentialRef }] = await Promise.all([
  load('@deepseek-ai/cordis'),
  load('@deepseek-ai/dsh-credentials-local'),
  load('@deepseek-ai/dsh-credentials'),
])

const ctx = new Context()
const provider = ctx.plugin(LocalCredentialProvider, { path: join(dshHome, '.credentials.yaml') })
const send = message => {
  if (!process.stdout.destroyed) process.stdout.write(`${JSON.stringify(message)}\n`)
}
const safeError = error => error instanceof Error ? error.message : String(error)

try {
  await provider
} catch (error) {
  send({ event: 'fatal', error: safeError(error) })
  process.exit(1)
}

const apiKeyRef = credentialRef('DEEPSEEK_API_KEY')
const status = async () => ({ apiKey: await ctx.credentials.describe(apiKeyRef) })
const methods = new Map([
  ['status', status],
  ['saveApiKey', async ({ value }) => {
    if (typeof value !== 'string' || !/^[\x21-\x7e]+$/.test(value)
      || /^[A-Z0-9_]+=[^=]/.test(value)
      || /^(['"`]).*\1$/.test(value)) {
      throw new Error('请输入 API Key 本身，不要包含引号、空格或环境变量赋值')
    }
    await ctx.credentials.set(apiKeyRef, value)
    return status()
  }],
  ['removeApiKey', async () => {
    await ctx.credentials.unset(apiKeyRef)
    return status()
  }],
])

const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity })
lines.on('line', (line) => {
  void (async () => {
    let request
    try {
      request = JSON.parse(line)
      if (!request || typeof request.id !== 'number' || typeof request.method !== 'string') return
      const method = methods.get(request.method)
      if (!method) throw new Error(`unsupported credential method: ${request.method}`)
      const result = await method(request.params ?? {})
      send({ id: request.id, result })
    } catch (error) {
      if (typeof request?.id === 'number') send({ id: request.id, error: safeError(error) })
    }
  })()
})

let closing = false
const close = async () => {
  if (closing) return
  closing = true
  lines.close()
  await provider.dispose()
  process.exit(0)
}
process.stdin.on('end', () => { void close() })
process.on('SIGTERM', () => { void close() })
process.on('SIGINT', () => { void close() })
send({ event: 'ready' })
