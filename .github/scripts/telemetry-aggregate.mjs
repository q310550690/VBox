#!/usr/bin/env node
// 汇总带 `shell-family` label 的兼容性反馈 Issue（Issue 即数据源，零服务端）。
//
// 解析规则与 `crates/vbox_jvm/src/telemetry.rs::extract_payload` 保持一致：
//   <!-- vbox-telemetry:start -->
//   ```json
//   { "schema": 1, ... }
//   ```
//   <!-- vbox-telemetry:end -->
//
// 输出：幂等更新一个汇总 Issue（标题固定），正文为机器生成表格。
// 手动触发：Actions → telemetry-aggregate → Run workflow。

import process from 'node:process';

const TOKEN = process.env.GITHUB_TOKEN || process.env.GH_TOKEN;
const REPO = process.env.GITHUB_REPOSITORY;
const LABEL = process.env.TELEMETRY_LABEL || 'shell-family';
const SUMMARY_TITLE = process.env.TELEMETRY_SUMMARY_TITLE || '兼容性反馈汇总（自动更新）';
const MAX_PAGES = Number(process.env.TELEMETRY_MAX_PAGES || 5);
const SUMMARY_MARKER = '<!-- vbox-telemetry-summary -->';
const START = '<!-- vbox-telemetry:start -->';
const END = '<!-- vbox-telemetry:end -->';

const API = 'https://api.github.com';

function fail(msg) {
  console.error(`[telemetry-aggregate] ${msg}`);
  process.exit(1);
}

async function api(path, init = {}) {
  const res = await fetch(`${API}${path}`, {
    ...init,
    headers: {
      accept: 'application/vnd.github+json',
      authorization: `Bearer ${TOKEN}`,
      'x-github-api-version': '2022-11-28',
      'user-agent': 'vbox-telemetry-aggregate',
      ...(init.body ? { 'content-type': 'application/json' } : {}),
      ...(init.headers || {}),
    },
  });
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    throw new Error(`${init.method || 'GET'} ${path} -> ${res.status} ${text.slice(0, 300)}`);
  }
  return res.status === 204 ? null : res.json();
}

function extractPayload(body) {
  if (!body) return null;
  const a = body.indexOf(START);
  if (a < 0) return null;
  const rest = body.slice(a + START.length);
  const b = rest.indexOf(END);
  if (b < 0) return null;
  let json = rest.slice(0, b).trim();
  if (json.startsWith('```json')) json = json.slice('```json'.length).trim();
  if (json.endsWith('```')) json = json.slice(0, -3).trim();
  try {
    return JSON.parse(json);
  } catch {
    return null;
  }
}

function shortSha(v) {
  return typeof v === 'string' ? v.slice(0, 12) : '?';
}

function fmtBytes(n) {
  if (typeof n !== 'number' || !Number.isFinite(n)) return '?';
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  return `${(n / 1024 / 1024).toFixed(2)} MiB`;
}

function esc(v) {
  return String(v ?? '').replaceAll('|', '\\|').replaceAll('\n', ' ');
}

async function listIssues() {
  const out = [];
  for (let page = 1; page <= MAX_PAGES; page++) {
    const batch = await api(
      `/repos/${REPO}/issues?state=all&labels=${encodeURIComponent(LABEL)}&per_page=100&page=${page}&sort=created&direction=desc`,
    );
    if (!Array.isArray(batch) || batch.length === 0) break;
    out.push(...batch.filter((i) => !i.pull_request));
    if (batch.length < 100) break;
  }
  return out;
}

async function ensureLabel(name) {
  try {
    await api(`/repos/${REPO}/labels`, {
      method: 'POST',
      body: JSON.stringify({ name, color: '0e8a16', description: '自动汇总的兼容性反馈' }),
    });
  } catch (e) {
    // 已存在时 GitHub 返回 422，忽略；其它错误继续抛。
    if (!String(e.message).includes(' 422 ')) throw e;
  }
}

