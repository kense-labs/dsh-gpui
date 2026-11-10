//! Keep standard ACP on stdio and route GPUI's session-only operations to the
//! same Cordis services used by the official Harness.
import { once } from 'node:events';
import { randomUUID } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { createInterface } from 'node:readline';
import { PassThrough } from 'node:stream';
import { pathToFileURL } from 'node:url';

const [service, profileEntry, ...patchFiles] = process.argv.slice(2);
const require = createRequire(join(service, 'apps/cli/package.json'));
const { loadLayeredEnv, StartupError, getDshRuntimeVersion } =
  await import(pathToFileURL(require.resolve('@deepseek-ai/dsh-app-boot')).href);
const { runProfile } = await import(pathToFileURL(profileEntry).href);
const input = createInterface({ input: process.stdin, crlfDelay: Infinity });
const acpInput = new PassThrough();

// The official ACP transport still consumes the normal process stdin stream.
// GPUI's reserved frames are removed before they reach the SDK's ACP parser.
Object.defineProperty(process, 'stdin', { value: acpInput, configurable: true });

const ready = runProfile({
  environment: loadLayeredEnv('dsh'),
  profile: 'acp',
  patchFiles,
  args: [],
}).then(runtime => {
  observeSessionUpdates(runtime.ctx);
  observeModelProviders(runtime.ctx);
  return runtime;
});

const recordMethod = '_dsh/gpui/session/feedback';
const flushMethod = '_dsh/gpui/session/flush';
const forkMethod = '_dsh/gpui/session/fork';
const renameMethod = '_dsh/gpui/session/rename';
const planModeMethod = '_dsh/gpui/session/plan-mode';
const planReviewMethod = '_dsh/gpui/session/plan-review';
const permissionModeMethod = '_dsh/gpui/session/permission-mode';
const runtimeSettingsMethod = '_dsh/gpui/runtime-settings';
const modelProviderMethods = new Set([
  '_dsh/gpui/providers/list', '_dsh/gpui/providers/save',
  '_dsh/gpui/providers/remove', '_dsh/gpui/providers/discover',
]);
const questionStateMethod = '_dsh/gpui/session/questions';
const questionDecisionMethod = '_dsh/gpui/session/question-decision';
const planReviews = new Map();
const questionRequests = new Map();
const messageMethods = new Map([
  ['_dsh/gpui/message-feedback/list', 'list'],
  ['_dsh/gpui/message-feedback/put', 'put'],
  ['_dsh/gpui/message-feedback/delete', 'delete'],
]);
const extensionMethods = [
  recordMethod, flushMethod, forkMethod, renameMethod, planModeMethod, planReviewMethod,
  permissionModeMethod, runtimeSettingsMethod,
  questionStateMethod, questionDecisionMethod,
  ...modelProviderMethods, ...messageMethods.keys(),
];

let loadingMessageFeedback;
let startingQuestions;
let sessionMutationTail = Promise.resolve();
function messageFeedback(ctx) {
  const installed = ctx.get('messageFeedback');
  if (installed) return installed;
  return loadingMessageFeedback ??= (async () => {
    const local = join(service, 'packages/feedback/message-feedback',
      profileEntry.endsWith('.ts') ? 'src/index.ts' : 'lib/index.js');
    const entry = existsSync(local) ? local : require.resolve('@deepseek-ai/dsh-message-feedback');
    const { default: MessageFeedback } = await import(pathToFileURL(entry).href);
    await ctx.plugin(MessageFeedback, { maxNoteBytes: 8192 });
    return ctx.get('messageFeedback');
  })();
}

class SessionOperationError extends Error {
  constructor(code, message = code) {
    super(message);
    this.code = code;
  }
}

function runtimeSettings(ctx, { page, change } = {}) {
  const sections = {
    shell: { namespaces: ['pwsh-sandbox', 'bash-sandbox'], fields: ['timeoutMs', 'maxOutputBytes'] },
    agentLoop: { namespaces: ['agent-loop'], fields: ['maxParallelToolCalls'] },
  };
  if (!Object.hasOwn(sections, page)) {
    throw new SessionOperationError('settings/page', '找不到这个设置页面。');
  }
  const { namespaces, fields } = sections[page];
  const settings = ctx.get('settings');
  const read = () => {
    const entries = settings?.describe({ redactSecrets: true }) ?? [];
    const entry = namespaces.map(ns => entries.find(row => row.ns === ns)).find(Boolean);
    if (!entry || !fields.every(field => typeof entry.value[field] === 'number')) return null;
    return {
      namespace: entry.ns, revision: entry.revision, writable: settings.writable,
      fields: Object.fromEntries(fields.map(field => [field, {
        value: entry.value[field],
        base: entry.base[field],
        overridden: Object.hasOwn(entry.user, field),
      }])),
    };
  };
  if (change == null) return runtimeSettingsTail.then(read);

  const operation = runtimeSettingsTail.then(async () => {
    const current = read();
    if (!current?.writable) {
      throw new SessionOperationError('settings/read-only', '当前部署无法保存这项设置。');
    }
    if (change.namespace !== current.namespace || change.revision !== current.revision) {
      throw new SessionOperationError('settings/conflict', '设置已更新，请重新打开页面后再试。');
    }
    if (!change.fields || typeof change.fields !== 'object' || Array.isArray(change.fields)) {
      throw new SessionOperationError('settings/fields', '设置内容无效。');
    }
    const ops = Object.entries(change.fields).map(([field, edit]) => {
      if (!fields.includes(field)) {
        throw new SessionOperationError('settings/value', '该字段无法在此页面编辑。');
      }
      if (edit?.op === 'unset') return { op: 'unset', path: [field] };
      const value = edit?.value;
      if (edit?.op !== 'set' || !Number.isFinite(value) || value <= 0 ||
          (page === 'agentLoop' && !Number.isSafeInteger(value))) {
        throw new SessionOperationError('settings/value', '请输入有效的正数，或留空以恢复默认值。');
      }
      return { op: 'set', path: [field], value };
    });
    if (ops.length) {
      try { await settings.mutate(current.namespace, ops, current.revision); }
      catch { throw new SessionOperationError('settings/save', '保存失败。运行时未接受这些值，已保留供你修改。'); }
    }
    return read();
  });
  runtimeSettingsTail = operation.then(() => undefined, () => undefined);
  return operation;
}

