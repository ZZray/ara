# Headroom 依赖策略批准记录

2026-10-30，线程 `01a18bf7-058a-7893-98c2-623998f3ed3e`。用户在逐项评估后明确批准：“批准三项，接受 paste 的根策略 ID 级例外”。本次将批准范围应用到仓库根 `deny.toml`。原 H0 失败与候选审计记录保留。

## 批准范围与后续义务

| 项目 | 根策略中的范围 | 后续义务 |
|---|---|---|
| option-ext 0.2.0 | 精确包名、版本 `=0.2.0`，允许 MPL-2.0 | 分发时保留许可证与通知，提供受 MPL 约束的源码访问；修改 covered files 时继续履行相应义务 |
| webpki-roots 1.0.11 | 精确包名、版本 `=1.0.11`，允许 CDLA-Permissive-2.0 | 分享证书数据时附 CDLA-Permissive-2.0 全文；依赖审核通过不代替分发材料检查 |
| paste / RUSTSEC-2024-0436 | 根策略按公告 ID 忽略；无法限定包版本 | 用户已接受所有匹配版本及既有 Full 依赖图的影响；仅豁免该未维护公告，其他公告继续检查；依赖锁或接入路线变化时复审 |

paste 1.0.15 是本次固定 H0 图中的版本。公告内容为项目不再维护，没有可升级的修复版本；这不证明不存在其他安全问题。本次没有替换依赖，也没有批准忽略其他公告。

固定上游保持 `17d38e5699b7d7c69ac6addcd762f39e42dd7f00`。实际编译链包括 `hf-hub → dirs → dirs-sys → option-ext`、`hf-hub → ureq → webpki-roots` 和 `tokenizers → paste`。前两项不能仅凭关闭 ML feature 排除，paste 是无条件 proc-macro 依赖。

## 验证范围

批准前的三项候选审计在固定 H0 图上退出 0、0 errors、9 warnings；仅许可证候选退出 1，唯一 error 为 paste 未维护。最早两次超时保留；后续在 locked/offline fetch 成功后完成对照。记录位于 `D:/WorkSpace/tmp/ara-lite-capability/current-01a18bf7/headroom-postfetch-candidate-dry-audits.json`。

应用后的审计与独立复审另存本目录，不将候选结果当作实际策略复验。检查固定原型 manifest、lock 与 metadata 的哈希，记录实际根策略哈希和 cargo-deny 原始输出。根策略影响整个共享策略使用范围，但本次执行只验证固定 H0 图，不宣称 Full 全图通过。

本次不修改 Lite 依赖锁、上游源码或 Windows 权限，不代表 P2-f 适配、真实任务效果、产品部署或发布验收完成。正式适配仍须检查实际交付锁文件，并准备许可证、通知与源码材料。