function aggregate(issues) {
  const jars = new Map();
  const failures = new Map();
  const families = new Map();
  const installs = new Set();
  let parsedIssues = 0;

  for (const issue of issues) {
    if ((issue.title || '').includes(SUMMARY_TITLE)) continue;
    const payload = extractPayload(issue.body || '');
    if (!payload || payload.schema !== 1) continue;
    parsedIssues++;
    if (payload.install_id) installs.add(String(payload.install_id));
    for (const j of payload.jars || []) {
      const sha = String(j.sha256 || '');
      if (!/^[0-9a-f]{64}$/i.test(sha)) continue;
      const cur = jars.get(sha) || {
        sha,
        size: 0,
        family: 'unknown',
        needs_native_crypto: false,
        markers: new Set(),
        issues: new Set(),
        first: issue.created_at,
        last: issue.created_at,
      };
      cur.size = Math.max(cur.size, Number(j.size) || 0);
      if (j.family) cur.family = String(j.family);
      if (j.needs_native_crypto) cur.needs_native_crypto = true;
      for (const m of j.markers || []) cur.markers.add(String(m));
      cur.issues.add(issue.number);
      if (issue.created_at && issue.created_at < cur.first) cur.first = issue.created_at;
      if (issue.created_at && issue.created_at > cur.last) cur.last = issue.created_at;
      jars.set(sha, cur);
    }
    for (const f of payload.failures || []) {
      const code = String(f.code || '').trim();
      if (!code) continue;
      const cur = failures.get(code) || { code, count: 0, last: issue.created_at, sample: '' };
      cur.count++;
      if (!cur.sample && f.detail) cur.sample = String(f.detail);
      if (issue.created_at && issue.created_at > cur.last) cur.last = issue.created_at;
      failures.set(code, cur);
    }
  }

  for (const j of jars.values()) {
    const f = families.get(j.family) || { jars: 0, needsCrypto: 0, issues: new Set() };
    f.jars++;
    if (j.needs_native_crypto) f.needsCrypto++;
    for (const n of j.issues) f.issues.add(n);
    families.set(j.family, f);
  }

  return { parsedIssues, jars, failures, families, installs };
}

function render(issues, agg) {
  const now = new Date().toISOString().replace('T', ' ').slice(0, 16) + ' UTC';
  const lines = [];
  lines.push(SUMMARY_MARKER);
  lines.push(`# ${SUMMARY_TITLE}`);
  lines.push('');
  lines.push(
    `> 本文由 \`.github/workflows/telemetry-aggregate.yml\` 自动生成，请勿手工编辑。数据源 = 带 \`${esc(LABEL)}\` label 的 Issue，解析规则与客户端 \`telemetry.rs\` 一致。`,
  );
  lines.push(`> 更新时间：${now}`);
  lines.push('');
  lines.push('## 概览');
  lines.push('');
  lines.push(`- 已解析反馈 Issue：**${agg.parsedIssues}** / 带 label 的 Issue 总数 ${issues.length}`);
  lines.push(`- 独立安装（install_id）：**${agg.installs.size}**`);
  lines.push(`- 涉及 jar：**${agg.jars.size}**，壳家族：**${agg.families.size}**，失败码种类：**${agg.failures.size}**`);
  lines.push('');

  lines.push('## 按壳家族');
  lines.push('');
  if (agg.families.size === 0) {
    lines.push('_暂无数据。_');
  } else {
    lines.push('| 家族 | jar 数 | 反馈数 | 依赖 native 解密 |');
    lines.push('|---|---:|---:|---|');
    for (const [name, f] of [...agg.families].sort((a, b) => b[1].jars - a[1].jars)) {
      lines.push(
        `| \`${esc(name)}\` | ${f.jars} | ${f.issues.size} | ${f.needsCrypto ? '是' : '否'} |`,
      );
    }
  }
  lines.push('');

  lines.push('## jar 清单（按首次出现）');
  lines.push('');
  if (agg.jars.size === 0) {
    lines.push('_暂无数据。_');
  } else {
    lines.push('| jar sha256 | 大小 | 家族 | markers | 首次 | 最近 | 相关 Issue |');
    lines.push('|---|---:|---|---|---|---|---|');
    const rows = [...agg.jars.values()].sort((a, b) => String(a.first).localeCompare(String(b.first)));
    for (const j of rows.slice(0, 50)) {
      const issueLinks = [...j.issues]
        .sort((a, b) => a - b)
        .slice(0, 5)
        .map((n) => `#${n}`)
        .join(' ');
      lines.push(
        `| \`${shortSha(j.sha)}\` | ${fmtBytes(j.size)} | \`${esc(j.family)}\` | ${esc([...j.markers].join(', ') || '-')} | ${esc(String(j.first).slice(0, 10))} | ${esc(String(j.last).slice(0, 10))} | ${issueLinks || '-'} |`,
      );
    }
    if (rows.length > 50) lines.push(`\n_共 ${rows.length} 个 jar，仅列最早出现的 50 个。_`);
  }
  lines.push('');

  lines.push('## 失败码排行');
  lines.push('');
  if (agg.failures.size === 0) {
    lines.push('_暂无数据。_');
  } else {
    lines.push('| 错误码 | 次数 | 最近 | 样例（首行） |');
    lines.push('|---|---:|---|---|');
    for (const f of [...agg.failures.values()].sort((a, b) => b.count - a.count).slice(0, 30)) {
      lines.push(
        `| \`${esc(f.code)}\` | ${f.count} | ${esc(String(f.last).slice(0, 10))} | ${esc(String(f.sample).slice(0, 160))} |`,
      );
    }
  }
  lines.push('');
  lines.push('## 怎么提交一条反馈');
  lines.push('');
  lines.push('设置 → JVM 车道 → 兼容性反馈 → 生成反馈 Issue（预填机器可解析块）→ 提交。');
  lines.push('也可以点这里新建：');
  lines.push('');
  lines.push(
    `https://github.com/${REPO}/issues/new?labels=${encodeURIComponent(LABEL)}&title=${encodeURIComponent('兼容性反馈：新加固壳 jar / JVM 车道问题')}`,
  );
  lines.push('');
  return lines.join('\n');
}

