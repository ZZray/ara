# 每设备云端知识与记忆：M1 实施与后续设计

日期：2026-10-15。当前：**M1 已上线；真实跨会话召回和撤销已验证。M2 本地 outbox/双向同步、M3 后台整理与 M4 复杂检索尚未实施。** [最终版本与验收边界](../evidence/cloud-memory-20261015.md)。
用户已确认：设备记忆默认隔离，个人与项目记忆可以明确共享。
本设计属于 HandWave Host，不改变根仓库固定 OMP 的移植或验收状态。

## 目标、范围与现有事实

目标是让同一台电脑换会话、换浏览器后仍能使用已保存知识；不同电脑
不误用彼此的路径、环境和故障经验；共享项目可以复用经过确认的规则。
EXE 保持执行与少量缓存职责，云端拥有持久化、检索、版本和共享授权。

- 保留：独立 EXE、原生 Session、工具收据、候选/采纳边界、统一验证码入口。
- 本次 M1：Host 云端 JSON 持久化、设备/项目/Ray 工作区分域，显式保存、编辑、撤销，管理卡片和有界每 Run 快照。
- 明确延期：个人 principal、本地 outbox/双向同步、自动整理、embedding 和云端按需读取工具。
  本文记录源码行为；最终线上与真实模型验收由独立收据补齐，不以右侧卡片存在宣称完整验收。

源码核验范围是当前 dirty 工作区；交付仅锚定 evidence 中 Node/Ray 哈希，
不代表整个 dirty 仓库已发布。图谱对 node 源码为 not_tracked，旧 ARA Hub 文件为
metadata_changed；以下结论已直接回读源码。

| 能力 | 已有实现 | 当前缺口 |
|---|---|---|
| 本地来源与记忆 | [context_memory.rs](../../src/context_memory.rs)：Reference 版本/hash、Memory candidate/accepted/revoked、带来源 Summary；原子替换 JSON sidecar | 云端快照只供本次请求；没有 outbox 或双向同步 |
| 本地作用域 | [config.rs](../../src/config.rs)：controller fingerprint、canonical cwd 和 session-v1 Web key 派生独立原生 Session 路径 | 不是稳定个人 principal；不复制旧 native 历史 |
| Agent 工具 | `reference_read`、`reference_search`、`memory_propose` | 当前检索为 active reference 字面搜索；不是按需云端检索 |
| 采纳与撤销 | 本地候选仅由真正用户 objective 精确命令采纳；云端手工修订使用 expected_revision/CAS 和 op_id | 不自动提取上传；撤销在下次新请求生效，不即时中断运行/排队任务 |
| 云端 KB | ARA owner `apps/handwave/kb.go`：保存原文件、700 字分块、关键词倒排，只有 global/workspace 两级 | 缺设备/主体命名空间和结构化来源收据 |
| 云端到节点 | Ray Host `cloud_memory.go` 包装已有上下文入口；能力协商后的结构化快照与旧 KB/Skill brief 分帧 | 仍无独立云端 search/read/propose Agent 工具；旧 KB 不自动迁域 |

当前上下文上限为工作视图 192 KiB、sidecar 4 MiB；原始 checkpoint 另有
256 条消息/16 MiB 限制。压缩并没有解除原始会话容量限制。长会话分段
journal 应单独验收，不能靠删除上限来宣称无限记忆。

## 值得借鉴的思想

