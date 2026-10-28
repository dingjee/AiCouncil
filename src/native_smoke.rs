//! 原生冒烟 harness(GLM-F 所有;debug-only,显式 opt-in,绝不进 release)。
//!
//! 目的:在**真实 Tauri webview**里通过 `window.__TAURI_INTERNALS__.invoke`
//! 走**真实 IPC**(无任何 mock transport),按 docs/TAURI_IPC.md 的契约
//! 验证:bootstrap/设置/分组/会话/草稿/目录/偏好/发送(本地 mock 提供方)/
//! 流式落库/取消/usage/导出/错误码,并断言前端 DOM 真正渲染会话。
//!
//! 安全约束:
//! - 仅 `cfg(debug_assertions)` 有意义;release 构建不含本模块逻辑;
//! - 仅当 `AICOUNCIL_NATIVE_SMOKE=1` 时启用(驱动脚本显式 opt-in);
//! - 数据目录由 `AICOUNCIL_DATA_DIR` 严格隔离(脚本负责),`.env` 为
//!   假 fixture;本地 mock 经 `AICOUNCIL_BASE_URL` 注入,零真实花费;
//! - 结果 JSON 由 debug-only 命令 `smoke_report` 写入隔离数据目录
//!   (`native_smoke_report.json`),供脚本断言;值不打印密钥。
//!
//! K4C glue 契约(desktop/smoke.rs 保持最小):
//!   1. on_page_load(Finished) 且 `native_smoke::enabled()` 时
//!      `window.eval(native_smoke::page_load_script())`;
//!   2. 注册 debug-only 命令 `smoke_report`(参数 `payload: serde_json::Value`)
//!      → `native_smoke::write_report_json(&payload.to_string())`。
//!   (旧内联 JS 已废弃——其 `const invoke` 作用域缺陷由本模块修复。)

/// 冒烟是否启用:debug 构建 + 显式 opt-in。
pub fn enabled() -> bool {
    cfg!(debug_assertions) && std::env::var_os("AICOUNCIL_NATIVE_SMOKE").is_some_and(|v| v == "1")
}

/// 结果 JSON 落点(隔离数据目录内)。
pub fn report_path() -> Option<std::path::PathBuf> {
    crate::storage::repo::data_dir()
        .ok()
        .map(|d| d.join("native_smoke_report.json"))
}

/// debug-only `smoke_report` 命令的落盘实现:原子写报告 JSON。
/// 仅在冒烟启用(debug + opt-in)时写;否则恒为空操作(release 无副作用)。
/// 返回写入路径(未启用/隔离目录未配置时为 None,调用方记日志即可)。
pub fn write_report_json(payload: &str) -> std::io::Result<Option<std::path::PathBuf>> {
    if !enabled() {
        eprintln!("[native-smoke] report write skipped: harness not enabled");
        return Ok(None);
    }
    let Some(path) = report_path() else {
        eprintln!("[native-smoke] report write skipped: no data dir");
        return Ok(None);
    };
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, payload)?;
    std::fs::rename(&tmp, &path)?;
    eprintln!(
        "[native-smoke] report written: {} ({} bytes)",
        path.display(),
        payload.len()
    );
    Ok(Some(path))
}

/// 注入 webview 的冒烟脚本。约定:
/// - `invoke` 在 IIFE 顶部绑定一次,全程在作用域内(修复旧 glue 的缺陷);
/// - 所有断言失败都**记录**而不中断,最终一次性上报;
/// - 上报前同时挂 `window.__AICOUNCIL_SMOKE_RESULT__` 作为辅助标记;
/// - 120s 硬超时兜底,超时也上报。
pub fn page_load_script() -> &'static str {
    r#"
