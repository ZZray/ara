# ARA Agent Core 跨产品复用实施任务

本清单对应 [spec.md](spec.md)。未勾选任务不表示运行时代码已经交付；已勾选任务以对应证据和验证结果为准。所有任务默认在当前 `dev` 工作树上执行，保留与本计划无关的 staged、unstaged、untracked 和 stash 改动；本轮不提交、不推送、不部署。

## 执行约定

- 先冻结接口和证据，再改实现；每个阶段都在 delivered snapshot 上运行适用测试。
- 受控 fake 只证明协议和失败路径；真实进程证明跨进程副作用；真实模型、真实 MCP、真实 QSpin/远端设备分别记录。
- 任何失败、跳过、零执行、缺少退出码、未知效果或证据缺失都保持 open，不改期望值来换绿。
- Core 不引入产品状态；Host 负责身份、权限、存储、模型、设备、Gateway、UI/RPC/MCP 连接和业务验收。
- `ara-agent-lite-skill`、`ara-agent-lite-memory` 在公共接口和最小闭环完成前保持 experimental；不承诺自动学习或自动晋升。

## 阶段 0：基线与契约冻结

- [ ] 0.1 记录目标工作树 SHA、dirty 文件清单、Lite 四 crate 版本、`upstream/omp.lock.json` 固定提交和现有 `support-matrix` 版本。产出：`docs/evidence/core-reuse-baseline-<date>.md`。
  - Requirements: R1, R12
  - 检查：`git status --short`、`cargo metadata --locked --offline --format-version 1`（Core/Harness/JSONL/MCP 各 workspace）。
- [ ] 0.2 把 `support-matrix.md/json` 的 10 个规划项逐项绑定到源码、测试、证据等级和 owner；把“测试存在”与“交付提交已执行”分列。
  - Requirements: R1, R8, R12
  - 退出标准：每项有 `source`、`test`、`owner`、`state`、`unverified` 字段；矩阵与 Markdown ID 集合一致。
- [ ] 0.3 冻结稳定 API 提升规则：Core、Harness、JSONL 帧、MCP crate、Skill capability、Memory capability 各自列 stable/experimental/internal 及主版本变更条件。
  - Requirements: R1, R8
  - 检查：树外 consumer 编译；stable 删除演示在 gate 中实际运行。

## 阶段 1：Core 连续性与调用边界

依赖：0.1–0.3。先完成 Core 内部语义，再暴露 Harness/Sidecar。

- [x] 1.1 核对并补齐 steering 的 execute 端到端契约证据：注入时机、来源、顺序、取消、迟到消息和 Journal/event 状态。
  - Requirements: R3
  - 已完成：Core controlled provider 端到端测试验证下一次模型请求包含 steering，且 `applied` 回执只表示上下文准入；证据见 `docs/evidence/core-steering-execute-20261110.md`。
  - 未完成：无 steering golden 对照、取消/迟到类型化结果、真实 sidecar/Host 实时观察。
  - 退出标准：本子任务的受控 execute 证据已通过；真实 sidecar/Host 和失败路径仍由后续子任务收口。
- [x] 1.2 处理 Session fork 的 archived-summary prefix：Journal 保留完整 raw records，受控 compaction 后可从 `active_base > 0` 的源 Session 重建两个 `active_base=0` fork；补齐前缀字节、source digest、目标可继续运行和 512 条/8 MiB 检查点边界。证据：`docs/evidence/core-session-fork-archived-prefix-20261110.md`；真实 Host/产品验收和跨进程并发写入延期。
  - Requirements: R4
  - 测试：归档前缀、非完整 exchange、pending Run、cwd mismatch、权限拒绝、失败清理。
  - 退出标准：源/fork 读写隔离；若仍拒绝，拒绝原因和版本边界进入 support-matrix，不称为完整 fork。
- [x] 1.3 设计 per-call hook/event v1：新增拥有数据的 `HostCallHooks` DTO/trait，覆盖 tool before deny、after success/error、after failure 停止下一次模型请求、无 hook 回归和 hook failure 传播；默认 `None` 保持旧行为。证据：`docs/evidence/core-call-hooks-20261110.md`、`crates/ara-agent-lite-core/tests/call_hooks.rs`；模型调用、异步 Host hook、取消/超时后的结算和真实产品 Host 验收延期。
  - Requirements: R5
  - 已覆盖：无 hook、hook 正常、hook 拒绝、before/after hook failure 传播，以及 after failure 后不再发下一次模型请求。
  - 延期：模型调用 hook、异步 Host hook、取消/超时 settlement、未知效果的 Host 结算和产品 Host 验收；不把 Host 业务验收放进 Core。
  - 退出标准：Core 只发拥有数据的请求上下文和 typed decision；旧 Host 无需实现即可运行。
- [x] 1.4 复核本地 compaction/refresh 的现有预算与事件，不在 Core 内硬编码 remote compaction；补充 Host/Provider capability 草案和 unavailable/timeout/canceled/partial 结果边界。证据：`docs/evidence/core-compaction-boundary-20261110.md`；未实现远程协议或真实 provider 验收。
  - Requirements: R6
  - 退出标准：文档定义 capability、请求/结果证明和失败类别；未声明时本地路径和类型化停止保持不变。