let runtimeSettingsTail = Promise.resolve();

async function checkpointTitle(ctx, session) {
  await ctx.get('sessionProjectionCache')?.write(session).catch(error => {
    ctx.logger.warn('Could not checkpoint the session title: %s', String(error));
  });
}

async function renameSession(ctx, { sessionId, title }) {
  if (typeof title !== 'string') {
    throw new SessionOperationError('session/title-invalid', 'session title must contain visible characters');
  }
  const sessions = ctx.get('sessions');
  const titles = ctx.get('sessionTitle');
  let session = sessions.get(sessionId);
  let handle;
  if (!session) {
    const stored = await ctx.get('sessionPersistence').stat(sessionId);
    session = sessions.get(sessionId);
    if (!session) {
      if (stored?.header.cwd === undefined) throw new SessionOperationError('session/not-found');
      if (stored.header.origin === 'subagent') throw new SessionOperationError('session/agent-busy');
      try {
        const { provider, model } = ctx.get('agentDefaultModel').currentSelection();
        handle = await ctx.get('agents').resume({
          resumeSessionId: sessionId,
          agentOptions: { provider, model },
        });
        session = handle.agent.session;
      } catch (error) {
        session = sessions.get(sessionId);
        if (!session) {
          if (error?.name === 'SessionAlreadyOwnedError') {
            throw new SessionOperationError('session/writer-held', error.message);
          }
          throw error;
        }
      }
    }
  }
  try {
    if (session.header.origin === 'subagent') throw new SessionOperationError('session/agent-busy');
    const accepted = titles.rename(session, title);
    await sessions.flush(session);
    await checkpointTitle(ctx, session);
    return { title: accepted.title, seq: accepted.eventSeq };
  } catch (error) {
    if (error?.name === 'SessionTitleInvalidError') {
      throw new SessionOperationError('session/title-invalid', error.message);
    }
    throw error;
  } finally {
    await handle?.dispose();
  }
}

function latestCompletedBoundary(events) {
  const end = events.findLast(event => event.type === 'turn/end');
  if (!end) return;
  let boundary = end.seq;
  for (const next of events.slice(boundary + 1)) {
    if (next.type === 'turn/start' || next.type === 'agent/inbox/spliced'
        || (next.type === 'user/message' && next.surfaceOp === 'append')) break;
    boundary = next.seq;
  }
  return boundary;
}

function listedTitle(ctx, header) {
  const live = ctx.get('sessions').get(header.id);
  if (live) return ctx.get('sessionTitle')?.get(live)?.title;
  const cache = ctx.get('sessionProjectionCache');
  return cache?.cachedSnapshot(header, ['title'])?.values.title
    ?? cache?.cachedPredecessorTitle(header)?.values.title;
}

function nextTitle(title) {
  const ascii = /^(.*?)\((\d+)\)$/u.exec(title);
  if (ascii) return `${ascii[1]}(${BigInt(ascii[2]) + 1n})`;
  const full = /^(.*?)（(\d+)）$/u.exec(title);
  if (full) return `${full[1]}（${BigInt(full[2]) + 1n}）`;
  return `${title} (1)`;
}