(async () => {
  // 运行一次守卫:防 on_page_load 与 fallback 重复注入导致并发实例互相覆盖报告
  if (window.__AICOUNCIL_SMOKE_RUNNING__) return;
  window.__AICOUNCIL_SMOKE_RUNNING__ = true;
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const out = { ok: true, enabled: typeof invoke === 'function', steps: [], started_at: new Date().toISOString() };
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  // 最早检查点:证明 harness 主体确实开始执行(区别于“注入了但没跑”)
  out.steps.push({ name: 'harness:entered', ok: true, value: navigator.userAgent.slice(0, 60) });
  // 单步硬超时:invoke 永不设超时,后端/前端卡死时保证套件继续并留下记录
  const withTimeout = (p, ms, label) =>
    Promise.race([
      p,
      sleep(ms).then(() => { throw new Error(`hard timeout ${ms}ms: ${label}`); }),
    ]);
  // 增量上报:每步完成即尽力落盘(失败不中断),卡住时能定位到最后一步
  let flushTimer = null;
  const flush = () => {
    if (flushTimer) return; // 去抖:避免同一毫秒多次 invoke
    flushTimer = setTimeout(() => {
      flushTimer = null;
      invoke('smoke_report', { payload: out }).catch((e) => {
        // 上报失败本身记录进 out:若后续任一次 flush 成功即可见
        out.steps.push({ name: 'flush_error', ok: false, error: String(e) });
      });
    }, 50);
  };
  const step = async (name, fn) => {
    const t0 = Date.now();
    try {
      const value = await fn();
      out.steps.push({ name, ok: true, value: value === undefined ? null : value, ms: Date.now() - t0 });
      flush();
      return value;
    } catch (e) {
      out.ok = false;
      out.steps.push({ name, ok: false, code: e && e.code, error: String((e && e.message) || e), ms: Date.now() - t0 });
      flush();
      return undefined;
    }
  };

  const run = async () => {
    if (!out.enabled) throw new Error('window.__TAURI_INTERNALS__.invoke 不可用(非 Tauri 环境或注入过早)');

    // 1. bootstrap:真实启动状态
    const boot = await step('bootstrap', async () => {
      const b = await invoke('bootstrap');
      if (!b || b.app_version !== '0.2.1') throw new Error('app_version != 0.2.1: ' + JSON.stringify(b && b.app_version));
      if (!b.credentials || b.credentials.openrouter !== true) throw new Error('fixture .env 未生效: credentials.openrouter != true');
      if (!Array.isArray(b.conversations)) throw new Error('conversations 不是数组');
      if (!b.catalog || typeof b.catalog !== 'object') throw new Error('catalog 缺失');
      return {
        version: b.app_version,
        credentials: b.credentials,
        conversations: b.conversations.length,
        // 载荷形状探针:目录字段的实际结构(前端必须能容忍)
        catalog_is_array: Array.isArray(b.catalog),
        catalog_keys: b.catalog && typeof b.catalog === 'object' ? Object.keys(b.catalog) : null,
        catalog_entries: b.catalog && b.catalog.entries ? b.catalog.entries.length : null,
        top_keys: Object.keys(b).sort(),
      };
    });
    if (!boot) return;

    // 2. 分组/会话 CRUD
    const group = await step('create_group', async () => {
      const g = await invoke('create_group', { name: 'Smoke Group' });
      if (!g || !g.id || g.name !== 'Smoke Group') throw new Error('group 异常: ' + JSON.stringify(g));
      return g;
    });
    const conv = await step('create_conversation', async () => {
      const c = await invoke('create_conversation');
      if (!c || !c.id) throw new Error('conversation 缺 id');
      return c;
    });
    if (!conv) return;
    const convId = conv.id;
    await step('rename_conversation', async () => {
      const c = await invoke('rename_conversation', {
        conversation_id: convId, title: 'Smoke Conv', expected_title_revision: conv.title_revision ?? 0,
      });
      if (!c || c.title !== 'Smoke Conv') throw new Error('rename 未生效: ' + JSON.stringify(c));
      return { title: c.title };
    });
    if (group) {
      await step('set_conversation_group', () => invoke('set_conversation_group', { conversation_id: convId, group_id: group.id }));
    }
    await step('set_system_prompt', () => invoke('set_system_prompt', { conversation_id: convId, system_prompt: 'You are a smoke test.' }));

    // 3. 草稿:字节级保真 + revision 语义
    const draft = await step('draft_roundtrip', async () => {
      const text = '  smoke draft \n混排 CJK 文本\n';
      const r1 = await invoke('save_draft', { conversation_id: convId, text, attachments: [], changed: true });
      const d1 = await invoke('get_draft', { conversation_id: convId });
      if (!d1 || d1.text !== text) throw new Error('draft 文本非字节保真: ' + JSON.stringify(d1 && d1.text));
      if (d1.revision !== r1.revision) throw new Error('draft revision 不一致');
      const r2 = await invoke('save_draft', { conversation_id: convId, text, attachments: [], changed: false });
      if (r2.revision !== r1.revision) throw new Error('changed:false 不应 bump revision');
      return { revision: r1.revision };
    });

    // 4. 设置合并
    await step('update_settings', async () => {
      const s = await invoke('update_settings', { patch: { body_text_size: 16 } });
      if (!s || s.body_text_size !== 16) throw new Error('settings 合并失败: ' + JSON.stringify(s));
      return s;
    });

    // 5. 列表与详情
    await step('list_conversations', async () => {
      const list = await invoke('list_conversations');
      if (!list.some((c) => c.id === convId)) throw new Error('新会话不在侧栏列表');
      return list.length;
    });
    await step('load_conversation', async () => {
      const d = await invoke('load_conversation', { conversation_id: convId });
      if (!d || !d.conversation || d.conversation.id !== convId) throw new Error('detail 会话不匹配');
      if (!Array.isArray(d.messages) || !Array.isArray(d.generations) || !Array.isArray(d.usage)) throw new Error('detail 数组缺失');
      return { messages: d.messages.length };
    });

    // 6. 错误码契约:不存在会话 → not_found
    await step('error_not_found_code', async () => {
      try {
        await invoke('load_conversation', { conversation_id: '00000000-0000-4000-8000-000000000000' });
        throw new Error('预期 reject 却成功返回');
      } catch (e) {
        if (e && e.code === 'not_found') return { code: e.code };
        throw new Error('错误码不是 not_found: ' + JSON.stringify({ code: e && e.code, message: e && e.message }));
      }
    });

    // 7. 目录(本地 mock /models)+ 偏好
    const entry = await step('refresh_catalog', async () => {
      const cat = await invoke('refresh_catalog');
      const first = cat && cat.entries && cat.entries[0];
      const modelId = first && (first.id || first.model_id);
      if (!first || !modelId) throw new Error('mock 目录为空: ' + JSON.stringify(cat));
      return { model_id: modelId, provider_id: first.provider_id };
    });
    if (entry) {
      await step('set_model_preference', () =>
        invoke('set_model_preference', {
          provider_id: entry.provider_id,
          model_id: entry.model_id,
          preference: { enabled: true, sort_order: 0, is_default: true, overrides: null },
        })
      );
      await step('list_preferences', async () => {
        const prefs = await invoke('list_preferences');
        if (!prefs.some((p) => p.enabled && p.model_id === entry.model_id)) {
          throw new Error('偏好未生效: ' + JSON.stringify(prefs));
        }
        return prefs.length;
      });
      await step('check_key_status', async () => {
        const k = await invoke('check_key_status');
        if (!k || k.status !== 'ok') throw new Error('key 状态异常: ' + JSON.stringify(k));
        return k.status;
      });
    }

    // 8. 真实发送(本地 mock 提供方)+ 流式落库 + usage
    const draftRev = draft ? draft.revision : 0;
    const receipt = await step('send_message_single', () => invoke('send_message', {
      request: {
        conversation_id: convId,
        mode: 'normal',
        prompt: 'Hello smoke 中文',
        attachments: [],
        expected_draft_revision: draftRev,
        selection: {
          kind: 'single',
          leaf: null,
          targets: [{ provider_id: entry.provider_id, model_id: entry.model_id }],
          primary: 0,
        },
      },
    }));
    // 契约回归:命令参数按**参数名**寻址 —— `send_message` 只有一个结构体
    // 参数 `request`,所以载荷必须包在 `{ request: {...} }` 下;顶层平铺
    // 会被后端拒绝("missing required key request")。
    await step('contract_send_arg_shape', async () => {
      const base = {
        conversation_id: convId,
        mode: 'normal',
        prompt: 'probe shape',
        attachments: [],
        expected_draft_revision: 0,
        selection: {
          kind: 'single',
          leaf: null,
          targets: [{ provider_id: entry.provider_id, model_id: entry.model_id }],
          primary: 0,
        },
      };
      const results = [];
      const tryOne = async (name, payload) => {
        try {
          const r = await invoke('send_message', payload);
          results.push({ name, ok: true, request_id: r && r.primary_request_id });
        } catch (e) {
          results.push({ name, ok: false, error: String((e && e.message) || e) });
        }
      };
      await tryOne('nested', { request: base });
      await tryOne('flat', base);
      const nested = results.find((r) => r.name === 'nested');
      const flat = results.find((r) => r.name === 'flat');
      if (!nested || !nested.ok) {
        throw new Error('send_message 必须用 { request: {...} } 形态: ' + JSON.stringify(results));
      }
      if (flat && flat.ok) {
        throw new Error('send_message 的平铺形态不应被接受(契约已变)');
      }
      return results;
    });

    const rid = receipt && receipt.primary_request_id;
    if (!rid) {
      out.ok = false;
      out.steps.push({ name: 'send_receipt', ok: false, error: '缺少 primary_request_id' });
    } else {
      await step('stream_persists_and_completes', async () => {
        const deadline = Date.now() + 10000;
        while (Date.now() < deadline) {
          const d = await invoke('load_conversation', { conversation_id: convId });
          const gen = (d.generations || []).find((g) => g.request_id === rid);
          if (gen && gen.status === 'completed') {
            const assistant = (d.messages || []).find((m) => m.id === gen.assistant_message_id);
            const expect = 'Mock reply: Hello smoke 中文';
            if (!assistant || assistant.content !== expect) {
              throw new Error('助手内容不匹配: ' + JSON.stringify(assistant && assistant.content));
            }
            const usage = (d.usage || []).find((u) => u.request_id === rid);
            if (!usage || usage.cost_text == null) throw new Error('usage/cost 未落库');
            return { status: gen.status, cost: usage.cost_text, content_len: assistant.content.length };
          }
          if (gen && (gen.status === 'failed' || gen.status === 'cancelled')) {
            throw new Error('生成提前终止 ' + gen.status + ': ' + (gen.error || ''));
          }
          await sleep(300);
        }
        throw new Error('等待流式完成超时');
      });
    }

    // 9. 取消:慢速流 + cancel_request
    const slow = await step('send_message_slow', async () => {
      const d0 = await invoke('get_draft', { conversation_id: convId });
      return invoke('send_message', {
        request: {
          conversation_id: convId,
          mode: 'normal',
          prompt: 'cancel me',
          attachments: [],
          expected_draft_revision: (d0 && d0.revision) || 0,
          selection: {
            kind: 'single',
            leaf: null,
            targets: [{ provider_id: entry.provider_id, model_id: 'mock-slow-model' }],
            primary: 0,
          },
        },
      });
    });
    if (slow && slow.primary_request_id) {
      await step('cancel_request', async () => {
        await invoke('cancel_request', { request_id: slow.primary_request_id });
        const deadline = Date.now() + 10000;
        while (Date.now() < deadline) {
          const d = await invoke('load_conversation', { conversation_id: convId });
          const g = (d.generations || []).find((x) => x.request_id === slow.primary_request_id);
          if (g && g.status === 'cancelled') return { status: g.status };
          if (g && g.status === 'failed' && ['cancelled', 'stream_aborted'].includes(g.failure_kind || '')) {
            return { status: g.status, kind: g.failure_kind };
          }
          if (g && g.status === 'completed') throw new Error('慢速请求未被取消反而完成');
          await sleep(300);
        }
        throw new Error('等待取消超时');
      });
    }

    // 10. 导出(内容断言)
    await step('export_markdown', async () => {
      const md = await invoke('export_markdown', { conversation_id: convId, scope: 'main' });
      if (!md || !md.filename) throw new Error('导出缺 filename');
      if (!String(md.content).includes('Hello smoke 中文') || !String(md.content).includes('Mock reply: Hello smoke 中文')) {
        throw new Error('导出内容缺用户提问或助手回复');
      }
      return { filename: md.filename, len: md.content.length };
    });

    // 11. 凭据重载语义(只回布尔状态)
    await step('reload_credentials', async () => {
      const c = await invoke('reload_credentials');
      if (!c || c.openrouter !== true || c.deepseek !== true) throw new Error('reload 后状态异常: ' + JSON.stringify(c));
      return c;
    });

    // 11.5 恢复主分支到**已完成**的首个回答。
    // 第 9 步的慢速请求把主叶改成了随后被取消的 assistant;UI 的普通发送
    // 默认从 `conversation.main_leaf_id` 继续,必须先指回一个可用的叶子。
    const firstAnswer = receipt && receipt.assistant_of && receipt.assistant_of[rid];
    if (firstAnswer) {
      await step('restore_main_leaf', async () => {
        await invoke('set_main_leaf', { conversation_id: convId, message_id: firstAnswer });
        const deadline = Date.now() + 5000;
        while (Date.now() < deadline) {
          const d = await withTimeout(
            invoke('load_conversation', { conversation_id: convId }),
            5000,
            'load_conversation after set_main_leaf'
          );
          if (d.conversation && d.conversation.main_leaf_id === firstAnswer) {
            // 给前端一点时间跟随 conversation-changed 刷新自己的主分支。
            await sleep(600);
            return { main_leaf_id: firstAnswer };
          }
          await sleep(200);
        }
        throw new Error('主分支未回到已完成的首个回答');
      });
    }

    // 12. 原生几何回归(计划用户问题 1/2/3/4,在真实 WebKit 布局里断言,
    //     不是源码文本断言):composer 可见可编辑、长问答真实滚轮滚动、
    //     20 次侧栏开合 main 宽度不塌缩、长问题只折叠正文且页脚始终可点。
    //     长内容来自**真实发送**(IPC → 本地 mock 回显),不伪造 DOM 状态。
    await step('geometry_native_layout', async () => {
      const ckpt = (name, value) => {
        out.steps.push({ name: `geo:${name}`, ok: true, value: value === undefined ? null : value });
        flush();
      };
      ckpt('entered');
      if (!entry) throw new Error('目录为空,无法进行几何发送');
      const mainEl = () => document.querySelector('[data-testid="main"]');
      const appEl = () => document.querySelector('[data-testid="app"]');
      const composerEl = () =>
        document.querySelector('.composer__input') ||
        document.querySelector('textarea');
      const inViewport = (el) => {
        const r = el.getBoundingClientRect();
        return (
          r.width > 0 && r.height > 0 &&
          r.top >= 0 && r.left >= 0 &&
          r.bottom <= window.innerHeight + 1 && r.right <= window.innerWidth + 1
        );
      };
      const setNativeValue = (el, v) => {
        const proto =
          el instanceof HTMLTextAreaElement
            ? HTMLTextAreaElement.prototype
            : HTMLInputElement.prototype;
        Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, v);
        el.dispatchEvent(new Event('input', { bubbles: true }));
      };
      // 打开会话(与 IME 步骤同一真实点击路径),等 thread + composer 挂载
      const deadlineOpen = Date.now() + 5000;
      let opened = false;
      while (Date.now() < deadlineOpen && !opened) {
        const items = Array.from(document.querySelectorAll('button, [role="button"], li, div')).filter(
          (el) => el.textContent && el.textContent.includes('Smoke Conv') && el.offsetParent !== null
        );
        const clickable = items.filter(
          (el) => el.tagName === 'BUTTON' || el.getAttribute('role') === 'button'
        );
        const pool = clickable.length ? clickable : items;
        const target = pool[pool.length - 1];
        if (target) {
          target.click();
          opened = true;
          break;
        }
        await sleep(300);
      }
      if (!opened) throw new Error('geometry: 侧栏未渲染可点击的会话项');
      let thread = null;
      let composer = null;
      const deadlineMount = Date.now() + 8000;
      while (Date.now() < deadlineMount && !(thread && composer)) {
        thread = document.querySelector('[data-testid="thread"]');
        composer = composerEl();
        if (!(thread && composer)) await sleep(200);
      }
      if (!thread) throw new Error('geometry: [data-testid="thread"] 未挂载');
      if (!composer) throw new Error('geometry: composer 未挂载');
      ckpt('mounted', { tag: composer.tagName });

      // (a) 问题 3:composer 在真实窗口内可见、可编辑(设置值可读回,不发送)。
      //     关系断言(UI_REPAIR_A 契约,测 .composer 包装层而非 textarea:
      //     textarea 只是包装层内的输入框):包装层底边≈窗口底边、右缘≈窗口右缘,
      //     main 高≈窗口高(WebKit 度量差异 ±4px 容差)。
      const composerWrap = composer.closest('.composer') || composer;
      const cw = composerWrap.getBoundingClientRect();
      if (
        Math.abs(cw.bottom - window.innerHeight) > 4 ||
        Math.abs(cw.right - window.innerWidth) > 4
      ) {
        throw new Error(
          'geometry: composer 包装层未贴满窗口底/右(用户问题 3): ' +
            JSON.stringify({
              wrap: [cw.left, cw.top, cw.width, cw.height],
              bottom: cw.bottom, innerH: window.innerHeight,
              right: cw.right, innerW: window.innerWidth,
            })
        );
      }
      const mr0 = mainEl().getBoundingClientRect();
      if (Math.abs(mr0.height - window.innerHeight) > 4) {
        throw new Error(
          'geometry: main 高度未占满窗口: ' + JSON.stringify({ h: mr0.height, innerH: window.innerHeight })
        );
      }
      if (!inViewport(composer)) {
        throw new Error(
          'geometry: composer 不在视口内: ' + JSON.stringify(composer.getBoundingClientRect())
        );
      }
      composer.focus();
      setNativeValue(composer, 'geometry-edit-probe');
      const probeOk = composer.value === 'geometry-edit-probe';
      setNativeValue(composer, '');
      if (!probeOk) throw new Error('geometry: composer 不可编辑(值无法设置/读回)');
      ckpt('composer_visible_editable');

      // (b) 真实 UI 路径的长内容:composer 写入长文本 + Enter(与真人同一路径;
      //     后端直接 IPC 发送不进入前端线程状态,不是用户路径,已验证)。
      const lines = Array.from({ length: 120 }, (_, i) =>
        `Geometry long history line ${String(i).padStart(3, '0')} ABCDEFGHIJKLMNOPQRSTUVWXYZ.`
      );
      const longText = lines.join('\n');
      composer.focus();
      setNativeValue(composer, longText);
      if (composer.value !== longText) throw new Error('geometry: 长文本未能写入 composer');
      composer.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
      ckpt('long_ui_send_entered', { chars: longText.length, lines: lines.length });

      // (c0) 等长问题渲染并展开(长问题默认折叠;展开后才有完整溢出——用户真实操作)
      let qtoggle = null;
      const toggleDeadline = Date.now() + 10000;
      while (Date.now() < toggleDeadline && !qtoggle) {
        qtoggle = Array.from(
          document.querySelectorAll('[data-testid="turn"] [data-testid="question-toggle"]')
        ).find((b) => b.offsetParent !== null);
        if (!qtoggle) await sleep(300);
      }
      if (!qtoggle) throw new Error('geometry: 长问题的 question-toggle 未渲染');
      qtoggle.click();
      const expandedDeadline = Date.now() + 3000;
      while (Date.now() < expandedDeadline) {
        const body = document.querySelector('[data-testid="turn"] [data-testid="question-body"]');
        if (body && body.getAttribute('data-collapsed') === 'false') break;
        await sleep(150);
      }
      ckpt('long_question_expanded');

      // (c) 问题 2:UI 真正渲染长历史 → thread 溢出 → 真实 wheel 改变 scrollTop
      let overflowed = false;
      let turnCount = -1;
      const scrollDeadline = Date.now() + 15000;
      while (Date.now() < scrollDeadline && !overflowed) {
        thread = document.querySelector('[data-testid="thread"]');
        if (thread) {
          turnCount = document.querySelectorAll('[data-testid="turn"]').length;
          overflowed = thread.scrollHeight > thread.clientHeight + 50;
        }
        if (!overflowed) await sleep(300);
      }
      if (!overflowed) {
        throw new Error(
          'geometry: 长历史未产生滚动溢出 scrollHeight=' +
            (thread ? thread.scrollHeight : -1) + ' clientHeight=' +
            (thread ? thread.clientHeight : -1) + ' turns=' + turnCount
        );
      }
      thread.scrollTop = thread.scrollHeight;
      await sleep(200);
      const bottomTop = thread.scrollTop;
      if (bottomTop <= 0) {
        throw new Error(
          'geometry: thread 无法滚动(scrollTop=0)' +
            ' scrollHeight=' + thread.scrollHeight + ' clientHeight=' + thread.clientHeight
        );
      }
      // 注:合成 dispatchEvent 的 wheel 事件是**不可信**的,WebKit/Chrome 都不会
      // 为它执行默认滚动;真实滚轮输入由 A 的可信 CDP 输入 harness(Chrome)覆盖。
      // 这里在真实 WebKit 里验证滚动机制本身:内容真溢出、scrollTop 真可变、
      // 程序化滚动触发真实 scroll 事件(与用户滚动同一事件通路)。
      let scrollEventFired = false;
      const onScroll = () => {
        scrollEventFired = true;
      };
      thread.addEventListener('scroll', onScroll);
      thread.scrollTop = Math.max(0, bottomTop - 400);
      await sleep(300);
      thread.removeEventListener('scroll', onScroll);
      const afterTop = thread.scrollTop;
      if (!(afterTop < bottomTop)) {
        throw new Error(
          'geometry: thread scrollTop 不可变 before=' + bottomTop + ' after=' + afterTop
        );
      }
      if (!scrollEventFired) {
        throw new Error('geometry: thread 滚动未触发真实 scroll 事件');
      }
      ckpt('thread_scrolled', { bottom: bottomTop, after: afterTop });

      // (d) 问题 1:20 次侧栏开合,main 宽度不塌缩、composer 始终可见
      const visibleToggle = () =>
        Array.from(document.querySelectorAll('button[aria-label="Toggle sidebar"]')).find(
          (b) => b.offsetParent !== null
        );
      const waitForSidebar = async (want) => {
        const deadline = Date.now() + 3000;
        while (Date.now() < deadline) {
          const app = appEl();
          if (app && app.getAttribute('data-sidebar') === want) return true;
          await sleep(80);
        }
        return false;
      };
      const measure = () => {
        const app = appEl();
        const m = mainEl().getBoundingClientRect();
        const c = composerEl();
        const cr = c ? c.getBoundingClientRect() : null;
        return {
          sidebar: app ? app.getAttribute('data-sidebar') : null,
          main_w: Math.round(m.width),
          main_h: Math.round(m.height),
          composer_visible: c ? inViewport(c) : false,
          composer_top: cr ? Math.round(cr.top) : null,
          composer_h: cr ? Math.round(cr.height) : null,
        };
      };
      if (!(await waitForSidebar('expanded'))) {
        throw new Error('geometry: 测量前未处于展开态');
      }
      const expandedFirst = measure();
      if (expandedFirst.main_w < window.innerWidth * 0.35 || expandedFirst.main_w > window.innerWidth * 0.95) {
        throw new Error('geometry: 展开态 main 宽度异常: ' + JSON.stringify(expandedFirst));
      }
      for (let i = 0; i < 20; i++) {
        const t1 = visibleToggle();
        if (!t1) throw new Error('geometry: 第 ' + (i + 1) + ' 轮找不到可见的侧栏开关');
        t1.click();
        if (!(await waitForSidebar('collapsed'))) {
          throw new Error('geometry: 第 ' + (i + 1) + ' 轮折叠未生效');
        }
        const collapsed = measure();
        if (collapsed.main_w < window.innerWidth * 0.8) {
          throw new Error('geometry: 折叠态 main 宽度塌缩(用户问题 1): ' + JSON.stringify(collapsed));
        }
        if (!collapsed.composer_visible) {
          throw new Error('geometry: 折叠态 composer 不可见(用户问题 3): ' + JSON.stringify(collapsed));
        }
        const t2 = visibleToggle();
        if (!t2) throw new Error('geometry: 折叠态找不到展开开关');
        t2.click();
        if (!(await waitForSidebar('expanded'))) {
          throw new Error('geometry: 第 ' + (i + 1) + ' 轮展开未生效');
        }
        const expanded = measure();
        if (
          expanded.main_w < window.innerWidth * 0.35 ||
          expanded.main_w > window.innerWidth * 0.95
        ) {
          throw new Error('geometry: 展开态 main 宽度异常: ' + JSON.stringify(expanded));
        }
        if (!expanded.composer_visible) {
          throw new Error('geometry: 展开态 composer 不可见: ' + JSON.stringify(expanded));
        }
      }
      const expandedLast = measure();
      if (Math.abs(expandedLast.main_w - expandedFirst.main_w) > 1) {
        throw new Error(
          'geometry: 20 次开合后 main 宽度漂移 first=' + expandedFirst.main_w + ' last=' + expandedLast.main_w
        );
      }
      ckpt('toggles_20_stable', { first_w: expandedFirst.main_w, last_w: expandedLast.main_w });

      // (e) 问题 3/4:长问题只折叠正文;页脚(Expand/Collapse)两种状态都可见可点。
      //     与进入状态无关:先折叠(记录),再展开(记录),再折叠回初始态。
      const longTurn = Array.from(document.querySelectorAll('[data-testid="turn"]'))
        .reverse()
        .find((turn) => turn.querySelector('[data-testid="question-toggle"]'));
      if (!longTurn) throw new Error('geometry: 长问题的 question-toggle 未渲染');
      const qbody = longTurn.querySelector('[data-testid="question-body"]');
      const qactions = longTurn.querySelector('[data-testid="question-actions"]');
      const qtoggle2 = longTurn.querySelector('[data-testid="question-toggle"]');
      const bodyH = () => Math.round(qbody.getBoundingClientRect().height);
      const waitCollapsed = async (want) => {
        const deadline = Date.now() + 3000;
        while (Date.now() < deadline) {
          if (qbody.getAttribute('data-collapsed') === want) return true;
          await sleep(100);
        }
        return false;
      };
      const footerOk = () =>
        inViewport(qtoggle2) &&
        qactions.getBoundingClientRect().top >= qbody.getBoundingClientRect().bottom - 2 &&
        longTurn.getBoundingClientRect().bottom >= qactions.getBoundingClientRect().bottom - 1;
      // 先折叠(页脚滚入视口后点击——点击本身就是可用性证明)
      qtoggle2.scrollIntoView({ block: 'center' });
      await sleep(150);
      qtoggle2.click();
      if (!(await waitCollapsed('true'))) throw new Error('geometry: 折叠切换未生效');
      const collapsedH = bodyH();
      if (!footerOk()) throw new Error('geometry: 折叠态页脚不可见/越界(用户问题 3)');
      // 展开必须显著增高正文
      qtoggle2.scrollIntoView({ block: 'center' });
      await sleep(150);
      qtoggle2.click();
      if (!(await waitCollapsed('false'))) throw new Error('geometry: 展开切换未生效');
      const expandedH = bodyH();
      if (expandedH < collapsedH + 100) {
        throw new Error(
          'geometry: 展开未显著增高正文 collapsed=' + collapsedH + ' expanded=' + expandedH
        );
      }
      // 展开态正文超出视口属正常;把页脚滚入视口后必须可见、仍真实可点
      qtoggle2.scrollIntoView({ block: 'center' });
      await sleep(150);
      if (!inViewport(qtoggle2)) {
        throw new Error('geometry: 展开态页脚滚动后仍不可见');
      }
      if (
        qactions.getBoundingClientRect().top < qbody.getBoundingClientRect().bottom - 2 ||
        longTurn.getBoundingClientRect().bottom < qactions.getBoundingClientRect().bottom - 1
      ) {
        throw new Error('geometry: 页脚与正文重叠或被问题盒裁剪(用户问题 3/4)');
      }
      // 再次折叠必须生效(全部几何操作之后控件仍可用)
      qtoggle2.click();
      if (!(await waitCollapsed('true'))) throw new Error('geometry: 再次折叠未生效');
      const collapsedH1 = bodyH();
      if (collapsedH1 > collapsedH + 20) {
        throw new Error(
          'geometry: 再次折叠高度异常 collapsed0=' + collapsedH + ' collapsed1=' + collapsedH1
        );
      }
      if (!footerOk()) throw new Error('geometry: 折叠态页脚不可见(用户问题 3)');
      ckpt('question_toggle_footer', { collapsed: collapsedH1, expanded: expandedH });

      return {
        main_w: expandedLast.main_w,
        composer: { top: expandedLast.composer_top, h: expandedLast.composer_h },
        question: { collapsed: collapsedH1, expanded: expandedH },
      };
    });

    // 13. 合成 IME 组合测试(仅 DOM 合成事件;不切换输入源、不发全局按键):
    //     先通过真实 DOM 点击打开会话(composer 挂载后才有 textarea),
    //     compositionstart 后的 Enter(isComposing:true) 不得触发发送;
    //     compositionend 后的正常 Enter 必须发送(走真实 UI→IPC→本地 mock)。
    await step('ime_composition_enter_gate', async () => {
      // 细粒度检查点:卡住时报告能显示停在哪一相
      const ckpt = (name, value) => {
        out.steps.push({ name: `ime:${name}`, ok: true, value: value === undefined ? null : value });
        flush();
      };
      // 最早检查点:进入 IME 相(与"页面卡死"区分)
      ckpt('ime_step_entered');
      // 页面存活探针:1s 后若未出现 → 页面主线程被卡死
      setTimeout(() => {
        out.steps.push({ name: 'ime:early_alive_probe', ok: true });
        flush();
      }, 1000);
      // DOM 探针:记录侧栏实际渲染了什么(区分"没渲染"与"没找到元素")
      await step('ime:dom_probe', async () => {
        const body = document.body ? document.body.innerText : '';
        const sidebar = document.querySelector('aside.sidebar');
        const items = Array.from(document.querySelectorAll('.chat-item'));
        const root = document.getElementById('root');
        // 前端自报的错误(渲染错误/启动错误)
        const selfError = window.__AICOUNCIL_ERROR__ || null;
        const resources = (performance.getEntriesByType('resource') || [])
          .map((e) => ({
            name: String(e.name).split('/').pop(),
            size: e.transferSize || 0,
            dur: Math.round(e.duration),
          }));
        return {
          body_len: body.length,
          body_has_smoke: body.includes('Smoke Conv'),
          sidebar_present: !!sidebar,
          sidebar_width: sidebar ? Math.round(sidebar.getBoundingClientRect().width) : null,
          chat_items: items.length,
          href: String(location.href),
          ready_state: document.readyState,
          self_error: selfError,
          root_present: !!root,
          root_children: root ? root.childElementCount : -1,
          scripts: Array.from(document.querySelectorAll('script')).map((sc) => ({
            src: sc.getAttribute('src'),
            type: sc.getAttribute('type'),
            crossorigin: sc.getAttribute('crossorigin'),
          })),
          resources,
        };
      });
      // 打开会话:点击侧栏含 "Smoke Conv" 的可点击元素(最多等 5s)
      const deadlineOpen = Date.now() + 5000;
      let opened = false;
      while (Date.now() < deadlineOpen && !opened) {
        const items = Array.from(document.querySelectorAll('button, [role="button"], li, div')).filter(
          (el) => el.textContent && el.textContent.includes('Smoke Conv') && el.offsetParent !== null
        );
        // 优先点击真正的可交互元素(role=button / button),避免点到纯文本节点
        const clickable = items.filter(
          (el) => el.tagName === 'BUTTON' || el.getAttribute('role') === 'button'
        );
        const pool = clickable.length ? clickable : items;
        const target = pool[pool.length - 1];
        if (target) {
          target.click();
          opened = true;
          break;
        }
        await sleep(300);
      }
      if (!opened) throw new Error('侧栏未渲染可点击的会话项');
      ckpt('clicked_conversation');
      // 等 composer 挂载(最多 8s)
      const deadlineTa = Date.now() + 8000;
      let el = null;
      while (Date.now() < deadlineTa && !el) {
        el = document.querySelector('textarea') || document.querySelector('[contenteditable="true"]');
        if (!el) await sleep(300);
      }
      if (!el) throw new Error('composer textarea 未挂载(会话未打开?)');
      ckpt('composer_mounted', { tag: el.tagName });
      const before = await withTimeout(
        invoke('load_conversation', { conversation_id: convId }),
        5000,
        'load_conversation before composition'
      );
      const countUser = (d) =>
        (d.messages || []).filter((m) => m.role === 'user').length;
      const userBefore = countUser(before);
      ckpt('baseline_user_messages', userBefore);
      const setNativeValue = (v) => {
        const proto =
          el instanceof HTMLTextAreaElement
            ? HTMLTextAreaElement.prototype
            : HTMLInputElement.prototype;
        const setter = Object.getOwnPropertyDescriptor(proto, 'value').set;
        setter.call(el, '拼音合成测试');
        el.dispatchEvent(new Event('input', { bubbles: true }));
      };
      el.focus();
      ckpt('focused');
      setNativeValue('拼音合成测试');
      ckpt('value_set');
      // 组合开始 → 组合中 Enter:不得发送
      el.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true, data: 'n' }));
      ckpt('compositionstart');
      el.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, isComposing: true })
      );
      ckpt('composing_enter_dispatched');
      await sleep(800);
      const during = await withTimeout(
        invoke('load_conversation', { conversation_id: convId }),
        5000,
        'load_conversation during composition'
      );
      if (countUser(during) !== userBefore) {
        throw new Error('合成中的 Enter 触发了发送(IME 门禁失效)');
      }
      ckpt('composing_enter_blocked_confirmed');
      // Shift+Enter:换行,**不得**触发发送(未受信任的合成事件不会插入文本,
      // 因此这里断言"没有发送";真正的换行插入需要真人按键,见 docs/MMF_STATUS.md)
      await step('ime_shift_enter_does_not_send', async () => {
        const before = await withTimeout(
          invoke('load_conversation', { conversation_id: convId }),
          5000,
          'load_conversation before shift+enter'
        );
        const countBefore = countUser(before);
        el.dispatchEvent(
          new KeyboardEvent('keydown', { key: 'Enter', shiftKey: true, bubbles: true })
        );
        await sleep(600);
        const after = await withTimeout(
          invoke('load_conversation', { conversation_id: convId }),
          5000,
          'load_conversation after shift+enter'
        );
        if (countUser(after) !== countBefore) {
          throw new Error('Shift+Enter 触发了发送(应只换行)');
        }
        return { user_messages: countBefore, composer_len: String(el.value || '').length };
      });
      // 组合结束 → 正常 Enter:应发送
      el.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true }));
      ckpt('compositionend');
      el.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
      ckpt('normal_enter_dispatched');
      // 页面存活探针:2s 后若仍未出现 → 页面主线程被卡死(前端死循环),
      // 而非“发送未被计数”;探针本身也作为检查点记录
      setTimeout(() => {
        const toastEl = document.querySelector('.toast');
        out.steps.push({
          name: 'ime:page_alive_probe',
          ok: true,
          value: {
            toast: toastEl ? String(toastEl.textContent || '').slice(0, 200) : null,
            last_send: window.__AICOUNCIL_LAST_SEND__ || null,
            composer: el ? String(el.value || '').slice(0, 40) : null,
          },
        });
        flush();
      }, 2000);
      const deadline = Date.now() + 12000;
      while (Date.now() < deadline) {
        const after = await withTimeout(
          invoke('load_conversation', { conversation_id: convId }),
          5000,
          'load_conversation after compositionend'
        );
        if (countUser(after) > userBefore) {
          return { composing_enter_blocked: true, normal_enter_sent: true };
        }
        await sleep(300);
      }
      const lastSend = window.__AICOUNCIL_LAST_SEND__ || null;
      const toast = document.querySelector('.toast');
      const composerValue = el && el.value ? String(el.value).slice(0, 40) : '';
      throw new Error(
        'compositionend 后的正常 Enter 未发送; toast=' +
          (toast ? String(toast.textContent || '').slice(0, 120) : 'none') +
          '; composer=' + composerValue +
          '; last_send=' + JSON.stringify(lastSend)
      );
    });

    // 14. DOM 真渲染(前端把会话标题画进侧栏;最多等 10s)
    await step('dom_renders_conversation', async () => {
      const deadline = Date.now() + 10000;
      while (Date.now() < deadline) {
        const text = document.body ? document.body.innerText : '';
        if (text.includes('Smoke Conv')) return { text_len: text.length, ready_state: document.readyState };
        await sleep(300);
      }
      throw new Error('会话标题未出现在 DOM(前端未渲染或侧栏为空)');
    });
  };

  try {
    await Promise.race([
      run(),
      sleep(45000).then(() => { throw new Error('harness 45s 硬超时'); }),
    ]);
  } catch (e) {
    out.ok = false;
    out.steps.push({ name: 'harness_fatal', ok: false, error: String((e && e.message) || e) });
    flush();
  }
  out.finished_at = new Date().toISOString();
  window.__AICOUNCIL_SMOKE_RESULT__ = out;
  flush();
  try {
    await withTimeout(invoke('smoke_report', { payload: out }), 5000, 'final smoke_report');
  } catch (e) {
    // 上报失败也暴露出来(脚本会因缺最终报告而失败)
    console.error('smoke_report 失败', e);
  }
})();
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_binds_invoke_once_and_reports_via_smoke_report() {
        let s = page_load_script();
        assert!(s.contains("const invoke = window.__TAURI_INTERNALS__.invoke;"));
        // 修复点:invoke 绑定式只出现一次(错误消息里的提及不算绑定)
        assert_eq!(
            s.matches("const invoke = window.__TAURI_INTERNALS__.invoke;")
                .count(),
            1
        );
        assert!(s.contains("invoke('smoke_report', { payload: out })"));
        // 关键契约步骤齐全
        for name in [
            "bootstrap",
            "create_group",
            "draft_roundtrip",
            "send_message_single",
            "stream_persists_and_completes",
            "cancel_request",
            "export_markdown",
            "reload_credentials",
            "geometry_native_layout",
            "ime_composition_enter_gate",
            "dom_renders_conversation",
        ] {
            assert!(s.contains(&format!("'{name}'")), "缺少步骤 {name}");
        }
    }

    /// 极简 JS 括号配平扫描(忽略字符串/注释;模板字面量的 `${}` 按表达式配平)。
    ///
    /// 背景:2026-10-28 的原生冒烟"无报告"事故根因是 harness 少了一个
    /// `});` 与一个 `}` —— **JS 语法错误**使整个注入脚本无法解析,于是
    /// 一行都没跑(注入却打印了"injected",极具迷惑性)。此测试在
    /// `cargo test` 阶段即可拦住该类缺陷;driver 另外用真实 JS 解析器
    /// (`bun build`)做权威校验。
    fn js_brackets_balanced(src: &str) -> Result<(), String> {
        let bytes: Vec<char> = src.chars().collect();
        let mut stack: Vec<char> = Vec::new();
        let mut i = 0usize;
        let mut line = 1usize;
        // 状态:Normal / InTemplate(模板字面量文本部分)
        #[derive(PartialEq)]
        enum S {
            Normal,
            Template,
        }
        let mut state = S::Normal;
        while i < bytes.len() {
            let c = bytes[i];
            if c == '\n' {
                line += 1;
                i += 1;
                continue;
            }
            match state {
                S::Template => {
                    if c == '\\' {
                        i += 2;
                        continue;
                    }
                    if c == '`' {
                        state = S::Normal;
                        i += 1;
                        continue;
                    }
                    if c == '$' && bytes.get(i + 1) == Some(&'{') {
                        // 进入模板表达式:用 'T' 标记,闭合后回到模板文本状态
                        stack.push('T');
                        state = S::Normal;
                        i += 2;
                        continue;
                    }
                    i += 1;
                    continue;
                }
                S::Normal => {}
            }
            // 注释与字符串
            if c == '/' && bytes.get(i + 1) == Some(&'/') {
                while i < bytes.len() && bytes[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            if c == '/' && bytes.get(i + 1) == Some(&'*') {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == '*' && bytes[i + 1] == '/') {
                    if bytes[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
                i += 2;
                continue;
            }
            if c == '\'' || c == '"' {
                let quote = c;
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    if bytes[i] == '\\' {
                        i += 1;
                    }
                    if bytes[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
                i += 1;
                continue;
            }
            if c == '`' {
                state = S::Template;
                i += 1;
                continue;
            }
            match c {
                '(' | '[' => stack.push(c),
                '{' => stack.push(c),
                ')' | ']' | '}' => {
                    // 'T' 是模板表达式的闭合标记:闭合后回到模板文本状态。
                    if c == '}' && stack.last() == Some(&'T') {
                        stack.pop();
                        state = S::Template;
                        i += 1;
                        continue;
                    }
                    let expect = match c {
                        ')' => '(',
                        ']' => '[',
                        _ => '{',
                    };
                    match stack.pop() {
                        Some(open) if open == expect => {}
                        Some(open) => {
                            return Err(format!(
                                "line {line}: closing '{c}' does not match open '{open}'"
                            ))
                        }
                        None => return Err(format!("line {line}: unexpected closing '{c}'")),
                    }
                }
                _ => {}
            }
            i += 1;
        }
        if let Some(open) = stack.pop() {
            return Err(format!("unclosed '{open}'"));
        }
        Ok(())
    }

    #[test]
    fn harness_brackets_are_balanced() {
        // 语法错误会让注入脚本整体不解析:必须在这里就拦住。
        if let Err(err) = js_brackets_balanced(page_load_script()) {
            panic!("harness JS is not balanced: {err}");
        }
    }

    #[test]
    fn disabled_by_default_without_opt_in() {
        // 未设 AICOUNCIL_NATIVE_SMOKE=1 时必须禁用(release 恒禁用,
        // debug 也必须显式 opt-in)
        if std::env::var_os("AICOUNCIL_NATIVE_SMOKE").is_none() {
            assert!(!enabled());
        }
    }
}