## 阶段 2：Harness 与 JSONL 恢复

依赖：1.1–1.3、0.3。

- [x] 2.1 将 `resume_checked` 的拒绝原因稳定化：authority changed、budget enlarged、run limit enlarged、fence mismatch、settlement incomplete、evidence missing。
  - Requirements: R2, R7
  - 测试：Rust Harness 受控拒绝矩阵；每类拒绝不调用 Host 工具、不新建 Session、不重放调用。证据：`docs/evidence/core-harness-jsonl-resume-20261110.md`。
- [x] 2.2 将拒绝类别安全地映射到 JSONL stderr/typed frame；保持“拒绝时不发送上游请求”的协议约束，避免把秘密写入 stdout/证据。
  - Requirements: R7
  - 测试：Python、Node、跨语言 resume；正常恢复、未知效果、进程杀死、权限/预算变化。
  - 退出标准：Host 能区分拒绝类别，原 Task/Session/fence/预算证据保持可追溯。证据：`docs/evidence/core-harness-jsonl-resume-20261110.md`。
- [x] 2.3 核对跨 Run steering、completion gate、stop review 和 unknown-effects 的 authority hash；拒绝 authority drift。
  - Requirements: R2, R3, R7
  - 退出标准：同一 Session、模型 Host、工具集合、steering 和授权字段才可续作；成功 Run 不自动 accepted。证据：`docs/evidence/core-harness-jsonl-resume-20261110.md`，并由 Harness 的 `task_set`、`tasks`、`completion_gate_accept`、`stop_review`、`unknown_effects` 测试覆盖。

## 阶段 3：Stable API 与交付门禁

依赖：2.1–2.3。

- [ ] 3.1 收敛 Core/Harness stable surface，补 stable 删除编译失败演示和版本化事件/帧兼容矩阵。
  - Requirements: R8
  - 测试入口：Core/Harness `tests/stability_surface.rs`、`tools/lite-consumer-check`、`lite-eval gate --stability-demo`。
- [ ] 3.2 让 `lite-eval gate` 在真实 delivered snapshot 上执行 Support_Matrix 全量引用、fmt、clippy、Rust/Python/Node/MCP/consumer 和依赖方向检查。
  - Requirements: R8, R12
  - 退出标准：任一 failed/skipped/zero_executed/missing/no_exit_code 即 gate 失败；保存命令、退出码、日志路径、测试数量和提交 SHA。
- [ ] 3.3 运行 `ara-git-review`、`ara-rust-core-review`，对每个阶段 diff 记录独立 reviewer、结果和未解决风险；再运行 `ara-backend-verification` 与 point-delivery-audit。
  - Requirements: R12
  - 退出标准：审查范围与 delivered snapshot 一致；没有用旧证据替代新代码验收。

## 阶段 4：MCP 与 Provider 边界

依赖：3.1–3.2。

- [ ] 4.1 保持 MCP crate 与 Core 解耦，补齐 supported/unsupported transport、OAuth、动态 listChanged 和 Unix 进程边界的协议错误分类。
  - Requirements: R1, R6
  - 测试：真实 sidecar + 受控 stdio/Streamable HTTP server；超时、取消、断连、只读 hint 和未知效果。
- [ ] 4.2 为 remote compaction 增加 Provider/Host capability、wire fixture、能力未声明拒绝、超时/取消/partial response 和 receipt provenance。
  - Requirements: R6
  - 退出标准：有真实 provider 证据时才标 supported；否则保持 unsupported/host-owned。

## 阶段 5：可审计 Skill Loader 最小闭环

依赖：0.3、3.1；不阻塞 Core 的基本 Run。

- [ ] 5.1 建立 `ara-agent-lite-skill` 的 manifest、registry 和 namespace：name、version、source、content hash、scope、permission、status、rollback target。
  - Requirements: R9
  - 测试：格式拒绝、哈希变化、来源缺失、scope 越界、重复版本和权限不足。
- [ ] 5.2 接入 discovery/read/install/verify/activate/deactivate/rollback 事件；复用 Core `skill_library` 的路径、大小、链接、预算检查，避免复制不一致实现。
  - Requirements: R9
  - 退出标准：所有变更有审计事件和来源；激活不等于授权；默认不可执行脚本。
- [ ] 5.3 加入显式 Host 审核和晋升状态机：`unverified → execution_evidence_attached → host_approved → active`，失败或内容变化可回滚。
  - Requirements: R9, R11
  - 退出标准：一次 Run 或模型自述不能自动晋升；Host 拒绝、取消和回滚都保留原件。

## 阶段 6：Memory provenance 最小闭环

依赖：0.3、3.1；可与阶段 5 并行，但三类 Host 验收前必须完成。

- [ ] 6.1 从 `context_memory`/`experience` 提取窄 capability：namespace、source refs、content hash、creator Run/Task、consent/policy、status、revoke、audit。
  - Requirements: R10
  - 测试：来源缺失、哈希冲突、撤销、epoch 变化、云 carrier 完整性、未知来源和重复写入。