async function forkSession(ctx, { sessionId, atSeq, allowMigration }) {
  if (atSeq !== undefined && (!Number.isSafeInteger(atSeq) || atSeq < 0)) {
    throw new SessionOperationError('session/fork-unavailable');
  }
  if (allowMigration === false && !ctx.get('sessions').get(sessionId)) {
    const stored = await ctx.get('sessionPersistence').stat(sessionId);
    if (stored?.formatStatus === 'migration-required' && !ctx.get('sessions').get(sessionId)) {
      throw new SessionOperationError('session/migration-required');
    }
  }
  const query = ctx.get('sessionQuery');
  let source;
  try {
    source = await query.observeSession(sessionId, { projectionMode: 'none' });
  } catch (error) {
    if (error?.code === 'SESSION_QUERY_SESSION_NOT_FOUND') throw new SessionOperationError('session/not-found');
    throw error;
  }
  try {
    const { header, events } = source;
    const boundary = atSeq ?? latestCompletedBoundary(events);
    if (boundary === undefined || events[boundary]?.seq !== boundary) {
      throw new SessionOperationError('session/fork-unavailable');
    }
    const local = join(service, 'packages/core/session', profileEntry.endsWith('.ts') ? 'src/fork.ts' : 'lib/fork.js');
    const entry = existsSync(local) ? local : require.resolve('@deepseek-ai/dsh-session/fork');
    const { buildForkSeed } = await import(pathToFileURL(entry).href);
    const sourceTitle = listedTitle(ctx, header);
    let title;
    if (sourceTitle !== undefined && sourceTitle !== null) {
      const records = await query.listSessions();
      const taken = new Set(records.map(record => listedTitle(ctx, record.header)));
      title = nextTitle(sourceTitle);
      while (taken.has(title)) title = nextTitle(title);
    }
    const id = `session-${randomUUID()}`;
    const { provider, model } = ctx.get('agentDefaultModel').currentSelection();
    const handle = await ctx.get('agents').create({
      sessionId: id,
      seed: buildForkSeed(events, boundary),
      inheritedEventCount: boundary + 1,
      meta: {
        isSeeded: true,
        ...(header.cwd === undefined ? {} : { cwd: header.cwd }),
        ...(header.agentPreset === undefined ? {} : { agentPreset: header.agentPreset }),
      },
      agentOptions: { provider, model },
    });
    try {
      const child = handle.agent.session;
      if (title !== undefined) {
        title = ctx.get('sessionTitle').rename(child, title).title;
        // The sidebar can fork again immediately after this child goes cold.
        await checkpointTitle(ctx, child);
      }
      return { sessionId: id, cwd: child.header.cwd, createdAt: child.header.createdAt, title };
    } finally {
      // Persist the prefix and release the write lease before ACP resumes the child.
      await handle.dispose();
    }
  } finally {
    source[Symbol.dispose]();
  }
}

function notify(sessionId, update) {
  process.stdout.write(JSON.stringify({
    jsonrpc: '2.0', method: 'session/update', params: { sessionId, update },
  }) + '\n');
}

const providerSettings = 'llm-pi-ai';
const managedProvider = /^kwork-[a-f0-9-]{36}$/;
const managedCredential = /^KWORK_[A-F0-9]{32}_API_KEY$/;
const compatibilityApis = new Set(['openai-completions', 'openai-responses']);
let providerMutationTail = Promise.resolve();

function providerError(message) {
  return new SessionOperationError('providers/invalid', message);
}

function providerConfiguration(ctx) {
  const config = ctx.get('settings')?.describe().find(form => form.ns === providerSettings);
  if (!config || !ctx.get('credentials')) throw providerError('当前 dsh-acp 不支持编辑模型服务。');
  return { profiles: config.value.providers ?? {}, revision: config.revision };
}

async function publicProviders(ctx, profiles) {
  return Promise.all(Object.entries(profiles).filter(([id]) => managedProvider.test(id))
    .map(async ([id, profile]) => {
      const keyConfigured = typeof profile.apiKeyEnv === 'string' &&
        (await ctx.credentials.describe(profile.apiKeyEnv)).configured;
      return {
        id, name: profile.displayName ?? id, baseUrl: profile.baseURL, protocol: profile.api,
        models: (profile.models ?? []).map(({ id, name, contextWindow, maxTokens, input }) => ({
          id, name, contextWindow, maxTokens, inputModalities: input,
        })),
        keyConfigured, available: !profile.apiKeyEnv || keyConfigured,
      };
    }));
}

// A configured compatible route is the startup choice only when the built-in
// DeepSeek route cannot authenticate. Explicit session choices stay in place.
async function chooseCompatibleDefault(ctx, agent, profiles, preferred, signal) {
  if (agent.model.current()?.provider !== 'deepseek-official' || agent.session.header.origin === 'subagent') return;
  const settings = ctx.get('settings');
  const credentials = ctx.get('credentials');
  if (!settings || !credentials) return;
  const forms = settings.describe();
  const providers = profiles ?? forms.find(form => form.ns === providerSettings)?.value?.providers ?? {};
  const ids = Object.keys(providers).filter(id => managedProvider.test(id) && providers[id].models?.length);
  if (!ids.length) return;
  const official = forms.find(form => form.ns === 'llm-deepseek')?.value;
  if (ctx.llm.listProviders().some(provider => provider.id === 'deepseek-official') &&
      (await credentials.describe(official?.apiKeyEnv ?? 'DEEPSEEK_API_KEY')).configured) return;
  const preferredIndex = ids.indexOf(preferred);
  if (preferredIndex > 0) ids.unshift(ids.splice(preferredIndex, 1)[0]);
  for (const provider of ids) {
    const profile = providers[provider];
    if (profile.apiKeyEnv && !(await credentials.describe(profile.apiKeyEnv)).configured) continue;
    for (const { id: model } of profile.models) {
      try { await ctx.llm.resolveModelInfo(provider, model, signal); }
      catch { signal?.throwIfAborted(); continue; }
      agent.model.select({ provider, model });
      return;
    }
  }
}

