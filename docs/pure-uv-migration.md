# 纯 uv 改造方案（pure-uv 分支）

> 目标分支：`fjqz177/pyappify` 的 **pure-uv** 分支——launcher 的 Python 管理、依赖解析、安装、校验**全链路只接受 uv**。不保留旧链路（python.org 下载 / pip / KNOWN_PATCHES）。
> 方案状态：**设计定稿 2026-09-03**；实现按 M1→M3 里程碑推进，每个里程碑独立可验收、可在分支内提交。
> 上游提示：本分支基于 fork（ok-oldking/pyappify），上游合并量增加——分支化改造默认接受后续 cherry-pick 成本。

---

## 1. 目标与原则

1. **全栈纯 uv**：Python 分发 = `uv python install`；依赖同步 = `uv pip install`（hash 锁定）；不保留 pip / python.org 下载 / tar-gz 解压/清理等任何旧路径。
2. **现有安装不中断**：老用户升级后**一次性自动迁移**到 uv 管理（见 §6）；迁移失败保持旧状态可回退（两阶段安全），成功后才删旧目录。
3. **契约不变**：`pyappify.yml` 结构、`{PIP_TORCH_INDEX_URL}` 占位符、`release.yml` 的 version tag 三契约不动（App 侧受影响的只有 `requirements-full-*.txt`——已是 `uv export` 产物，天然兼容）。
4. **fail-closed**：启动前环境正确性校验（指纹 + uv 幂等同步），修复失败拒启。

## 2. 关键架构决策（D1–D6）

| # | 决策 | 理由 |
|---|---|---|
| D1 | **依赖同步用 `uv pip install`（+ hash 锁）而非 `uv sync`** | `uv sync` 绑定 `pyproject.toml + uv.lock` 单一项目锁；pyappify 是**多 profile 模型**（cpu/gpu 各自 `requirements-full-*.txt` 共用同一 repo ），单锁装不下多变体。`uv pip -r` + 全行 hash = 相同"确定装配"语义。**单 profile 且声明 pyproject 的 app 后续可选支持 sync**（profile 可加 `sync: true`）。 |
| D2 | **应用运行仍用 venv 内 python 直启 + 既有 bootstrap**，不用 `uv run` | `start_app` 的 pythonw supervisor / `PYAPPIFY_*` env 注入 / 环境清理契约不能被 uv run 绕过（uv run 会重新解析项目配置）。uv 管"装"，python 跑"应用"，边界清晰。 |
| D3 | **uv 管理 Python 放独立分区目录** `data/apps/<app>/python-uv/`（`UV_PYTHON_INSTALL_DIR` 指向它） | uv 的 managed 目录布局（`cpython-3.12.x-*/`）与现有 raw `python./python.exe` 不同；分区共存避免混淆，也让 §6 迁移判定简单（目录不存在=未迁移）。 |
| D4 | **`uv.exe` 作为 sidecar 内嵌进 launcher 分发**（构建期下载，pin 版本 + sha256 校验，附 LICENSE） | 用户首开零额外下载；免去"每次装 python 先下载 uv"的启动延迟。约 +30MB 安装器，可接受（vs GPU 依赖 4.5GB）。 |
| D5 | **一次性自动迁移**（升级后首次 start/setup 触发），成功前后双保险 | 纯 uv 分支内**无运行时双路径**（D5 是"迁移事件"不是"回退路径"）：迁移失败 → 旧环境删除进程停止，保留旧目录 + 控制台/通知提示，重试按钮。 |
| D6 | **新增「Python 源」配置项**（映射 `UV_PYTHON_INSTALL_MIRROR`）：GitHub 官方 / 华为镜像候选 / 自定义 URL（含 `file://`） | python-build-standalone 由 uv 从镜像下载；缺省按 locale（zh-CN → 镜像）。镜像 URL 需实现前验证 astral pbs index 格式（`instances.json`），验证失败则为首选项 GitHub + 文档说明。 |

## 3. 取消的系统配置

| 配置/代码 | 处置 |
|---|---|
| `KNOWN_PATCHES`（3.7–3.13 硬编码 URL 表） | 删除（`python_env.rs`）。新 python 版本由 uv 决定，无需改代码。 |
| `get_download_urls` / `get_filename_from_url` / `download_file`（含 modelscope UA 伪装逻辑） | 删除。 |
| `extract_archive` / `extract_zip` / `extract_tar_gz` / `clean_python_install` | 删除（uv 安装即完成清理）。 |
| `get_user_agent`、`PIP_UPDATE_NEEDED_MARKER` 手工 marker 机制 | marker 保留（中断恢复 = 启动前重试 install，语义不变），但标记改为 uv 版本。 |
| `Default Python Version` 死配置 | 已删（上一轮），无需处理。 |

