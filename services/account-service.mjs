#!/usr/bin/env node
/** Host-only Harness account and credential bridge; stdout carries NDJSON only. */
import { createRequire } from 'node:module'
import { dirname, join } from 'node:path'
import { pathToFileURL } from 'node:url'
import process from 'node:process'
import readline from 'node:readline'

const [servicePath, dshHome] = process.argv.slice(2)
if (!servicePath || !dshHome) {
  console.error('usage: account-service.mjs <dsh-acp-path> <dsh-home>')
  process.exit(2)
}

const accountPackage = join(servicePath, 'packages/credentials/deepseek-account-platform/package.json')
const localPackage = join(servicePath, 'packages/credentials/credentials-local/package.json')
const requireAccount = createRequire(accountPackage)
const requireLocal = createRequire(localPackage)
const load = async (require, name) => import(pathToFileURL(require.resolve(name)).href)
const [{ Context }, { default: WebServer }, { default: LocalCredentialProvider },
  { default: AuthorizationService }, { PlatformAccount }, { credentialRef }] = await Promise.all([
  load(requireAccount, '@deepseek-ai/cordis'),
  load(requireAccount, '@deepseek-ai/dsh-host-webserver'),
  load(requireLocal, '@deepseek-ai/dsh-credentials-local'),
  load(requireAccount, '@deepseek-ai/dsh-authorization'),
  load(requireAccount, '@deepseek-ai/dsh-deepseek-account-platform'),
  load(requireAccount, '@deepseek-ai/dsh-credentials'),
])

const ctx = new Context()
const lifetime = new AbortController()
const plugins = []
const mount = async (plugin, config) => {
  const fiber = ctx.plugin(plugin, config)
  plugins.push(fiber)
  await fiber
}
const send = (message) => {
  if (!process.stdout.destroyed) process.stdout.write(`${JSON.stringify(message)}\n`)
}
const safeError = (error) => error instanceof Error ? error.message : String(error)
const client = {
  version: '0.1.0',
  locale: 'zh-CN',
  timezoneOffsetSeconds: -new Date().getTimezoneOffset() * 60,
}

try {
  await mount(WebServer, { host: '127.0.0.1', port: 0 })
  await mount(LocalCredentialProvider, { path: join(dshHome, '.credentials.yaml') })
  await mount(AuthorizationService)
  await mount(PlatformAccount, {
    desktopPlatform: process.platform === 'darwin' || process.platform === 'win32' ? process.platform : null,
  })
} catch (error) {
  send({ event: 'fatal', error: safeError(error) })
  process.exit(1)
}

const account = ctx.deepseekAccount
const apiKeyRef = credentialRef('DEEPSEEK_API_KEY')
let lastAccount
let lastSerialized = ''
const phaseRank = new Map(['initializing', 'waiting-browser', 'exchanging', 'committing', 'cancelled', 'expired', 'failed', 'succeeded']
  .map((phase, index) => [phase, index]))
const publishAccount = (state) => {
  if (lastAccount?.attempt?.id !== undefined && lastAccount.attempt.id === state.attempt?.id
    && (phaseRank.get(state.attempt?.phase) ?? -1) < (phaseRank.get(lastAccount.attempt?.phase) ?? -1)) return
  const serialized = JSON.stringify(state)
  if (serialized === lastSerialized) return
  lastAccount = state
  lastSerialized = serialized
  send({ event: 'account', state })
}
const status = async () => ({
  account: await account.getState(),
  apiKey: await ctx.credentials.describe(apiKeyRef),
})
const profile = async () => {
  const [profileResult, balanceResult] = await Promise.allSettled([
    account.getProfile(client),
    account.getBalance(client),
  ])
  return {
    profile: profileResult.status === 'fulfilled' ? profileResult.value : { status: 'failed' },
    balance: balanceResult.status === 'fulfilled' ? balanceResult.value : { status: 'failed' },
  }
}