function readProviderDraft(params) {
  let url;
  try { url = new URL(params.baseUrl?.trim()); }
  catch { throw providerError('请输入有效的 HTTP 或 HTTPS API 地址。'); }
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) {
    throw providerError('API 地址不能包含用户名、密码、查询参数或片段。');
  }
  const baseUrl = url.href.replace(/\/+$/, '');
  if (/\/(?:chat\/completions|responses|models)$/.test(baseUrl)) {
    throw providerError('请填写 API 基础地址，例如 https://example.com/v1。');
  }
  const protocol = params.protocol;
  if (!compatibilityApis.has(protocol)) throw providerError('请选择 OpenAI 兼容协议。');
  const key = params.apiKey;
  if (key !== undefined && (typeof key !== 'string' || key.length > 8192 ||
      !/^[\x21-\x7e]+$/.test(key) || /^[A-Z0-9_]+=[^=]/.test(key) || /^(['"`]).*\1$/.test(key))) {
    throw providerError('请填写原始 API Key。');
  }
  return { baseUrl, protocol, key };
}

function sameProviderOrigin(current, baseUrl) {
  if (!current?.baseURL) return false;
  return new URL(current.baseURL).origin === new URL(baseUrl).origin;
}

function configuredModels(items, existing) {
  if (!Array.isArray(items) || items.length < 1 || items.length > 512) {
    throw providerError('请添加 1 到 512 个模型。');
  }
  const seen = new Set();
  return items.map(item => {
    const id = typeof item?.id === 'string' ? item.id.trim() : '';
    if (!id || id.length > 512 || /[\x00-\x20\x7f]/.test(id) || seen.has(id)) {
      throw providerError('模型 ID 必须唯一，且不能包含空格。');
    }
    seen.add(id);
    const previous = existing?.find(model => model.id === id);
    const result = { ...previous, id };
    if (typeof item.name === 'string' && item.name.trim()) result.name = item.name.trim().slice(0, 512);
    for (const field of ['contextWindow', 'maxTokens']) {
      if (Number.isSafeInteger(item[field]) && item[field] > 0) result[field] = item[field];
    }
    if (Array.isArray(item.inputModalities)) {
      result.input = item.inputModalities.filter(value => value === 'text' || value === 'image');
    }
    return result;
  });
}

function modelProviderOperation(ctx, method, params) {
  if (method === '_dsh/gpui/providers/list') {
    return publicProviders(ctx, providerConfiguration(ctx).profiles);
  }
  if (method === '_dsh/gpui/providers/discover') {
    const { profiles } = providerConfiguration(ctx);
    const { baseUrl, protocol, key } = readProviderDraft(params);
    const current = managedProvider.test(params.id) ? profiles[params.id] : undefined;
    const provider = params.clearKey || !sameProviderOrigin(current, baseUrl) ? undefined : params.id;
    return ctx.llm.discoverModels(providerSettings, {
      provider, api: protocol, baseURL: baseUrl, apiKey: key,
    }, AbortSignal.timeout(12000)).catch(() => {
      throw providerError('无法同步模型。请检查 API 地址和密钥，或手动添加模型 ID。');
    });
  }

  const operation = providerMutationTail.then(async () => {
    const { profiles, revision } = providerConfiguration(ctx);
    const removing = method === '_dsh/gpui/providers/remove';
    const id = params.id ?? (removing ? '' : 'kwork-' + randomUUID());
    if (!managedProvider.test(id) || ((removing || params.id) && !Object.hasOwn(profiles, id))) {
      throw providerError('找不到这个 KWork 服务商。');
    }
    const current = profiles[id];
    const oldRef = current?.apiKeyEnv;
    const path = ['providers', id];
    let newRef;
    try {
      if (removing) {
        await ctx.settings.mutate(providerSettings, [{ op: 'unset', path }], revision);
      } else {
        const { baseUrl, protocol, key } = readProviderDraft(params);
        const name = typeof params.name === 'string' ? params.name.trim() : '';
        if (!name || name.length > 80) throw providerError('请输入不超过 80 个字符的服务商名称。');
        const models = configuredModels(params.models, current?.models);
        let apiKeyEnv = params.clearKey || !sameProviderOrigin(current, baseUrl) ? undefined : oldRef;
        if (key !== undefined) {
          newRef = 'KWORK_' + randomUUID().replaceAll('-', '').toUpperCase() + '_API_KEY';
          await ctx.credentials.set(newRef, key);
          apiKeyEnv = newRef;
        } else if (oldRef && !params.clearKey && !sameProviderOrigin(current, baseUrl)) {
          throw providerError('更改服务地址后，请重新输入此地址的 API Key，或选择清除原密钥。');
        }
        const value = { ...current, displayName: name, api: protocol, baseURL: baseUrl, models };
        delete value.apiKeyEnv;
        if (apiKeyEnv) value.apiKeyEnv = apiKeyEnv;
        await ctx.settings.mutate(providerSettings, [{ op: 'set', path, value }], revision);
      }
    } catch (error) {
      if (newRef) await ctx.credentials.unset(newRef).catch(() => {});
      if (error instanceof SessionOperationError) throw error;
      throw providerError('保存模型服务失败。请重新打开设置后再试。');
    }
    const updated = providerConfiguration(ctx).profiles;
    if (managedCredential.test(oldRef) && updated[id]?.apiKeyEnv !== oldRef) {
      await ctx.credentials.unset(oldRef).catch(() => {});
    }
    if (!removing && typeof params.sessionId === 'string') {
      const agent = ctx.get('agents')?.get(params.sessionId);
      if (agent?.status === 'idle' && !agent.session.requestHeader()) {
        await chooseCompatibleDefault(ctx, agent, updated, id).catch(error => {
          ctx.logger.warn('KWork could not select the compatible model: %s', String(error));
        });
      }
    }
    // Updating an existing profile's model list may leave the adapter route set
    // unchanged. ACP sessions still need a fresh model picker in this case.
    try { ctx.emit('llm/adapters-updated'); }
    catch { ctx.logger.warn('KWork could not refresh the model picker'); }
    return { id, providers: await publicProviders(ctx, updated) };
  });
  providerMutationTail = operation.then(() => undefined, () => undefined);
  return operation;
}

function observeModelProviders(ctx) {
  ctx.on('llm-pi-ai/request-headers', (request, headers, next) => {
    if (managedProvider.test(request.provider) && !headers.has('authorization')) {
      const profile = providerConfiguration(ctx).profiles[request.provider];
      // The OpenAI SDK requires an Authorization value even for a local
      // gateway without a key. This placeholder carries no credential.
      if (profile && !profile.apiKeyEnv) headers.set('authorization', 'Bearer kwork-no-key');
    }
    return next();
  }, { global: true });
  ctx.on('agent/created', async ({ agent, source, signal }) => {
    if (source === 'startup') {
      await chooseCompatibleDefault(ctx, agent, undefined, undefined, signal).catch(error => {
        signal?.throwIfAborted();
        ctx.logger.warn('KWork could not resolve a compatible startup model: %s', String(error));
      });
    }
    const selection = agent.model.current();
    if (!selection || !managedProvider.test(selection.provider)) return;
    try { await ctx.llm.resolveModelInfo(selection.provider, selection.model); }
    catch {
      const fallback = ctx.get('agentDefaultModel')?.currentSelection();
      if (fallback && !managedProvider.test(fallback.provider)) agent.model.select(fallback);
    }
  }, { global: true });
}

function questionError(code, message) {
  return Object.assign(new Error(message), { name: 'UserQuestionError', code });
}

function startQuestions(ctx) {
  return startingQuestions ??= (async () => {
    observeQuestions(ctx);
    await mountQuestionTool(ctx).catch(error => {
      ctx.logger.warn('GPUI could not mount ask_user_question: %s', String(error));
    });
  })();
}

async function mountQuestionTool(ctx) {
  if (!ctx.get('userQuestions') || ctx.get('tools')?.get('ask_user_question') || typeof bundledAskUser !== 'string') return;
  const base = createRequire(require.resolve('@deepseek-ai/dsh-base/package.json'));
  const tools = base.resolve('@deepseek-ai/dsh-tools');
  const schemaPackage = createRequire(tools).resolve('@deepseek-ai/schemastery/package.json');
  const schemaEntry = JSON.parse(readFileSync(schemaPackage, 'utf8')).exports['.'].import;
  const dependencies = {
    '@deepseek-ai/dsh-tools': tools,
    '@deepseek-ai/dsh-user-questions': base.resolve('@deepseek-ai/dsh-user-questions'),
    '@deepseek-ai/schemastery': join(dirname(schemaPackage), schemaEntry),
  };
  // The vendor module is the installed Harness build. Bind it to this
  // profile's service instances instead of importing a second Cordis runtime.
  const source = bundledAskUser.replace(/from "(@deepseek-ai\/[^"]+)"/g,
    (_match, name) => 'from ' + JSON.stringify(pathToFileURL(dependencies[name]).href));
  const plugin = await import('data:text/javascript;base64,' + Buffer.from(source).toString('base64'));
  await ctx.plugin(plugin, {});
}

