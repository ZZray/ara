# Core 0.1.1 编码工具验证

2026-10-22。基线为 `56d55d209ecea00e80dfefecdd3e9d47fb62c47a`。
本点增加完整文件快照、哈希前提、多块原文匹配和只读预览，不改变旧请求格式、
Run 状态机、模型选择、登录、设备授权、调度或界面。

## 工具行为

`read_file(snapshot=true)` 返回精确 UTF-8 原字节的 SHA256 和完整内容。
`edit_file` 的增强字段为 `expected_sha256`、`additional_edits`、`dry_run`。
同一请求的所有块在原始文本中匹配，全部预检通过后才写入；参数、匹配或哈希
错误不执行写入。没有新增编辑字段的旧请求保留原行为。

提交使用同一文件句柄并读回校验，保留硬链接与文件元数据语义。
它不是文件系统 CAS 或原子事务；外部写入和路径替换仍可能竞争。
I/O 或读回失败保留 `effects_unknown`，不能用原请求盲目重试。
单测用真实写入一个字节后注入错误，验证部分副作用与未知结果分支。

## 当前代码执行结果

| 检查 | 结果 | 范围 |
| --- | --- | --- |
| Core unit / public API | 180 + 3 PASS，1 ignored | 跳过项为原有公网可用性试验 |
| Host unit | 33 PASS | 独立 HandWave workspace |
| 两 workspace fmt / clippy | PASS | all targets / all features，warnings as errors |
| 两 workspace 离线 cargo deny | PASS | advisories / bans / licenses / sources；审计库新鲜度未确认 |
| doctests | 0 项，执行成功 | 不计入测试数量 |
| 新编辑实际 Host | 4 组 PASS | 快照/预览/多块/读回、旧哈希、后块失败、Plan 拒绝 |
| 原独立消费者 | 12 组 PASS | 自定义工具、恢复、重复 Run、取消、Worker 与终端 |
| 最终 Windows Node | 27 + 5 组 PASS | 实际 Hub、文件/编译/进程/代理/恢复及知识技能链路；受控 provider |

原始日志与收据位于本机 `D:/WorkSpace/tmp/ara-core-coding-20261022/` 和
`D:/WorkSpace/tmp/ara-core-release-20261022/`。发布记录保存安全摘要与哈希，
不提交真实模型的原生 Session、工作目录或私有配置。

独立 Codex `/root/coding_edit_pre_review` 完整审查四个运行时文件及八个新增测试，
结论 `APPROVE_SOURCE_SCOPED`，0 findings。主代理另读实际实现及受控 Host 验收器，
并核对落盘字节、调用身份和失败路径。独立 `/root/lite_release_harness_review`
审查真实编码基线；其发现与修复结果另随发布收据记录。

## 保留的失败和限制

初次扩展 Host 回归误用了旧 Hub 镜像 `27c9b265…`，在控制器身份握手阶段超时，
0 模型请求。核对此前已验证镜像后改用 `70e895af…`，在新目录重验；没有修改
协议或降低断言。首次 WSL 探测的用户名不正确，未执行构建，后续使用实查的非 root 账户。

受控 provider 只证明执行与错误处理，不证明模型自主编码、逆向或效率提升。
真实模型结果、跨平台范围和部署状态以本轮
[发布记录](../../../../apps/handwave-node/docs/evidence/stage-publication-0.2.10-20261022.md)为准。
新会话自主读取沉淀知识正文仍为 FAIL / OPEN；逆向工具后端与通用自我进化未在本点交付。
