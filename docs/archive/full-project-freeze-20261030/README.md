# Full 项目冻结归档 · 2026-10-30

用户决定 Full 暂停维护，先提交、推送主仓源码，再清理 Full 的工作树与缓存，保留主仓代码。本目录保存当前 Task 模块的最小接续材料。归档不表示 Full 或当前补证候选已完成，也没有在本步骤执行提交、推送或删除。

## 已验证范围

固定上游为 `b91db66e9b0b65a2c977e6a5bc74b0f4b1d58d1c`。历史 V4 受控执行通过原 9 组、89 个函数身份，失败与忽略均为 0。独立审计的原 19 条标准为 15 条通过、4 条 partial：M5、M8、O1、O8；完整 M1–M9 point 未接受。MCP/RPC grant 的额外补证也未接受。旧 `7/111`、新目标完整模块数 `6`、`ported_through_commit=null` 保持不变。

[EXEC 审查](evidence/EXEC-REVIEW.json)、[point 审计](evidence/POINT-AUDIT.json)、[Root 决定](evidence/ROOT-RECOVERY-DECISION-V4.json)和[原标准映射](evidence/SOURCE-CRITERIA-MAP.json)是历史受控执行的审查记录。它们引用的原始运行文件未全部归档；删除原缓存后，不能仅凭这些报告重新逐文件审计原始运行，需要恢复环境并重跑受控验证。

## 未采用的源码候选

[candidate/SOURCE-DIFF.json](candidate/SOURCE-DIFF.json)保留 8 个文件的原哈希、候选哈希和完整差异；[CANDIDATE.patch](candidate/CANDIDATE.patch)使用仓库相对路径，可从本次核定的主仓 V4 源码重建同样的 8 个候选文件。归档时，当前主仓这 8 个文件与 V4 原哈希全部一致；内存重建也逐文件匹配候选哈希，没有改写主仓源码。

F1–F5 已写入一个新增 bank，覆盖普通 child 运行中父仓库字节观察、物化后的启动/attachment 失败、真实 Bash/Wait/Core callback 等待、取消、Git 部分提交和 nested 持久化故障的候选场景。候选未采用、未编译、未执行，只有语法解析与部分源码审查；整包类型、接口、时序和独立审查仍待完成。Core callback 场景不代表完整 OMP extension 产品已实现。

[F6 片段](candidate/F6-CANDIDATE-FRAGMENT.rs)未追加到主 bank、未调用；8 文件 patch 已包含其 cfg permission/readback 支持。恢复时必须保留这个区别，先读[F6 作者记录](candidate/F6-AUTHOR-MANIFEST.json)与[接线记录](candidate/F6-ROUTE-NOTES.json)。exact-owner None/foreign 仍 OPEN；catalog-absent 不替代它们。原 19 条标准、9 组和 89 个身份的断言均未削弱。

## 恢复顺序

1. 从保留的主仓代码恢复 LF 工作副本，再按 [MANIFEST.json](MANIFEST.json) 核对归档副本与 8 个基线源码 SHA256。例如新 clone 时使用 `git -c core.autocrlf=false clone <仓库地址> <目录>`，只对该命令指定设置，不改全局配置。归档目录的 `.gitattributes` 使用 `* -text`，保证报告、片段和补丁原始字节不被 Git 换行归一化。原 D 盘路径只作历史来源，不是恢复依赖。
2. 决定恢复 Full 后，在独立工作副本对 `candidate/CANDIDATE.patch` 先做 `git apply --check`，再按授权应用。补丁只恢复当时未验收的候选，不接续任何未知外部副作用。
3. 对照 PLAN、PRE-REVIEW、HELPER-POST 和 F6 记录完成整包独立审查，处理 F6 未合并及剩余接口/权限缺口，再固定新的源码与构建输入。不得从语法解析推断编译或运行通过。
4. 重建普通 `ara`、既有 MCP fixture 和受影响的既有 integration targets，集中执行补证 bank 与审定的相关原测试。重新保存真实目录、Git、进程、journal、effect 和退出码证据，再决定 point 是否可接受。历史通过数不能代替恢复后的执行。

## 缓存清理边界

本归档不含二进制、target、完整 source 副本、原始模型请求、账户配置或凭据。计划清理的 Full 材料包括原 `task-m1m9-resume-20261029/source`、`target`、原 runtime/raw service-artifacts、Full candidate/cache 树及 Full 专用工作树。最终对象由 Root 本轮实核清单确定；Lite、共享工具缓存和主仓代码不由本归档扩大清理范围。此归档步骤未执行任何清理。