function questionProjection(ctx, agent) {
  if (!agent || agent.session.header.origin === 'subagent') return { rows: [], settled: [], queued: [] };
  const { userQuestions } = ctx.get('sessionProjections')?.snapshot(agent.session, ['userQuestions']).values ?? {};
  const replies = new Map([...agent.inbox.nextTurn, ...agent.inbox.nextStep]
    .filter(message => message.source.kind === 'user-question-reply').map(message => [message.source.callId, message]));
  const queued = [];
  for (const [callId, message] of replies) {
    try {
      const payload = JSON.parse(message.content.find(block => block.type === 'text')?.text ?? '');
      if (payload?.kind === 'answer_to_pending_question' && payload.callId === callId && Array.isArray(payload.answers)) {
        queued.push({ callId, answers: payload.answers });
      }
    } catch { /* Leave malformed replies out of the review surface. */ }
  }
  return {
    rows: (userQuestions?.active ?? []).filter(row => row.state === 'continued' && !replies.has(row.callId)),
    settled: userQuestions?.settled ?? [],
    queued,
  };
}

const previousContinuedQuestions = new WeakMap();
function publishContinuedQuestions(ctx, agent, force = false) {
  const view = questionProjection(ctx, agent);
  if (agent) {
    const fingerprint = JSON.stringify(view);
    const previous = previousContinuedQuestions.get(agent);
    if (force || (previous !== undefined && fingerprint !== previous)
        || (previous === undefined && (view.rows.length || view.settled.length || view.queued.length))) {
      previousContinuedQuestions.set(agent, fingerprint);
      notify(agent.id, { sessionUpdate: 'gpui_questions', phase: 'continued', ...view });
    }
  }
  return view.rows;
}

