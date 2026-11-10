// HTML uses the same opaque, scriptless browsing context as the default Harness viewer.
const staticPolicy = "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; media-src data:; connect-src 'none'; frame-src 'none'; form-action 'none'; base-uri 'none'";

export async function prepareStaticHtml(source) {
  const { default: purifier } = await import('dompurify');
  const clean = purifier.sanitize(source.replace(/^\uFEFF/u, ''), {
    WHOLE_DOCUMENT: true,
    FORBID_TAGS: ['noscript', 'base', 'link', 'meta', 'iframe', 'frame', 'object', 'embed', 'set', 'animate', 'animateMotion', 'animateTransform'],
    FORBID_ATTR: ['href', 'xlink:href'],
  });
  const parsed = new DOMParser().parseFromString(clean, 'text/html');
  const policy = parsed.createElement('meta');
  policy.httpEquiv = 'Content-Security-Policy';
  policy.content = staticPolicy;
  parsed.head.prepend(policy);
  return { srcdoc: '<!doctype html>' + parsed.documentElement.outerHTML };
}

export function htmlAssetReferences(source) {
  const template = document.createElement('template');
  template.innerHTML = source.replace(/^\uFEFF/u, '');
  if (template.content.querySelector('base[href]')) return [];
  const seen = new Set();
  const assets = [];
  for (const element of template.content.querySelectorAll('script[src],link[href]')) {
    const script = element.localName === 'script';
    const type = element.getAttribute('type')?.trim().toLowerCase() ?? '';
    if (script && !['', 'text/javascript', 'application/javascript'].includes(type)) continue;
    if (!script && !(element.getAttribute('rel') ?? '').toLowerCase().split(/\s+/u).includes('stylesheet')) continue;
    const reference = element.getAttribute(script ? 'src' : 'href');
    if (!reference || /^(?:[a-z][a-z\d+.-]*:|[/\\#?])/iu.test(reference) || reference.includes('\0')) continue;
    const suffix = reference.search(/[?#]/u);
    const path = suffix === -1 ? reference : reference.slice(0, suffix);
    if (!(script ? /\.js$/iu : /\.css$/iu).test(path)) continue;
    const kind = script ? 'script' : 'stylesheet';
    const key = kind + ':' + reference;
    if (seen.has(key)) continue;
    let decoded = null;
    try {
      const candidate = decodeURIComponent(path);
      if (candidate && !/^(?:[a-z][a-z\d+.-]*:|\/)/iu.test(candidate) && !/[\\\0]/u.test(candidate)) decoded = candidate;
    } catch {
      // Keep the failed candidate in order so preceding dependencies are watched.
    }
    assets.push({ kind, reference, path: decoded });
    seen.add(key);
    // One more candidate lets the packer fail at the official limit while
    // retaining watches for any assets it already encountered.
    if (assets.length === 65) break;
  }
  return assets;
}

function loadInteractiveFrame(frame, source, signal) {
  const token = Array.from(crypto.getRandomValues(new Uint8Array(16)),
    byte => byte.toString(16).padStart(2, '0')).join('');
  const location = navigator.platform.startsWith('Win') ? 'http://dsh-preview.localhost/' : 'dsh-preview://localhost/';
  const send = event => {
    if (event.origin !== 'null' || event.source !== frame.contentWindow ||
        event.data?.type !== 'dsh-preview-ready' || event.data.token !== token) return;
    window.removeEventListener('message', send);
    frame.contentWindow.postMessage({
      type: 'dsh-preview-content', token, html: source.html.replace(/^\uFEFF/u, ''), assets: source.assets,
    }, '*');
  };
  window.addEventListener('message', send, { signal });
  frame.src = location + '#' + token;
}

const frames = new Map();
let batch;
let focusListener;
let reportedFocus;
let focusRevision = 0;
let shellFocus;

function focusedPreview() {
  if (!document.hasFocus()) return;
  const active = document.activeElement;
  for (const entry of frames.values()) {
    if (entry.acceptsKeys && entry.frame === active) return entry;
  }
}

function reportFocus() {
  const active = document.activeElement;
  if (document.hasFocus() && active instanceof HTMLElement && active !== document.body &&
      !(active instanceof HTMLIFrameElement)) shellFocus = active;
  const current = focusedPreview();
  if (current === reportedFocus) return;
  reportedFocus = current;
  focusListener?.(current?.key ?? '', ++focusRevision);
}

export function restoreHtmlShellFocus() {
  const preview = focusedPreview();
  if (!preview) return;
  queueMicrotask(() => {
    if (document.activeElement !== preview.frame) return;
    if (shellFocus?.isConnected) shellFocus.focus({ preventScroll: true });
    if (document.activeElement === preview.frame) {
      document.querySelector('canvas[tabindex]')?.focus({ preventScroll: true });
    }
    reportFocus();
  });
}

export function htmlFocusMatches(key, revision) {
  return focusRevision === revision && reportedFocus?.key === key &&
    reportedFocus === focusedPreview();
}

export function htmlOwnsFocus() {
  return focusedPreview() !== undefined;
}

export function republishHtmlFocus() {
  queueMicrotask(() => {
    reportFocus();
    if (reportedFocus) focusListener?.(reportedFocus.key, focusRevision);
  });
}

export function listenHtmlFocus(listener) {
  focusListener = listener;
  const reconcile = () => queueMicrotask(reportFocus);
  window.addEventListener('blur', reconcile);
  window.addEventListener('focus', reconcile);
  document.addEventListener('focusin', reconcile);
  document.addEventListener('focusout', reconcile);
  document.addEventListener('pointerdown', reconcile, true);
}

// GPUI reports measured rectangles during layout. Commit after the synchronous
// draw so delayed layers, including the viewer menu, can contribute their bounds.
export function beginHtmlFrames() {
  const next = { visible: new Map(), covers: new Map(), sharedCovers: [] };
  batch = next;
  queueMicrotask(() => {
    if (batch !== next) return;
    commit(next);
    batch = undefined;
  });
}

export function stageHtmlFrame(key, document, layout) {
  batch?.visible.set(key, { document, ...layout });
}

export function coverHtmlFrame(key, bounds) {
  if (!batch) return;
  const existing = batch.covers.get(key);
  if (existing) existing.push(bounds);
  else batch.covers.set(key, [bounds]);
}

export function coverAllHtmlFrames(cover) {
  batch?.sharedCovers.push(cover);
}

function intersect(a, b) {
  return [Math.max(a[0], b[0]), Math.max(a[1], b[1]), Math.min(a[2], b[2]), Math.min(a[3], b[3])];
}

const hasArea = ([left, top, right, bottom]) => left < right && top < bottom;
const svgNamespace = 'http://www.w3.org/2000/svg';

function rectPath([x1, y1, x2, y2], x, y, radius = 0) {
  const [left, top, right, bottom] = [x1 - x, y1 - y, x2 - x, y2 - y];
  const r = Math.min(radius, (right - left) / 2, (bottom - top) / 2);
  if (!r) return `M${left} ${top}H${right}V${bottom}H${left}Z`;
  const arc = `A${r} ${r} 0 0 1 `;
  return `M${left + r} ${top}H${right - r}${arc}${right} ${top + r}` +
    `V${bottom - r}${arc}${right - r} ${bottom}H${left + r}${arc}${left} ${bottom - r}` +
    `V${top + r}${arc}${left + r} ${top}Z`;
}

function visualMask(bounds, outer, holes) {
  if (!holes.length) return 'none';
  const [left, top, right, bottom] = bounds;
  // WebKit paints this image as an alpha mask without changing iframe hit testing.
  const definitions = holes.map((hole, index) =>
    `<clipPath id="c${index}"><path clip-rule="evenodd" d="${outer} ${hole}"/></clipPath>`).join('');
  const shape = holes.reduce((shape, _, index) =>
    `<g clip-path="url(#c${index})">${shape}</g>`, `<path d="${outer}"/>`);
  const svg = `<svg xmlns="${svgNamespace}" viewBox="0 0 ${right - left} ${bottom - top}"><defs>${definitions}</defs>${shape}</svg>`;
  return `url("data:image/svg+xml,${encodeURIComponent(svg)}")`;
}

function clipFrame(entry, key, bounds, viewport, covers) {
  const [x, y] = bounds;
  const clipped = intersect(bounds, viewport);
  const outer = rectPath(bounds, x, y);
  const shapes = [rectPath(clipped, x, y)];
  const visualHoles = [];
  const shadows = [];
  for (const { bounds: cover, radius, shadow, visual_only: visualOnly } of covers) {
    if (hasArea(intersect(cover, clipped))) {
      const hole = rectPath(cover, x, y, radius);
      if (visualOnly) visualHoles.push(hole);
      else shapes.push(outer + ' ' + hole);
    }
    if (shadow && hasArea(intersect([cover[0] - 20, cover[1] - 20, cover[2] + 20, cover[3] + 20], clipped))) {
      const [left, top, right, bottom] = cover;
      const decoration = document.createElement('div');
      Object.assign(decoration.style, {
        position: 'absolute', left: `${left - x}px`, top: `${top - y}px`,
        width: `${right - left}px`, height: `${bottom - top}px`, borderRadius: `${radius}px`,
        boxShadow: '0 3px 8px #0000000a, 0 0 20px #0000000d',
      });
      shadows.push(decoration);
    }
  }
  entry.shadows.replaceChildren(...shadows);
  // Intersect successive clips so overlapping cover holes cannot reveal content.
  const clips = shapes.length === 1 ? [] : shapes.map((shape, index) => {
    const clip = document.createElementNS(svgNamespace, 'clipPath');
    clip.id = `dsh-html-clip-${key}-${index}`;
    const path = document.createElementNS(svgNamespace, 'path');
    path.setAttribute('d', shape);
    path.setAttribute('clip-rule', 'evenodd');
    if (index > 0) path.setAttribute('clip-path', `url(#dsh-html-clip-${key}-${index - 1})`);
    clip.append(path);
    return clip;
  });
  entry.definitions.replaceChildren(...clips);
  return {
    clipPath: clips.length ? `url(#${clips.at(-1).id})` : `path("${shapes[0]}")`,
    maskImage: visualMask(bounds, outer, visualHoles),
  };
}

function createFrame(key, source) {
  const host = document.createElement('div');
  host.dataset.documentHtmlHost = key;
  Object.assign(host.style, {
    position: 'fixed', overflow: 'hidden', zIndex: '2', maskSize: '100% 100%', maskRepeat: 'no-repeat',
  });
  const scroller = document.createElement('div');
  Object.assign(scroller.style, { position: 'absolute', inset: '0', overflow: 'auto' });
  const shadows = document.createElement('div');
  Object.assign(shadows.style, { position: 'absolute', inset: '0', pointerEvents: 'none' });
  const svg = document.createElementNS(svgNamespace, 'svg');
  svg.setAttribute('width', '0');
  svg.setAttribute('height', '0');
  svg.setAttribute('aria-hidden', 'true');
  svg.style.position = 'fixed';
  const definitions = document.createElementNS(svgNamespace, 'defs');
  svg.append(definitions);
  const frame = document.createElement('iframe');
  frame.title = 'HTML 文档预览';
  frame.name = `dsh-sidebar-html-${key}`;
  const lifetime = new AbortController();
  const interactive = Object.hasOwn(source, 'html');
  frame.setAttribute('sandbox', interactive ? 'allow-scripts' : '');
  frame.dataset.htmlPreview = '';
  frame.addEventListener('focus', reportFocus);
  Object.assign(frame.style, { display: 'block', width: '100%', height: '100%', minHeight: '240px', border: '0' });
  // Set sandbox before giving the element any source or mounting it.
  if (interactive) loadInteractiveFrame(frame, source, lifetime.signal);
  else frame.srcdoc = source.srcdoc;
  scroller.append(frame);
  host.append(scroller, shadows);
  document.body.append(svg, host);
  return { key, host, frame, source, geometry: '', svg, definitions, shadows, lifetime, acceptsKeys: false };
}

function commit({ visible, covers, sharedCovers }) {
  for (const [key, entry] of frames) {
    if (visible.get(key)?.document !== entry.source) {
      entry.lifetime.abort();
      entry.host.remove();
      entry.svg.remove();
      frames.delete(key);
    }
  }
  for (const [key, placement] of visible) {
    let entry = frames.get(key);
    if (!entry) {
      entry = createFrame(key, placement.document);
      frames.set(key, entry);
    }
    const { bounds, viewport, radius, background, hidden, inert } = placement;
    const covered = [...placement.covers, ...(covers.get(key) ?? []), ...sharedCovers];
    const geometry = JSON.stringify([bounds, viewport, radius, background, hidden, inert, covered]);
    if (geometry === entry.geometry) continue;
    entry.geometry = geometry;
    const [x, y, right, bottom] = bounds;
    const visible = !hidden && hasArea(intersect(bounds, viewport));
    entry.acceptsKeys = visible && !inert;
    Object.assign(entry.host.style, {
      left: `${x}px`, top: `${y}px`, width: `${right - x}px`, height: `${bottom - y}px`,
      visibility: visible ? 'visible' : 'hidden',
      pointerEvents: inert ? 'none' : 'auto',
      borderRadius: `0 0 ${radius}px ${radius}px`,
      backgroundColor: background,
      ...(visible ? clipFrame(entry, key, bounds, viewport, covered) : { clipPath: 'none', maskImage: 'none' }),
    });
    entry.frame.style.backgroundColor = background;
  }
  reportFocus();
}