## 4. 改造清单（文件 × 变更）

### 4.1 `python_env.rs`（核心）

新增：
```text
fn locate_uv() -> Result<PathBuf>        // 1) 环境变量 UV_EXECUTABLE 显式指定
                                          // 2) launcher exe 同级 uv.exe（frozen sidecar）
                                          // 3) PATH 里的 uv
async fn ensure_python_via_uv(app_name, spec) -> Result<PathBuf>
  // 设 UV_PYTHON_INSTALL_DIR=<app>/python-uv
  // 调 `uv python install <spec>`（spec 形如 "3.12"→uv 会装最新 patch）
  // 定位 exe：优先 `uv python find <spec>`；兜底 glob `<dir>/cpython-*/python.exe`
  // 返回 (exe, 实际版本)
fn translate_pip_args(pip_args: &str) -> Result<Vec<String>, Error>
  // 白名单转译（见 §5 映射表）；未知参数 → 明确报错（含建议），不静默吞
fn expand_torch_placeholder  // 保持不变（契约 B）
pub async fn install_requirements(app_name, requirements, project_dir, pip_args)
  // 内部改为 `uv pip install`：
  //   uv pip install -r <file>/<spec> --python <managed exe>
  //   --no-warn-script-location 移除（uv 无此概念）
  //   --cache-dir <config>（见 4.3），--require-hashes（当 requirements 全行 hash 时）
  //   --index-url <config>（沿用 use_config_index_url 红线：profile 显式 --index-url/-i 时绕开）
  //   --extra-index-url（torch 占位符展开结果）
pub async fn setup_python_env(app_name, python_version_spec) -> Result<PathBuf>
  // 改为调用 ensure_python_via_uv
```

删除：见 §3 表。

### 4.2 `utils/path.rs`

新增 `get_managed_python_dir(app_name) -> PathBuf`（`<app>/python-uv/`）；
`check_python_env_exists`（`app_service.rs`）逻辑改为：**已迁移**（存在 managed 标记）→ 检查 managed exe；**未迁移**（旧 raw 目录）→ 检查旧 exe（迁移流程负责处理）。

### 4.3 `config_manager.rs`

- 新增配置："Python Source"（`UV_PYTHON_INSTALL_MIRROR` 值），options：`https://github.com/astral-sh/python-build-standalone`（官方）/ 华为镜像候选（待验证）/ `file://` 本地；缺省按 locale。
- "Pip Cache Directory" 语义扩展：映射 `UV_CACHE_DIR`（uv 的 wheel/依赖缓存）+ `pips` 缓存目录；"App Install Directory" → `<data>/cache/uv/`。`--cache-dir` 不传（用 env）。
- 前端（`SettingsPage.tsx`）：`Python Source` 下拉 + 现有项扩展 helperText。i18n 六语言补键。

### 4.4 `app_service.rs`

- `setup_app` / `update_to_version_inner` / `start_app` 调用面**不变**（接口由 4.1 函数承接）。
- 新增 `migrate_legacy_env_if_needed(app)`：启动准备阶段（`load_and_prepare_app_state` 内）检测老 raw 目录且无 `env_backend=uv` 标记时执行 §6 迁移。
- `app.json` 新增字段：`env_backend: "uv"`（迁移成功写入；防重复迁移）+ `env_fingerprint`（下表）。
- **启动门（fail-closed）**：`start_app` 前置校验：
  ```text
  指纹 = sha256(requirements 文件内容) + pip_args + python spec + uv 版本
  一致 → 启动
  失配/缺失 → uv pip install（幂等）→ 成功后写指纹 → 启动；失败 → 拒启 + 控制台错误 + 通知
  ```

### 4.5 `lib.rs` / 构建链（action）

- 菜单/命令无需变化。
- **pyappify-action 侧**（另仓库，本方案配套）：新增 input `uv_version`（pin，如 `0.8.x`）+ 构建 job 下载 `uv-x86_64-pc-windows-msvc.zip`、`sha256sum` 校验、解包 `uv.exe` 放入 launcher resources 目录（与 `pyappify.exe` 同级），并连同 `uv LICENSE`。
- 发布检查单（主仓库 CLAUDE.md）不变。

### 4.6 `pyappify.yml`（App 侧）——**无变更**

- `requires_python` / `requirements` / `pip_args` 声明不变；契约 A/B/C 不受影响。

## 5. pip → uv pip 参数映射表（`translate_pip_args` 白名单）