function observeQuestions(ctx) {
  const callsBySession = new WeakMap();
  ctx.on('user-questions/request', async ({ agent, questions, signal, wait }, next) => {
    if (!agent || questions.some(question => question.intent !== undefined)) return next();
    const lifetime = new AbortController();
    const stream = wait?.timed && wait.callId
      ? ctx.get('userQuestions').attachWait(agent, wait.callId, lifetime.signal) : undefined;
    const frame = await stream?.next();
    if (frame?.done || signal?.aborted) {
      lifetime.abort();
      await stream?.return();
      return next();
    }
    const requestId = wait?.callId ?? randomUUID();
    const key = JSON.stringify([agent.id, requestId]);
    const result = Promise.withResolvers();
    const channel = { result, questions, timed: frame !== undefined, timedOut: false };
    const abort = () => {
      channel.timedOut = signal?.reason?.code === 'ASK_TIMED_OUT';
      result.reject(questionError('ASK_ABORTED', 'The question request was aborted'));
    };
    signal?.addEventListener('abort', abort, { once: true });
    questionRequests.set(key, channel);
    const update = { sessionUpdate: 'gpui_questions', requestId };
    notify(agent.id, { ...update, phase: 'open', questions, callId: wait?.callId,
      remainingMs: frame?.value.remainingMs });
    try {
      if (signal?.aborted) abort();
      return await result.promise;
    } finally {
      signal?.removeEventListener('abort', abort);
      lifetime.abort();
      await stream?.return();
      questionRequests.delete(key);
      notify(agent.id, { ...update, phase: 'closed', willContinue: channel.timedOut });
    }
  }, { global: true });

  ctx.on('session/event', (session, event) => {
    const { type, data, surfaceOp } = event;
    if (type === 'tool/call' && data.name === 'ask_user_question') {
      let calls = callsBySession.get(session);
      if (!calls) callsBySession.set(session, calls = new Map());
      calls.set(data.callId, data.arguments);
    } else if (type === 'tool/result' && surfaceOp === 'append') {
      const calls = callsBySession.get(session);
      const id = data.message.source?.callId ?? data.message.toolCallId;
      if (calls?.has(id)) {
        const questionArguments = calls.get(id);
        calls.delete(id);
        notify(session.id, { sessionUpdate: 'gpui_question_record', event: { ...event, questionArguments } });
      }
    } else if (type === 'user/message' && surfaceOp === 'append' && data.source?.kind === 'user-question-reply') {
      notify(session.id, { sessionUpdate: 'gpui_question_record', event });
    }
    if (event.type === 'tool/result' || event.type === 'tool/ptc-dispatch' || event.type === 'turn/end' ||
        (event.type === 'user/message' && event.data.source?.kind === 'user-question-reply')) {
      publishContinuedQuestions(ctx, ctx.get('agents').get(session.id));
    }
  }, { global: true });
  for (const name of ['agent/inbox/inserted', 'agent/inbox/discarded']) {
    ctx.on(name, ({ agent, message }) => {
      if (message.source.kind === 'user-question-reply') publishContinuedQuestions(ctx, agent);
    }, { global: true });
  }
}

function validateAnswers(questions, answers) {
  const ids = new Set();
  if (!Array.isArray(answers) || answers.length !== questions.length || answers.some(answer => {
    if (!answer || ids.has(answer.id) || !Array.isArray(answer.selected)) return true;
    ids.add(answer.id);
    const question = questions.find(question => question.id === answer.id);
    if (!question || new Set(answer.selected).size !== answer.selected.length ||
        answer.selected.some(label => !question.options?.some(option => option.label === label))) return true;
    if (answer.custom !== undefined && (typeof answer.custom !== 'string' || !answer.custom.trim())) return true;
    return !question.multiSelect && answer.selected.length + Number(answer.custom !== undefined) > 1;
  })) throw new SessionOperationError('question/invalid-answer', '答案与待回答的问题不匹配。');
  return { answers };
}

function respondQuestion(ctx, { sessionId, requestId, decision, answers }) {
  const key = JSON.stringify([sessionId, requestId]);
  const channel = questionRequests.get(key);
  if (channel) {
    if (decision === 'answer') {
      const batch = validateAnswers(channel.questions, answers);
      questionRequests.delete(key);
      channel.result.resolve(batch);
    } else if (decision === 'cancel' && !channel.timed) {
      questionRequests.delete(key);
      channel.result.reject(questionError('ASK_CANCELLED', 'The user dismissed the questions'));
    } else if (decision === 'timeout' && channel.timed) {
      questionRequests.delete(key);
      channel.timedOut = true;
      channel.result.reject(questionError('ASK_TIMED_OUT', 'The question wait expired'));
    } else {
      throw new SessionOperationError('question/invalid-decision');
    }
    return {};
  }
  if (decision === 'answer') {
    const agent = ctx.get('agents').get(sessionId);
    const row = questionProjection(ctx, agent).rows.find(row => row.callId === requestId);
    if (row) {
      const batch = validateAnswers(row.questions, answers);
      try {
        if (ctx.get('userQuestions').answer(agent, requestId, batch)) return {};
      } catch (error) {
        if (error?.name === 'UserQuestionError' && error.code === 'REPLY_QUEUED') {
          throw new SessionOperationError('question/reply-queued', '这些问题已提交。');
        }
        throw error;
      }
    }
  }
  throw new SessionOperationError('question/closed', '这些问题已结束或已提交。');
}