| 来源 | 可借鉴的内容 | 本项目取舍 |
|---|---|---|
| [Anthropic context engineering](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents) | 上下文是有限预算；按需查源、压缩、结构化笔记 | 当前目标/约束和少量有效记忆常驻，其余通过目录与读取工具展开；不把整个云库塞进每次请求 |
| [LangMem](https://github.com/langchain-ai/langmem/blob/main/docs/docs/concepts/conceptual_guide.md) | 区分语义事实、情节经验、程序性做法；检索相关性不只靠语义相似 | 分开存用户资料、设备事实、任务案例和可复用做法；保留已有 fact/preference/decision/lesson 类型 |
| [Mem0 当前设计](https://github.com/mem0ai/mem0/blob/main/docs/core-concepts/how-it-works.mdx) | 提取、去重、按实体/时间等信号检索；原始聊天与抽取事实不同；Platform 与 OSS 检索实现有差异 | 复用候选提取与去重思想，不引入整套运行时。当前官方文档描述自动提取 ADD-only、纠正/删除为显式操作；不可把旧版四操作宣传套给所有版本 |
| [Letta Sleep-time Compute](https://www.letta.com/blog/sleep-time-compute/) | 将部分理解与记忆整理移到对话之外 | 云端有预算的异步整理候选，前台读取已提交快照；异步只移动等待，不保证减少总费用 |
| [Graphiti](https://github.com/getzep/graphiti) | 带来源和有效期的事实，历史失效、关键词/语义/图关系结合 | 先采用时间与修订字段；暂不为小规模设备记忆部署 Python+图数据库，更不放进 EXE |
| [A-MEM 系统](https://github.com/WujiangXu/A-mem-sys) | 原子笔记、关键词、标签、记忆关联 | 可保留关联边与标签，不允许派生笔记覆盖原件。系统安装与论文结果复现是不同边界 |
| [ACE](https://github.com/ace-agent/ace) | 经验 playbook 分条增量更新，反思与整理分工，保留有效经验 | 从可验证任务收据形成条件化经验；避免反复整篇重写摘要造成信息塌缩；模型输出不能自动升级成项目规则或 Skill |
| [LongMemEval](https://github.com/xiaowu0162/LongMemEval) | 测信息提取、跨会话、时间、知识更新、未知问题 | 将“检索是否找到”“答案是否正确”“执行是否完成”分开验收；不拿聊天 QA 分数代替设备隔离与工具执行 |
| [LongMemEval-V2（2026）](https://github.com/xiaowu0162/LongMemEval-V2) | 由 Web Agent 多模态轨迹测试静态状态、动态状态、工作流、环境陷阱与前提识别；检索证据交固定 reader 评估 | 将前提识别迁移为本项目的设备/版本适用性测试是工程建议；V2 准确率与 query 延迟不能代表完整工具任务成功率 |
| [MemRL（2026）](https://github.com/MemTensor/MemRL) | 作者提出模型权重不变，在情节记忆上用环境反馈和两阶段检索寻找有用策略 | 保留反馈与反例的数据字段，暂缓移植运行时 RL；无可信验收反馈时，模型自报成功会强化错误经验 |

以上是工程取舍，不是框架性能排名。未在本项目做框架对照、费用或延迟实验。
自动读取供应商私有 Thinking、自动模型训练、无边界反思、多 Agent 常驻整理
和全盘上传均不属于推荐首版。

## 数据组织：三个共享域，四种用途

域由服务端授权确定，模型参数或浏览器随传的 scope 不是权限证明。

| 域 | 稳定归属 | 内容与共享 |
|---|---|---|
| device | Ray 工作区 + 已授权 device ID | 路径、环境、设备约束、故障经验；默认不跨电脑 |
| project | Ray 工作区 + 明确绑定的 project ID | 项目文档、规则、经过确认的经验；设备显式加入后可共享 |
| workspace | 共同 Ray 工作区，固定 scope ID `ray` | 用户显式选择的 Ray 共享内容，不称个人记忆 |
| personal（延期） | 尚未接入的已认证 principal ID | 个人跨电脑共享尚未实施，不能用浏览器指纹冒充 |

**现有身份限制：** Ray 的共享验证码和 Hub cookie 证明访问平台，但没有
提供每个人的 principal。源码中的认证 Session 保存会话与期限，不能推导
出用户身份。首版可先做工作区内设备隔离和项目共享；个人域在主体接线前
不得伪装成多用户隔离。若只有共同 Ray 身份，明确称“Ray 工作区共享”。
不因此擅自改动现有登录方式。

每个域内部再区分：

1. **知识资料**：用户选定/上传的原件、版本与分段引用。
2. **事实与偏好**：有来源的短事实，区分当前值和历史值。
3. **情节案例**：目标、环境、操作收据、失败、结果与验收结论。
4. **条件化经验**：适用条件、做法、反例、来源与验证；不把单次成功当通则。

设备更换名字不改变身份；重装后出现新 device ID 不自动按同名电脑合并。
跨设备项目也不按相似文件路径自动合并，显式绑定 project ID。
设备经验共享为项目经验应有单独的晋升记录，参数化本机路径并复核来源。

## 记忆生命周期与检索

原件先保存，模型派生视图再按需生成。候选提取、用户显式保存、确认采纳、
更正、撤销是不同操作。模型只提候选；直接用户指令可以按既有规则采纳。
推断与工具观测要标明来源，未知版本和未验收结果不得变成确定事实。

建议新增的云端元数据包括：

```text
id, scope, scope_id, kind, revision, status
source_id, source_revision, source_hash, source_span
observed_at, valid_from, valid_to, verified_at
run_id, task_receipt_id, supersedes, adoption_receipt
```

时间来自可信事件，不能由模型补造。来源权威性、时效和结果验证分开记录，
不以模型自评的一个 confidence 分数代替证据。过期记录可标记待复核，而非
立即清空或继续当现况；用户问历史时仍应能引用原版本。

检索顺序建议是：授权域过滤 → 状态/版本过滤 → 精确 ID/路径与关键词召回
→ 必要时语义召回与重排 → 去重与来源展开 → 按本次任务预算注入。
重要的用户约束与当前任务状态不能仅依赖向量 top-k。相关记忆应返回来源
收据，用户可以从右侧卡片查看“本轮实际使用”与“只是可用”的区别。

异步整理仅处理必要增量，去重并保留输入 revision。提交时再验证来源仍有效；
用户已撤销的来源不能被晚到后台任务重新写回。生成流程用不超过预算的
模型请求，失败不阻塞正常聊天，也不把未完成作业标成完成。

## 建议的实现切片

### M1：已实现的云端显式知识和记忆

ARA owner `apps/handwave/memorystore/store.go` 使用单进程 mutex、CAS 和原子
JSON 替换；不是 SQLite，也不是跨进程写入锁。状态文件上限 16 MiB，记录与
绑定历史合计 4096 条；容量满时报错，不能静默删除证据。文本单条 8192 bytes，
标题 120 字，kind 为 knowledge/fact/preference/decision/lesson。记录的 revision、
source_id/hash、用户输入 origin 和时间跟随原件保存；更新追加版本，撤销保留历史。
写入带 op_id 和 expected_revision，重复操作幂等，旧版本冲突返回 409。

Ray `cloud_memory.go` 校验网页登录、同源写入和 Hub 工作区/设备绑定，项目共享
需要显式加入。浏览器只把已配对真实设备与当前工作区一致的请求交给节点；上下文
读取失败或设备/会话变化直接阻断派发，不以空上下文掩盖错误。未绑定工作区仅能
绑定到当前已经配对的设备，不能覆盖其他绑定。右侧管理卡不自动提取、上传或采纳。

每 Run 按授权域生成 `cloud-memory-v1` 快照，最多 32 条且完整 JSON 不超过
32768 bytes；每条带 ID、revision、kind、text、hash、scope/ID。快照不是用户
授权或本地候选采纳收据。节点校验设备、边界与内容 hash；正文不落 native raw
或 sidecar，只保存失效 stamp。epoch 覆盖当前授权域的修订与撤销；变化、缺失或
unavailable 清理派生摘要并推进投影 watermark，原始审计历史保留，旧候选不能
凭新 Run 成功自动晋升。queued follow 不继承 active Run 的旧快照，缺新快照时
保守移除旧云端上下文，不承诺该分支已经重新检索全部云端记忆。

撤销对下次新请求生效；已运行或排队任务不即时中断。这里只实现请求前检索注入，
没有云端按需读取工具、完整二进制知识文档管线或旧 KB 自动迁域。EXE 未新增数据库
或 embedding 运行时。

### Web Session 与 native Session 的明确映射

`session-v1` 用经过格式验证的 `workspace_id/session_id` Web key，连同 controller
和 canonical cwd 派生独立 `ui-<hash>.json` 原生路径。新 Web 会话不会退回旧 cwd
会话，也不复制其历史；已有旧会话文件原样保留。没有该能力的旧客户端继续原契约，
不能据此声称它已有 Web 会话隔离。follow/steer/取消绑定实际发出的 Run 与 key，
切换 UI 会话不重新归属正在执行的任务。原生恢复失败仍报错，不新建对话冒充恢复。

### M2：节点缓存、同步和按需工具

保留现有 sidecar 的来源原件与本地 raw audit；另建云端记录的投影缓存与
outbox，不用云端快照覆盖本地原始证据。云端保存已确认版本；幂等操作 ID、
expected revision/CAS、ack 和同步水位防重试重复与并发覆盖。已有本地来源 ID
在不同 controller/cwd 会重复，上传映射必须包含稳定设备、原作用域与原 ID，
不能仅以 `mem-1` 或 `ref-1` 作云端主键。

再增加窄的云端 search/read/propose 工具，复用候选/采纳机制。执行模型的
grant 不等于记忆写入授权；服务端需验证实际 actor、device、project 与操作。
连接归属或明确授权的短期凭据决定权限，不能只信请求中填的 device ID。

断网时明确区分本地待同步、已获云端 ack 和带版本缓存。云端无法观察离线
电脑的新状态，缓存要显示 last-seen。共享缓存的失效/可用期限应有契约；
服务端撤销不能承诺立即删除离线电脑的字节。重连先同步 tombstone，再接收
旧增量；修正/撤销同时使索引与派生摘要失效。

### M3：有验证依据的记忆整理

在云端按事件与预算整理候选、冲突与经验，采用 ACE 式条目增量更新。
有用/有害统计绑定实际任务与审查收据，不由模型自己宣告学习成功。
个人/项目共享晋升仍保留明确边界与来源。

### M4：按实际效果增加复杂检索

先用真实任务建立基线；只有字面检索确有召回缺口才增加 embedding/混合
检索。关系查询足够复杂、时间变更足够频繁时再评估 Graphiti 类图后端。
用户知识、记忆和云端模型的供应商协议不绑定，换模型不重建证据来源。

## 可观察验收与效率指标

- B 设备的执行凭据与默认检索域不能在 list/search/read/download 取到 A 的
  独有路径/故障。同一获授权的人在管理界面显式选择 A 后能否访问，按管理
  权限另行校验，不能与 B Agent 的权限混同。显式共享
  项目规则后 B 可读取该规则，仍不能读取 A 的其余设备资料。
- 同一设备跨会话、跨浏览器能读取已云端确认的记录；已确认个人主体后才测
  跨设备个人记忆。服务器拒绝伪造 actor/device/project 的直接 API 请求。
- 路径从旧值纠正到新值，经历压缩、重启后，“现在”答新值，“以前”答旧值，
  都能给出对应来源；从未提供的信息应回答未知。
- 同一做法迁移到不同 OS、安装方式或软件版本前，先查环境前提；本机已验证
  的陷阱应能召回，新的无关报错不得直接套用旧根因。该测试借鉴 V2 的能力
  分类，不声称已在其完整 benchmark 上运行或取得成绩。
- 重复上传、ack 丢失、并发编辑、断网重试和重连不产生重复或覆盖新 revision；
  后台整理与撤销交错时，旧来源不会经索引、摘要或离线副本复活。
- 任务退出 0 但产物未验收，不能晋升为已验证经验；资料内指令不能采纳记忆
  或扩大授权。来源撤销与物理清除分别有明确契约。
- 用实际 Windows EXE、Hub 进程和有界公网模型任务验证保存→新会话召回
  →设备隔离/项目共享→修正/撤销→重启。受控 fake 故障测试单独记录。

记录任务成功率、来源召回率、过期/错误记忆采用率、输入字节/真实 usage、
首个输出延迟、检索 p50/p95、前台/后台模型调用数与费用、EXE 字节数。
没有实测就不承诺提速或节省比例；字节估计不冒充供应商 token 计量。

独立源码审查、受控进程/故障测试与实际线上召回/撤销支持 M1 的已实现边界；
最终版本、模型额外只读工具调用等限制见 evidence。不能从这些结果推导
所有模型、Linux 发行版、跨个人身份或自动学习能力均已验收。


## Cloud suggestion boundary (2026-10-15)

Ray's subsequent suggestion/settings release derives a transient summary from a
bounded cloud conversation view and accepted-memory snapshot. It does not read
or publish this node's native compaction summary, change the per-Run memory
carrier, auto-adopt facts or require another EXE. The node remains the40669969
published image. Cloud derivation acceptance is owned by the Ray repository's
`docs/evidence/ray-smart-suggestions-20261015.md`, separately from this Host's
memory-recall and native-session evidence above.
