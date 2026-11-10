# Core 复用规划执行基线

日期：2026-11-10
仓库：`E:\\repos\\ara-github`
分支：`dev`
HEAD：`9a65b7992179ecf91f6a440ff250a4bd7e0fe295`

## 工作树边界

开始执行时已有以下改动，均不属于本任务：

- `apps/handwave-node/` 下的 Cargo、README、知识文档、许可证和测试改动。
- `docs/knowledge/INDEX.md`、`docs/knowledge/ai-assisted-engineering-evidence.md` 的既有改动。
- `apps/reach/docs/evidence/`、`docs/design/` 下的既有未跟踪内容。

本任务新增的规划/证据文件为：

- `docs/spec.md`
- `docs/tasks.md`
- `docs/evidence/core-reuse-baseline-20261110.md`
- `docs/evidence/core-steering-execute-20261110.md`
- `crates/ara-agent-lite-core/tests/steering_execute.rs`
- `crates/ara-agent-lite-core/docs/support-matrix.md`
- `crates/ara-agent-lite-core/docs/support-matrix.json`

`.codebase-memory/` 是本次 codebase-memory 索引生成的未跟踪缓存，未纳入规划内容；删除动作受本机执行策略拒绝，交付时仍保留为未跟踪状态。

## 依赖图检查

以下命令在三个 Lite workspace 上均成功，使用锁文件和离线模式：

```powershell
cargo metadata --manifest-path crates/ara-agent-lite-core/Cargo.toml --locked --offline --format-version 1
cargo metadata --manifest-path crates/ara-agent-lite-harness/Cargo.toml --locked --offline --format-version 1
cargo metadata --manifest-path crates/ara-agent-lite-jsonl/Cargo.toml --locked --offline --format-version 1
```

本次结果只证明 workspace metadata 可解析，不证明编译、测试、真实模型、真实 MCP、真实设备或产品验收通过。

## 版本与证据限制

- `crates/ara-agent-lite-core/docs/support-matrix.md` 当前矩阵把 MCP、Session fork、steering、JSONL resume、Harness stable API 和 delivery gate 标为部分支持，把 remote compaction 和 per-call Host hooks 标为不支持。
- 当前源码与测试证据用于规划 owner 和最小实现顺序；实现任务完成前不得把 support-matrix 的测试名存在性当作 delivered-code acceptance。
- 本基线没有提交、推送、部署或改变既有 WIP。