function observePlanReviews(ctx) {
  ctx.on('user-questions/request', async ({ agent, questions, signal, wait }, next) => {
    const [question] = questions;
    if (!agent || wait || question?.intent?.kind !== 'plan-review') return next();
    const requestId = randomUUID();
    const sessionId = agent.id;
    const review = Promise.withResolvers();
    const abort = () => review.reject(questionError('ASK_ABORTED', 'Plan review was aborted'));
    signal?.addEventListener('abort', abort, { once: true });
    const { approve, callId } = question.intent;
    const channel = { sessionId, review, questionId: question.id, approve };
    const update = { sessionUpdate: 'gpui_plan_review', requestId };
    planReviews.set(requestId, channel);
    notify(sessionId, {
      ...update, phase: 'open', callId, question: question.question, markdown: question.detail,
      approvalDescription: question.options.find(option => option.label === approve)?.description,
    });
    try {
      if (signal?.aborted) abort();
      return await review.promise;
    } finally {
      signal?.removeEventListener('abort', abort);
      planReviews.delete(requestId);
      notify(sessionId, { ...update, phase: 'closed' });
    }
  }, { global: true });
}

function respondPlanReview({ sessionId, requestId, decision }) {
  const channel = planReviews.get(requestId);
  if (channel?.sessionId !== sessionId) {
    throw new SessionOperationError('plan/review-closed', '该计划已结束审阅。');
  }
  if (decision !== 'approve' && decision !== 'discuss') {
    throw new SessionOperationError('plan/invalid-decision');
  }
  planReviews.delete(requestId);
  const { review, questionId, approve } = channel;
  if (decision === 'approve') {
    review.resolve({ answers: [{ id: questionId, selected: [approve] }] });
  } else {
    review.reject(questionError('ASK_CANCELLED', 'The user dismissed the plan review'));
  }
  return {};
}

function planMode(ctx, { sessionId, active }) {
  const agent = ctx.get('agents').get(sessionId);
  const mode = ctx.get('planMode');
  if (!agent || !mode) throw new SessionOperationError('plan/unavailable', '当前会话不支持计划模式。');
  if (typeof active === 'boolean') mode.set(agent, active);
  const state = mode.get(agent);
  return { active: state.pending ?? state.active };
}

async function permissionMode(ctx, { sessionId, preset, acknowledged }) {
  const agent = ctx.get('agents').get(sessionId);
  const permissions = ctx.get('permissionPresets');
  if (!agent || agent.session.header.origin === 'subagent' || !permissions) {
    throw new SessionOperationError('permission/unavailable', '当前会话不支持切换权限。');
  }
  const { options } = permissions.catalog();
  if (preset != null) {
    if (!options.some(option => option.value === preset)) {
      throw new SessionOperationError('permission/invalid-preset', '该权限模式已不可用。');
    }
    if (preset !== permissions.current(agent.session)) {
      if ((preset === 'danger-full-access' || preset === 'auto') && acknowledged !== true) {
        throw new SessionOperationError('permission/confirmation-required', '请先确认权限变更。');
      }
      const execution = await ctx.get('commands')?.execute(
        agent, `/permission ${preset}`, [], new AbortController().signal,
      );
      if (execution?.result.kind !== 'success') {
        throw new SessionOperationError('permission/switch-failed',
          execution?.result.text ?? '当前会话无法切换权限。');
      }
    }
  }
  const currentValue = permissions.current(agent.session);
  return { currentValue, options };
}

function submittedPlanEvents(session, end) {
  const plans = [];
  for (let seq = end - 1; seq >= 0; --seq) {
    const event = session.eventAt(seq);
    if (!event || event.type === 'turn/start' || event.type === 'turn/end') break;
    if (event.data?.name === 'exit_plan_mode' &&
        (event.type === 'tool/call' || event.type === 'tool/ptc-dispatch-start' ||
         event.type === 'tool/ptc-dispatch')) {
      plans.push(event);
    }
  }
  return plans.reverse();
}

