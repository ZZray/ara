# P0 任务 3.3：Git 版本采集保持索引只读

2026-10-27；仓库 `56d55d209ecea00e80dfefecdd3e9d47fb62c47a` 加现有 dirty 工作区。此点属于 Headroom 正式适配之前的原 P0，不计入 H0 压缩效果或 Core 能力验收。

## 初始结果与定位

原 `cargo test --manifest-path tools/lite-eval/Cargo.toml --locked --test digest_props -- --nocapture` 退出 101：树摘要属性通过，Git 工作区摘要属性失败，实际测试时间 464.60s。最小反例为原文件保持、无新增文件、一个排除目录下的空写；失败断言是 `worktree_diff modified .git/index`。

不能据此把排除目录空写认定为触发。测试在空写前已调用一次 `diff_keeping_index`。仓外原生 Git 逐命令复现确认：复制模板后、尚未写入任何隐藏文件时，首轮 `git diff --binary --no-textconv --no-ext-diff HEAD ...` 就改了索引的 stat 缓存。3 次原配置均发生；命令级 `-c diff.autoRefreshIndex=false` 的 3 次对照均未发生。`rev-parse` 和 `ls-files` 未出现该写入。

本机 Git 2.55 的 `git-config.html` 中 `diff.autoRefreshIndex` 说明其默认 true，普通 `git diff` 可静默执行 `update-index --refresh`。`GIT_OPTIONAL_LOCKS=0` 单独不足以兑现“不改 index”的契约。原生复现检查了每个子命令退出码，不使用未检查退出码的旧诊断脚本作为成功依据。

## 本次修改

- `tools/lite-eval/src/digest.rs`：两类 diff argv 在 `diff` 之前加 `-c diff.autoRefreshIndex=false`，保留原 `GIT_OPTIONAL_LOCKS=0`；`DiffRule` 由实际 argv 生成命令描述，相关断言同步。
- `tools/lite-eval/tests/digest_props.rs`：保留两项 256 例属性，增加复制后干净仓库、排除目录空写和可见改动三次索引字节检查；与原 argv 的 diff stdout 逐字节对照。
- `tools/lite-eval/tests/zz_diag_tmp.rs`：保留原诊断草稿，仅按 workspace 格式整理，未删除或改变诊断行为。

不改全局/仓库 Git 配置，不引入 index 副本或新依赖。规则 ID 保持 `lite-eval-worktree-diff-v1`：修复只去除原契约禁止的采集副作用，不改变 diff 字节定义。实际 argv 与代码版本仍会记录到新的 Manifest；旧失败、deny 基线和未冻结的 B0 状态不重写。

## 验证进度

- 定向回归：1 PASS，2.45s；索引保持不变。
- workspace `cargo fmt -- --check`：PASS；第一次检查的格式问题及后续结果均保留。
- `cargo clippy --locked --all-targets --all-features -- -D warnings`：PASS，16.30s。
- 最终格式化源码的完整属性测试：3 PASS、0失败，383.24s；两项属性各256例，另1项定向回归。
- 补充测试：lib 27 PASS；旧诊断1 PASS，12.55s；bin/example无测试。旧诊断使用原命令、未检查Git退出码，只作为保留的诊断，不作为修复验收。
- 独立审查：`/root/estimate_review` 已按 ara-git-review 与 ara-rust-core-review 适用项核对快照差异、原生对照、冻结源码及日志；无源码发现。最终执行结果由主模型再核对，未调用真实模型。

首次修复后的未格式化批次在最终源码整理前被明确终止，退出 -1；保留其日志，不算通过。之后只对最终源码启动一个完整属性批次，仍保持每项 256 例，不缩减验收规模。

原始证据根：`D:/WorkSpace/tmp/ara-headroom-20261027/`。

| 证据 | 位置 |
|---|---|
| 初始源码快照 | `p0-before/` |
| 初始失败 | `evidence/p0-digest.*` |
| 原生 Git 逐命令对照 | `repro_index.py`、`p0-index-repro/results.json` |
| 定向回归 | `evidence/p0-regression.*` |
| 被终止批次 | `evidence/p0-digest-fixed.*` |
| 最终属性批次 | `evidence/p0-digest-final.*` |
| 最终格式与 lint | `evidence/p0-fmt-final.*`、`evidence/p0-clippy.*` |
| 补充测试 | `evidence/p0-other-tests.*` |

任务 3.3 及父任务 3 按已执行范围勾选；没有把 P0 阶段或任何模块标为 verified。未覆盖真实模型、Core/Harness 行为变化、B0 基线和正式 Headroom 适配。版本哈希与审查收据见 `p0-digest-record.json`。
