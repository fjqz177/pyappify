# CLAUDE.md — pyappify（运行时 launcher fork）

> 仓库工作指南。Core 事实以 `src-tauri/src/*.rs` + `src/*.tsx` + 主仓库 `LiveTranslate-NG/CLAUDE.md §10` 为准。

## 一句话

本仓库是 **LiveTranslate-NG 分发模型的「运行时 launcher」**（`fjqz177/pyappify`，fork 自 ok-oldking/pyappify；本地 `D:\biancheng\pyappify`，master 分支）。它是 Tauri 2 桌面 app：读应用的 `pyappify.yml` → git clone/fetch → 管 Python → 装依赖 → 启动应用。**它不知道 LiveTranslate-NG 是什么**，只按声明行事——但具体契约见「三仓库地图」。

## 三仓库地图（联合开发必读）

| 仓库 | 本地 | GitHub | 角色 |
|---|---|---|---|
| **App 源码** | `D:\biancheng\LiveTranslate-NG` | `fjqz177/LiveTranslate-NG` | 被打包应用：`pyappify.yml` + `requirements-full-{cpu,gpu}.txt` + git tags（版本） |
| **本仓库（runtime）** | `D:\biancheng\pyappify` | `fjqz177/pyappify` | launcher：读 yml/装环境/启应用/UI |
| **打包器** | `D:\biancheng\pyappify-action` | `fjqz177/pyappify-action` | CI action：`git clone ry-appify` + `checkout tags/<version>` 编译 launcher + 产出 installer |

**跨仓库契约（配置/版本级，无编译器检查，变更=发布检查单）**：
- **A**：`pyappify.yml` 结构（profiles/main_script/requires_python/requirements/pip_args/icon/website...）——serde `default` 向后兼容。
- **B**：`{PIP_TORCH_INDEX_URL}` 占位符 token（App 的 `pip_args` 声明 ↔ 本仓库 `python_env.rs::expand_torch_placeholder` + 前端 "Pip Torch Index URL" 配置项）。
- **C**：runtime **tag 版本**（主仓库 `release.yml` 的 `version: v0.0.27` 硬编码两处）——**唯一真正的版本耦合/漂移点**；修 runtime 功能后必须 bump。

> 联合开发定稿（2026-09-03）：**只在本仓库之外的主仓库会话进行**（跨仓库因果链只有一张图）；单仓库纯改动可在此仓库会话。本文件保证任何会话自带正确上下文。

## 结构速览

```
src/                      # React + Tauri v2 前端（MUI）
  App.tsx                 # 主界面（list/安装/版本流/控制台/设置/profile 切换），单 App 卡片模型
  SettingsPage.tsx        # 语言/主题/pip 缓存/pip 镜像/torch 源
  ConsolePage.tsx         # 日志流 + 复制/打开日志目录
  UpdateLogPage.tsx       # 版本变更确认面板（inline 升级说明；说明失败不阻塞按钮）
  i18n.ts                 # 前端文案（六语言；key 必须与 en 完全对齐，有校验脚本）
src-tauri/src/
  lib.rs                  # 命令注册 + CLI 解析(单实例转发) + 托盘/周期任务挂载
  app.rs                  # App/Profile 结构 + app.json 原子写 + 缺省 profile 兜底
  app_service.rs          # 核心：setup/start/stop/update(带回滚)/load_app(自动更新+auto_start)/周期检查
  config_manager.rs       # 全局配置（pypi/torch 镜像、cache 目录）；已移除 Default Python Version 死配置
  python_env.rs           # 纯 uv：uv python install(UV_PYTHON_INSTALL_DIR 到 data/apps/<app>/python/) + uv pip install(镜像红线+占位符)；无 pip/python.org 回退链
  git.rs                  # git2 封装（tags/commit notes/checkout 回滚）
  utils/window.rs         # 托盘菜单(Open/Start/Stop/Quit) + 左键行为 + 快捷方式 + open_logs_directory
  locales/app.yml         # rust_i18n 文案（%{var} 语法）
```

**关键机制（改前必读）**：
- `app.json`（`data/apps/<app>/`）持久化 `update_state(Idle/Updating/Failed)`、`current_profile`、`auto_start`、`update_method`——重启恢复/重试失败更新靠它。
- 启动 = `use_pythonw` 时生成 `.pyappify-shortcut.py` **pythonw supervisor**（防原生崩溃二次错误对话框；`os.execve` vs `subprocess.run` 两条路径，有单测锁定）。
- pip 红线：profile `pip_args` 含 `--index-url/-i` 时**绕开用户所选镜像**（`use_config_index_url=false`）；GPU 用 `--extra-index-url` + 占位符（不触发该分支）。
- **环境=fail-closed**：启动前 sha256 指纹比对（requirements 内容+pip_args+python spec），失配才幂等 `uv pip install`（快路径零进程零网络）；`env_backend=uv` 与 `env_fingerprint` 存 app.json。legacy（旧嵌入式 pip 版）环境在启动时**一次性迁移**（raw `python/` → `python.legacy/` → uv 重建 → 验证后删备份，失败恢复并拦截）；`PIP_UPDATE_NEEDED_MARKER` 中断恢复保留。
- 红线（uv 版）：`pip_args` 含 `--index-url/-i` 时绕开用户所选镜像（`use_config_index_url=false`）；GPU 用 `--extra-index-url` + `{PIP_TORCH_INDEX_URL}` 占位符展开（空格/等号两种形式，有单测）。
- uv.exe 定位：`UV_EXECUTABLE` env > launcher 旁 sidecar（构建期由 action `uv_version` input 注入）> PATH；缺失即明确报错。
- 纯 uv 分支无 pip 回退；`uv` 装的 python 无 pip（顺手堵死 app 子进程偷装 pip）。

## 常用命令

```bash
cargo check        # 编译检查（改 Rust 后**串行**跑，勿与 Edit 并行——会读到旧文件）
cargo test         # 单测（36 个，含占位符展开/托盘 bootstrap/app.json 原子性）
pnpm build         # tsc + vite 前端构建（改 TS 后必跑）
pnpm dev           # vite dev（本地 UI 预览）
```

## 发布检查单（改本仓库功能后 → 要进用户安装器）

1. `cargo check` + `cargo test` 全绿；`pnpm build` 通过。
2. `git commit`（中文，conventional 前缀）+ `git push origin master`。
3. **打新 tag**（远程已存在的 tag 不可重打）：`git tag v0.0.27 → v0.0.28`… + `git push origin v0.0.28`。
4. 回主仓库 `LiveTranslate-NG/release.yml`：**两处** `version: v0.0.27` bump 到新 tag → commit（中文）→ push main。
5. 触发（二选一）：tag push 自动触发 / `gh workflow run release.yml --ref main -f version=<应用版本>`（workflow_dispatch 需要 `version` input，否则 Publish 报 "GitHub Releases requires a tag"）。
6. 构建 3 段：Validate(质量门+烟测) → Package(pyappify-action 构建) → Publish(gh-release)；`gh run watch <id>` 监控。

## 注意

- 本仓库无前端测试框架（无 vitest），TS 逻辑靠 `tsc` + 人工/主仓库 E2E。
- i18n 新键必须六语言齐（en/zh-CN/zh-TW/ja/ko/es），且 key 与 en 完全一致（es 曾有反引号 key 事故）。
- 改 `python_env.rs` 记得跑 `expand_torch_placeholder` 三单测（空格/等号/无占位符）。