void (async () => {
  try {
    for await (const state of account.watch(lifetime.signal)) publishAccount(state)
  } catch (error) {
    if (!lifetime.signal.aborted) send({ event: 'error', error: safeError(error) })
  }
})()

const pollAccount = setInterval(() => {
  void account.getState().then(publishAccount).catch((error) => {
    if (!lifetime.signal.aborted) send({ event: 'error', error: safeError(error) })
  })
}, 500)

const methods = {
  status,
  profile,
  async avatar({ url }) {
    const target = new URL(url)
    if (target.protocol !== 'https:' || target.username || target.password) {
      throw new Error('头像地址必须是安全的 HTTPS 图片地址')
    }
    const response = await fetch(target, {
      headers: { accept: 'image/avif,image/webp,image/png,image/jpeg,image/gif' },
      redirect: 'follow',
      referrerPolicy: 'no-referrer',
      signal: AbortSignal.timeout(12000),
    })
    if (!response.ok || new URL(response.url).protocol !== 'https:') {
      throw new Error(`头像读取失败：HTTP ${response.status}`)
    }
    const declaredLength = Number(response.headers.get('content-length') ?? 0)
    if (declaredLength > 4 * 1024 * 1024) throw new Error('头像图片超过 4 MB 限制')
    const bytes = Buffer.from(await response.arrayBuffer())
    if (bytes.length > 4 * 1024 * 1024) throw new Error('头像图片超过 4 MB 限制')
    const declaredType = response.headers.get('content-type')?.split(';', 1)[0].toLowerCase()
    const type = ['image/avif', 'image/webp', 'image/png', 'image/jpeg', 'image/gif'].includes(declaredType)
      ? declaredType
      : bytes.subarray(0, 12).includes(Buffer.from('ftypavif')) ? 'image/avif'
        : bytes[0] === 0xff && bytes[1] === 0xd8 ? 'image/jpeg'
          : bytes.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) ? 'image/png'
            : bytes.subarray(0, 4).toString() === 'GIF8' ? 'image/gif'
              : bytes.subarray(0, 4).toString() === 'RIFF' && bytes.subarray(8, 12).toString() === 'WEBP' ? 'image/webp'
                : null
    if (!type) throw new Error('账户服务返回的不是受支持的图片')
    return { url, data: `data:${type};base64,${bytes.toString('base64')}` }
  },
  async saveApiKey({ value }) {
    if (typeof value !== 'string' || !/^[\x21-\x7e]+$/.test(value)
      || /^[A-Z][A-Z0-9_]*=[^=]/.test(value)) {
      throw new Error('请输入 API Key 本身，不要包含引号、空格或环境变量赋值')
    }
    await ctx.credentials.set(apiKeyRef, value)
    return status()
  },
  async removeApiKey() {
    await ctx.credentials.unset(apiKeyRef)
    return status()
  },
  async startSignIn() {
    const state = await account.startSignIn(client, ctx.webServer.getOrigin(), 'desktop')
    publishAccount(state)
    return state
  },
  async cancelSignIn({ attemptId }) {
    const state = await account.cancelSignIn(attemptId)
    publishAccount(state)
    return state
  },
  async signOut() {
    const state = await account.signOut(client)
    publishAccount(state)
    return state
  },
}

const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity })
lines.on('line', (line) => {
  void (async () => {
    let request
    try {
      request = JSON.parse(line)
      if (!request || typeof request.id !== 'number' || typeof request.method !== 'string') return
      const method = methods[request.method]
      if (!method) throw new Error(`unsupported account method: ${request.method}`)
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
  clearInterval(pollAccount)
  lifetime.abort()
  lines.close()
  for (const plugin of plugins.reverse()) await plugin.dispose()
  process.exit(0)
}
process.stdin.on('end', () => { void close() })
process.on('SIGTERM', () => { void close() })
process.on('SIGINT', () => { void close() })
send({ event: 'ready' })