- [ ] 6.2 建立 `ara-agent-lite-memory` 的 Host-owned store adapter；Core 只提交 typed proposal/reference，不直接决定租户持久化。
  - Requirements: R1, R10
  - 退出标准：SpinMind、Code Review、Remote Assist 使用不同 namespace；跨 namespace 查询和写入均被拒绝。
- [ ] 6.3 明确学习边界：只记录经验和候选 Skill/Memory，不自动改变系统规则、权限、工具 allowlist 或稳定 API。
  - Requirements: R9, R10
  - 退出标准：审计记录能还原来源、审核者、时间、版本、回滚点和实际使用 Run。

## 阶段 7：三类 Host 复用验收

依赖：阶段 2、3、5、6；真实模型/设备/MCP 证据按层分别记录。

- [ ] 7.1 SpinMind Host 场景：模拟 QSpin V3 工具先行，读取授权实验上下文，生成实验计划，Host 检查设备/样品约束，保存操作 receipt，并覆盖 steering、取消和未知效果。
  - Requirements: R11
  - 受控入口：fake QSpin tool + controlled provider。真实入口：QSpin V3 SDK、实际设备、真实采集/保存重开；没有设备时保持未验证。
- [ ] 7.2 Code Review Host 场景：在隔离 Git 副本中读取源码，调用编译/测试/静态分析，生成补丁候选，Host 按 diff/test receipt 续作或停止。
  - Requirements: R11
  - 真实模型试验：核实模型 ID、协议、路由、授权和预算，记录实际产物、工具效果、用量和延迟；HTTP 200 不算验收。
- [ ] 7.3 Remote Assist Host 场景：通过已授权 SSH/Starferry 读取状态，执行一项可回滚动作，核对远端 receipt；覆盖断连、取消、unknown effect 和不重放。
  - Requirements: R11
  - 真实入口：现场 Windows/SSH 主机和实际权限；未连接现场时只做受控 fake，不标记产品验收。
- [ ] 7.4 三场景交叉隔离：不同身份、Session key、Memory namespace、Skill scope、工具授权和证据目录不能互读或互写。
  - Requirements: R1, R2, R10, R11
  - 退出标准：同一 Core/Harness 构建通过三类 Host 的正向、失败和恢复检查；任何产品特有状态仍在 Host。

## 最小实现顺序与退出标准

| 顺序 | 先解决 | 最小退出标准 | 依赖 |
|---|---|---|---|
| 1 | 事实/稳定契约 | 矩阵有源码、测试、owner、证据等级；stable surface 可编译 | 无 |
| 2 | Core steering、fork、hook 边界 | 受控故障与默认回归通过；事件/Journal 可追溯 | 1 |
| 3 | Harness/JSONL resume | 拒绝类别可区分；原 Session/预算/fence 不变；未知效果不重放 | 2 |
| 4 | delivered-code gate | 全矩阵在交付快照执行并留 receipts；失败不会被吞 | 3 |
| 5 | MCP/provider adapter | MCP 限制和 remote capability 有 wire/fault 证据；Core 不依赖适配器 | 4 |
| 6 | Skill/Memory 审计闭环 | 显式授权、来源、namespace、晋升/回滚和撤销可重放审计 | 4 |
| 7 | 三类 Host | 同一 Core/Harness 通过三种有界任务及失败路径；真实设备/模型边界单列 | 5, 6 |

## 测试入口总表

| 层 | 入口 | 证明范围 |
|---|---|---|
| Core 单元/集成 | `cargo test --manifest-path crates/ara-agent-lite-core/Cargo.toml --locked --offline --all-targets` | Session、事件、压缩、回执、工具和负向状态 |
| Harness | `cargo test --manifest-path crates/ara-agent-lite-harness/Cargo.toml --locked --offline --all-targets` | Task、跨 Run 预算、验收、fence、resume、unknown effects |
| JSONL Rust/客户端 | `cargo test --manifest-path crates/ara-agent-lite-jsonl/Cargo.toml --locked --offline --all-targets`、Node `--test`、Python `unittest` | 帧、进程、跨语言 resume、取消和客户端回调 |
| MCP | `cargo test --manifest-path crates/ara-agent-lite-mcp/Cargo.toml --locked --offline --all-targets` | 配置、stdio/HTTP、结果映射、超时/取消、Harness 接入 |
| 交付门禁 | `cargo run --manifest-path tools/lite-eval/Cargo.toml -- gate --matrix --consumer` | delivered snapshot 的矩阵、依赖方向、稳定 API、consumer |
| 真实模型 | ARA Manager → OMP management → CAS；OpenRouter 仅作备选 | 真实任务产物、工具效果、上下文连续性、用量、延迟 |
| 真实设备/远端 | SpinMind/QSpin V3 Host；SSH/Starferry Host | 设备、现场权限、外部副作用和产品验收 |

规划阶段不执行以上运行时测试；任务完成前必须按适用层补齐证据，不以本文件勾选代替验收。
