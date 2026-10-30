# Headroom Rust 接入进度与 H0 验证

2026-10-27。线程 `01a17b06-eee9-7bc2-8353-7982bba2281c`。

用户已授权把 Headroom 接入加入 Kiro 计划并推进。追加 Headroom 计划时新增需求 39–41、任务 63–66，保留当时的原需求 1–38、任务 1–62 正文、编号和勾选状态；执行图有 154 个主线 wave、5 个 H0 独立 wave。H0 的门禁缺口只阻塞正式 Headroom 适配，不阻塞原任务 1–55。规格位于 `E:/repos/.kiro/specs/ara-lite-core-capability/`，旧版本快照在仓外证据根的 `spec-before/`。

扩展完成后继续原 P0：修复任务 3.3 的只读 Git 采集，完成两项256例属性及回归后，按实际结果勾选原任务3和3.3；原要求未削弱。该进展单独记录在 [P0 证据](../p0-digest-readonly.md)，不计为 Headroom 接入成功。

## 当前结论

H0 的 Windows Rust 构建、6 组离线输入和 2 项真实子进程测试通过，固定上游可以通过公共 API 调用。2026-10-30 用户批准三项依赖例外后，实际根策略审计退出0，0 errors、9 warnings，耗时25.099秒。H0记done 6/6，未标verified；原LNK4098链接警告与失败历史保留。Lite 代码、workspace 和锁文件未因本次原型修改；正式 P2-f、持久原文存储、Session 权限、跨进程取回和真实模型任务尚未实施。

正式执行顺序为原任务 1–55 → 新任务 64–66 → 原任务 56–62；任务 63 必须先完成。P2-f 会使用同二进制、同工具及预算的压缩关闭/开启对照，不能把原 B0 到 P2-e 的累计变化归因于 Headroom。

## 固定输入和产物

| 项目 | 位置或身份 |
|---|---|
| 上游 | `https://github.com/headroomlabs-ai/headroom` |
| 固定 SHA | `17d38e5699b7d7c69ac6addcd762f39e42dd7f00` |
| 源码与 LICENSE | `D:/WorkSpace/tmp/ara-headroom-20261027/upstream/` |
| 独立原型 | `D:/WorkSpace/tmp/ara-headroom-20261027/probe/` |
| 原始证据 | `D:/WorkSpace/tmp/ara-headroom-20261027/evidence/` |
| 特性 | `headroom-core`, `default-features = false`；解析节点特性为空，图中没有 ort/fastembed/magika |
| 工具链 | rustc 1.97.1、cargo 1.97.1，Windows x86_64 MSVC |
| 原执行的 debug exe | `D:/workspace/env/cache/rust-target-headroom-h0/debug/ara-headroom-h0.exe`，原记录18,207,744字节；当前缓存文件缺失，未重建；不是发布包大小 |
| 模型调用 | 0；无真实模型或产品效果验收 |

原型 Cargo 使用相对 path 依赖该固定 checkout，manifest、lock 和 SHA 均收录于 [机器可读记录](record.json)。原型不调用 Lite，不实现或复制 Agent 循环。未下载或启用 ML 权重；本次案例没有配置模型服务。

## 实际结果

下表是 UTF-8 字节数，不是模型 token 或实际费用。每例执行两次检查相同输入/配置的确定性，并保存原始输入和输出。

| 输入 | 原文 | 输出 | 检查 |
|---|---:|---:|---|
| 250 行重复 JSON，默认表格模式 | 30,152 | 16,479 | 唯一 ERROR 记录保留 |
| 同组 JSON，显式采样路径 | 39,153 | 1,878 | 唯一 ERROR 保留；实际 CCR marker 取回后 JSON 语义相等 |
| 2,000 行日志，中间唯一 FATAL | 102,906 | 480 | 唯一错误保留；必须生成上游 cache key，取回逐字节相等 |
| 短 JSON | 11 | 11 | 原样保留 |
| 短日志，含 NUL、中文、emoji | 22 | 22 | 原样保留 |
| 非法 JSON | 27 | 27 | 原样保留 |

Host 完整输入另用内容键保存并检查字节取回。它与上游 JSON CCR 的 canonical JSON 是两条检查；canonical payload 不承诺保留原始排版。缺失键返回 None，调用方仍须将其转换为明确工具错误。当前 store 仅在进程内，不能证明恢复后的引用仍可用。