function observeSessionUpdates(ctx) {
  observePlanReviews(ctx);
  const notifyAgentStatus = (agent, running) => {
    const { origin, parentSession } = agent.session.header;
    notify(agent.id, {
      sessionUpdate: 'gpui_agent_status', running,
      ...(origin === 'subagent' && parentSession ? { parentSessionId: parentSession } : {}),
    });
  };
  ctx.on('agent/status', ({ agent, status }) => notifyAgentStatus(agent, status === 'running'));
  ctx.on('agent/disposed', ({ agent }) => notifyAgentStatus(agent, false));
  ctx.on('session/event', (session, event) => {
    if (event.type === 'tool/ptc-dispatch-start' || event.type === 'tool/ptc-dispatch') {
      notify(session.id, { sessionUpdate: 'gpui_ptc_dispatch', event });
    }
  }, { global: true });
  ctx.on('session/event', (session, event) => {
    const { type, data, surfaceOp } = event;
    if (type === 'plan/mode') {
      notify(session.id, { sessionUpdate: 'gpui_plan_mode', active: data.active });
      return;
    }
    if (type === 'user/message') {
      const source = data.source?.kind;
      if (source === 'user') {
        notify(session.id, { sessionUpdate: 'gpui_session_activity', time: event.time });
      }
      if (['user', 'goal', 'schedule', 'webhook', 'github', 'agent', 'agent-message'].includes(source) ||
          (source === 'user-question-reply' && data.source.outcome === 'answered')) {
        notify(session.id, { sessionUpdate: 'gpui_turn_tail', type, seq: event.seq });
      }
      return;
    }
    const relevant = ['session/title', 'turn/start', 'step/start', 'turn/end', 'llm/retry', 'tool/call'].includes(type)
      || ((type === 'assistant/message' || type === 'tool/result') && surfaceOp === 'append');
    if (!relevant) return;
    const update = type === 'session/title' ? {
      sessionUpdate: 'gpui_session_title', seq: event.seq, title: data.title,
    } : {
      sessionUpdate: 'gpui_turn_tail', type, seq: event.seq, turn: data.turn, step: data.step, time: event.time,
    };
    if (type === 'tool/call') {
      update.toolCall = {
        sessionUpdate: 'tool_call', toolCallId: data.callId,
        title: data.name, rawInput: data.arguments, status: 'in_progress', time: event.time,
      };
    }
    if (type === 'tool/result') update.callId = data.message.source?.callId ?? data.message.toolCallId;
    if (type === 'llm/retry') update.retryId = data.retryId;
    if (type === 'turn/end') {
      update.reason = data.reason?.kind;
      const plans = submittedPlanEvents(session, event.seq);
      if (plans.length > 0) update.planEvents = plans;
    }
    if (type === 'assistant/message') {
      const { id, content } = data.message;
      update.assistantId = id;
      update.hasToolCalls = content.some(block => block.type === 'tool-call');
      update.hasReply = content.some(block => block.type === 'text'
        ? block.text.trim() !== '' : block.type !== 'tool-call' && block.type !== 'reasoning');
      update.hasVisibleContent = update.hasReply || content.some(block =>
        block.type === 'reasoning' && block.text.trim() !== '');
      if (content.some(block => block.type === 'text' && block.text.trim() !== '')) update.messageId = id;
    }
    notify(session.id, update);
  });
}

async function extension({ id, method, params }) {
  let response;
  try {
    const { ctx } = await ready;
    if (method === runtimeSettingsMethod) {
      response = { result: await runtimeSettings(ctx, params) };
    } else if (modelProviderMethods.has(method)) {
      response = { result: await modelProviderOperation(ctx, method, params ?? {}) };
    } else if (method === questionStateMethod) {
      await startQuestions(ctx);
      const sessionId = params?.sessionId;
      const rows = typeof sessionId === 'string'
        ? publishContinuedQuestions(ctx, ctx.get('agents').get(sessionId), true) : [];
      response = { result: rows };
    } else if (typeof params?.sessionId !== 'string' || params.sessionId.length === 0) {
      response = { error: { code: -32602, message: 'Invalid session id' } };
    } else if (method === recordMethod) {
      const feedback = ctx.get('sessionFeedback');
      response = feedback
        ? { result: await feedback.record(params) }
        : { error: { code: -32601, message: 'Session feedback is unavailable' } };
    } else if (method === flushMethod) {
      const sessions = ctx.get('sessions');
      const session = sessions?.get(params.sessionId);
      if (session) await sessions.flush(session);
      response = { result: { live: session !== undefined } };
    } else if (method === planReviewMethod) {
      response = { result: respondPlanReview(params) };
    } else if (method === planModeMethod) {
      response = { result: planMode(ctx, params) };
    } else if (method === permissionModeMethod) {
      response = { result: await permissionMode(ctx, params) };
    } else if (method === questionDecisionMethod) {
      response = { result: respondQuestion(ctx, params) };
    } else if (method === forkMethod || method === renameMethod) {
      const mutate = method === forkMethod ? forkSession : renameSession;
      const task = sessionMutationTail.then(() => mutate(ctx, params));
      sessionMutationTail = task.then(() => undefined, () => undefined);
      response = { result: await task };
    } else {
      const feedback = await messageFeedback(ctx);
      response = feedback
        ? { result: await feedback[messageMethods.get(method)](params) }
        : { error: { code: -32601, message: 'Message feedback is unavailable' } };
    }
  } catch (error) {
    response = { error: error instanceof SessionOperationError
      ? { code: -32602, message: error.message, ...(error.message === error.code ? {} : { data: error.code }) }
      : { code: -32603, message: 'Session operation failed' } };
  }
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id, ...response }) + '\n');
}

async function forward() {
  try {
    for await (const line of input) {
      if (extensionMethods.some(method => line.includes(method))) {
        let request;
        try { request = JSON.parse(line); } catch { /* ACP owns ordinary protocol errors. */ }
        if (request?.id != null && extensionMethods.includes(request.method)) {
          void extension(request);
          continue;
        }
      }
      if (!acpInput.write(line + '\n')) await once(acpInput, 'drain');
    }
  } finally {
    acpInput.end();
  }
}

try {
  await Promise.all([ready, forward()]);
} catch (error) {
  if (error instanceof StartupError) {
    const extension = profileEntry.endsWith('.ts') ? 'ts' : 'js';
    const { reportStartupFailure } =
      await import(pathToFileURL(join(dirname(profileEntry), 'startup-diagnostics.' + extension)).href);
    const { resolveDshHome } = await import(pathToFileURL(require.resolve('@deepseek-ai/dsh-home-paths')).href);
    await reportStartupFailure(error, { home: resolveDshHome(), version: getDshRuntimeVersion(), profile: 'acp' });
  } else {
    console.error('dsh-gpui ACP bridge:', error);
  }
  process.exit(1);
}
