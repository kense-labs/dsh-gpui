//! Keep standard ACP on stdio and route GPUI's session-only operations to the
//! same Cordis services used by the official Harness.
import { once } from 'node:events';
import { randomUUID } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { createInterface } from 'node:readline';
import { PassThrough } from 'node:stream';
import { pathToFileURL } from 'node:url';
import { stripVTControlCharacters } from 'node:util';

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
}).then(async runtime => {
  observeSessionUpdates(runtime.ctx);
  observeModelProviders(runtime.ctx);
  observePluginInstall(runtime.ctx);
  await startSchedule(runtime.ctx);
  return runtime;
});

const recordMethod = '_dsh/gpui/session/feedback';
const flushMethod = '_dsh/gpui/session/flush';
const forkMethod = '_dsh/gpui/session/fork';
const renameMethod = '_dsh/gpui/session/rename';
const planModeMethod = '_dsh/gpui/session/plan-mode';
const planReviewMethod = '_dsh/gpui/session/plan-review';
const permissionModeMethod = '_dsh/gpui/session/permission-mode';
const sessionCommandsMethod = '_dsh/gpui/session/commands';
const executeSessionCommandMethod = '_dsh/gpui/session/command-execute';
const runtimeSettingsMethod = '_dsh/gpui/runtime-settings';
const webSettingsMethod = '_dsh/gpui/web-settings';
const pluginCenterMethod = '_dsh/gpui/plugin-center';
const pluginInstallMethod = '_dsh/gpui/plugin-install';
const scheduleMethod = '_dsh/gpui/schedule';
const openHostSessionMethod = '_dsh/gpui/session/open';
const sessionArchiveMethod = '_dsh/gpui/session/archive';
const modelProviderMethods = new Set([
  '_dsh/gpui/providers/list', '_dsh/gpui/providers/save',
  '_dsh/gpui/providers/remove', '_dsh/gpui/providers/discover',
]);
const questionStateMethod = '_dsh/gpui/session/questions';
const questionDecisionMethod = '_dsh/gpui/session/question-decision';
// The ACP prompt slot is single-flight, so a message composed while the agent
// is busy cannot ride `session/prompt`. It becomes a durable Inbox item instead
// (official ComposerSubmissionPolicy): `queue` appends a turn, `steer` joins
// the running turn at its next step boundary.
const queuePromptMethod = '_dsh/gpui/session/queue-prompt';
const inboxStateMethod = '_dsh/gpui/session/inbox-state';
const inboxActionMethod = '_dsh/gpui/session/inbox-action';
// The composer's GoalBar reads the same live goal view the official client
// projects, and its pause/resume/clear verbs are the goal service's own.
const goalStateMethod = '_dsh/gpui/session/goal-state';
const goalActionMethod = '_dsh/gpui/session/goal-action';
// Non-human messages either wake a Turn (official TurnTriggerNodeView) or are
// injected into model history (official ContextInjectionRow). Only the waking
// producers get a trigger card; the rest are collapsed context rows, exactly
// like the official projection distinguishes them.
// Compaction summaries are logged just before the checkpoint message that
// replaces the shadowed history; cache them so the live marker can cite the
// official 已压缩 N 条历史记录 count like a reopened session does.
const compactionSummaries = new Map()
const TRIGGER_SOURCES = new Set([
  'goal', 'schedule', 'webhook', 'github', 'agent', 'agent-message', 'team',
  'team-message', 'subagent', 'subagent-settled', 'job', 'tool-jobs', 'tool-goal',
  'plugin',
]);
const planReviews = new Map();
const questionRequests = new Map();
const messageMethods = new Map([
  ['_dsh/gpui/message-feedback/list', 'list'],
  ['_dsh/gpui/message-feedback/put', 'put'],
  ['_dsh/gpui/message-feedback/delete', 'delete'],
]);
const extensionMethods = [
  recordMethod, flushMethod, forkMethod, renameMethod, planModeMethod, planReviewMethod,
  permissionModeMethod, sessionCommandsMethod, executeSessionCommandMethod,
  runtimeSettingsMethod, webSettingsMethod, pluginCenterMethod, pluginInstallMethod,
  questionStateMethod, questionDecisionMethod, scheduleMethod, openHostSessionMethod, sessionArchiveMethod,
  queuePromptMethod, inboxStateMethod, inboxActionMethod, goalStateMethod, goalActionMethod,
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

async function startSchedule(ctx) {
  ctx.on('schedule/changed', () => {
    process.stdout.write('{"jsonrpc":"2.0","method":"_dsh/gpui/schedule-changed"}\n');
  }, { global: true });
  const sessions = ctx.get('acpSessionHost');
  ctx.on('schedule/resolve-agent', (sessionId, next) => sessions
    ? sessions.activate(sessionId)
    : ctx.get('agents')?.get(sessionId) ?? next());
  if (sessions && !ctx.get('sessionController')) {
    ctx.provide('sessionController', {
      async create({ sessionId, cwd, agentPreset }) {
        if (agentPreset !== undefined) {
          // The companion dsh-acp ACP profile does not mount the
          // agent-preset-registry service and acpSessionHost.create ignores a
          // third argument. Report that precisely instead of implying a
          // client-side limitation; silently dropping the preset would create
          // the task with the wrong agent configuration.
          throw new SessionOperationError('schedule/preset',
            '配套 dsh-acp 未在 ACP profile 中启用 Agent 预设服务，无法按指定预设创建会话。');
        }
        const { session } = await sessions.create(sessionId, cwd ?? process.env.KWORK_WORKSPACE ?? process.cwd());
        process.stdout.write(JSON.stringify({
          jsonrpc: '2.0', method: '_dsh/gpui/schedule-session', params: {
            sessionId, cwd: session.header.cwd, createdAt: session.header.createdAt,
          },
        }) + '\n');
        return { sessionId };
      },
      async resolveAgent(sessionId) { return { agent: await sessions.activate(sessionId) }; },
    });
  }
  const runtime = process.env.KWORK_SCHEDULE_MODULE;
  if (!runtime || !ctx.get('storageDomain') || !ctx.get('sessionPersistence')) return;
  const existing = ctx.get('schedule');
  const mountTools = ctx.get('sessionController') && ctx.get('tools') && ctx.get('systemPrompt') &&
    !ctx.get('tools').get('schedule_create');
  if (existing && !mountTools && ctx.get('workspaceRegistry')) {
    await importArchivedSessions(ctx);
    return;
  }
  const base = createRequire(require.resolve('@deepseek-ai/dsh-base/package.json'));
  const manager = createRequire(require.resolve('@deepseek-ai/dsh-plugin-manager'));
  const persistence = createRequire(base.resolve('@deepseek-ai/dsh-session-persistence-jsonl'));
  const schemaPackage = manager.resolve('@deepseek-ai/schemastery/package.json');
  const schemaEntry = JSON.parse(readFileSync(schemaPackage, 'utf8')).exports['.'].import;
  const entries = {
    '@deepseek-ai/schemastery': join(dirname(schemaPackage), schemaEntry),
    '@deepseek-ai/cordis': require.resolve('@deepseek-ai/cordis'),
    '@deepseek-ai/dsh-llm': base.resolve('@deepseek-ai/dsh-llm'),
    '@deepseek-ai/dsh-llm/brand': base.resolve('@deepseek-ai/dsh-llm/brand'),
    '@deepseek-ai/dsh-typert-protocol': manager.resolve('@deepseek-ai/dsh-typert-protocol'),
    '@deepseek-ai/dsh-subagent': base.resolve('@deepseek-ai/dsh-subagent'),
    '@deepseek-ai/dsh-session': base.resolve('@deepseek-ai/dsh-session'),
    '@deepseek-ai/dsh-session-persistence': persistence.resolve('@deepseek-ai/dsh-session-persistence'),
    '@deepseek-ai/dsh-storage-domain': base.resolve('@deepseek-ai/dsh-storage-domain'),
    '@deepseek-ai/dsh-tools': base.resolve('@deepseek-ai/dsh-tools'),
    zod: manager.resolve('zod'),
  };
  const source = readFileSync(runtime, 'utf8').replace(/^import (.*) from "([^"]+)";$/gm,
    (statement, bindings, name) => Object.hasOwn(entries, name)
      ? `import ${bindings} from ${JSON.stringify(pathToFileURL(entries[name]).href)};`
      : statement);
  const { default: Schedule, scheduleTools, WorkspaceRegistry } = await import('data:text/javascript;base64,' + Buffer.from(source).toString('base64'));
  if (!ctx.get('workspaceRegistry') && WorkspaceRegistry) await ctx.plugin(WorkspaceRegistry, {});
  await importArchivedSessions(ctx);
  if (!existing) await ctx.plugin(Schedule, {});
  if (mountTools) {
    try {
      await ctx.plugin(scheduleTools, {});
    } catch (error) {
      ctx.logger.warn('GPUI could not mount Schedule tools: %s', String(error));
    }
  }
}