真实子进程测试共 2 项：执行 6 组案例并核对上游 JSON/日志取回收据；拒绝覆盖已有证据目录并保留原文件。0 失败。测试日志有 `LNK4098: defaultlib LIBCMT conflicts with use of other libs`，尚未定位或消除，不能称无警告。

## 命令与失败记录

| 命令（均指定原型 manifest） | 结果 | 原始记录 |
|---|---|---|
| `cargo build --message-format json` | PASS；首次 2m55s；新增采样检查后 12.73s | `build*`、`build-v2*` |
| `cargo fmt -- --check` | PASS | 机器可读记录中的最终检查 |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | PASS，22.78s；原型本身无 ML feature | `clippy-final.*` |
| `cargo test --locked --all-targets -- --nocapture` | 2 PASS；保留上述链接警告 | `test.*` |
| 直接运行原型 | 6 例 PASS | `run-v2/`、`run-v2.stdout.json` |
| `cargo deny --config <ARA deny.toml> --format json check` | FAIL，退出 5 | `deny-v2.*`、`dependency-summary.json` |
| 规格结构和执行图检查 | PASS；41 需求、66 任务、159 wave；原条款保持，任务3/3.3按新执行结果推进 | `spec-check.json`（扩展时）、`spec-check-final.json`（P0推进后）、`verify_spec.py` |

全仓 `python scripts/verify_bootstrap.py` 退出 1，发现既有文档的本地路径和发布目录链接缺失。本次两个知识索引中的 Headroom 链接及该页的 record/review 链接已单独检查有效；全仓失败不记为通过，也不在本次改写其他文档。原始结果在 `bootstrap.stderr.log`，本次链接检查在 `links.json`。

第一次 cargo-deny 把 `--config` 放在子命令后，退出 2，未执行审计；`deny.stdout.jsonl`/`deny.stderr.jsonl` 保留此调用错误。随后更正参数位置得到退出 5 的真实审计，未修改策略。原 `run-v1` 只验证 Host 预存原文与默认 JSON 表格路径，缺少 JSON 上游 marker 检查；修复后的 `run-v2` 及进程测试才覆盖该项。初始不足未被删除或冒充同等证据。

## 依赖门禁与适配限制

无 ML 仍解析出 289 个包；这是 Cargo 元数据中的解析图，不等于 289 个实际编译/运行依赖。tokenizer、HF Hub、tree-sitter 和 SQLite 仍在依赖树内。

| 阻断 | 与现有工作区的关系 |
|---|---|
| `option-ext 0.2.0` / MPL-2.0 被策略拒绝 | 根及三个 Lite 锁文件均无此包，是新增项 |
| `webpki-roots 1.0.11` / CDLA-Permissive-2.0 被拒绝 | Lite 已有 1.0.10 的同类问题；新版本不能据此豁免 |
| `paste 1.0.15` / RUSTSEC-2024-0436 未维护 | 根 workspace 已存在，三个 Lite 锁文件没有，是 Lite 新增项 |

原审计退出5的失败记录保留。2026-10-30按用户明确批准更新根`deny.toml`：两个许可证采用包名与精确版本例外，paste采用根策略公告ID例外，影响所有匹配版本及已有Full图。详见[批准与义务](dependency-policy-20261030.md)、[实际策略审计](approved-policy-audit-20261030.json)和[独立复审](approved-policy-review-20261030.json)。上游SHA与锁文件未改；正式适配须在实际交付锁文件上重跑，并准备分发所需材料。上游 `CcrStore::put` 返回 `()`，不能证明持久化成功；“先存成功、再发布 marker”须由 Host 的可失败写入契约负责。上游 memory cache 不代替 Session 原文存储。

## 独立审查与未完成项

独立 Codex `/root/estimate_review` 读取具名规格/原型文件、此前快照及实际运行日志，按 `ara-git-review` 与 `ara-rust-core-review` 复审。首次发现 JSON marker 检查缺失、默认关闭与 A/B 关闭混淆、证据索引路径不一致，均已修复并复审；无剩余源码发现。详见 [审查记录](review.json)。审查者没有重跑测试；实际执行由本线程完成。

H0 不证明 Lite 投影、持久存储、恢复、权限、provider 协议、真实任务正确性或产品收益。任务63.5已完成，逐项需求39映射见[本次收尾记录](closeout-20261030.json)。原型源码、锁文件与运行证据哈希仍匹配；当前缓存exe已不存在，无法复核其当前哈希。本次只改策略，复用原构建/行为收据，未重建或重新执行行为，不宣称新二进制验收。任务64–66尚未开始，原55检查点仍未通过，不能仅凭依赖批准进入正式适配。