| pip 声明 | uv pip 处理 | 备注 |
|---|---|---|
| `--index-url` / `-i` | 直传 | 触发 `use_config_index_url=false` 红线（**保留原逻辑**，无行为回归） |
| `--extra-index-url`（含 `{PIP_TORCH_INDEX_URL}` 展开后） | 直传 | 契约 B |
| `--no-deps` | 直传 | |
| `--require-hashes` / requirements 全行 hash | 直传 / 自动启用 | uv 同语义 |
| `--cache-dir` | **忽略并警告**（用 `UV_CACHE_DIR`}) | 避免双缓存路径 |
| `--upgrade` / `--pre` / `--find-links` | **报错 + 建议**（多数场景无需） | uv 不支持即"确定失败"，符合纯 uv 原则 |
| `--no-warn-script-location` | 忽略（uv 无此警告） | |

## 6. 一次性迁移流程（老安装 → uv）

1. 升级后的首次 `start_app`/`setup` 触发 `migrate_legacy_env_if_needed`。
2. 检测：`python/python.exe` 存在 且 `app.json.env_backend != "uv"` → 进入迁移。
3. 迁移 = 全新 `ensure_python_via_uv` + `uv pip install -r <当前 profile requirements>`（**一次重装代价**，Release notes 提示首启会重装依赖，提示镜像选择）。
4. 成功：写 `env_backend="uv"` + 指纹 → 删除旧 raw `python/` → emit 通知"迁移完成"。
5. 失败：**不删旧目录**，报错到控制台 + 通知"迁移失败，环境未变"；`app.json` 保持旧状态（`env_backend` 缺失），用户可重试（再次启动即重跑）；已在旧环境可继续用（**D5 的"回退"=迁移失败保留旧环境，不是代码双路径**）。

## 7. 验收清单

**M1（uv 管依赖，python 仍旧链）**：
- [ ] `cargo test` 全绿（含新参数转译单测、占位符三单测保留）
- [ ] 全新装 CPU/GPU 变体 → `start` 成功（依赖走 uv）
- [ ] 版本升降级（pip sync 由 uv 完成）+ 失败回滚
- [ ] `--index-url` 红线不回归（镜像绕过 behavior 与旧一致）
- [ ] `uv.exe` 缺失时报错友好（M1 允许 PATH 提供）

**M2（uv 管 Python + 迁移）**：
- [ ] 新机器首装：零手工下载 python（uv install 自动/从所选源）
- [ ] 老安装环境升级 → 迁移成功、旧目录删除、二次启动不再触达迁移
- [ ] 迁移失败（断网/低效镜像）→ 报错 + 旧环境仍可用
- [ ] Python Source 配置生效（切换测：GitHub ↔ 镜像 ↔ file://）

**M3（门控 + 收尾）**：
- [ ] `start_app` 指纹失配 → 自动修复；修复失败 → 拒启（控制台+通知）
- [ ] 删除残留：`KNOWN_PATCHES`/extract/clean 等函数 grep 零引用
- [ ] 升级文档（README "Launcher UI" / 主仓库 CLAUDE.md §10）同步

## 8. 风险与对策

| 风险 | 对策 |
|---|---|
| `UV_PYTHON_INSTALL_MIRROR` 国内镜像不可用/格式不兼容 | D6 的镜像选项**实机验证后**才开放；未验证前仅 "GitHub 官方 / file:// 自定义" |
| `uv.exe` 供应链 | 构建期 sha256 pin + 固定版本 input；发布前人工核 hash |
| `pip_args` 边角语义漂移 | §5 白名单 + 未知参数**显式报错**（纯 uv 分支宁可报错不静默） |
| 迁移失败卡死用户 | §6 两阶段；失败保留旧环境 + 重试按钮 |
| uv 管理的 python 不带 pip | **免维护**（funasr `trust_remote_code` 偷装 pip 现状即被彻底堵死；与 App 侧 `neutralize_funasr_requirements()` 双保险） |
| 纯 uv 分支与上游合并成本 | 每次上游变更在 fork 上 rebase 并单独回归；M1 先行降低整体风险 |

## 9. 里程碑顺序与预估

- **M1**：`install_requirements` uv 化（uv.exe 经 PATH/UV_EXECUTABLE）——~1 天。
- **M2**：`ensure_python_via_uv` + Python Source 配置 + 迁移流程——~1–1.5 天（含迁移实机验证）。
- **M3**：指纹门控 + 删除旧链代码 + 文档同步——~0.5–1 天。
- 配套：action 侧 `uv_version` input + uv.exe sidecar 打包——~0.5 天（可与 M1 并行）。

## 10. 契约与影响面（重申）

- **App（LiveTranslate-NG）**：`pyappify.yml`/requirements/pip_args 零改动；`requirements-full-*.txt` 已是 `uv export` 产物直接兼容。
- **runtime 本分支**：内部实现替换，对外（yml/命令/UI 语义）不稳。
- **action/release.yml**：仅加 `uv_version` input 与 sidecar 打包（版本 A/B/C 契约不变量）。