async function findSummaryIssue() {
  for (let page = 1; page <= 3; page++) {
    const batch = await api(`/repos/${REPO}/issues?state=open&per_page=100&page=${page}`);
    if (!Array.isArray(batch) || batch.length === 0) break;
    const hit = batch.find(
      (i) =>
        !i.pull_request &&
        (i.title === SUMMARY_TITLE || String(i.body || '').includes(SUMMARY_MARKER)),
    );
    if (hit) return hit;
    if (batch.length < 100) break;
  }
  return null;
}

function selfTest() {
  const sample = (sha, family, code, install) => [
    '### 自动采集的兼容性信息（本机，未上传）',
    '',
    START,
    '```json',
    JSON.stringify(
      {
        schema: 1,
        app_version: '0.1.2',
        jvm_rev: 'deadbeef',
        os: 'windows',
        arch: 'x86_64',
        install_id: install,
        jars: [
          {
            sha256: sha,
            size: 1234567,
            family,
            markers: ['DexNative', 'MyCrypto'],
            native_libs: ['assets/wexguard_v8.so'],
            needs_native_crypto: true,
          },
        ],
        failures: [{ code, detail: `蜘蛛失败（${code}）：demo` }],
      },
      null,
      2,
    ),
    '```',
    END,
  ].join('\n');
  const sha = 'a'.repeat(64);
  const issues = [
    { number: 101, title: '兼容性反馈：demo', body: sample(sha, 'aowu-enc', 'catvod.host.shell_unsupported', 'i-1'), created_at: '2026-11-24T01:00:00Z' },
    { number: 102, title: '兼容性反馈：demo2', body: sample(sha, 'aowu-enc', 'catvod.host.shell_unsupported', 'i-2'), created_at: '2026-11-24T02:00:00Z' },
    { number: 103, title: '无关 issue', body: 'no payload', created_at: '2026-11-24T03:00:00Z' },
  ];
  const agg = aggregate(issues);
  const md = render(issues, agg);
  const ok =
    agg.parsedIssues === 2 &&
    agg.jars.size === 1 &&
    agg.families.get('aowu-enc')?.jars === 1 &&
    agg.failures.get('catvod.host.shell_unsupported')?.count === 2 &&
    agg.installs.size === 2 &&
    md.includes('aowu-enc') &&
    md.includes('catvod.host.shell_unsupported');
  console.log(md);
  if (!ok) fail('self-test 断言未通过');
  console.log('[telemetry-aggregate] self-test OK');
}

async function main() {
  if (!TOKEN) fail('缺少 GITHUB_TOKEN');
  if (!REPO || !REPO.includes('/')) fail(`GITHUB_REPOSITORY 非法: ${REPO}`);

  const issues = await listIssues();
  const agg = aggregate(issues);
  const body = render(issues, agg);
  console.log(
    `[telemetry-aggregate] issues=${issues.length} parsed=${agg.parsedIssues} jars=${agg.jars.size} families=${agg.families.size} failures=${agg.failures.size}`,
  );
  const existing = await findSummaryIssue();
  if (existing) {
    if ((existing.body || '') === body) {
      console.log(`[telemetry-aggregate] 汇总 Issue #${existing.number} 内容未变`);
      return;
    }
    await api(`/repos/${REPO}/issues/${existing.number}`, {
      method: 'PATCH',
      body: JSON.stringify({ body }),
    });
    console.log(`[telemetry-aggregate] 已更新汇总 Issue #${existing.number}`);
  } else {
    await ensureLabel(LABEL);
    const created = await api(`/repos/${REPO}/issues`, {
      method: 'POST',
      body: JSON.stringify({ title: SUMMARY_TITLE, body, labels: [LABEL] }),
    });
    console.log(`[telemetry-aggregate] 已创建汇总 Issue #${created.number}`);
  }
}

if (process.argv.includes('--self-test')) {
  selfTest();
} else {
  main().catch((e) => fail(e.message || String(e)));
}