async function importArchivedSessions(ctx) {
  const path = process.env.KWORK_WORKSPACE_PREFERENCES;
  const registry = ctx.get('workspaceRegistry');
  if (!path || !registry) return;
  const { resolveDshHome } = await import(pathToFileURL(require.resolve('@deepseek-ai/dsh-home-paths')).href);
  const marker = join(resolveDshHome(), 'kwork-archive-migration-v1.json');
  try {
    if (JSON.parse(await readFile(marker, 'utf8')).version === 1) return;
    throw new Error('Invalid KWork archive migration record');
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  let archived;
  try {
    archived = JSON.parse(await readFile(path, 'utf8')).archived ?? [];
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
    archived = [];
  }
  if (!Array.isArray(archived) || archived.some(id => typeof id !== 'string' || !id)) {
    throw new Error('Invalid archived session list in KWork workspace preferences');
  }
  for (const sessionId of new Set(archived)) {
    try {
      await registry.archiveSession(sessionId);
    } catch (error) {
      if (error.name !== 'WorkspaceUnknownSessionError') throw error;
      ctx.logger.warn('KWork archive migration skipped missing Session %s', sessionId);
    }
  }
  await mkdir(dirname(marker), { recursive: true });
  const temporary = marker + '.' + randomUUID() + '.tmp';
  try {
    await writeFile(temporary, JSON.stringify({ version: 1 }), { mode: 0o600 });
    await rename(temporary, marker);
  } finally {
    await rm(temporary, { force: true });
  }
}

async function sessionArchive(ctx, { action, sessionId, stopActivity } = {}) {
  const registry = ctx.get('workspaceRegistry');
  if (!registry) {
    if (action === 'read') return { available: false };
    throw new SessionOperationError('workspace/unavailable', '当前服务无法归档会话。');
  }
  if (action !== 'read') {
    if (typeof sessionId !== 'string' || !sessionId) {
      throw new SessionOperationError('workspace/session-id', '缺少会话 ID。');
    }
    if (action === 'archive') {
      try {
        await registry.archiveSession(sessionId, { stopActivity: stopActivity === true });
      } catch (error) {
        if (error.name === 'WorkspaceActiveSessionError') return { activity: error.activity };
        throw error;
      }
    } else if (action === 'restore') {
      await registry.unarchiveSession(sessionId);
    } else {
      throw new SessionOperationError('workspace/action', '找不到这个会话操作。');
    }
  }
  return { available: true, archived: [...registry.archivedSessionIds] };
}

async function scheduleRequest(ctx, operation) {
  const schedule = ctx.get('schedule');
  if (operation?.action === 'catalog') {
    return schedule ? {
      available: true,
      tasks: await schedule.catalog(),
      limits: schedule.deliveryLimits(),
      canCreateSession: ctx.get('sessionController') !== undefined,
      conversationTools: ctx.get('tools')?.get('schedule_create') !== undefined,
    } : { available: false };
  }
  if (!schedule) throw new SessionOperationError('schedule/unavailable', '当前 ACP 服务无法管理自动化任务。');
  try {
    switch (operation?.action) {
      case 'create': return await schedule.remoteCreate(operation.request);
      case 'edit': return await schedule.remoteEdit({ expectedUpdatedAt: null, ...operation.request });
      case 'run': return await schedule.run({ id: operation.id });
      case 'history': return await schedule.remoteHistory(operation.request);
      case 'clearEnded': return await schedule.deleteInactive();
      default: throw new SessionOperationError('schedule/action', '找不到这个自动化操作。');
    }
  } catch (error) {
    if (typeof error?.code === 'string' && error.code.startsWith('schedule/')) {
      throw new SessionOperationError(error.code, error.message);
    }
    throw error;
  }
}

const runtimeSections = {
  shell: { namespaces: ['pwsh-sandbox', 'bash-sandbox'], fields: ['timeoutMs', 'maxOutputBytes'] },
  agentLoop: { namespaces: ['agent-loop'], fields: ['maxParallelToolCalls'] },
  subagent: { namespaces: ['subagent'], fields: ['maxDepth', 'maxActiveSubagents'] },
};

function runtimeSettings(ctx, { page, change } = {}) {
  if (!Object.hasOwn(runtimeSections, page)) {
    throw new SessionOperationError('settings/page', '找不到这个设置页面。');
  }
  const { namespaces, fields } = runtimeSections[page];
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
      const valid = page === 'shell'
        ? Number.isFinite(value) && value > 0
        : Number.isSafeInteger(value) && value >= (field === 'maxDepth' ? 0 : 1);
      if (edit?.op !== 'set' || !valid) {
        throw new SessionOperationError('settings/value', '请输入此字段允许的数字，或留空以恢复默认值。');
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

// Older runtimes may not export the presentation sets. Keep profile layers
// out of both the cards and the mutation endpoint on those runtimes as well.
const hiddenProfileBundles = new Set([
  'base', 'web-app', 'headless', 'sdk-app', 'acp-app', 'sdk-minimal', 'ui-app',
  'experimental-terminal-bundle', 'experimental-tool-worktree',
].map(name => '@deepseek-ai/dsh-' + name));
let pluginCatalog;
function pluginPresentation() {
  return pluginCatalog ??= (async () => {
    try {
      return await import(pathToFileURL(require.resolve('@deepseek-ai/dsh-official-bundles')).href);
    } catch {
      return {};
    }
  })();
}

async function pluginCenter(ctx) {
  const manager = ctx.get('pluginManager');
  const namespaces = new Set((ctx.get('settings')?.describe({ redactSecrets: true }) ?? [])
    .map(entry => entry.ns));
  const configs = Object.entries(runtimeSections)
    .filter(([, section]) => section.namespaces.some(ns => namespaces.has(ns)))
    .map(([page]) => page);
  if (namespaces.has('web') && ctx.get('web')) configs.push('webSearch');
  if (!manager) return { configs, entries: [], bundles: [], managementAvailable: false };

  const [entries, bundles, catalog] = await Promise.all([
    manager.listPlugins(), manager.listBundles(), pluginPresentation(),
  ]);
  const hidden = name => hiddenProfileBundles.has(name) || catalog.HIDDEN_BUNDLES?.has(name);
  const live = new Set(entries.filter(entry => entry.enabled).map(entry => entry.entryId));
  const mainOrder = catalog.EXTENSION_BUNDLES ?? [];
  const moreOrder = catalog.MORE_BUNDLES ?? [];
  return {
    configs,
    managementAvailable: true,
    entries: entries.map(({ entryId, moduleName, enabled, fiberPhase, diagnostic, readOnlyReason, meta }) => ({
      entryId, moduleName, enabled, fiberPhase, diagnostic, readOnlyReason,
      title: localizedText(meta?.title), description: localizedText(meta?.description),
    })),
    bundles: bundles.filter(bundle => !hidden(bundle.name) &&
      (bundle.installed || bundle.official || bundle.error !== undefined) &&
      (bundle.listedByDefault !== false || bundle.enabled || bundle.installed ||
        bundle.rows.some(row => live.has(row.entryId)))).map(bundle => {
      const { name, version, enabled, installed, official, removable, availability,
        readOnlyReason, error, meta, rows, installTarget, updatable } = bundle;
      const first = mainOrder.indexOf(name);
      const last = moreOrder.indexOf(name);
      return {
        name, version, enabled, installed, official, removable, availability, readOnlyReason, error: error?.code,
        catalogVersion: installTarget?.version,
        catalogUpdate: catalogUpdateAvailable(bundle),
        updatable: updatable === true && typeof manager.listVersions === 'function' &&
          typeof manager.updateBundle === 'function',
        moreOnly: last >= 0 && !enabled,
        rank: first >= 0 ? first : last >= 0 ? mainOrder.length + 1 + last : mainOrder.length,
        title: localizedText(meta?.title),
        description: localizedText(meta?.description) ?? bundle.description,
        rows: rows.map(({ rowId, moduleName, entryId, meta }) => ({
          rowId, moduleName, entryId,
          title: localizedText(meta?.title), description: localizedText(meta?.description),
        })),
      };
    }),
  };
}

let pluginCenterTail = Promise.resolve();

function pluginCenterRequest(ctx, { operation } = {}) {
  if (operation == null) return pluginCenterTail.then(() => pluginCenter(ctx));
  const task = pluginCenterTail.then(async () => {
    const snapshot = await pluginCenter(ctx);
    const manager = ctx.get('pluginManager');
    const bundle = snapshot.bundles.find(item => item.name === operation.name);
    const fail = (error) => ({ snapshot, outcome: { application: 'failed', error } });
    if (pluginInstallJob?.running) return fail('install-running');
    if (!manager || !bundle) return fail('unknown-bundle');
    let result;
    switch (operation.kind) {
      case 'bundle':
        if (typeof operation.enabled !== 'boolean') return fail('invalid-operation');
        if (bundle.readOnlyReason) return fail(bundle.readOnlyReason);
        if (operation.enabled && bundle.availability === 'missing') return fail('missing-bundle');
        if (operation.enabled && bundle.error) return fail(bundle.error);
        result = await manager.setBundleEnabled(bundle.name, operation.enabled);
        break;
      case 'entry': {
        if (typeof operation.enabled !== 'boolean') return fail('invalid-operation');
        const row = bundle.rows.find(item => item.rowId === operation.rowId);
        const entry = row?.entryId && snapshot.entries.find(item => item.entryId === row.entryId);
        if (!bundle.enabled || !entry) return fail('unknown-plugin');
        if (entry.readOnlyReason) return fail(entry.readOnlyReason);
        result = await manager.setPluginEnabled(entry.entryId, operation.enabled);
        break;
      }
      case 'remove':
        if (bundle.readOnlyReason || !bundle.removable) return fail(bundle.readOnlyReason ?? 'not-removable');
        result = await manager.removeBundle(bundle.name);
        break;
      default: return fail('invalid-operation');
    }
    // A profile write can succeed even if re-reading the inventory fails.
    // Keep the Host outcome separate from the refreshed presentation.
    return {
      outcome: {
        application: result.application,
        changed: result.changed,
        restartReason: result.restartReason,
        error: result.error?.code,
      },
      snapshot: await pluginCenter(ctx).catch(() => null),
    };
  });
  pluginCenterTail = task.then(() => undefined, () => undefined);
  return task;
}

let pluginInstallJob;
const installRequestId = /^[0-9a-f]{32}$/;

function pluginInstallEvent(requestId, event) {
  process.stdout.write(JSON.stringify({
    jsonrpc: '2.0', method: '_dsh/gpui/plugin-install-event', params: { requestId, ...event },
  }) + '\n');
}

function observePluginInstall(ctx) {
  ctx.on('plugin-manager/install-state', ({ requestId, phase, attempt }) => {
    if (requestId === pluginInstallJob?.id && pluginInstallJob.running) {
      pluginInstallEvent(requestId, { type: 'phase', phase, attempt });
    }
  }, { global: true });
  ctx.on('plugin-manager/update-state', ({ requestId, step }) => {
    if (requestId === pluginInstallJob?.id && pluginInstallJob.running) {
      pluginInstallEvent(requestId, { type: 'update', step });
    }
  }, { global: true });
  ctx.on('plugin-manager/install-log', ({ requestId, jobId, stream, text, exitCode }) => {
    if (requestId === pluginInstallJob?.id && pluginInstallJob.running) {
      // Install logs are for the main UI only. Do not forward argv, which can
      // contain a user-supplied URL, or the local profile directory.
      pluginInstallEvent(requestId, {
        type: 'log', jobId, stream, text: stripVTControlCharacters(text), exitCode,
      });
    }
  }, { global: true });
}

function safePluginRegistry(value) {
  if (value == null) return value;
  const url = typeof value === 'string' && URL.canParse(value) ? new URL(value) : null;
  if (!url || !['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.hash) {
    throw new SessionOperationError('plugins/registry', '请输入有效的 HTTP 或 HTTPS 安装源地址。');
  }
  return url.href;
}

function installSummary(result) {
  return {
    kind: 'settled',
    application: result.application,
    stage: result.stage,
    changed: result.changed,
    bundle: result.bundle,
    version: result.version,
    enabled: result.enabled,
    restartReason: result.restartReason,
    problem: result.error?.code,
    failure: result.packageResult?.kind,
    pendingBuilds: result.pendingBuilds,
  };
}

function catalogUpdateAvailable(bundle) {
  return bundle.official && bundle.installed && bundle.removable && !bundle.readOnlyReason &&
    bundle.availability === 'profile' && bundle.installTarget !== undefined &&
    !matchesCatalogTarget(bundle, bundle.installTarget);
}

function matchesCatalogTarget(bundle, target) {
  return bundle.version === target.version &&
    ((!target.spec.startsWith('link:') && !bundle.source?.startsWith('link:')) || bundle.source === target.spec);
}

async function updateSettlement(manager, name, from, to, result, restore, catalog) {
  let observed;
  let inventoryRead = false;
  try {
    observed = (await manager.listBundles()).find(bundle => bundle.name === name);
    inventoryRead = true;
  } catch { /* The Host result still describes the attempted change. */ }
  const restored = restore && ['applied', 'restart-required'].includes(restore.application);
  const changed = observed?.version !== from || (catalog && catalog.previous !== observed?.source);
  const recovery = !inventoryRead ? 'unknown' : observed?.installed !== true ? 'missing' :
    changed ? 'changed' : restored ? 'restored' : 'kept';
  const installed = observed?.installed === true &&
    (result.application === 'failed' || !catalog || matchesCatalogTarget(observed, catalog.target));
  return {
    ...installSummary(result),
    bundle: installed ? name : undefined,
    version: installed ? observed.version : undefined,
    enabled: installed ? observed.enabled : undefined,
    update: { from, to, recovery },
  };
}

async function runPluginVersionUpdate(manager, job, input) {
  const { name, from, version, registry } = input;
  const bundle = (await manager.listBundles()).find(bundle => bundle.name === name);
  if (bundle?.updatable !== true || bundle.version !== from || bundle.readOnlyReason ||
      typeof manager.updateBundle !== 'function') {
    return { kind: 'refused', problem: 'stale-version' };
  }
  pluginInstallEvent(job.id, { type: 'subject', subject: {
    name, version, description: localizedText(bundle.meta?.description) ?? bundle.description,
    source: 'registry',
  } });
  job.started = true;
  const result = await manager.updateBundle(name, version, { registry, requestId: job.id });
  return updateSettlement(manager, name, result.from || from, result.to, result.result, result.restore);
}

async function runCatalogInstall(ctx, manager, job, input, approvedBuilds) {
  const { name, from, version, registry } = input;
  const bundle = (await manager.listBundles()).find(bundle => bundle.name === name);
  const target = bundle?.installTarget;
  if (!bundle?.official || !target || target.version !== version || bundle.readOnlyReason ||
      (from === null
        ? bundle.installed || bundle.availability !== 'missing' || bundle.error
        : (bundle.version ?? '') !== from || !catalogUpdateAvailable(bundle))) {
    return { kind: 'refused', problem: 'stale-version' };
  }
  if (job.controller.signal.aborted) return { kind: 'cancelled' };
  const enabled = from === null ? input.enabled : bundle.enabled;
  const local = target.spec.startsWith('link:');
  const subject = {
    name, version, description: localizedText(bundle.meta?.description) ?? bundle.description,
    source: local ? 'path' : 'registry',
  };
  const replaceLive = from !== null && ctx.get('hmr') !== undefined;
  const source = bundle.source;
  // HMR updates stop and remove the running package first. If the update fails
  // after removal, reinstall the exact previous release or its saved source.
  if (replaceLive && !source) return { kind: 'refused', problem: 'not-updatable' };
  job.input = input;
  pluginInstallEvent(job.id, { type: 'subject', subject });
  job.started = true;
  if (replaceLive) {
    pluginInstallEvent(job.id, { type: 'update', step: 'removing' });
    const removal = await manager.removeBundle(name).catch(() => ({
      application: 'failed', stage: 'remove', changed: false, error: { code: 'operation-error' },
    }));
    if (removal.application === 'failed' || removal.application === 'cancelled') {
      return updateSettlement(manager, name, from, version, removal);
    }
  }
  if (from !== null) pluginInstallEvent(job.id, { type: 'update', step: 'installing' });
  let result;
  try {
    result = await manager.installBundle(target.spec, {
      requestId: job.id, registry, enabled, saveExact: !local,
      ...(approvedBuilds === undefined ? {} : { approvedBuilds }),
    });
  } catch (error) {
    if (!replaceLive) throw error;
    result = { application: 'failed', stage: 'install', changed: false, error: { code: 'operation-error' } };
  }
  if (from === null) return installSummary(result);
  let restore;
  if (replaceLive && (result.application === 'failed' || result.application === 'cancelled')) {
    pluginInstallEvent(job.id, { type: 'update', step: 'restoring' });
    if (result.stage === 'enable') await manager.removeBundle(name).catch(() => undefined);
    const range = source.startsWith(name + '@') ? source.slice(name.length + 1) : null;
    const previous = from && range !== null && !range.includes(':') ? name + '@' + from : source;
    restore = await manager.installBundle(previous, { enabled, saveExact: true }).catch(() => null);
  }
  return updateSettlement(manager, name, from, version, result, restore, { target, previous: source });
}

async function runPluginInstall(ctx, manager, job, input, approvedBuilds) {
  await pluginCenterTail;
  if (job.controller.signal.aborted) return { kind: 'cancelled' };
  if (input.kind === 'update') return runPluginVersionUpdate(manager, job, input);
  if (input.kind === 'catalog') return runCatalogInstall(ctx, manager, job, input, approvedBuilds);
  const { spec, registry } = input;
  let subject = input.subject;
  if (!subject) {
    const catalog = (await manager.listBundles()).find(bundle =>
      bundle.name === spec && bundle.official && bundle.availability === 'missing' && bundle.installTarget);
    if (catalog) return runCatalogInstall(ctx, manager, job, {
      kind: 'catalog', name: spec, version: catalog.installTarget.version, from: null, enabled: false, registry,
    });
    if (job.controller.signal.aborted) return { kind: 'cancelled' };
    const checked = await manager.inspect(spec, { registry }, job.controller.signal);
    if (job.controller.signal.aborted) return { kind: 'cancelled' };
    if (checked.status !== 'accepted') return { kind: 'refused', problem: checked.problem };
    subject = {
      name: checked.name, version: checked.version, description: checked.description,
      registry: checked.registry, source: checked.kind,
    };
  }
  job.input = { spec, registry, subject };
  pluginInstallEvent(job.id, { type: 'subject', subject });
  job.started = true;
  const result = await manager.installBundle(spec, {
    requestId: job.id, registry: subject.registry, enabled: false,
    ...(approvedBuilds === undefined ? {} : { approvedBuilds }),
  });
  return installSummary(result);
}

async function pluginInstallRequest(ctx, operation) {
  const manager = ctx.get('pluginManager');
  if (!manager) throw new SessionOperationError('plugins/unavailable', '当前 ACP 服务无法安装插件。');
  if (operation?.action === 'registries') return manager.registries();
  if (operation?.action === 'versions') {
    if (pluginInstallJob?.running || typeof manager.listVersions !== 'function') {
      return { status: 'refused', problem: 'not-updatable', attempts: [] };
    }
    return manager.listVersions(operation.name, {});
  }
  const id = operation?.requestId;
  if (typeof id !== 'string' || !installRequestId.test(id)) {
    throw new SessionOperationError('plugins/request', '插件安装请求无效。');
  }
  if (operation.action === 'observe') {
    return pluginInstallJob?.id === id ? pluginInstallJob.promise : null;
  }
  if (operation.action === 'cancel') {
    const job = pluginInstallJob;
    if (job?.id !== id || !job.running) return { status: 'not-running' };
    if (job.nonCancellable) return { status: 'too-late' };
    if (job.started) return manager.cancelInstall(id);
    job.controller.abort();
    await job.promise;
    return { status: 'cancelled' };
  }
  if (['run', 'approve', 'catalog', 'update'].includes(operation.action) && pluginInstallJob?.id === id) {
    return pluginInstallJob.promise;
  }
  if (pluginInstallJob?.running) {
    throw new SessionOperationError('plugins/busy', '另一个插件安装仍在进行。');
  }
  let input;
  let approvedBuilds;
  if (operation.action === 'approve') {
    const previous = pluginInstallJob;
    approvedBuilds = previous?.result?.pendingBuilds;
    if (!previous || previous.id !== operation.previousRequestId || id === previous.id ||
        !previous.input || previous.result?.application !== 'failed' ||
        !Array.isArray(approvedBuilds) || approvedBuilds.length === 0) {
      throw new SessionOperationError('plugins/approval', '安装脚本的授权已失效，请重新检查插件。');
    }
    input = previous.input;
  } else if (operation.action === 'catalog' || operation.action === 'update') {
    const { name, from, version, enabled, registry } = operation;
    if (typeof name !== 'string' || typeof version !== 'string' || (from !== null && typeof from !== 'string') ||
        (operation.action === 'catalog' ? typeof enabled !== 'boolean' : from === null)) {
      return { kind: 'refused', problem: 'invalid-spec' };
    }
    input = { kind: operation.action, name, from, version, enabled, registry: safePluginRegistry(registry) };
  } else if (operation.action === 'run') {
    const spec = typeof operation.spec === 'string' ? operation.spec.trim() : '';
    if (!spec || spec.length > 4096 || spec.startsWith('-') || /[\x00-\x1f\x7f]/u.test(spec)) {
      return { kind: 'refused', problem: 'invalid-spec' };
    }
    try {
      input = { spec, registry: safePluginRegistry(operation.registry) };
    } catch (error) {
      if (error instanceof SessionOperationError) return { kind: 'refused', problem: 'invalid-registry' };
      throw error;
    }
  } else {
    throw new SessionOperationError('plugins/request', '插件安装请求无效。');
  }
  const nonCancellable = input.kind === 'update' || (input.kind === 'catalog' && input.from !== null);
  const job = { id, running: true, started: false, nonCancellable, controller: new AbortController() };
  pluginInstallJob = job;
  job.promise = runPluginInstall(ctx, manager, job, input, approvedBuilds).catch(() => {
    if (job.controller.signal.aborted) return { kind: 'cancelled' };
    return job.started ? { kind: 'unknown' } : { kind: 'refused', problem: 'unknown' };
  }).then(result => {
    job.result = result;
    job.running = false;
    return result;
  });
  return job.promise;
}

const webFieldSections = {
  count: 'web-deepseek-api',
  baseURL: 'web-search-deepseek',
  maxUses: 'web-search-deepseek',
};
const webSections = ['web-deepseek-api', 'web-search-deepseek', 'web'];
const legacySearch = 'deepseek-official';
let webSettingsTail = Promise.resolve();

function webSettingsError(message) {
  return new SessionOperationError('settings/web', message);
}

function webSettingsEntries(settings) {
  return new Map((settings?.describe({ redactSecrets: true }) ?? [])
    .filter(entry => webSections.includes(entry.ns)).map(entry => [entry.ns, entry]));
}

function legacyCredentialRef(entries) {
  return entries.get('web-search-deepseek')?.value.apiKeyEnv || 'DEEPSEEK_API_KEY';
}

function localizedText(text) {
  return typeof text === 'string' ? text : text?.zh ?? text?.en;
}

async function webSettingsSnapshot(ctx) {
  const settings = ctx.get('settings');
  const entries = webSettingsEntries(settings);
  const web = entries.get('web');
  const runtime = ctx.get('web');
  if (!web || !runtime) return null;
  let providers;
  try { providers = runtime.listProviders(); } catch { /* Keep the current selection visible. */ }
  const selection = capability => {
    const key = capability + 'Provider';
    return {
      value: web.value[key], base: web.base[key],
      options: providers?.[capability].map(({ id, title, description }) => ({
        id, title: localizedText(title), description: localizedText(description),
      })) ?? [],
    };
  };
  const fields = {};
  for (const [name, namespace] of Object.entries(webFieldSections)) {
    const entry = entries.get(namespace);
    if (entry) fields[name] = {
      value: entry.value[name], base: entry.base[name], overridden: Object.hasOwn(entry.user, name),
    };
  }
  let credential;
  if (entries.has('web-search-deepseek')) {
    const ref = legacyCredentialRef(entries);
    try {
      const info = await ctx.get('credentials')?.describe(ref);
      if (info) credential = { ref, configured: info.configured, writable: info.writable };
    } catch { /* Invalid or unavailable references cannot be edited here. */ }
  }
  return {
    search: selection('search'), fetch: selection('fetch'), fields, credential,
    writable: settings.writable, providersFailed: !providers,
    revisions: Object.fromEntries([...entries].map(([ns, entry]) => [ns, entry.revision])),
  };
}

function webFieldEdit(name, edit) {
  if (edit?.op === 'unset') return { op: 'unset', path: [name] };
  const value = edit?.value;
  if (edit?.op !== 'set') throw webSettingsError('设置内容无效。');
  if (name === 'baseURL') {
    const url = typeof value === 'string' && URL.canParse(value) ? new URL(value) : undefined;
    if (!url || !['http:', 'https:'].includes(url.protocol) || url.username || url.password) {
      throw webSettingsError('请输入有效的 HTTP 或 HTTPS 地址。');
    }
  } else if (!Number.isSafeInteger(value) || value < 1 || (name === 'count' && value > 50)) {
    throw webSettingsError('搜索结果数须为 1–50 的整数，单次请求的搜索上限须为正整数。');
  }
  return { op: 'set', path: [name], value };
}

async function saveWebSettings(ctx, change) {
  const settings = ctx.get('settings');
  const entries = webSettingsEntries(settings);
  const web = entries.get('web');
  const runtime = ctx.get('web');
  if (!web || !runtime) throw webSettingsError('该插件当前未加载，暂时无法配置。');
  const sections = new Map();
  const add = (namespace, edit) => {
    if (!sections.has(namespace)) sections.set(namespace, []);
    sections.get(namespace).push(edit);
  };
  for (const [name, edit] of Object.entries(change.fields ?? {})) {
    if (!Object.hasOwn(webFieldSections, name)) throw webSettingsError('该字段无法在此页面编辑。');
    add(webFieldSections[name], webFieldEdit(name, edit));
  }
  let search = web.value.searchProvider;
  if (change.search !== undefined || change.fetch !== undefined) {
    const providers = runtime.listProviders();
    for (const capability of ['search', 'fetch']) {
      const id = change[capability];
      if (id === undefined) continue;
      const name = capability + 'Provider';
      const base = web.base[name];
      if (typeof id !== 'string' || (id === '' ? base !== undefined :
          !providers[capability].some(provider => provider.id === id))) {
        throw webSettingsError('所选提供方已不可用，请重新打开页面后再试。');
      }
      if (capability === 'search') search = id || undefined;
      add('web', id === '' || id === base ? { op: 'unset', path: [name] } :
        { op: 'set', path: [name], value: id });
    }
  }
  const credentials = ctx.get('credentials');
  const credential = change.credential;
  if (credential !== undefined) {
    if (search !== legacySearch || !entries.has('web-search-deepseek') || !credentials ||
        credential?.ref !== legacyCredentialRef(entries) || typeof credential.key !== 'string' ||
        !credential.key.trim() || !(await credentials.describe(credential.ref)).writable) {
      throw webSettingsError('当前提供方的 API Key 无法保存。');
    }
  }
  if (sections.size && !settings.writable) throw webSettingsError('本部署的设置为只读。');
  for (const namespace of sections.keys()) {
    const entry = entries.get(namespace);
    if (!entry || entry.revision !== change.revisions?.[namespace]) {
      throw webSettingsError('设置已更新，请重新打开页面后再试。');
    }
  }

  const applied = [];
  let failure;
  try {
    for (const namespace of webSections) {
      const edits = sections.get(namespace);
      if (!edits) continue;
      await settings.mutate(namespace, edits, entries.get(namespace).revision);
      applied.push(...edits.map(edit => edit.path[0]));
    }
    if (credential !== undefined) {
      // Revalidate the reference after settings writes. A draft must never be
      // redirected to a different reference while saving.
      const current = webSettingsEntries(settings);
      if (current.get('web')?.value.searchProvider !== legacySearch ||
          credential.ref !== legacyCredentialRef(current)) {
        throw webSettingsError('提供方或 API Key 引用已更新。');
      }
      await credentials.set(credential.ref, credential.key.trim());
      applied.push('credential');
    }
  } catch {
    failure = '本部署没有接受全部设置，未保存的内容已保留供你修改。';
  }
  return { snapshot: await webSettingsSnapshot(ctx), applied, failure };
}

function webSettings(ctx, { change } = {}) {
  if (change == null) return webSettingsTail.then(async () => ({ snapshot: await webSettingsSnapshot(ctx), applied: [] }));
  const operation = webSettingsTail.then(() => saveWebSettings(ctx, change));
  webSettingsTail = operation.then(() => undefined, () => undefined);
  return operation;
}

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

/** Flatten a message's content blocks to the plain text a trigger card shows. */
function contentBlocks(content) {
  if (!Array.isArray(content)) return '';
  return content
    .map(block => (block?.type === 'text' ? block.text : ''))
    .filter(text => typeof text === 'string' && text.length > 0)
    .join('\n');
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

function sessionCommands(ctx, { sessionId }) {
  const agent = ctx.get('agents')?.get(sessionId);
  const commands = ctx.get('commands');
  if (!agent || !commands) return [];
  // The session-scoped catalog is the same source Harness uses for its "/" menu.
  return commands.list(agent).map(({ name, description, input }) => ({
    name,
    description,
    ...input?.hint === undefined ? {} : { inputHint: input.hint },
    ...input?.attachments === true ? { attachments: true } : {},
  }));
}

async function executeSessionCommand(ctx, { sessionId, line, attachments }) {
  if (typeof line !== 'string' || !line.trimStart().startsWith('/')) {
    throw new SessionOperationError('command/invalid', '无效的斜杠指令。');
  }
  const agent = ctx.get('agents')?.get(sessionId);
  const commands = ctx.get('commands');
  if (!agent || !commands) {
    throw new SessionOperationError('command/unavailable', '当前会话不支持该指令。');
  }
  const submitted = Array.isArray(attachments) ? attachments : [];
  const execution = await commands.execute(agent, line, submitted, new AbortController().signal);
  if (!execution) {
    throw new SessionOperationError('command/unknown', `未知指令：${line.trim().split(/\s+/)[0]}`);
  }
  if (execution.result.kind === 'error') {
    throw new SessionOperationError('command/rejected', execution.result.text || '指令执行失败。');
  }
  return {
    kind: 'success',
    text: execution.result.text ?? '',
    commandId: execution.commandId,
  };
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

// --- Durable mid-turn submissions -------------------------------------------
// Everything below reuses the Host's own Inbox: the message is admitted with
// the same content vocabulary ACP prompts use (`admitAcpPrompt`), then either
// appended to the next turn (`followup`) or steered into the running one.

/** Accepted inline-image media types, identical to the ACP prompt admission. */
const QUEUE_IMAGE_TYPES = new Set(['image/png', 'image/jpeg', 'image/webp', 'image/gif']);
/** Canonical base64: whole quantum groups, one trailing padded group at most. */
const CANONICAL_BASE64 = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;

let llmModule;
/** The Host's own LLM module, so message identity and freezing stay one implementation. */
async function llmApi() {
  return llmModule ??= await import(pathToFileURL(
    createRequire(require.resolve('@deepseek-ai/dsh-base/package.json'))
      .resolve('@deepseek-ai/dsh-llm')).href);
}

function queueRejected(message) {
  return new SessionOperationError('session/queue-invalid', message);
}

/**
 * Admit one composer payload into ordered core content.
 * Mirrors `admitAcpPrompt`: validate every block first, persist the image batch
 * as durable attachments, then rebuild the text/image order around the refs.
 */
async function queuePromptContent(ctx, agent, blocks) {
  if (!Array.isArray(blocks) || blocks.length === 0) throw queueRejected('排队消息内容为空。');
  const images = [];
  for (const block of blocks) {
    if (block?.type === 'text') {
      if (typeof block.text !== 'string') throw queueRejected('排队消息的文本块无效。');
      continue;
    }
    if (block?.type === 'image') {
      const mediaType = typeof block.mimeType === 'string' && QUEUE_IMAGE_TYPES.has(block.mimeType)
        ? block.mimeType : undefined;
      if (mediaType === undefined) throw queueRejected('图片仅支持 PNG、JPG、WebP、GIF 格式。');
      if (typeof block.data !== 'string' || !CANONICAL_BASE64.test(block.data)) {
        throw queueRejected('图片数据不是规范的 base64。');
      }
      const data = Buffer.from(block.data, 'base64');
      if (data.toString('base64') !== block.data) throw queueRejected('图片数据不是规范的 base64。');
      images.push({ data, mediaType });
      continue;
    }
    throw queueRejected('排队消息只支持文本和图片。');
  }
  let refs = [];
  if (images.length > 0) {
    const attachments = ctx.get('attachments');
    const selection = agent.model.current();
    if (attachments === undefined || selection === undefined) {
      throw queueRejected('当前会话未准备好接收图片，请稍后重试。');
    }
    let info;
    try {
      info = await ctx.llm.resolveModelInfo(selection.provider, selection.model);
    } catch {
      throw queueRejected('无法确认当前模型是否支持图片。');
    }
    if (!Array.isArray(info?.inputModalities) || !info.inputModalities.includes('image')) {
      throw queueRejected(`模型“${selection.model}”不支持图片输入。`);
    }
    try {
      refs = await attachments.saveImages(images);
    } catch (error) {
      throw queueRejected(error instanceof Error ? error.message : String(error));
    }
  }
  const content = [];
  let pendingText = '';
  let imageIndex = 0;
  for (const block of blocks) {
    if (block.type === 'text') {
      pendingText += block.text;
      continue;
    }
    if (pendingText.length > 0) {
      content.push({ type: 'text', text: pendingText });
      pendingText = '';
    }
    content.push({ type: 'image', attachment: refs[imageIndex++] });
  }
  if (pendingText.length > 0) content.push({ type: 'text', text: pendingText });
  return content;
}

/** Resolve one live agent for an extension request. */
function liveAgent(ctx, sessionId) {
  const agent = ctx.get('agents')?.get(sessionId);
  if (agent === undefined) throw new SessionOperationError('session/not-found', '会话不存在或尚未打开。');
  return agent;
}

/** Pending Inbox rows in display order: queued turns first, then next-step steering. */
function inboxRows(ctx, agent) {
  let state;
  try {
    state = ctx.get('sessionProjections')?.snapshot(agent.session, ['inbox']).values.inbox;
  } catch {
    state = undefined;
  }
  const queued = state?.['next-turn'] ?? agent.inbox.nextTurn;
  const steering = state?.['next-step'] ?? agent.inbox.nextStep;
  const row = (message, mode) => ({
    itemId: message.id,
    mode,
    text: contentBlocks(message.content),
    hasImage: Array.isArray(message.content) && message.content.some(block => block?.type === 'image'),
    sourceKind: message.source?.kind ?? 'user',
  });
  return [
    ...queued.map(message => row(message, 'queue')),
    ...steering.map(message => row(message, 'steer')),
  ];
}

async function queuePrompt(ctx, params) {
  const agent = liveAgent(ctx, params.sessionId);
  const content = await queuePromptContent(ctx, agent, params.prompt);
  const { createUserMessage } = await llmApi();
  const message = createUserMessage({ content, source: { kind: 'user' } });
  // Steering only owns the running turn; an idle agent takes the same message
  // as the next turn instead of losing it (official `resolveSubmitMode`).
  const steering = params.mode === 'steer' && agent.status === 'running';
  if (steering) agent.steer(message);
  else agent.followup(message);
  return { itemId: message.id, mode: steering ? 'steer' : 'queue' };
}

function inboxState(ctx, params) {
  const agent = liveAgent(ctx, params.sessionId);
  // Echo the client's request sequence: a snapshot taken before a newer
  // submission must not delete the row that submission added.
  const requestSeq = Number.isSafeInteger(params.requestSeq) ? params.requestSeq : 0;
  return { requestSeq, running: agent.status === 'running', items: inboxRows(ctx, agent) };
}

function inboxAction(ctx, params) {
  const agent = liveAgent(ctx, params.sessionId);
  const queued = agent.inbox.nextTurn.find(message => message.id === params.itemId);
  const steering = agent.inbox.nextStep.find(message => message.id === params.itemId);
  if (queued === undefined && steering === undefined) {
    throw new SessionOperationError('session/queue-item', '排队消息已不在队列中。');
  }
  if (params.action === 'remove') {
    return { removed: agent.inbox.remove(params.itemId) };
  }
  if (params.action === 'steer') {
    // Official `session/steer-unavailable`: only a queued next-turn row can
    // join a still-running turn.
    if (queued === undefined || agent.status !== 'running') {
      throw new SessionOperationError('session/steer-unavailable', '当前回合已结束，无法插话发送。');
    }
    agent.inbox.remove(params.itemId);
    agent.steer(queued);
    return { steered: true };
  }
  throw new SessionOperationError('session/queue-item', '不支持的排队消息操作。');
}

/** Current live goal view, or null when no goal is set. */
function currentGoalView(ctx, agent) {
  const goals = ctx.get('goals');
  if (goals === undefined) return null;
  try {
    return goals.get(agent) ?? null;
  } catch {
    // The agent may have been disposed between the event and this read.
    return null;
  }
}

function goalState(ctx, params) {
  const agent = liveAgent(ctx, params.sessionId);
  return { goal: currentGoalView(ctx, agent) };
}

function goalAction(ctx, params) {
  const goals = ctx.get('goals');
  if (goals === undefined) {
    throw new SessionOperationError('session/goal-unavailable', '当前服务未启用目标功能。');
  }
  const agent = liveAgent(ctx, params.sessionId);
  const current = goals.get(agent);
  if (current === undefined) {
    throw new SessionOperationError('session/goal-missing', '当前会话没有进行中的目标。');
  }
  // Every mutation is compare-and-set on the revision the client saw, so a
  // stale bar cannot overwrite a newer goal.
  const ref = {
    id: current.id,
    revision: Number.isSafeInteger(params.revision) ? params.revision : current.revision,
  };
  switch (params.action) {
    case 'pause': goals.pause(agent, ref); break;
    case 'resume': goals.resume(agent, ref); break;
    case 'clear': goals.clear(agent, ref); break;
    case 'edit': {
      if (typeof params.objective !== 'string' || params.objective.trim().length === 0) {
        throw new SessionOperationError('session/goal-invalid', '目标内容不能为空。');
      }
      goals.edit(agent, ref, { objective: params.objective.trim() });
      break;
    }
    default:
      throw new SessionOperationError('session/goal-invalid', '不支持的目标操作。');
  }
  return { goal: currentGoalView(ctx, agent) };
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
  // A goal created, paused, resumed, edited, blocked, or cleared commits one
  // `goal/change`; forwarding the fresh live view keeps the composer strip in
  // step without polling.
  ctx.on('goal/changed', ({ agent }) => notify(agent.id, {
    sessionUpdate: 'gpui_goal_changed',
    goal: currentGoalView(ctx, agent),
  }));
  ctx.on('agent/assistant-stream', ({ agent, frame }) => {
    if (frame.type === 'chunk') {
      const chunk = frame.chunk;
      if (chunk.type !== 'reasoning-delta' && chunk.type !== 'text-delta' &&
          !(chunk.type === 'block-end' && ['reasoning', 'text'].includes(chunk.block.type))) return;
    }
    const update = { sessionUpdate: 'gpui_assistant_stream', frame };
    if (frame.type === 'end' && frame.outcome.kind === 'committed' &&
        frame.outcome.eventType === 'assistant/message') {
      const event = agent.session.eventAt(frame.outcome.seq);
      if (event?.type === 'assistant/message') {
        const { id, content } = event.data.message;
        update.messageId = id;
        update.hasThought = content.some(block => block.type === 'reasoning' && block.text.length > 0);
        update.hasText = content.some(block => block.type === 'text' && block.text.length > 0);
      }
    }
    notify(agent.id, update);
  });
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
        // The transcript row for a human message is durable: it is logged when
        // the Inbox claims it, not when the client submits it. Forwarding the
        // logged message lets the client reconcile its optimistic row, and is
        // the only live source for a steering message admitted mid-turn.
        notify(session.id, {
          sessionUpdate: 'gpui_user_message',
          messageId: data.id,
          content: contentBlocks(data.content),
          hasImage: Array.isArray(data.content)
            && data.content.some(block => block?.type === 'image'),
          time: event.time,
          seq: event.seq,
        });
      }
      const human = source === 'user' || source === 'user-question-reply';
      if (!human) {
        // Injected context (time snapshots, runtime snapshots, catalogs, tool
        // notices): a collapsed row, never a trigger card and never a model
        // prompt bubble. Sending these as triggers put a giant
        // "Time sampled while preparing…" bubble into every live step while a
        // reopened session dropped them.
        if (source === 'compact-checkpoint' && typeof data.source?.compactionId === 'string') {
          // The checkpoint text is hidden model context; the official transcript
          // shows the compaction marker at this seq instead.
          const compactionId = data.source.compactionId
          const cached = compactionSummaries.get(compactionId) ?? {}
          notify(session.id, {
            sessionUpdate: 'gpui_compaction',
            compactionId,
            summary: cached.summary ?? '',
            items: cached.items ?? null,
            tokens: cached.tokens ?? null,
            time: event.time,
          })
          return
        }
        if (!TRIGGER_SOURCES.has(source)) {
          notify(session.id, {
            sessionUpdate: 'context_injection',
            messageId: data.id,
            sourceKind: source,
            form: data.source?.form,
            entries: data.source?.entries,
            update: data.source?.update === true,
            incomplete: data.source?.incomplete === true,
            content: contentBlocks(data.content),
            time: event.time,
          });
          return;
        }
        // Waking notifications keep their trigger card (official
        // TurnTriggerNodeView) so a goal round or job settle is visible. The
        // variant facts (subagent stop reason, webhook provider) select the
        // card's localized title.
        notify(session.id, {
          sessionUpdate: 'turn_trigger',
          messageId: data.id,
          sourceKind: source,
          form: data.source?.form,
          stopReason: data.source?.stopReason,
          provider: data.source?.provider,
          content: contentBlocks(data.content),
          time: event.time,
        });
      }
      if (['user', 'goal', 'schedule', 'webhook', 'github', 'agent', 'agent-message'].includes(source) ||
          (source === 'user-question-reply' && data.source.outcome === 'answered')) {
        notify(session.id, { sessionUpdate: 'gpui_turn_tail', type, seq: event.seq });
      }
      return;
    }
    // Command results are log-only session events (`command/run` then
    // `command/done`). The official client turns them into a command row in the
    // transcript, so they must reach the UI instead of being dropped here.
    if (type === 'command/run' || type === 'command/done') {
      notify(session.id, {
        sessionUpdate: type === 'command/run' ? 'gpui_command_run' : 'gpui_command_done',
        seq: event.seq,
        time: event.time,
        commandId: data.commandId,
        name: data.name,
        args: data.args,
        kind: data.kind,
        text: data.text,
      });
      return;
    }
    // The complete model-visible system prompt is a collapsed disclosure row in
    // the official transcript (SystemPromptRow), so forward its text once.
    if (type === 'system/message') {
      notify(session.id, {
        sessionUpdate: 'gpui_system_prompt',
        seq: event.seq,
        text: contentBlocks(data.message?.content),
        update: surfaceOp !== undefined && surfaceOp !== 'append',
        time: event.time,
      });
      return;
    }
    // A compaction summary is evidence for the marker, not a row of its own.
    if (type === 'compaction/summary') {
      const compactionId = data.compactionId
      const text = contentBlocks(data.summary)
      const items = Array.isArray(data.shadowedSeqs) ? data.shadowedSeqs.length : null
      const tokens = Number.isSafeInteger(data.shadowedTokenCount) ? data.shadowedTokenCount : null
      if (typeof compactionId === 'string' && compactionId !== '') {
        compactionSummaries.set(compactionId, { summary: text, items, tokens })
        notify(session.id, {
          sessionUpdate: 'gpui_compaction_summary',
          compactionId,
          summary: text,
          items,
          tokens,
          time: event.time,
        })
      }
      return
    }
    // Tool-registry notices are `developer/message` events whose content is a
    // list of tool additions/removals. The official transcript renders them as
    // ContextInjectionRow variants (已添加工具：… / 工具已更新 + 新增/移除 lists);
    // without this they never reach the UI at all.
    if (type === 'developer/message') {
      const source = data.source?.kind ?? data.message?.source?.kind;
      if (source !== 'tool-registry') return;
      const changes = (Array.isArray(data.content) ? data.content : [])
        .filter(block => block?.type === 'tool-addition' || block?.type === 'tool-removal')
        .map(block => ({ added: block.type === 'tool-addition', name: block.toolName }));
      if (changes.length === 0) return;
      notify(session.id, {
        sessionUpdate: 'context_injection',
        messageId: data.message?.id ?? `tool-registry-${event.seq}`,
        sourceKind: 'tool-registry',
        toolChanges: changes,
        time: event.time,
      });
      return;
    }
    // Direct-child discovery facts are what the official lineage catalog lists;
    // the parent run can open each child's transcript from them.
    if (type === 'subagent/catalog') {
      notify(session.id, { sessionUpdate: 'gpui_subagent_catalog', ...data });
      return;
    }
    // Every pending-Inbox mutation is durable and already carries its whole
    // splice. The client re-reads the pending rows instead of rebuilding the
    // splice chain itself, so a claimed steering item leaves the dock exactly
    // when the Host admits it.
    if (type === 'agent/inbox/spliced') {
      notify(session.id, { sessionUpdate: 'gpui_inbox_changed', target: data.target });
      return;
    }
    // Background job lifecycle. `job/registered` opens a row and `job/settled`
    // closes the same one, keyed by jobId; the durable events carry the kind,
    // label and terminal status the transcript needs.
    if (type === 'job/registered' || type === 'job/settled') {
      notify(session.id, {
        sessionUpdate: type === 'job/registered' ? 'gpui_job_registered' : 'gpui_job_settled',
        jobId: data.jobId,
        kind: data.kind,
        label: data.label,
        startedAt: data.startedAt,
        finishedAt: data.finishedAt,
        status: data.status,
        detail: data.detail,
        time: event.time,
      });
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
      // The client renders the terminal failure row (official TurnErrorItem),
      // which needs the recorded code and message, not just the end kind.
      update.reasonObject = data.reason;
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
    if (method === openHostSessionMethod) {
      const sessions = ctx.get('acpSessionHost');
      if (typeof params?.sessionId !== 'string' || !params.sessionId || typeof params.cwd !== 'string') {
        throw new SessionOperationError('session/activate', '缺少会话 ID 或工作区路径。');
      }
      try {
        const opened = sessions ? await sessions.openForClient(params.sessionId, params.cwd) : null;
        response = { result: opened && {
          ...opened,
          hostRunning: ctx.get('agents')?.get(params.sessionId)?.status === 'running',
        } };
      } catch (error) {
        throw new SessionOperationError('session/activate', error instanceof Error ? error.message : String(error));
      }
    } else if (method === sessionArchiveMethod) {
      response = { result: await sessionArchive(ctx, params) };
    } else if (method === scheduleMethod) {
      response = { result: await scheduleRequest(ctx, params?.operation) };
    } else if (method === runtimeSettingsMethod) {
      response = { result: await runtimeSettings(ctx, params) };
    } else if (method === webSettingsMethod) {
      response = { result: await webSettings(ctx, params) };
    } else if (method === pluginCenterMethod) {
      response = { result: await pluginCenterRequest(ctx, params) };
    } else if (method === pluginInstallMethod) {
      response = { result: await pluginInstallRequest(ctx, params?.operation) };
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
    } else if (method === queuePromptMethod) {
      response = { result: await queuePrompt(ctx, params) };
    } else if (method === inboxStateMethod) {
      response = { result: inboxState(ctx, params) };
    } else if (method === inboxActionMethod) {
      response = { result: inboxAction(ctx, params) };
    } else if (method === goalStateMethod) {
      response = { result: goalState(ctx, params) };
    } else if (method === goalActionMethod) {
      response = { result: goalAction(ctx, params) };
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
    } else if (method === sessionCommandsMethod) {
      response = { result: sessionCommands(ctx, params) };
    } else if (method === executeSessionCommandMethod) {
      response = { result: await executeSessionCommand(ctx, params) };
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
